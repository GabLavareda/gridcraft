//! `Table.*`: building tables, reading them, and the row and column operations.
//! Grouping, joins, pivots and the other reshaping operations are in `table_shape`.

use std::cmp::Ordering;
use std::rc::Rc;

use super::keys::{Comparer, Grouper, try_sort_by};
use super::record::{Missing, missing, rename_pairs, transform_pairs};
use super::*;
use crate::error::{EResult, raise};
use crate::interp::{Interp, MAX_CELLS, check_len, index_of, table_column, too_large};
use crate::value::{Builtin, Field, ListData, Row, Slot, Table, Type, Value, compare_values};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "#table", 2, 2, new_table;
    "Table.FromRows", 1, 2, from_rows;
    "Table.FromRecords", 1, 3, from_records;
    "Table.FromColumns", 1, 2, from_columns;
    "Table.FromValue", 1, 2, from_value;
    "Table.ToRecords", 1, 1, to_records;
    "Table.ToRows", 1, 1, |_, a| Ok(Value::list_of(table(a, 0)?.rows.iter().map(|r| Value::list(r.to_vec())).collect()));
    "Table.ToColumns", 1, 1, |_, a| { let t = table(a, 0)?; Ok(Value::list_of((0..t.width()).map(|i| table_column(&t, i)).collect())) };
    "Table.Column", 2, 2, |_, a| { let t = table(a, 0)?; Ok(table_column(&t, col(&t, &text(a, 1)?)?)) };
    "Table.ColumnNames", 1, 1, |_, a| Ok(Value::list_of(table(a, 0)?.columns.iter().map(|n| Value::Text(n.clone())).collect()));
    "Table.ColumnCount", 1, 1, |_, a| Ok(Value::Number(table(a, 0)?.width() as f64));
    "Table.RowCount", 1, 1, |_, a| Ok(Value::Number(table(a, 0)?.rows.len() as f64));
    "Table.ApproximateRowCount", 1, 1, |_, a| Ok(Value::Number(table(a, 0)?.rows.len() as f64));
    "Table.IsEmpty", 1, 1, |_, a| Ok(Value::Logical(table(a, 0)?.rows.is_empty()));
    "Table.First", 1, 2, |_, a| edge(a, true);
    "Table.Last", 1, 2, |_, a| edge(a, false);
    "Table.FirstValue", 1, 2, first_value;
    "Table.HasColumns", 2, 2, |_, a| { let t = table(a, 0)?; Ok(Value::Logical(names(arg(a, 1))?.iter().all(|n| t.column_index(n).is_some()))) };
    "Table.Schema", 1, 1, schema;
    "Table.ColumnsOfType", 2, 2, columns_of_type;
    "Table.SelectRows", 2, 2, select_rows;
    "Table.FirstN", 2, 2, first_n;
    "Table.LastN", 2, 2, last_n;
    "Table.Skip", 1, 2, skip;
    "Table.RemoveFirstN", 1, 2, skip;
    "Table.RemoveLastN", 1, 2, remove_last_n;
    "Table.Range", 2, 3, range;
    "Table.RemoveRows", 2, 3, remove_rows;
    "Table.InsertRows", 3, 3, insert_rows;
    "Table.ReverseRows", 1, 1, |_, a| { let t = table(a, 0)?; Ok(with_rows(&t, t.rows.iter().rev().cloned().collect())) };
    "Table.AlternateRows", 4, 4, alternate_rows;
    "Table.Repeat", 2, 2, repeat;
    "Table.SelectColumns", 2, 3, select_columns;
    "Table.RemoveColumns", 2, 3, remove_columns;
    "Table.RenameColumns", 2, 3, rename_columns;
    "Table.ReorderColumns", 2, 3, reorder_columns;
    "Table.DuplicateColumn", 3, 4, duplicate_column;
    "Table.PrefixColumns", 2, 2, prefix_columns;
    "Table.TransformColumnNames", 2, 3, transform_column_names;
    "Table.AddColumn", 3, 4, add_column;
    "Table.AddIndexColumn", 2, 5, add_index_column;
    "Table.TransformColumns", 2, 4, transform_columns;
    "Table.TransformColumnTypes", 2, 3, transform_column_types;
    "Table.TransformRows", 2, 2, transform_rows;
    "Table.ReplaceValue", 5, 5, replace_value;
    "Table.ReplaceErrorValues", 2, 2, replace_error_values;
    "Table.RemoveRowsWithErrors", 1, 2, |_, a| rows_by_errors(a, false);
    "Table.SelectRowsWithErrors", 1, 2, |_, a| rows_by_errors(a, true);
    "Table.Sort", 2, 2, sort;
    "Table.Distinct", 1, 2, distinct;
    "Table.IsDistinct", 1, 2, is_distinct;
    "Table.Contains", 2, 3, contains;
    "Table.FillDown", 2, 2, |_, a| fill(a, false);
    "Table.FillUp", 2, 2, |_, a| fill(a, true);
    "Table.PromoteHeaders", 1, 2, promote_headers;
    "Table.DemoteHeaders", 1, 1, demote_headers;
    "Table.Transpose", 1, 2, transpose;
    "Table.Combine", 1, 2, |_, a| combine_arg(a);
    "Table.Buffer", 1, 2, |_, a| Ok(Value::Table(table(a, 0)?));
    "Table.StopFolding", 1, 1, |_, a| Ok(Value::Table(table(a, 0)?));
    "Table.Max", 2, 3, |it, a| extreme(it, a, Ordering::Greater);
    "Table.Min", 2, 3, |it, a| extreme(it, a, Ordering::Less);
    "Table.MatchesAllRows", 2, 2, |it, a| matches_rows(it, a, false);
    "Table.MatchesAnyRows", 2, 2, |it, a| matches_rows(it, a, true);
    "Table.FindText", 2, 2, find_text;
    "Table.ReplaceRows", 4, 4, replace_rows;
];

