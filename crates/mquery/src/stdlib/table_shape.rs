//! `Table.*` operations that reshape: grouping, joins, expanding nested columns, pivoting and
//! unpivoting, splitting and merging columns.

use std::collections::HashMap;
use std::rc::Rc;

use super::keys::{Comparer, Grouper, try_sort_by};
use super::table::{check_size, col, cols, key_of, make, slot_at};
use super::*;
use crate::error::{EResult, EvalError, raise};
use crate::interp::{Interp, check_len, table_column};
use crate::value::{Builtin, Row, Slot, Table, Type, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Table.Group", 3, 5, group;
    "Table.NestedJoin", 5, 7, nested_join;
    "Table.AddJoinColumn", 5, 5, nested_join;
    "Table.Join", 4, 7, join;
    "Table.ExpandTableColumn", 3, 4, expand_table_column;
    "Table.ExpandRecordColumn", 3, 4, expand_record_column;
    "Table.ExpandListColumn", 2, 2, expand_list_column;
    "Table.AggregateTableColumn", 3, 3, aggregate_table_column;
    "Table.Pivot", 4, 5, pivot;
    "Table.Unpivot", 4, 4, |_, a| unpivot(a, false);
    "Table.UnpivotOtherColumns", 4, 4, |_, a| unpivot(a, true);
    "Table.SplitColumn", 3, 6, split_column;
    "Table.CombineColumns", 4, 4, combine_columns;
];

fn sub_table(t: &Table, rows: Vec<Row>) -> Value {
    Value::table(Table { columns: t.columns.clone(), types: t.types.clone(), rows })
}

/// `{name, function}` or `{name, function, type}` aggregations: one, or a list of them.
fn aggregations(v: &Value) -> EResult<Vec<(Rc<str>, Value, Type)>> {
    let Value::List(l) = v else { return mismatch(v, "a list of {name, function} aggregations") };
    let one = |vals: &[Value]| match vals {
        [Value::Text(name), f @ Value::Function(_)] => Ok((name.clone(), f.clone(), Type::Any)),
        [Value::Text(name), f @ Value::Function(_), Value::Type(ty)] => Ok((name.clone(), f.clone(), ty.clone())),
        [Value::Text(name), f @ Value::Function(_), Value::Null] => Ok((name.clone(), f.clone(), Type::Any)),
        _ => raise("Each aggregation is a list of a new column name, a function and optionally a type."),
    };
    let vals = l.values()?;
    if matches!(vals.first(), Some(Value::Text(_))) {
        return Ok(vec![one(&vals)?]);
    }
    vals.iter()
        .map(|v| match v {
            Value::List(p) => one(&p.values()?),
            v => mismatch(v, "a {name, function} aggregation"),
        })
        .collect()
}

