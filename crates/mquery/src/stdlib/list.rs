//! `List.*`.

use std::cmp::Ordering;
use std::rc::Rc;

use super::keys::{Comparer, Grouper, try_sort_by};
use super::*;
use crate::datetime::TICKS_PER_DAY;
use crate::error::{EResult, raise};
use crate::interp::{Interp, check_len, index_of};
use crate::ops;
use crate::value::{Builtin, ListData, Slot, Value, compare_values};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "List.Count", 1, 1, |_, a| Ok(Value::Number(list(a, 0)?.len() as f64));
    "List.IsEmpty", 1, 1, |_, a| Ok(Value::Logical(list(a, 0)?.is_empty()));
    "List.NonNullCount", 1, 1, |_, a| Ok(Value::Number(list(a, 0)?.items.iter().filter(|s| !s.is_null()).count() as f64));
    "List.First", 1, 2, |_, a| edge(a, true);
    "List.Last", 1, 2, |_, a| edge(a, false);
    "List.FirstN", 2, 2, first_n;
    "List.LastN", 1, 2, last_n;
    "List.Skip", 1, 2, skip;
    "List.RemoveFirstN", 1, 2, skip;
    "List.RemoveLastN", 1, 2, remove_last_n;
    "List.Range", 2, 3, range;
    "List.Reverse", 1, 1, |_, a| Ok(Value::list(list(a, 0)?.items.iter().rev().cloned().collect()));
    "List.Sum", 1, 2, sum;
    "List.Product", 1, 2, product;
    "List.Average", 1, 2, average;
    "List.Min", 1, 4, |it, a| extreme(it, a, Ordering::Less);
    "List.Max", 1, 4, |it, a| extreme(it, a, Ordering::Greater);
    "List.MinN", 2, 4, |it, a| extreme_n(it, a, false);
    "List.MaxN", 2, 4, |it, a| extreme_n(it, a, true);
    "List.Median", 1, 2, median;
    "List.Mode", 1, 2, |it, a| modes(it, a, false);
    "List.Modes", 1, 2, |it, a| modes(it, a, true);
    "List.StandardDeviation", 1, 1, standard_deviation;
    "List.RemoveNulls", 1, 1, |_, a| Ok(Value::list(list(a, 0)?.items.iter().filter(|s| !s.is_null()).cloned().collect()));
    "List.Distinct", 1, 2, distinct;
    "List.IsDistinct", 1, 2, is_distinct;
    "List.Contains", 2, 3, contains;
    "List.ContainsAny", 2, 3, |it, a| contains_some(it, a, false);
    "List.ContainsAll", 2, 3, |it, a| contains_some(it, a, true);
    "List.PositionOf", 2, 4, position_of;
    "List.PositionOfAny", 2, 4, position_of_any;
    "List.Positions", 1, 1, |_, a| Ok(Value::list_of((0..list(a, 0)?.len()).map(|i| Value::Number(i as f64)).collect()));
    "List.Transform", 2, 2, transform;
    "List.TransformMany", 3, 3, transform_many;
    "List.Select", 2, 2, select;
    "List.Accumulate", 3, 3, accumulate;
    "List.Generate", 3, 4, generate;
    "List.Numbers", 2, 3, numbers;
    "List.Dates", 3, 3, |_, a| series(a);
    "List.DateTimes", 3, 3, |_, a| series(a);
    "List.Times", 3, 3, |_, a| series(a);
    "List.Durations", 3, 3, |_, a| series(a);
    "List.Random", 1, 2, random;
    "List.Repeat", 2, 2, repeat;
    "List.Combine", 1, 1, combine;
    "List.Zip", 1, 1, zip;
    "List.Sort", 1, 2, sort;
    "List.Union", 1, 2, union;
    "List.Intersect", 1, 2, intersect;
    "List.Difference", 2, 3, difference;
    "List.RemoveItems", 2, 2, |it, a| remove_matching(it, a);
    "List.RemoveMatchingItems", 2, 3, |it, a| remove_matching(it, a);
    "List.ReplaceMatchingItems", 2, 3, replace_matching;
    "List.ReplaceValue", 4, 4, replace_value;
    "List.ReplaceRange", 4, 4, replace_range;
    "List.RemoveRange", 2, 3, remove_range;
    "List.InsertRange", 3, 3, insert_range;
    "List.AnyTrue", 1, 1, |_, a| all_or_any(a, true);
    "List.AllTrue", 1, 1, |_, a| all_or_any(a, false);
    "List.MatchesAny", 2, 2, |it, a| matches(it, a, true);
    "List.MatchesAll", 2, 2, |it, a| matches(it, a, false);
    "List.Single", 1, 1, |_, a| single(a, false);
    "List.SingleOrDefault", 1, 2, |_, a| single(a, true);
    "List.FindText", 2, 2, find_text;
    "List.Split", 2, 2, split;
    "List.Alternate", 2, 4, alternate;
    "List.Buffer", 1, 2, |_, a| Ok(Value::List(list(a, 0)?));
];