// ---------------------------------------------------------------- helpers

pub(crate) fn no_column<T>(name: &str) -> EResult<T> {
    raise(format!("The table has no column named '{name}'."))
}

/// The position of the column `name`.
pub(crate) fn col(t: &Table, name: &str) -> EResult<usize> {
    match t.column_index(name) {
        Some(i) => Ok(i),
        None => no_column(name),
    }
}

pub(crate) fn cols(t: &Table, names: &[Rc<str>]) -> EResult<Vec<usize>> {
    names.iter().map(|n| col(t, n)).collect()
}

/// Fails when a table of this shape would be too large.
pub(crate) fn check_size(rows: usize, width: usize) -> EResult<()> {
    check_len(rows)?;
    if rows.saturating_mul(width) > MAX_CELLS { too_large() } else { Ok(()) }
}

/// The same table with other rows.
pub(crate) fn with_rows(t: &Table, rows: Vec<Row>) -> Value {
    Value::table(Table { columns: t.columns.clone(), types: t.types.clone(), rows })
}

/// A table from its parts: column names must be unique.
pub(crate) fn make(columns: Vec<Rc<str>>, types: Vec<Type>, rows: Vec<Row>) -> EResult<Value> {
    for (i, n) in columns.iter().enumerate() {
        if columns.iter().skip(i + 1).any(|m| m == n) {
            return raise(format!("The table would have two columns named '{n}'."));
        }
    }
    check_size(rows.len(), columns.len())?;
    Ok(Value::table(Table { columns: columns.into(), types: types.into(), rows }))
}

fn untyped(columns: Vec<Rc<str>>, rows: Vec<Row>) -> EResult<Value> {
    let types = columns.iter().map(|_| Type::Any).collect();
    make(columns, types, rows)
}

pub(crate) fn default_names(n: usize) -> Vec<Rc<str>> {
    (1..=n).map(|i| Rc::from(format!("Column{i}"))).collect()
}

pub(crate) fn slot_at(row: &Row, i: usize) -> Slot {
    row.get(i).cloned().unwrap_or(Slot::NULL)
}

fn pick(row: &Row, idx: &[usize]) -> Row {
    idx.iter().map(|i| slot_at(row, *i)).collect()
}

/// Names and types from a columns argument: a list of names, a count, or a table type.
fn columns_spec(v: &Value, width: usize) -> EResult<(Vec<Rc<str>>, Vec<Type>)> {
    match v {
        Value::Null => Ok((default_names(width), vec![Type::Any; width])),
        Value::Number(n) => {
            let n = index_of(*n)?;
            Ok((default_names(n), vec![Type::Any; n]))
        }
        Value::Type(Type::Table(c)) => Ok((c.fields.iter().map(|f| f.name.clone()).collect(), c.fields.iter().map(|f| f.ty.clone()).collect())),
        v => {
            let n = names(v)?;
            let types = vec![Type::Any; n.len()];
            Ok((n, types))
        }
    }
}

/// Pads or checks a row against the table width.
fn fit_row(items: &[Slot], width: usize) -> EResult<Row> {
    if items.len() != width {
        return raise(format!("A row has {} value(s), but the table has {width} column(s).", items.len()));
    }
    Ok(items.iter().cloned().collect())
}

fn row_lists(l: &ListData) -> EResult<Vec<Rc<ListData>>> {
    l.items
        .iter()
        .map(|s| match s.get()? {
            Value::List(x) => Ok(x),
            v => mismatch(&v, "a list of the row's values"),
        })
        .collect()
}

// ---------------------------------------------------------------- building

fn new_table(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let rows = row_lists(&*list(a, 1)?)?;
    let width = rows.first().map_or(0, |r| r.len());
    let (columns, types) = columns_spec(arg(a, 0), width)?;
    let width = columns.len();
    let rows = rows.iter().map(|r| fit_row(&r.items, width)).collect::<EResult<Vec<Row>>>()?;
    make(columns, types, rows)
}

fn from_rows(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    new_table(it, &[arg(a, 1).clone(), arg(a, 0).clone()])
}

fn from_records(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let mode = missing(arg(a, 2))?;
    let mut records = Vec::with_capacity(l.len());
    for s in &l.items {
        match s.get()? {
            Value::Record(r) => records.push(r),
            v => return mismatch(&v, "a record"),
        }
    }
    let (columns, types) = match arg(a, 1) {
        Value::Null => {
            let names: Vec<Rc<str>> = records.first().map_or(Vec::new(), |r| r.names.to_vec());
            let types = vec![Type::Any; names.len()];
            (names, types)
        }
        v => columns_spec(v, 0)?,
    };
    // The usual case: the columns are the first record's fields, and the other records share
    // its names, so their values are rows as they are.
    let shared = match (arg(a, 1), records.first()) {
        (Value::Null, Some(first)) => Some(first.names.clone()),
        _ => None,
    };
    let mut rows = Vec::with_capacity(records.len());
    for r in &records {
        if shared.as_ref().is_some_and(|names| Rc::ptr_eq(names, &r.names)) {
            rows.push(r.values.clone());
            continue;
        }
        let mut row = Vec::with_capacity(columns.len());
        for c in &columns {
            match r.slot(c) {
                Some(s) => row.push(s.clone()),
                None if mode == Missing::Error => return raise(format!("A record has no field named '{c}'.")),
                None => row.push(Slot::NULL),
            }
        }
        rows.push(row.into());
    }
    make(columns, types, rows)
}