fn group(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let idx = cols(&t, &names(arg(a, 1))?)?;
    let aggs = aggregations(arg(a, 2))?;
    let local = matches!(arg(a, 3), Value::Number(n) if *n == 0.0);
    let comparer = Comparer::from(arg(a, 4))?;
    // Rows per group, in order of first appearance.
    let mut groups: Vec<Vec<Row>> = Vec::new();
    if local {
        // Consecutive rows with the same key.
        let mut prev: Option<Row> = None;
        for r in &t.rows {
            let mut same = prev.is_some();
            if let Some(p) = &prev {
                for i in &idx {
                    let equal = match (p.get(*i), r.get(*i)) {
                        (Some(Slot::Val(x)), Some(Slot::Val(y))) => comparer.equal(it, x, y)?,
                        _ => false,
                    };
                    if !equal {
                        same = false;
                        break;
                    }
                }
            }
            match groups.last_mut() {
                Some(g) if same => g.push(r.clone()),
                _ => groups.push(vec![r.clone()]),
            }
            prev = Some(r.clone());
        }
    } else {
        let mut grouper = Grouper::new(comparer);
        for r in &t.rows {
            let (g, new) = grouper.group(it, &key_of(r, &idx))?;
            if new {
                groups.push(Vec::new());
            }
            if let Some(rows) = groups.get_mut(g) {
                rows.push(r.clone());
            }
        }
    }
    let mut columns: Vec<Rc<str>> = idx.iter().filter_map(|i| t.columns.get(*i).cloned()).collect();
    let mut types: Vec<Type> = idx.iter().filter_map(|i| t.types.get(*i).cloned()).collect();
    for (name, _, ty) in &aggs {
        columns.push(name.clone());
        types.push(ty.clone());
    }
    let mut rows = Vec::with_capacity(groups.len());
    for g in groups {
        let mut cells: Vec<Slot> = match g.first() {
            Some(first) => idx.iter().map(|i| slot_at(first, *i)).collect(),
            None => continue,
        };
        let sub = sub_table(&t, g);
        for (_, f, _) in &aggs {
            cells.push(Slot::capture(it.call1(f, sub.clone()))?);
        }
        rows.push(cells.into());
    }
    make(columns, types, rows)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JoinKind {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
    LeftAnti,
    RightAnti,
}

fn join_kind(v: &Value, default: JoinKind) -> EResult<JoinKind> {
    Ok(match v {
        Value::Null => default,
        Value::Number(n) => match *n as i64 {
            0 | 6 => JoinKind::Inner,
            1 => JoinKind::LeftOuter,
            2 => JoinKind::RightOuter,
            3 => JoinKind::FullOuter,
            4 => JoinKind::LeftAnti,
            5 => JoinKind::RightAnti,
            _ => return raise("This kind of join isn't supported."),
        },
        v => return mismatch(v, "a JoinKind value"),
    })
}

/// For each row of `left`, the rows of `right` with the same key; and for each row of `right`,
/// whether any row of `left` matched it. Nulls match nulls.
struct Matches {
    by_left: Vec<Vec<usize>>,
    right_matched: Vec<bool>,
}

fn match_rows(it: &mut Interp, left: &Table, left_keys: &[usize], right: &Table, right_keys: &[usize], comparer: Comparer) -> EResult<Matches> {
    if left_keys.len() != right_keys.len() {
        return raise("Both tables need the same number of key columns.");
    }
    let mut grouper = Grouper::new(comparer);
    let mut members: Vec<Vec<usize>> = Vec::new();
    for (j, r) in right.rows.iter().enumerate() {
        let (g, new) = grouper.group(it, &key_of(r, right_keys))?;
        if new {
            members.push(Vec::new());
        }
        if let Some(m) = members.get_mut(g) {
            m.push(j);
        }
    }
    let mut by_left = Vec::with_capacity(left.rows.len());
    let mut right_matched = vec![false; right.rows.len()];
    for r in &left.rows {
        let found = match grouper.find(it, &key_of(r, left_keys))? {
            Some(g) => members.get(g).cloned().unwrap_or_default(),
            None => Vec::new(),
        };
        for j in &found {
            if let Some(m) = right_matched.get_mut(*j) {
                *m = true;
            }
        }
        by_left.push(found);
    }
    Ok(Matches { by_left, right_matched })
}

fn nulls(n: usize) -> impl Iterator<Item = Slot> {
    std::iter::repeat_n(Slot::NULL, n)
}

fn nested_join(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (left, right) = (table(a, 0)?, table(a, 2)?);
    let (lk, rk) = (cols(&left, &names(arg(a, 1))?)?, cols(&right, &names(arg(a, 3))?)?);
    let name = text(a, 4)?;
    let kind = join_kind(arg(a, 5), JoinKind::LeftOuter)?;
    let comparer = Comparer::from(match arg(a, 6) {
        f @ Value::Function(_) => f,
        _ => &Value::Null,
    })?;
    let m = match_rows(it, &left, &lk, &right, &rk, comparer)?;
    let nested = |js: &[usize]| Slot::Val(sub_table(&right, js.iter().filter_map(|j| right.rows.get(*j).cloned()).collect()));
    let mut rows: Vec<Row> = Vec::new();
    for (r, js) in left.rows.iter().zip(&m.by_left) {
        let keep = match kind {
            JoinKind::LeftOuter | JoinKind::FullOuter => true,
            JoinKind::Inner | JoinKind::RightOuter => !js.is_empty(),
            JoinKind::LeftAnti => js.is_empty(),
            JoinKind::RightAnti => false,
        };
        if keep {
            rows.push(r.iter().cloned().chain(std::iter::once(nested(js))).collect());
        }
    }
    if matches!(kind, JoinKind::RightOuter | JoinKind::FullOuter | JoinKind::RightAnti) {
        for (j, matched) in m.right_matched.iter().enumerate() {
            if !matched {
                rows.push(nulls(left.width()).chain(std::iter::once(nested(&[j]))).collect());
            }
        }
    }
    let mut columns = left.columns.to_vec();
    let mut types = left.types.to_vec();
    columns.push(name);
    types.push(right.table_type());
    make(columns, types, rows)
}

fn join(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (left, right) = (table(a, 0)?, table(a, 2)?);
    let (lk, rk) = (cols(&left, &names(arg(a, 1))?)?, cols(&right, &names(arg(a, 3))?)?);
    let kind = join_kind(arg(a, 4), JoinKind::Inner)?;
    let comparer = Comparer::from(match arg(a, 6) {
        f @ Value::Function(_) => f,
        _ => &Value::Null,
    })?;
    let m = match_rows(it, &left, &lk, &right, &rk, comparer)?;
    let width = left.width() + right.width();
    let mut rows: Vec<Row> = Vec::new();
    for (r, js) in left.rows.iter().zip(&m.by_left) {
        match kind {
            JoinKind::Inner | JoinKind::LeftOuter | JoinKind::RightOuter | JoinKind::FullOuter => {
                for j in js {
                    if let Some(s) = right.rows.get(*j) {
                        rows.push(r.iter().chain(s.iter()).cloned().collect());
                    }
                }
                if js.is_empty() && matches!(kind, JoinKind::LeftOuter | JoinKind::FullOuter) {
                    rows.push(r.iter().cloned().chain(nulls(right.width())).collect());
                }
            }
            JoinKind::LeftAnti => {
                if js.is_empty() {
                    rows.push(r.iter().cloned().chain(nulls(right.width())).collect());
                }
            }
            JoinKind::RightAnti => {}
        }
        check_size(rows.len(), width)?;
    }
    if matches!(kind, JoinKind::RightOuter | JoinKind::FullOuter | JoinKind::RightAnti) {
        for (s, matched) in right.rows.iter().zip(&m.right_matched) {
            if !matched {
                rows.push(nulls(left.width()).chain(s.iter().cloned()).collect());
            }
        }
    }
    let columns = left.columns.iter().chain(right.columns.iter()).cloned().collect();
    let types = left.types.iter().chain(right.types.iter()).cloned().collect();
    make(columns, types, rows)
}

/// Replaces column `at` by the columns `new`, each input row giving the rows that `expand`
/// returns for its cell (each a vector of `new.len()` cells).
fn replace_column(
    t: &Table,
    at: usize,
    new: Vec<Rc<str>>,
    new_types: Vec<Type>,
    mut expand: impl FnMut(&Slot) -> EResult<Vec<Vec<Slot>>>,
) -> EResult<Value> {
    let width = t.width() - 1 + new.len();
    let mut rows: Vec<Row> = Vec::with_capacity(t.rows.len());
    for r in &t.rows {
        let Some(cell) = r.get(at) else { continue };
        for cells in expand(cell)? {
            rows.push(r.iter().take(at).cloned().chain(cells).chain(r.iter().skip(at + 1).cloned()).collect());
        }
        check_size(rows.len(), width)?;
    }
    let columns = t.columns.iter().take(at).cloned().chain(new).chain(t.columns.iter().skip(at + 1).cloned()).collect();
    let types = t.types.iter().take(at).cloned().chain(new_types).chain(t.types.iter().skip(at + 1).cloned()).collect();
    make(columns, types, rows)
}

/// The new names of an expand: the ones given, or the inner names.
fn new_names(inner: &[Rc<str>], v: &Value) -> EResult<Vec<Rc<str>>> {
    match v {
        Value::Null => Ok(inner.to_vec()),
        v => {
            let n = names(v)?;
            if n.len() != inner.len() { raise("There must be one new column name for each expanded column.") } else { Ok(n) }
        }
    }
}

fn error_cells(e: &Rc<crate::error::MError>, n: usize) -> Vec<Vec<Slot>> {
    vec![vec![Slot::Err(e.clone()); n]]
}

fn not_expandable(v: &Value, want: &str, n: usize) -> Vec<Vec<Slot>> {
    let e = match crate::error::error(crate::error::EXPRESSION_ERROR, format!("A value of type {} can't be expanded: {want} is expected.", v.kind()))
    {
        EvalError::Raised(e) => e,
        EvalError::Fatal(_) => return vec![vec![Slot::NULL; n]],
    };
    error_cells(&e, n)
}

fn expand_table_column(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let at = col(&t, &text(a, 1)?)?;
    let inner = names(arg(a, 2))?;
    let new = new_names(&inner, arg(a, 3))?;
    let n = inner.len();
    // Column types come from the first nested table.
    let first = t.rows.iter().find_map(|r| match r.get(at).map(Slot::result) {
        Some(Ok(Value::Table(x))) => Some(x),
        _ => None,
    });
    let types =
        inner.iter().map(|c| first.as_ref().and_then(|x| x.column_index(c).and_then(|i| x.types.get(i).cloned())).unwrap_or(Type::Any)).collect();
    replace_column(&t, at, new, types, |cell| {
        Ok(match cell.result() {
            Ok(Value::Table(x)) if !x.rows.is_empty() => {
                let idx: Vec<Option<usize>> = inner.iter().map(|c| x.column_index(c)).collect();
                x.rows.iter().map(|r| idx.iter().map(|i| i.map_or(Slot::NULL, |i| slot_at(r, i))).collect()).collect()
            }
            Ok(Value::Table(_) | Value::Null) => vec![vec![Slot::NULL; n]],
            Ok(v) => not_expandable(&v, "a table", n),
            Err(e) => error_cells(&e, n),
        })
    })
}

fn expand_record_column(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let at = col(&t, &text(a, 1)?)?;
    let inner = names(arg(a, 2))?;
    let new = new_names(&inner, arg(a, 3))?;
    let n = inner.len();
    replace_column(&t, at, new, vec![Type::Any; n], |cell| {
        Ok(match cell.result() {
            Ok(Value::Record(r)) => vec![inner.iter().map(|c| r.slot(c).cloned().unwrap_or(Slot::NULL)).collect()],
            Ok(Value::Null) => vec![vec![Slot::NULL; n]],
            Ok(v) => not_expandable(&v, "a record", n),
            Err(e) => error_cells(&e, n),
        })
    })
}

fn expand_list_column(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let name = text(a, 1)?;
    let at = col(&t, &name)?;
    replace_column(&t, at, vec![name], vec![Type::Any], |cell| {
        Ok(match cell.result() {
            Ok(Value::List(l)) if !l.is_empty() => l.items.iter().map(|s| vec![s.clone()]).collect(),
            Ok(Value::List(_) | Value::Null) => vec![vec![Slot::NULL]],
            Ok(v) => not_expandable(&v, "a list", 1),
            Err(e) => error_cells(&e, 1),
        })
    })
}

/// `Table.AggregateTableColumn(table, column, {{inner column, function, new name}, …})`.
fn aggregate_table_column(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let at = col(&t, &text(a, 1)?)?;
    let mut specs: Vec<(Rc<str>, Value, Rc<str>)> = Vec::new();
    for s in &*list(a, 2)?.items {
        match s.get()? {
            Value::List(p) => match p.values()?.as_slice() {
                [Value::Text(inner), f @ Value::Function(_), Value::Text(new)] => specs.push((inner.clone(), f.clone(), new.clone())),
                _ => return raise("Each aggregation is a list of an inner column, a function and a new column name."),
            },
            v => return mismatch(&v, "an aggregation"),
        }
    }
    let n = specs.len();
    let new = specs.iter().map(|s| s.2.clone()).collect();
    replace_column(&t, at, new, vec![Type::Any; n], |cell| {
        Ok(match cell.result() {
            Ok(Value::Table(x)) => {
                let mut cells = Vec::with_capacity(n);
                for (inner, f, _) in &specs {
                    let column = table_column(&x, col(&x, inner)?);
                    cells.push(Slot::capture(it.call1(f, column))?);
                }
                vec![cells]
            }
            Ok(Value::Null) => vec![vec![Slot::NULL; n]],
            Ok(v) => not_expandable(&v, "a table", n),
            Err(e) => error_cells(&e, n),
        })
    })
}

fn pivot(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let culture = it.culture(None);
    let (attr, value) = (col(&t, &text(a, 2)?)?, col(&t, &text(a, 3)?)?);
    let aggregate = opt_function(a, 4)?;
    // The new columns, named by the pivot values.
    let mut pivots: Vec<Rc<str>> = Vec::new();
    let mut position: HashMap<Rc<str>, usize> = HashMap::new();
    for s in &*list(a, 1)?.items {
        let name = match convert::to_text(&s.get()?, culture)? {
            Value::Text(s) => s,
            _ => continue,
        };
        if !position.contains_key(&name) {
            position.insert(name.clone(), pivots.len());
            pivots.push(name);
        }
    }
    check_len(pivots.len())?;
    let keep: Vec<usize> = (0..t.width()).filter(|i| *i != attr && *i != value).collect();
    // Per group of the other columns: its first row and the values under each pivot.
    let mut grouper = Grouper::new(Comparer::Ordinal);
    let mut groups: Vec<(Row, Vec<Vec<Slot>>)> = Vec::new();
    for r in &t.rows {
        let (g, new) = grouper.group(it, &key_of(r, &keep))?;
        if new {
            groups.push((r.clone(), vec![Vec::new(); pivots.len()]));
        }
        let name = match r.get(attr) {
            Some(Slot::Val(v)) => match convert::to_text(v, culture)? {
                Value::Text(s) => s,
                _ => continue,
            },
            _ => continue,
        };
        if let (Some(p), Some((_, cells))) = (position.get(&name), groups.get_mut(g))
            && let Some(c) = cells.get_mut(*p)
        {
            c.push(slot_at(r, value));
        }
    }
    check_size(groups.len(), keep.len() + pivots.len())?;
    // Rows come out in the order of the other columns' values, as Power Query gives them
    // (observed in Excel by the author of storytold/gridcraft#1532), not in the order met.
    let groups = try_sort_by(groups, |(a, _), (b, _)| {
        for i in &keep {
            let o = crate::value::compare_values(&slot_at(a, *i).get().unwrap_or(Value::Null), &slot_at(b, *i).get().unwrap_or(Value::Null));
            if o != std::cmp::Ordering::Equal {
                return Ok(o);
            }
        }
        Ok(std::cmp::Ordering::Equal)
    })?;
    let mut rows = Vec::with_capacity(groups.len());
    for (first, cells) in groups {
        let mut row: Vec<Slot> = keep.iter().map(|i| slot_at(&first, *i)).collect();
        for values in cells {
            row.push(match aggregate {
                Some(f) => Slot::capture(it.call1(f, Value::list(values)))?,
                None => match values.len() {
                    0 => Slot::NULL,
                    1 => values.into_iter().next().unwrap_or(Slot::NULL),
                    n => Slot::capture(raise(format!("{n} values fall in one cell of the pivot: give an aggregation function to combine them.")))?,
                },
            });
        }
        rows.push(row.into());
    }
    let columns = keep.iter().filter_map(|i| t.columns.get(*i).cloned()).chain(pivots).collect::<Vec<_>>();
    let mut types: Vec<Type> = keep.iter().filter_map(|i| t.types.get(*i).cloned()).collect();
    types.resize(columns.len(), Type::Any);
    make(columns, types, rows)
}

fn unpivot(a: &[Value], others: bool) -> EResult<Value> {
    let t = table(a, 0)?;
    let named = cols(&t, &names(arg(a, 1))?)?;
    let (attribute, value) = (text(a, 2)?, text(a, 3)?);
    let (keep, turn): (Vec<usize>, Vec<usize>) = if others {
        (named.clone(), (0..t.width()).filter(|i| !named.contains(i)).collect())
    } else {
        ((0..t.width()).filter(|i| !named.contains(i)).collect(), named)
    };
    let mut rows: Vec<Row> = Vec::new();
    for r in &t.rows {
        for i in &turn {
            let Some(cell) = r.get(*i) else { continue };
            if cell.is_null() {
                continue;
            }
            let name = t.columns.get(*i).cloned().unwrap_or_else(|| Rc::from(""));
            rows.push(keep.iter().map(|k| slot_at(r, *k)).chain([Slot::Val(Value::Text(name)), cell.clone()]).collect());
        }
        check_size(rows.len(), keep.len() + 2)?;
    }
    let columns = keep.iter().filter_map(|i| t.columns.get(*i).cloned()).chain([attribute, value]).collect();
    let types = keep.iter().filter_map(|i| t.types.get(*i).cloned()).chain([Type::Text, Type::Any]).collect();
    make(columns, types, rows)
}

fn split_column(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let source = text(a, 1)?;
    let at = col(&t, &source)?;
    let splitter = function(a, 2)?;
    let default = arg(a, 4).clone();
    // Split every cell first: without names, the widest split decides the number of columns.
    let mut parts: Vec<Result<Vec<Slot>, Slot>> = Vec::with_capacity(t.rows.len());
    for r in &t.rows {
        parts.push(match r.get(at).map(Slot::result) {
            Some(Ok(Value::Null)) | None => Ok(Vec::new()),
            Some(Ok(v)) => match Slot::capture(it.call1(splitter, v))? {
                Slot::Val(Value::List(l)) => Ok(l.items.clone()),
                Slot::Val(other) => Ok(vec![Slot::Val(other)]),
                e => Err(e),
            },
            Some(Err(e)) => Err(Slot::Err(e)),
        });
    }
    let new: Vec<Rc<str>> = match arg(a, 3) {
        Value::Null => {
            let widest = parts.iter().map(|p| p.as_ref().map_or(1, Vec::len)).max().unwrap_or(1).max(1);
            (1..=widest).map(|i| Rc::from(format!("{source}.{i}"))).collect()
        }
        Value::Number(_) => (1..=count(a, 3)?).map(|i| Rc::from(format!("{source}.{i}"))).collect(),
        v => names(v)?,
    };
    let n = new.len();
    let mut next = parts.into_iter();
    replace_column(&t, at, new, vec![Type::Any; n], |_| {
        Ok(vec![match next.next() {
            Some(Ok(items)) => (0..n).map(|i| items.get(i).cloned().unwrap_or_else(|| Slot::Val(default.clone()))).collect(),
            Some(Err(e)) => vec![e; n],
            None => vec![Slot::NULL; n],
        }])
    })
}

fn combine_columns(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let sources = cols(&t, &names(arg(a, 1))?)?;
    let combiner = function(a, 2)?;
    let name = text(a, 3)?;
    let Some(at) = sources.iter().copied().min() else { return raise("At least one column to combine is needed.") };
    let mut rows: Vec<Row> = Vec::with_capacity(t.rows.len());
    for r in &t.rows {
        let merged = Slot::capture(it.call1(combiner, Value::list(sources.iter().map(|i| slot_at(r, *i)).collect())))?;
        let mut cells = Vec::with_capacity(t.width());
        for (i, c) in r.iter().enumerate() {
            if i == at {
                cells.push(merged.clone());
            } else if !sources.contains(&i) {
                cells.push(c.clone());
            }
        }
        rows.push(cells.into());
    }
    let mut columns = Vec::new();
    let mut types = Vec::new();
    for (i, (c, ty)) in t.columns.iter().zip(t.types.iter()).enumerate() {
        if i == at {
            columns.push(name.clone());
            types.push(Type::Any);
        } else if !sources.contains(&i) {
            columns.push(c.clone());
            types.push(ty.clone());
        }
    }
    make(columns, types, rows)
}