fn edge(a: &[Value], first: bool) -> EResult<Value> {
    let l = list(a, 0)?;
    match if first { l.items.first() } else { l.items.last() } {
        Some(s) => s.get(),
        None => Ok(arg(a, 1).clone()),
    }
}

/// How many leading items a count-or-condition argument takes: a number, or a predicate that
/// holds for them.
fn leading(it: &mut Interp, items: &[Slot], spec: &Value) -> EResult<usize> {
    match spec {
        Value::Number(n) => Ok(index_of(*n)?.min(items.len())),
        f @ Value::Function(_) => {
            let mut n = 0;
            for s in items {
                if !it.test(f, s.get()?)? {
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
    let l = list(a, 0)?;
    let n = leading(it, &l.items, arg(a, 1))?;
    Ok(Value::list(l.items.iter().take(n).cloned().collect()))
}

fn skip(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let n = match arg(a, 1) {
        Value::Null => 1.min(l.len()),
        spec => leading(it, &l.items, spec)?,
    };
    Ok(Value::list(l.items.iter().skip(n).cloned().collect()))
}

/// How many trailing items a count-or-condition argument takes.
fn trailing(it: &mut Interp, l: &ListData, spec: &Value) -> EResult<usize> {
    match spec {
        Value::Null => Ok(1.min(l.len())),
        spec => {
            let reversed: Vec<Slot> = l.items.iter().rev().cloned().collect();
            leading(it, &reversed, spec)
        }
    }
}

fn last_n(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let n = trailing(it, &l, arg(a, 1))?;
    Ok(Value::list(l.items.iter().skip(l.len() - n).cloned().collect()))
}

fn remove_last_n(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let n = trailing(it, &l, arg(a, 1))?;
    Ok(Value::list(l.items.iter().take(l.len() - n).cloned().collect()))
}

fn range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let offset = count(a, 1)?;
    let n = opt_count(a, 2)?.unwrap_or(usize::MAX);
    Ok(Value::list(l.items.iter().skip(offset).take(n).cloned().collect()))
}

/// The items that aren't null, as values.
fn non_null(l: &ListData) -> EResult<Vec<Value>> {
    let mut out = Vec::with_capacity(l.len());
    for s in &l.items {
        match s.get()? {
            Value::Null => {}
            v => out.push(v),
        }
    }
    Ok(out)
}

/// The non-null items as numbers.
fn numbers_of(l: &ListData) -> EResult<Vec<f64>> {
    non_null(l)?
        .into_iter()
        .map(|v| match v {
            Value::Number(n) => Ok(n),
            v => mismatch(&v, "a number"),
        })
        .collect()
}

fn sum(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let mut total: Option<Value> = None;
    for v in non_null(&l)? {
        total = Some(match total {
            None => v,
            Some(t) => ops::binary(crate::ast::BinOp::Add, &t, &v)?,
        });
    }
    Ok(total.unwrap_or(Value::Null))
}

fn product(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let ns = numbers_of(&*list(a, 0)?)?;
    Ok(if ns.is_empty() { Value::Null } else { Value::Number(ns.iter().product()) })
}

fn average(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let vals = non_null(&*list(a, 0)?)?;
    let n = vals.len();
    let mut total: Option<Value> = None;
    for v in vals {
        total = Some(match total {
            None => v,
            Some(t) => ops::binary(crate::ast::BinOp::Add, &t, &v)?,
        });
    }
    match total {
        None => Ok(Value::Null),
        // Dates average through their distance from the first.
        Some(t) => ops::binary(crate::ast::BinOp::Div, &t, &Value::Number(n as f64)),
    }
}

fn extreme(it: &mut Interp, a: &[Value], want: Ordering) -> EResult<Value> {
    let l = list(a, 0)?;
    let cmp = Comparer::from(arg(a, 2))?;
    let mut best: Option<Value> = None;
    for v in non_null(&l)? {
        best = Some(match best {
            None => v,
            Some(b) => {
                if cmp.compare(it, &v, &b)? == want {
                    v
                } else {
                    b
                }
            }
        });
    }
    Ok(best.unwrap_or_else(|| arg(a, 1).clone()))
}

fn extreme_n(it: &mut Interp, a: &[Value], largest: bool) -> EResult<Value> {
    let l = list(a, 0)?;
    let cmp = Comparer::from(arg(a, 2))?;
    let mut vals = try_sort_by(non_null(&l)?, |x, y| cmp.compare(it, x, y))?;
    if largest {
        vals.reverse();
    }
    let slots: Vec<Slot> = vals.into_iter().map(Slot::Val).collect();
    let n = leading(it, &slots, arg(a, 1))?;
    Ok(Value::list(slots.into_iter().take(n).collect()))
}

fn median(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let mut vals = non_null(&*list(a, 0)?)?;
    vals.sort_by(compare_values);
    let n = vals.len();
    Ok(match (n, vals.get(n / 2), vals.get(n.saturating_sub(1) / 2)) {
        (0, _, _) => Value::Null,
        (_, Some(Value::Number(hi)), Some(Value::Number(lo))) => Value::Number((hi + lo) / 2.0),
        (_, _, Some(lo)) => lo.clone(),
        _ => Value::Null,
    })
}

fn modes(it: &mut Interp, a: &[Value], all: bool) -> EResult<Value> {
    let l = list(a, 0)?;
    let mut groups = Grouper::new(Comparer::from(arg(a, 1))?);
    let mut counts: Vec<(usize, Value)> = Vec::new();
    for s in l.items.iter().filter(|s| !s.is_null()) {
        let (g, new) = groups.group(it, &[s])?;
        if new {
            counts.push((0, s.get()?));
        }
        if let Some(c) = counts.get_mut(g) {
            c.0 += 1;
        }
    }
    let top = counts.iter().map(|c| c.0).max().unwrap_or(0);
    let mut best = counts.into_iter().filter(|c| c.0 == top).map(|c| c.1);
    if all {
        return Ok(Value::list_of(best.collect()));
    }
    // The last of the most frequent values.
    Ok(best.next_back().unwrap_or(Value::Null))
}

fn standard_deviation(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let ns = numbers_of(&*list(a, 0)?)?;
    let n = ns.len() as f64;
    if ns.len() < 2 {
        return Ok(Value::Null);
    }
    let mean = ns.iter().sum::<f64>() / n;
    let var = ns.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Ok(Value::Number(var.sqrt()))
}

fn distinct(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let mut groups = Grouper::new(Comparer::from(arg(a, 1))?);
    let mut out = Vec::new();
    for s in &l.items {
        if groups.group(it, &[s])?.1 {
            out.push(s.clone());
        }
    }
    Ok(Value::list(out))
}

fn is_distinct(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let mut groups = Grouper::new(Comparer::from(arg(a, 1))?);
    for s in &l.items {
        if !groups.group(it, &[s])?.1 {
            return Ok(Value::Logical(false));
        }
    }
    Ok(Value::Logical(true))
}

/// The groups of the items of `l` under `cmp`.
fn index(it: &mut Interp, l: &ListData, cmp: Comparer) -> EResult<Grouper> {
    let mut groups = Grouper::new(cmp);
    for s in &l.items {
        groups.group(it, &[s])?;
    }
    Ok(groups)
}

fn contains(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let cmp = Comparer::from(arg(a, 2))?;
    let want = arg(a, 1);
    for s in &l.items {
        if cmp.equal(it, &s.get()?, want)? {
            return Ok(Value::Logical(true));
        }
    }
    Ok(Value::Logical(false))
}

fn contains_some(it: &mut Interp, a: &[Value], all: bool) -> EResult<Value> {
    let (l, wanted) = (list(a, 0)?, list(a, 1)?);
    let mut have = index(it, &l, Comparer::from(arg(a, 2))?)?;
    for s in &wanted.items {
        let found = have.find(it, &[s])?.is_some();
        if found != all {
            return Ok(Value::Logical(found));
        }
    }
    Ok(Value::Logical(all))
}

/// Picks from positions by an occurrence argument: first (default), last, or all as a list.
fn pick(found: Vec<usize>, occurrence: &Value) -> EResult<Value> {
    let pos = |p: Option<&usize>| Value::Number(p.map_or(-1.0, |p| *p as f64));
    match occurrence {
        Value::Null => Ok(pos(found.first())),
        Value::Number(n) if *n == 0.0 => Ok(pos(found.first())),
        Value::Number(n) if *n == 1.0 => Ok(pos(found.last())),
        Value::Number(n) if *n == 2.0 => Ok(Value::list_of(found.into_iter().map(|p| Value::Number(p as f64)).collect())),
        v => mismatch(v, "an occurrence (first, last or all)"),
    }
}

fn position_of(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let cmp = Comparer::from(arg(a, 3))?;
    let want = arg(a, 1);
    let mut found = Vec::new();
    for (i, s) in l.items.iter().enumerate() {
        if cmp.equal(it, &s.get()?, want)? {
            found.push(i);
        }
    }
    pick(found, arg(a, 2))
}

fn position_of_any(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (l, wanted) = (list(a, 0)?, list(a, 1)?);
    let mut have = index(it, &wanted, Comparer::from(arg(a, 3))?)?;
    let mut found = Vec::new();
    for (i, s) in l.items.iter().enumerate() {
        if have.find(it, &[s])?.is_some() {
            found.push(i);
        }
    }
    pick(found, arg(a, 2))
}

fn transform(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let f = function(a, 1)?;
    let mut out = Vec::with_capacity(l.len());
    for s in &l.items {
        out.push(match s {
            Slot::Val(v) => Slot::capture(it.call1(f, v.clone()))?,
            Slot::Err(_) => s.clone(),
        });
    }
    Ok(Value::list(out))
}

fn transform_many(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let (collection, result) = (function(a, 1)?, function(a, 2)?);
    let mut out = Vec::new();
    for s in &l.items {
        let x = s.get()?;
        let Value::List(inner) = it.call1(collection, x.clone())? else { return raise("The collection function should give a list.") };
        for y in &inner.items {
            out.push(Slot::capture(it.call(result, &[x.clone(), y.get()?]))?);
        }
        check_len(out.len())?;
    }
    Ok(Value::list(out))
}

fn select(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let f = function(a, 1)?;
    let mut out = Vec::new();
    for s in &l.items {
        if it.test(f, s.get()?)? {
            out.push(s.clone());
        }
    }
    Ok(Value::list(out))
}

fn accumulate(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let f = function(a, 2)?;
    let mut state = arg(a, 1).clone();
    for s in &l.items {
        state = it.call(f, &[state, s.get()?])?;
    }
    Ok(state)
}

fn generate(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (initial, condition, next) = (function(a, 0)?, function(a, 1)?, function(a, 2)?);
    let selector = opt_function(a, 3)?;
    let mut out = Vec::new();
    let mut state = it.call(initial, &[])?;
    while it.test(condition, state.clone())? {
        out.push(match selector {
            Some(f) => Slot::capture(it.call1(f, state.clone()))?,
            None => Slot::Val(state.clone()),
        });
        check_len(out.len())?;
        state = it.call1(next, state)?;
    }
    Ok(Value::list(out))
}

fn numbers(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (start, n) = (number(a, 0)?, count(a, 1)?);
    let step = opt_number(a, 2)?.unwrap_or(1.0);
    check_len(n)?;
    Ok(Value::list_of((0..n).map(|i| Value::Number(start + step * i as f64)).collect()))
}

/// `List.Dates`, `List.DateTimes`, `List.Times`, `List.Durations`: `count` values from `start`,
/// each `step` (a duration) after the one before.
fn series(a: &[Value]) -> EResult<Value> {
    let n = count(a, 1)?;
    check_len(n)?;
    let Value::Duration(step) = arg(a, 2) else { return mismatch(arg(a, 2), "a duration") };
    let mut out = Vec::with_capacity(n);
    let mut cur = arg(a, 0).clone();
    if !matches!(cur, Value::Date(_) | Value::DateTime(_) | Value::Time(_) | Value::Duration(_) | Value::DateTimeZone(..)) {
        return mismatch(&cur, "a date, time or duration");
    }
    // Dates step by whole days.
    if matches!(cur, Value::Date(_)) && step % TICKS_PER_DAY != 0 {
        return raise("A list of dates needs a step of whole days.");
    }
    for _ in 0..n {
        out.push(Slot::Val(cur.clone()));
        cur = ops::binary(crate::ast::BinOp::Add, &cur, &Value::Duration(*step))?;
    }
    Ok(Value::list(out))
}

fn random(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let n = count(a, 0)?;
    check_len(n)?;
    Ok(Value::list_of((0..n).map(|_| Value::Number(it.random())).collect()))
}

fn repeat(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let n = count(a, 1)?;
    check_len(l.len().saturating_mul(n))?;
    let mut out = Vec::with_capacity(l.len() * n);
    for _ in 0..n {
        out.extend(l.items.iter().cloned());
    }
    Ok(Value::list(out))
}

/// The items of a list as lists.
fn lists_of(l: &ListData) -> EResult<Vec<Rc<ListData>>> {
    l.items
        .iter()
        .map(|s| match s.get()? {
            Value::List(x) => Ok(x),
            v => mismatch(&v, "a list"),
        })
        .collect()
}

fn combine(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let lists = lists_of(&*list(a, 0)?)?;
    let total = lists.iter().map(|l| l.len()).fold(0usize, usize::saturating_add);
    check_len(total)?;
    let mut out = Vec::with_capacity(total);
    for l in &lists {
        out.extend(l.items.iter().cloned());
    }
    Ok(Value::list(out))
}

fn zip(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let lists = lists_of(&*list(a, 0)?)?;
    let n = lists.iter().map(|l| l.len()).max().unwrap_or(0);
    check_len(n.saturating_mul(lists.len().max(1)))?;
    Ok(Value::list_of((0..n).map(|i| Value::list(lists.iter().map(|l| l.items.get(i).cloned().unwrap_or(Slot::NULL)).collect())).collect()))
}

/// What a sort criterion says: an optional key selector, the comparer, and the direction.
pub(crate) struct SortBy {
    pub key: Option<Value>,
    pub comparer: Comparer,
    pub descending: bool,
}

/// Reads one comparison criterion: null, an order, a key selector, a comparer function, or
/// `{key selector, order}`.
pub(crate) fn sort_by(spec: &Value) -> EResult<SortBy> {
    let mut by = SortBy { key: None, comparer: Comparer::Ordinal, descending: false };
    match spec {
        Value::Null => {}
        Value::Number(n) => by.descending = *n == 1.0,
        Value::Function(f) => {
            if f.arity().1 == 1 {
                by.key = Some(spec.clone());
            } else {
                by.comparer = Comparer::Custom(spec.clone());
            }
        }
        Value::List(l) => {
            let vals = l.values()?;
            match vals.first() {
                Some(f @ Value::Function(_)) => by.key = Some(f.clone()),
                Some(v) => return mismatch(v, "a function that gives the sort key"),
                None => {}
            }
            by.descending = matches!(vals.get(1), Some(Value::Number(n)) if *n == 1.0);
        }
        v => return mismatch(v, "a sort criterion"),
    }
    Ok(by)
}

fn sort(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let by = sort_by(arg(a, 1))?;
    // (key, item): the key is the item itself unless a selector gives another.
    let mut keyed = Vec::with_capacity(l.len());
    for s in &l.items {
        let v = s.get()?;
        let k = match &by.key {
            Some(f) => it.call1(f, v.clone())?,
            None => v.clone(),
        };
        keyed.push((k, s.clone()));
    }
    let sorted = try_sort_by(keyed, |x, y| {
        let o = by.comparer.compare(it, &x.0, &y.0)?;
        Ok(if by.descending { o.reverse() } else { o })
    })?;
    Ok(Value::list(sorted.into_iter().map(|(_, s)| s).collect()))
}

fn union(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let lists = lists_of(&*list(a, 0)?)?;
    let mut groups = Grouper::new(Comparer::from(arg(a, 1))?);
    let mut out = Vec::new();
    for l in &lists {
        for s in &l.items {
            if groups.group(it, &[s])?.1 {
                out.push(s.clone());
            }
        }
    }
    Ok(Value::list(out))
}

fn intersect(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let lists = lists_of(&*list(a, 0)?)?;
    let Some((first, rest)) = lists.split_first() else { return Ok(Value::list(Vec::new())) };
    let cmp = Comparer::from(arg(a, 1))?;
    let mut others = Vec::with_capacity(rest.len());
    for l in rest {
        others.push(index(it, l, cmp.clone())?);
    }
    let mut seen = Grouper::new(cmp);
    let mut out = Vec::new();
    for s in &first.items {
        let mut everywhere = true;
        for o in &mut others {
            if o.find(it, &[s])?.is_none() {
                everywhere = false;
                break;
            }
        }
        if everywhere && seen.group(it, &[s])?.1 {
            out.push(s.clone());
        }
    }
    Ok(Value::list(out))
}

fn difference(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (l, remove) = (list(a, 0)?, list(a, 1)?);
    let cmp = Comparer::from(arg(a, 2))?;
    // Each item of the second list cancels one occurrence in the first.
    let mut groups = Grouper::new(cmp);
    let mut budget: Vec<usize> = Vec::new();
    for s in &remove.items {
        let (g, new) = groups.group(it, &[s])?;
        if new {
            budget.push(0);
        }
        if let Some(b) = budget.get_mut(g) {
            *b += 1;
        }
    }
    let mut out = Vec::new();
    for s in &l.items {
        match groups.find(it, &[s])?.and_then(|g| budget.get_mut(g)).filter(|b| **b > 0) {
            Some(b) => *b -= 1,
            None => out.push(s.clone()),
        }
    }
    Ok(Value::list(out))
}

fn remove_matching(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (l, remove) = (list(a, 0)?, list(a, 1)?);
    let mut have = index(it, &remove, Comparer::from(arg(a, 2))?)?;
    let mut out = Vec::new();
    for s in &l.items {
        if have.find(it, &[s])?.is_none() {
            out.push(s.clone());
        }
    }
    Ok(Value::list(out))
}

/// `List.ReplaceMatchingItems(list, {{old, new}, …})`.
fn replace_matching(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let cmp = Comparer::from(arg(a, 2))?;
    let mut pairs = Vec::new();
    for p in lists_of(&*list(a, 1)?)? {
        match p.values()?.as_slice() {
            [old, new] => pairs.push((old.clone(), new.clone())),
            _ => return raise("Each replacement is a list of two items: the old value and the new one."),
        }
    }
    let mut out = Vec::with_capacity(l.len());
    'items: for s in &l.items {
        let v = s.get()?;
        for (old, new) in &pairs {
            if cmp.equal(it, &v, old)? {
                out.push(Slot::Val(new.clone()));
                continue 'items;
            }
        }
        out.push(s.clone());
    }
    Ok(Value::list(out))
}

fn replace_value(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let replacer = function(a, 3)?;
    let mut out = Vec::with_capacity(l.len());
    for s in &l.items {
        out.push(Slot::capture(it.call(replacer, &[s.get()?, arg(a, 1).clone(), arg(a, 2).clone()]))?);
    }
    Ok(Value::list(out))
}

/// The bounds of `n` items from `offset`, when inside the list.
fn bounds(l: &ListData, offset: usize, n: usize) -> EResult<(usize, usize)> {
    if offset > l.len() || n > l.len() - offset { raise("The range is outside the list.") } else { Ok((offset, offset + n)) }
}

fn replace_range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (l, with) = (list(a, 0)?, list(a, 3)?);
    let (from, to) = bounds(&l, count(a, 1)?, count(a, 2)?)?;
    check_len(l.len() + with.len())?;
    Ok(Value::list(l.items.iter().take(from).chain(&with.items).chain(l.items.iter().skip(to)).cloned().collect()))
}

fn remove_range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let (from, to) = bounds(&l, count(a, 1)?, opt_count(a, 2)?.unwrap_or(1))?;
    Ok(Value::list(l.items.iter().take(from).chain(l.items.iter().skip(to)).cloned().collect()))
}