fn from_columns(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let lists = row_lists(&*list(a, 0)?)?;
    let (columns, types) = columns_spec(arg(a, 1), lists.len())?;
    if columns.len() != lists.len() {
        return raise("The number of column names doesn't match the number of columns.");
    }
    let height = lists.iter().map(|l| l.len()).max().unwrap_or(0);
    check_size(height, lists.len())?;
    let rows = (0..height).map(|i| lists.iter().map(|l| l.items.get(i).cloned().unwrap_or(Slot::NULL)).collect()).collect();
    make(columns, types, rows)
}

fn from_value(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match arg(a, 0) {
        v @ Value::Table(_) => Ok(v.clone()),
        Value::List(l) => untyped(vec![Rc::from("Value")], l.items.iter().map(|s| std::iter::once(s.clone()).collect()).collect()),
        Value::Record(r) => untyped(
            vec![Rc::from("Name"), Rc::from("Value")],
            r.iter().map(|(n, s)| [Slot::Val(Value::Text(n.clone())), s.clone()].into_iter().collect()).collect(),
        ),
        v => untyped(vec![Rc::from("Value")], vec![std::iter::once(Slot::Val(v.clone())).collect()]),
    }
}

// ---------------------------------------------------------------- reading

fn to_records(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    Ok(Value::list_of(t.rows.iter().map(|r| Value::record(t.columns.clone(), r.clone())).collect()))
}

fn edge(a: &[Value], first: bool) -> EResult<Value> {
    let t = table(a, 0)?;
    let i = if first { 0 } else { t.rows.len().wrapping_sub(1) };
    Ok(t.row_record(i).unwrap_or_else(|| arg(a, 1).clone()))
}

fn first_value(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    match t.rows.first().and_then(|r| r.first()) {
        Some(s) => s.get(),
        None => Ok(arg(a, 1).clone()),
    }
}

fn type_name(t: &Type) -> String {
    match t {
        Type::Number(crate::value::Facet::Double) => "Number.Type".to_string(),
        Type::Number(f) => format!("{f:?}.Type"),
        Type::Nullable(_) => type_name(&t.non_nullable()),
        other => format!("{}.Type", other.name()),
    }
}

fn schema(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let rows = t
        .columns
        .iter()
        .zip(t.types.iter())
        .enumerate()
        .map(|(i, (n, ty))| {
            [
                Value::Text(n.clone()),
                Value::Number(i as f64),
                Value::text(type_name(ty)),
                Value::text(ty.name().to_ascii_lowercase()),
                Value::Logical(ty.is_nullable()),
            ]
            .into_iter()
            .map(Slot::Val)
            .collect()
        })
        .collect();
    untyped(["Name", "Position", "TypeName", "Kind", "IsNullable"].into_iter().map(Rc::from).collect(), rows)
}

fn columns_of_type(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let mut wanted = Vec::new();
    for s in &*list(a, 1)?.items {
        match s.get()? {
            Value::Type(ty) => wanted.push(ty),
            v => return mismatch(&v, "a type"),
        }
    }
    let names = t.columns.iter().zip(t.types.iter()).filter(|(_, ty)| wanted.iter().any(|w| ty.equals(w))).map(|(n, _)| Value::Text(n.clone()));
    Ok(Value::list_of(names.collect()))
}

// ---------------------------------------------------------------- rows

fn select_rows(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let f = function(a, 1)?;
    let mut rows = Vec::new();
    for r in &t.rows {
        if it.test(f, Value::record(t.columns.clone(), r.clone()))? {
            rows.push(r.clone());
        }
    }
    Ok(with_rows(&t, rows))
}

/// How many leading rows a count-or-condition argument takes.
fn leading<'r>(it: &mut Interp, t: &Table, rows: impl Iterator<Item = &'r Row>, spec: &Value) -> EResult<usize> {
    match spec {
        Value::Number(n) => Ok(index_of(*n)?.min(t.rows.len())),
        f @ Value::Function(_) => {
            let mut n = 0;
            for r in rows {
                if !it.test(f, Value::record(t.columns.clone(), r.clone()))? {
                    break;
                }
                n += 1;
            }
            Ok(n)
        }
        v => mismatch(v, "a count or a condition"),
    }
}

fn first_n(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let n = leading(it, &t, t.rows.iter(), arg(a, 1))?;
    Ok(with_rows(&t, t.rows.iter().take(n).cloned().collect()))
}

fn skip(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let n = match arg(a, 1) {
        Value::Null => 1.min(t.rows.len()),
        spec => leading(it, &t, t.rows.iter(), spec)?,
    };
    Ok(with_rows(&t, t.rows.iter().skip(n).cloned().collect()))
}

fn trailing(it: &mut Interp, t: &Table, spec: &Value) -> EResult<usize> {
    match spec {
        Value::Null => Ok(1.min(t.rows.len())),
        spec => leading(it, t, t.rows.iter().rev(), spec),
    }
}

fn last_n(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let n = trailing(it, &t, arg(a, 1))?;
    Ok(with_rows(&t, t.rows.iter().skip(t.rows.len() - n).cloned().collect()))
}

fn remove_last_n(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let n = trailing(it, &t, arg(a, 1))?;
    Ok(with_rows(&t, t.rows.iter().take(t.rows.len() - n).cloned().collect()))
}

fn range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let n = opt_count(a, 2)?.unwrap_or(usize::MAX);
    Ok(with_rows(&t, t.rows.iter().skip(count(a, 1)?).take(n).cloned().collect()))
}

fn remove_rows(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (offset, n) = (count(a, 1)?, opt_count(a, 2)?.unwrap_or(1));
    if offset > t.rows.len() || n > t.rows.len() - offset {
        return raise("The rows to remove are outside the table.");
    }
    Ok(with_rows(&t, t.rows.iter().take(offset).chain(t.rows.iter().skip(offset + n)).cloned().collect()))
}