fn insert_range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (l, with) = (list(a, 0)?, list(a, 2)?);
    let (from, _) = bounds(&l, count(a, 1)?, 0)?;
    check_len(l.len() + with.len())?;
    Ok(Value::list(l.items.iter().take(from).chain(&with.items).chain(l.items.iter().skip(from)).cloned().collect()))
}

fn all_or_any(a: &[Value], any: bool) -> EResult<Value> {
    for s in &*list(a, 0)?.items {
        match s.get()? {
            Value::Logical(b) if b == any => return Ok(Value::Logical(any)),
            Value::Logical(_) => {}
            v => return mismatch(&v, "true or false"),
        }
    }
    Ok(Value::Logical(!any))
}

fn matches(it: &mut Interp, a: &[Value], any: bool) -> EResult<Value> {
    let l = list(a, 0)?;
    let f = function(a, 1)?;
    for s in &l.items {
        if it.test(f, s.get()?)? == any {
            return Ok(Value::Logical(any));
        }
    }
    Ok(Value::Logical(!any))
}

fn single(a: &[Value], or_default: bool) -> EResult<Value> {
    let l = list(a, 0)?;
    match l.items.as_slice() {
        [one] => one.get(),
        [] if or_default => Ok(arg(a, 1).clone()),
        [] => raise("The list is empty, where exactly one item is expected."),
        _ => raise("The list has more than one item, where exactly one is expected."),
    }
}