/// Rows given as records, matched to the table's columns.
fn rows_from_records(t: &Table, l: &ListData) -> EResult<Vec<Row>> {
    l.items
        .iter()
        .map(|s| match s.get()? {
            Value::Record(r) => t
                .columns
                .iter()
                .map(|c| r.slot(c).cloned().ok_or(c))
                .collect::<Result<Row, _>>()
                .or_else(|c| raise(format!("A row has no field named '{c}'."))),
            v => mismatch(&v, "a record"),
        })
        .collect()
}

fn insert_rows(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let offset = count(a, 1)?;
    if offset > t.rows.len() {
        return raise("The position is outside the table.");
    }
    let new = rows_from_records(&t, &*list(a, 2)?)?;
    check_size(t.rows.len() + new.len(), t.width())?;
    Ok(with_rows(&t, t.rows.iter().take(offset).cloned().chain(new).chain(t.rows.iter().skip(offset).cloned()).collect()))
}

fn replace_rows(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (offset, n) = (count(a, 1)?, count(a, 2)?);
    if offset > t.rows.len() || n > t.rows.len() - offset {
        return raise("The rows to replace are outside the table.");
    }
    let new = rows_from_records(&t, &*list(a, 3)?)?;
    check_size(t.rows.len() + new.len(), t.width())?;
    Ok(with_rows(&t, t.rows.iter().take(offset).cloned().chain(new).chain(t.rows.iter().skip(offset + n).cloned()).collect()))
}

/// `Table.AlternateRows(table, offset, skip, take)`: keeps `offset` rows, then repeatedly skips
/// `skip` rows and keeps `take`.
fn alternate_rows(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (offset, skip_n, take_n) = (count(a, 1)?, count(a, 2)?, count(a, 3)?);
    let cycle = skip_n + take_n;
    let rows = t.rows.iter().enumerate().filter(|(i, _)| *i < offset || (cycle > 0 && (i - offset) % cycle >= skip_n)).map(|(_, r)| r.clone());
    Ok(with_rows(&t, rows.collect()))
}

fn repeat(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let n = count(a, 1)?;
    check_size(t.rows.len().saturating_mul(n), t.width())?;
    let mut rows = Vec::with_capacity(t.rows.len() * n);
    for _ in 0..n {
        rows.extend(t.rows.iter().cloned());
    }
    Ok(with_rows(&t, rows))
}

// ---------------------------------------------------------------- columns

/// A table with the columns at `idx`, in that order.
pub(crate) fn project(t: &Table, idx: &[usize]) -> EResult<Value> {
    let columns = idx.iter().filter_map(|i| t.columns.get(*i).cloned()).collect();
    let types = idx.iter().filter_map(|i| t.types.get(*i).cloned()).collect();
    make(columns, types, t.rows.iter().map(|r| pick(r, idx)).collect())
}

fn select_columns(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let mode = missing(arg(a, 2))?;
    let wanted = names(arg(a, 1))?;
    if mode == Missing::UseNull && wanted.iter().any(|n| t.column_index(n).is_none()) {
        let idx: Vec<Option<usize>> = wanted.iter().map(|n| t.column_index(n)).collect();
        let types = idx.iter().map(|i| i.and_then(|i| t.types.get(i).cloned()).unwrap_or(Type::Any)).collect();
        let rows = t.rows.iter().map(|r| idx.iter().map(|i| i.map_or(Slot::NULL, |i| slot_at(r, i))).collect()).collect();
        return make(wanted, types, rows);
    }
    let mut idx = Vec::with_capacity(wanted.len());
    for n in &wanted {
        match t.column_index(n) {
            Some(i) => idx.push(i),
            None if mode == Missing::Error => return no_column(n),
            None => {}
        }
    }
    project(&t, &idx)
}

fn remove_columns(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let remove = names(arg(a, 1))?;
    if missing(arg(a, 2))? == Missing::Error
        && let Some(n) = remove.iter().find(|n| t.column_index(n).is_none())
    {
        return no_column(n);
    }
    let idx: Vec<usize> = (0..t.width()).filter(|i| t.columns.get(*i).is_some_and(|n| !remove.contains(n))).collect();
    project(&t, &idx)
}

fn rename_columns(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let mode = missing(arg(a, 2))?;
    let mut columns = t.columns.to_vec();
    for (old, new) in rename_pairs(arg(a, 1))? {
        match t.column_index(&old) {
            Some(i) => {
                if let Some(n) = columns.get_mut(i) {
                    *n = new;
                }
            }
            None if mode == Missing::Error => return no_column(&old),
            None => {}
        }
    }
    make(columns, t.types.to_vec(), t.rows.clone())
}

fn reorder_columns(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let order = names(arg(a, 1))?;
    if missing(arg(a, 2))? == Missing::Error
        && let Some(n) = order.iter().find(|n| t.column_index(n).is_none())
    {
        return no_column(n);
    }
    // The named columns take each other's places in the given order; the rest stay put.
    let present: Vec<usize> = order.iter().filter_map(|n| t.column_index(n)).collect();
    let mut next = present.iter();
    let idx: Vec<usize> = (0..t.width()).map(|i| if present.contains(&i) { next.next().copied().unwrap_or(i) } else { i }).collect();
    project(&t, &idx)
}

/// The table with one more column at the end.
pub(crate) fn append_column(t: &Table, name: Rc<str>, ty: Type, mut cell: impl FnMut(usize, &Row) -> EResult<Slot>) -> EResult<Value> {
    if t.column_index(&name).is_some() {
        return raise(format!("The table already has a column named '{name}'."));
    }
    check_size(t.rows.len(), t.width() + 1)?;
    let mut rows = Vec::with_capacity(t.rows.len());
    for (i, r) in t.rows.iter().enumerate() {
        let extra = cell(i, r)?;
        rows.push(r.iter().cloned().chain(std::iter::once(extra)).collect());
    }
    let mut columns = t.columns.to_vec();
    let mut types = t.types.to_vec();
    columns.push(name);
    types.push(ty);
    make(columns, types, rows)
}

fn opt_type(a: &[Value], i: usize) -> EResult<Type> {
    match arg(a, i) {
        Value::Null => Ok(Type::Any),
        Value::Type(t) => Ok(t.clone()),
        v => mismatch(v, "a type"),
    }
}

fn duplicate_column(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let i = col(&t, &text(a, 1)?)?;
    let ty = match arg(a, 3) {
        Value::Null => t.types.get(i).cloned().unwrap_or(Type::Any),
        _ => opt_type(a, 3)?,
    };
    append_column(&t, text(a, 2)?, ty, |_, r| Ok(slot_at(r, i)))
}

fn prefix_columns(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let prefix = text(a, 1)?;
    make(t.columns.iter().map(|n| Rc::from(format!("{prefix}.{n}"))).collect(), t.types.to_vec(), t.rows.clone())
}

fn transform_column_names(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let f = function(a, 1)?;
    let mut columns = Vec::with_capacity(t.width());
    for n in t.columns.iter() {
        match it.call1(f, Value::Text(n.clone()))? {
            Value::Text(s) => columns.push(s),
            v => return mismatch(&v, "text for the column name"),
        }
    }
    make(columns, t.types.to_vec(), t.rows.clone())
}

fn add_column(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let f = function(a, 2)?;
    let columns = t.columns.clone();
    append_column(&t, text(a, 1)?, opt_type(a, 3)?, |_, r| Slot::capture(it.call1(f, Value::record(columns.clone(), r.clone()))))
}

fn add_index_column(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (start, step) = (opt_number(a, 2)?.unwrap_or(0.0), opt_number(a, 3)?.unwrap_or(1.0));
    let ty = match arg(a, 4) {
        Value::Null => Type::Number(crate::value::Facet::Int64),
        _ => opt_type(a, 4)?,
    };
    append_column(&t, text(a, 1)?, ty, |i, _| Ok(Slot::Val(Value::Number(start + step * i as f64))))
}

/// Applies `f` to the cells of the columns `idx`; other cells are shared with the input.
fn map_cells(t: &Table, idx: &[usize], mut f: impl FnMut(usize, &Row, &Slot) -> EResult<Slot>) -> EResult<Vec<Row>> {
    let mut rows = Vec::with_capacity(t.rows.len());
    for r in &t.rows {
        let mut cells: Vec<Slot> = r.to_vec();
        for i in idx {
            if let Some(c) = cells.get_mut(*i) {
                *c = f(*i, r, c)?;
            }
        }
        rows.push(cells.into());
    }
    Ok(rows)
}

fn transform_columns(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let default = opt_function(a, 2)?;
    let mode = missing(arg(a, 3))?;
    // One function per column, or none.
    let mut fns: Vec<Option<Value>> = vec![default.cloned(); t.width()];
    let mut types = t.types.to_vec();
    if default.is_some() {
        types.iter_mut().for_each(|ty| *ty = Type::Any);
    }
    for (name, f, ty) in transform_pairs(arg(a, 1))? {
        match t.column_index(&name) {
            Some(i) => {
                if let Some(slot) = fns.get_mut(i) {
                    *slot = Some(f);
                }
                if let Some(slot) = types.get_mut(i) {
                    *slot = match ty {
                        Some(Value::Type(ty)) => ty,
                        _ => Type::Any,
                    };
                }
            }
            None if mode == Missing::Error => return no_column(&name),
            None => {}
        }
    }
    let idx: Vec<usize> = (0..t.width()).filter(|i| fns.get(*i).is_some_and(Option::is_some)).collect();
    let rows = map_cells(&t, &idx, |i, _, cell| match (cell, fns.get(i)) {
        (Slot::Val(v), Some(Some(f))) => Slot::capture(it.call1(f, v.clone())),
        _ => Ok(cell.clone()),
    })?;
    make(t.columns.to_vec(), types, rows)
}

/// A list of `{column, type}` pairs, or one such pair.
fn type_pairs(v: &Value) -> EResult<Vec<(Rc<str>, Type)>> {
    let Value::List(l) = v else { return mismatch(v, "a list of {column, type} pairs") };
    let pair = |vals: &[Value]| match vals {
        [Value::Text(name), Value::Type(ty)] => Ok((name.clone(), ty.clone())),
        _ => raise("Each type change is a list of a column name and a type."),
    };
    let vals = l.values()?;
    if matches!(vals.first(), Some(Value::Text(_))) {
        return Ok(vec![pair(&vals)?]);
    }
    vals.iter()
        .map(|v| match v {
            Value::List(p) => pair(&p.values()?),
            v => mismatch(v, "a {column, type} pair"),
        })
        .collect()
}