fn find_text(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let needle = text(a, 1)?.to_lowercase();
    Ok(Value::list(l.items.iter().filter(|s| matches!(s, Slot::Val(Value::Text(t)) if t.to_lowercase().contains(&needle))).cloned().collect()))
}

fn split(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let size = count(a, 1)?;
    if size == 0 {
        return raise("The page size must be at least 1.");
    }
    Ok(Value::list_of(l.items.chunks(size).map(|c| Value::list(c.to_vec())).collect()))
}

/// `List.Alternate(list, count, repeatInterval?, offset?)`: after `offset` items, skips `count`
/// items then keeps `repeatInterval`, repeatedly (keeps the rest when no interval is given).
fn alternate(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let skip_n = count(a, 1)?;
    let keep_n = opt_count(a, 2)?;
    let offset = opt_count(a, 3)?.unwrap_or(0);
    let mut out: Vec<Slot> = l.items.iter().take(offset).cloned().collect();
    let rest = l.items.get(offset.min(l.len())..).unwrap_or(&[]);
    match keep_n {
        None => out.extend(rest.iter().skip(skip_n).cloned()),
        Some(keep_n) => {
            let cycle = skip_n + keep_n;
            if cycle > 0 {
                out.extend(rest.iter().enumerate().filter(|(i, _)| i % cycle >= skip_n).map(|(_, s)| s.clone()));
            }
        }
    }
    Ok(Value::list(out))
}