fn transform_column_types(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let culture_name = match arg(a, 2) {
        Value::Record(r) => match r.get("Culture")? {
            Some(Value::Text(s)) => Some(s),
            _ => None,
        },
        _ => opt_text(a, 2)?,
    };
    let culture = it.culture(culture_name.as_deref());
    let offset = it.local_offset();
    let mut targets: Vec<Option<Type>> = vec![None; t.width()];
    let mut types = t.types.to_vec();
    for (name, ty) in type_pairs(arg(a, 1))? {
        let i = col(&t, &name)?;
        if let Some(slot) = types.get_mut(i) {
            *slot = ty.clone();
        }
        if let Some(slot) = targets.get_mut(i) {
            *slot = Some(ty);
        }
    }
    let idx: Vec<usize> = (0..t.width()).filter(|i| targets.get(*i).is_some_and(Option::is_some)).collect();
    let rows = map_cells(&t, &idx, |i, _, cell| match (cell, targets.get(i)) {
        (Slot::Val(v), Some(Some(ty))) => Slot::capture(convert::convert(v, ty, culture, offset)),
        _ => Ok(cell.clone()),
    })?;
    make(t.columns.to_vec(), types, rows)
}

fn transform_rows(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let f = function(a, 1)?;
    let mut out = Vec::with_capacity(t.rows.len());
    for r in &t.rows {
        out.push(Slot::capture(it.call1(f, Value::record(t.columns.clone(), r.clone())))?);
    }
    Ok(Value::list(out))
}

fn replace_value(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let replacer = function(a, 3)?;
    let idx = cols(&t, &names(arg(a, 4))?)?;
    let (old, new) = (arg(a, 1), arg(a, 2));
    let columns = t.columns.clone();
    // The old and new values may be functions of the row.
    let rows = map_cells(&t, &idx, |_, row, cell| {
        let Slot::Val(v) = cell else { return Ok(cell.clone()) };
        let mut resolve = |x: &Value| match x {
            f @ Value::Function(_) => it.call1(f, Value::record(columns.clone(), row.clone())),
            x => Ok(x.clone()),
        };
        let (o, n) = (resolve(old)?, resolve(new)?);
        Slot::capture(it.call(replacer, &[v.clone(), o, n]))
    })?;
    Ok(with_rows(&t, rows))
}

fn replace_error_values(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let Value::List(l) = arg(a, 1) else { return mismatch(arg(a, 1), "a list of {column, value} pairs") };
    let vals = l.values()?;
    let pairs: Vec<Vec<Value>> = if matches!(vals.first(), Some(Value::Text(_))) {
        vec![vals]
    } else {
        vals.iter()
            .map(|v| match v {
                Value::List(p) => p.values(),
                v => mismatch(v, "a {column, value} pair"),
            })
            .collect::<EResult<_>>()?
    };
    let mut with: Vec<Option<Value>> = vec![None; t.width()];
    for p in &pairs {
        match p.as_slice() {
            [Value::Text(name), v] => {
                let i = col(&t, name)?;
                if let Some(slot) = with.get_mut(i) {
                    *slot = Some(v.clone());
                }
            }
            _ => return raise("Each replacement is a list of a column name and a value."),
        }
    }
    let idx: Vec<usize> = (0..t.width()).filter(|i| with.get(*i).is_some_and(Option::is_some)).collect();
    let rows = map_cells(&t, &idx, |i, _, cell| match (cell, with.get(i)) {
        (Slot::Err(_), Some(Some(v))) => Ok(Slot::Val(v.clone())),
        _ => Ok(cell.clone()),
    })?;
    Ok(with_rows(&t, rows))
}

fn rows_by_errors(a: &[Value], keep_errors: bool) -> EResult<Value> {
    let t = table(a, 0)?;
    let idx: Vec<usize> = match arg(a, 1) {
        Value::Null => (0..t.width()).collect(),
        v => cols(&t, &names(v)?)?,
    };
    let rows = t.rows.iter().filter(|r| idx.iter().any(|i| r.get(*i).is_some_and(Slot::is_error)) == keep_errors).cloned().collect();
    Ok(with_rows(&t, rows))
}

// ---------------------------------------------------------------- order and identity

/// Orders cells for sorting: errors come last.
fn compare_slots(a: &Slot, b: &Slot) -> Ordering {
    match (a, b) {
        (Slot::Val(x), Slot::Val(y)) => compare_values(x, y),
        (Slot::Err(_), Slot::Err(_)) => Ordering::Equal,
        (Slot::Err(_), _) => Ordering::Greater,
        (_, Slot::Err(_)) => Ordering::Less,
    }
}

/// The sort criteria of `Table.Sort`: `"A"`, `{"A", "B"}`, `{"A", Order.Descending}` or
/// `{{"A", Order.Descending}, {"B", Order.Ascending}}`.
fn sort_columns(t: &Table, v: &Value) -> EResult<Vec<(usize, bool)>> {
    let one =
        |name: &str, order: Option<&Value>| -> EResult<(usize, bool)> { Ok((col(t, name)?, matches!(order, Some(Value::Number(n)) if *n == 1.0))) };
    match v {
        Value::Text(name) => Ok(vec![one(name, None)?]),
        Value::List(l) => {
            let vals = l.values()?;
            if let [Value::Text(name), order @ Value::Number(_)] = vals.as_slice() {
                return Ok(vec![one(name, Some(order))?]);
            }
            vals.iter()
                .map(|v| match v {
                    Value::Text(name) => one(name, None),
                    Value::List(p) => match p.values()?.as_slice() {
                        [Value::Text(name)] => one(name, None),
                        [Value::Text(name), order] => one(name, Some(order)),
                        _ => raise("Each sort criterion is a column name, optionally with an order."),
                    },
                    v => mismatch(v, "a column name or a {column, order} pair"),
                })
                .collect()
        }
        v => mismatch(v, "the columns to sort by"),
    }
}

fn sort(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    // A function gives the sort key of each row.
    if let f @ Value::Function(_) = arg(a, 1) {
        let mut keyed = Vec::with_capacity(t.rows.len());
        for r in &t.rows {
            keyed.push((it.call1(f, Value::record(t.columns.clone(), r.clone()))?, r.clone()));
        }
        let sorted = try_sort_by(keyed, |x, y| Ok(compare_values(&x.0, &y.0)))?;
        return Ok(with_rows(&t, sorted.into_iter().map(|(_, r)| r).collect()));
    }
    let by = sort_columns(&t, arg(a, 1))?;
    let sorted = try_sort_by(t.rows.clone(), |x, y| {
        for (i, descending) in &by {
            let o = match (x.get(*i), y.get(*i)) {
                (Some(p), Some(q)) => compare_slots(p, q),
                _ => Ordering::Equal,
            };
            if o != Ordering::Equal {
                return Ok(if *descending { o.reverse() } else { o });
            }
        }
        Ok(Ordering::Equal)
    })?;
    Ok(with_rows(&t, sorted))
}

/// The key columns and comparer of an equation-criteria argument: null (every column), a column
/// name, a list of names, or `{name, comparer}` pairs.
pub(crate) fn key_columns(t: &Table, v: &Value) -> EResult<(Vec<usize>, Comparer)> {
    let all = || (0..t.width()).collect::<Vec<usize>>();
    match v {
        Value::Null => Ok((all(), Comparer::Ordinal)),
        Value::Text(name) => Ok((vec![col(t, name)?], Comparer::Ordinal)),
        f @ Value::Function(_) => Ok((all(), Comparer::from(f)?)),
        Value::List(l) => {
            let vals = l.values()?;
            let mut idx = Vec::new();
            let mut comparer = Comparer::Ordinal;
            let mut items: &[Value] = &vals;
            // `{"A", comparer}` is one pair, not two columns.
            if let [Value::Text(name), f @ Value::Function(_)] = items {
                idx.push(col(t, name)?);
                comparer = Comparer::from(f)?;
                items = &[];
            }
            for v in items {
                match v {
                    Value::Text(name) => idx.push(col(t, name)?),
                    Value::List(p) => match p.values()?.as_slice() {
                        [Value::Text(name)] => idx.push(col(t, name)?),
                        [Value::Text(name), f] => {
                            idx.push(col(t, name)?);
                            comparer = Comparer::from(f)?;
                        }
                        _ => return raise("Each criterion is a column name, optionally with a comparer."),
                    },
                    v => return mismatch(v, "a column name"),
                }
            }
            Ok((idx, comparer))
        }
        v => mismatch(v, "the columns to compare"),
    }
}

pub(crate) fn key_of<'r>(row: &'r Row, idx: &[usize]) -> Vec<&'r Slot> {
    idx.iter().filter_map(|i| row.get(*i)).collect()
}

fn distinct(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (idx, comparer) = key_columns(&t, arg(a, 1))?;
    let mut groups = Grouper::new(comparer);
    let mut rows = Vec::new();
    for r in &t.rows {
        if groups.group(it, &key_of(r, &idx))?.1 {
            rows.push(r.clone());
        }
    }
    Ok(with_rows(&t, rows))
}

fn is_distinct(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (idx, comparer) = key_columns(&t, arg(a, 1))?;
    let mut groups = Grouper::new(comparer);
    for r in &t.rows {
        if !groups.group(it, &key_of(r, &idx))?.1 {
            return Ok(Value::Logical(false));
        }
    }
    Ok(Value::Logical(true))
}

fn contains(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let want = record(a, 1)?;
    let comparer = Comparer::from(match arg(a, 2) {
        f @ Value::Function(_) => f,
        _ => &Value::Null,
    })?;
    let mut idx = Vec::with_capacity(want.len());
    for (n, s) in want.iter() {
        idx.push((col(&t, n)?, s.get()?));
    }
    for r in &t.rows {
        let mut all = true;
        for (i, w) in &idx {
            let same = match r.get(*i) {
                Some(Slot::Val(v)) => comparer.equal(it, v, w)?,
                _ => false,
            };
            if !same {
                all = false;
                break;
            }
        }
        if all {
            return Ok(Value::Logical(true));
        }
    }
    Ok(Value::Logical(false))
}

fn fill(a: &[Value], up: bool) -> EResult<Value> {
    let t = table(a, 0)?;
    let idx = cols(&t, &names(arg(a, 1))?)?;
    let mut last: Vec<Option<Slot>> = vec![None; t.width()];
    let mut rows: Vec<Row> = Vec::with_capacity(t.rows.len());
    let order: Box<dyn Iterator<Item = &Row>> = if up { Box::new(t.rows.iter().rev()) } else { Box::new(t.rows.iter()) };
    for r in order {
        let mut cells: Option<Vec<Slot>> = None;
        for i in &idx {
            let (Some(cell), Some(prev)) = (r.get(*i), last.get_mut(*i)) else { continue };
            if cell.is_null() {
                if let Some(p) = prev
                    && let Some(c) = cells.get_or_insert_with(|| r.to_vec()).get_mut(*i)
                {
                    *c = p.clone();
                }
            } else {
                *prev = Some(cell.clone());
            }
        }
        rows.push(match cells {
            Some(c) => c.into(),
            None => r.clone(),
        });
    }
    if up {
        rows.reverse();
    }
    Ok(with_rows(&t, rows))
}

// ---------------------------------------------------------------- headers and shape

/// Makes `names` unique by adding `_1`, `_2`… to repeats.
pub(crate) fn dedupe(names: Vec<String>) -> Vec<Rc<str>> {
    let mut out: Vec<Rc<str>> = Vec::with_capacity(names.len());
    for n in names {
        let mut candidate = n.clone();
        let mut k = 0;
        while out.iter().any(|m| **m == *candidate) {
            k += 1;
            candidate = format!("{n}_{k}");
        }
        out.push(Rc::from(candidate));
    }
    out
}

fn promote_headers(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let options = opt_record(a, 1)?;
    let all_scalars = matches!(option(options.as_ref(), "PromoteAllScalars")?, Value::Logical(true));
    let culture_name = match option(options.as_ref(), "Culture")? {
        Value::Text(s) => Some(s),
        _ => None,
    };
    let culture = it.culture(culture_name.as_deref());
    let Some(first) = t.rows.first() else { return Ok(Value::Table(t)) };
    let mut names = Vec::with_capacity(t.width());
    for (i, cell) in first.iter().enumerate() {
        let fallback = || format!("Column{}", i + 1);
        names.push(match cell {
            Slot::Val(Value::Text(s)) if !s.is_empty() => s.to_string(),
            Slot::Val(v @ (Value::Number(_) | Value::Logical(_) | Value::Date(_) | Value::DateTime(_) | Value::Time(_) | Value::Duration(_)))
                if all_scalars =>
            {
                match convert::to_text(v, culture)? {
                    Value::Text(s) => s.to_string(),
                    _ => fallback(),
                }
            }
            _ => fallback(),
        });
    }
    untyped(dedupe(names), t.rows.iter().skip(1).cloned().collect())
}

fn demote_headers(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    check_size(t.rows.len() + 1, t.width())?;
    let header: Row = t.columns.iter().map(|n| Slot::Val(Value::Text(n.clone()))).collect();
    untyped(default_names(t.width()), std::iter::once(header).chain(t.rows.iter().cloned()).collect())
}

fn transpose(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let height = t.rows.len();
    check_size(t.width(), height)?;
    let rows = (0..t.width()).map(|c| t.rows.iter().map(|r| slot_at(r, c)).collect()).collect();
    let (columns, types) = columns_spec(arg(a, 1), height)?;
    if columns.len() != height {
        return raise("The number of column names doesn't match the number of rows being transposed.");
    }
    make(columns, types, rows)
}

/// Appends tables: the columns of all of them, in order of first appearance; a table without a
/// column has nulls there.
pub(crate) fn combine(tables: &[Rc<Table>]) -> EResult<Value> {
    let mut columns: Vec<Rc<str>> = Vec::new();
    let mut types: Vec<Type> = Vec::new();
    for t in tables {
        for (n, ty) in t.columns.iter().zip(t.types.iter()) {
            match columns.iter().position(|c| c == n) {
                // A column keeps its type only when every table agrees on it.
                Some(i) => {
                    if let Some(known) = types.get_mut(i)
                        && !known.equals(ty)
                    {
                        *known = Type::Any;
                    }
                }
                None => {
                    columns.push(n.clone());
                    types.push(ty.clone());
                }
            }
        }
    }
    let total = tables.iter().map(|t| t.rows.len()).fold(0usize, usize::saturating_add);
    check_size(total, columns.len())?;
    let mut rows = Vec::with_capacity(total);
    for t in tables {
        if t.columns.len() == columns.len() && t.columns.iter().zip(&columns).all(|(x, y)| x == y) {
            rows.extend(t.rows.iter().cloned());
            continue;
        }
        let map: Vec<Option<usize>> = columns.iter().map(|c| t.column_index(c)).collect();
        rows.extend(t.rows.iter().map(|r| map.iter().map(|i| i.map_or(Slot::NULL, |i| slot_at(r, i))).collect::<Row>()));
    }
    make(columns, types, rows)
}

fn combine_arg(a: &[Value]) -> EResult<Value> {
    let mut tables = Vec::new();
    for s in &*list(a, 0)?.items {
        match s.get()? {
            Value::Table(t) => tables.push(t),
            v => return mismatch(&v, "a table"),
        }
    }
    let combined = combine(&tables)?;
    match (arg(a, 1), &combined) {
        (Value::Null, _) => Ok(combined),
        (spec, Value::Table(t)) => {
            let wanted = match spec {
                Value::Type(Type::Table(c)) => c.fields.iter().map(|f: &Field| f.name.clone()).collect(),
                v => names(v)?,
            };
            let idx = cols(t, &wanted)?;
            project(t, &idx)
        }
        _ => Ok(combined),
    }
}

/// The row with the largest or smallest value in a column, or by a key function.
fn extreme(it: &mut Interp, a: &[Value], want: Ordering) -> EResult<Value> {
    let t = table(a, 0)?;
    let mut best: Option<(Value, usize)> = None;
    for (i, r) in t.rows.iter().enumerate() {
        let key = match arg(a, 1) {
            Value::Text(name) => slot_at(r, col(&t, name)?).get()?,
            f @ Value::Function(_) => it.call1(f, Value::record(t.columns.clone(), r.clone()))?,
            v => return mismatch(v, "a column name or a function"),
        };
        if key.is_null() {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| compare_values(&key, b) == want) {
            best = Some((key, i));
        }
    }
    Ok(best.and_then(|(_, i)| t.row_record(i)).unwrap_or_else(|| arg(a, 2).clone()))
}

fn matches_rows(it: &mut Interp, a: &[Value], any: bool) -> EResult<Value> {
    let t = table(a, 0)?;
    let f = function(a, 1)?;
    for r in &t.rows {
        if it.test(f, Value::record(t.columns.clone(), r.clone()))? == any {
            return Ok(Value::Logical(any));
        }
    }
    Ok(Value::Logical(!any))
}

fn find_text(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let needle = text(a, 1)?.to_lowercase();
    let rows =
        t.rows.iter().filter(|r| r.iter().any(|s| matches!(s, Slot::Val(Value::Text(x)) if x.to_lowercase().contains(&needle)))).cloned().collect();
    Ok(with_rows(&t, rows))
}
