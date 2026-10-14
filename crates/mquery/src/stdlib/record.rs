//! `Record.*`.

use std::rc::Rc;

use super::*;
use crate::error::{EResult, raise};
use crate::interp::Interp;
use crate::ops::merge_records;
use crate::value::{Builtin, Record, Slot, Table, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Record.Field", 2, 2, field;
    "Record.FieldOrDefault", 2, 3, field_or_default;
    "Record.FieldNames", 1, 1, |_, a| Ok(Value::list_of(record(a, 0)?.names.iter().map(|n| Value::Text(n.clone())).collect()));
    "Record.FieldValues", 1, 1, |_, a| Ok(Value::list(record(a, 0)?.values.to_vec()));
    "Record.ToList", 1, 1, |_, a| Ok(Value::list(record(a, 0)?.values.to_vec()));
    "Record.FieldCount", 1, 1, |_, a| Ok(Value::Number(record(a, 0)?.len() as f64));
    "Record.HasFields", 2, 2, has_fields;
    "Record.AddField", 3, 4, add_field;
    "Record.RemoveFields", 2, 3, remove_fields;
    "Record.RenameFields", 2, 3, rename_fields;
    "Record.SelectFields", 2, 3, |_, a| select_fields(a);
    "Record.ReorderFields", 2, 3, reorder_fields;
    "Record.TransformFields", 2, 3, transform_fields;
    "Record.Combine", 1, 1, combine;
    "Record.FromList", 2, 2, from_list;
    "Record.FromTable", 1, 1, from_table;
    "Record.ToTable", 1, 1, to_table;
];

/// What to do about a named field that isn't there (a `MissingField` value).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missing {
    Error,
    Ignore,
    UseNull,
}

pub(crate) fn missing(v: &Value) -> EResult<Missing> {
    match v {
        Value::Null => Ok(Missing::Error),
        Value::Number(n) if *n == 0.0 => Ok(Missing::Error),
        Value::Number(n) if *n == 1.0 => Ok(Missing::Ignore),
        Value::Number(n) if *n == 2.0 => Ok(Missing::UseNull),
        v => mismatch(v, "a MissingField value"),
    }
}

fn no_field<T>(name: &str) -> EResult<T> {
    raise(format!("The record has no field named '{name}'."))
}

fn build(names: Vec<Rc<str>>, values: Vec<Slot>) -> Value {
    Value::record(names.into(), values.into())
}

fn field(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (r, name) = (record(a, 0)?, text(a, 1)?);
    match r.get(&name)? {
        Some(v) => Ok(v),
        None => no_field(&name),
    }
}

fn field_or_default(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(r) = opt_record(a, 0)? else { return Ok(arg(a, 2).clone()) };
    Ok(r.get(&text(a, 1)?)?.unwrap_or_else(|| arg(a, 2).clone()))
}

fn has_fields(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let r = record(a, 0)?;
    Ok(Value::Logical(names(arg(a, 1))?.iter().all(|n| r.index_of(n).is_some())))
}

fn add_field(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (r, name) = (record(a, 0)?, text(a, 1)?);
    if r.index_of(&name).is_some() {
        return raise(format!("The record already has a field named '{name}'."));
    }
    // A delayed value is a function to call for it.
    let value = match (opt_logical(a, 3)?, arg(a, 2)) {
        (Some(true), f @ Value::Function(_)) => Slot::capture(it.call(f, &[]))?,
        (_, v) => Slot::Val(v.clone()),
    };
    let mut names = r.names.to_vec();
    let mut values = r.values.to_vec();
    names.push(name);
    values.push(value);
    Ok(build(names, values))
}

fn remove_fields(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let r = record(a, 0)?;
    let remove = names(arg(a, 1))?;
    if missing(arg(a, 2))? == Missing::Error
        && let Some(n) = remove.iter().find(|n| r.index_of(n).is_none())
    {
        return no_field(n);
    }
    let (names, values) = r.iter().filter(|(n, _)| !remove.contains(n)).map(|(n, s)| (n.clone(), s.clone())).unzip();
    Ok(build(names, values))
}

/// A list of `{old, new}` pairs, or one such pair.
pub(crate) fn rename_pairs(v: &Value) -> EResult<Vec<(Rc<str>, Rc<str>)>> {
    let Value::List(l) = v else { return mismatch(v, "a list of {old name, new name} pairs") };
    let pair = |vals: &[Value]| match vals {
        [Value::Text(old), Value::Text(new)] => Ok((old.clone(), new.clone())),
        _ => raise("Each rename is a list of two names: the old one and the new one."),
    };
    let vals = l.values()?;
    if matches!(vals.first(), Some(Value::Text(_))) {
        return Ok(vec![pair(&vals)?]);
    }
    vals.iter()
        .map(|v| match v {
            Value::List(p) => pair(&p.values()?),
            v => mismatch(v, "an {old name, new name} pair"),
        })
        .collect()
}

fn rename_fields(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let r = record(a, 0)?;
    let pairs = rename_pairs(arg(a, 1))?;
    let mode = missing(arg(a, 2))?;
    let mut names = r.names.to_vec();
    let mut values = r.values.to_vec();
    for (old, new) in &pairs {
        match r.index_of(old) {
            Some(i) => {
                if let Some(n) = names.get_mut(i) {
                    *n = new.clone();
                }
            }
            None if mode == Missing::Error => return no_field(old),
            None if mode == Missing::UseNull => {
                names.push(new.clone());
                values.push(Slot::NULL);
            }
            None => {}
        }
    }
    for (i, n) in names.iter().enumerate() {
        if names.iter().skip(i + 1).any(|m| m == n) {
            return raise(format!("Renaming would give the record two fields named '{n}'."));
        }
    }
    Ok(build(names, values))
}

pub(crate) fn select_from(r: &Record, wanted: &[Rc<str>], mode: Missing) -> EResult<Value> {
    let mut names = Vec::with_capacity(wanted.len());
    let mut values = Vec::with_capacity(wanted.len());
    for n in wanted {
        match r.slot(n) {
            Some(s) => {
                names.push(n.clone());
                values.push(s.clone());
            }
            None if mode == Missing::Error => return no_field(n),
            None if mode == Missing::UseNull => {
                names.push(n.clone());
                values.push(Slot::NULL);
            }
            None => {}
        }
    }
    Ok(build(names, values))
}

fn select_fields(a: &[Value]) -> EResult<Value> {
    select_from(&*record(a, 0)?, &names(arg(a, 1))?, missing(arg(a, 2))?)
}

fn reorder_fields(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let r = record(a, 0)?;
    let order = names(arg(a, 1))?;
    let mode = missing(arg(a, 2))?;
    if mode == Missing::Error
        && let Some(n) = order.iter().find(|n| r.index_of(n).is_none())
    {
        return no_field(n);
    }
    // The named fields swap places among themselves; the others stay where they are.
    let present: Vec<&Rc<str>> = order.iter().filter(|n| r.index_of(n).is_some()).collect();
    let mut next = present.iter();
    let mut names = Vec::with_capacity(r.len());
    let mut values = Vec::with_capacity(r.len());
    for (n, s) in r.iter() {
        if present.contains(&n)
            && let Some(m) = next.next()
        {
            names.push((**m).clone());
            values.push(r.slot(m).cloned().unwrap_or(Slot::NULL));
            continue;
        }
        names.push(n.clone());
        values.push(s.clone());
    }
    Ok(build(names, values))
}

/// A list of `{field, function}` pairs, or one such pair.
pub(crate) fn transform_pairs(v: &Value) -> EResult<Vec<(Rc<str>, Value, Option<Value>)>> {
    let Value::List(l) = v else { return mismatch(v, "a list of {name, function} pairs") };
    let pair = |vals: &[Value]| match vals {
        [Value::Text(name), f @ Value::Function(_)] => Ok((name.clone(), f.clone(), None)),
        [Value::Text(name), f @ Value::Function(_), ty] => Ok((name.clone(), f.clone(), Some(ty.clone()))),
        _ => raise("Each transformation is a list of a name and a function."),
    };
    let vals = l.values()?;
    if matches!(vals.first(), Some(Value::Text(_))) {
        return Ok(vec![pair(&vals)?]);
    }
    vals.iter()
        .map(|v| match v {
            Value::List(p) => pair(&p.values()?),
            v => mismatch(v, "a {name, function} pair"),
        })
        .collect()
}

fn transform_fields(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let r = record(a, 0)?;
    let mode = missing(arg(a, 2))?;
    let mut names = r.names.to_vec();
    let mut values = r.values.to_vec();
    for (name, f, _) in transform_pairs(arg(a, 1))? {
        match r.index_of(&name) {
            Some(i) => {
                if let Some(slot) = values.get_mut(i) {
                    let v = slot.get()?;
                    *slot = Slot::capture(it.call1(&f, v))?;
                }
            }
            None if mode == Missing::Error => return no_field(&name),
            None if mode == Missing::UseNull => {
                names.push(name);
                values.push(Slot::capture(it.call1(&f, Value::Null))?);
            }
            None => {}
        }
    }
    Ok(build(names, values))
}

fn combine(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let mut acc = Value::record_of(Vec::new());
    for s in &*list(a, 0)?.items {
        acc = match (&acc, s.get()?) {
            (Value::Record(x), Value::Record(y)) => merge_records(x, &y),
            (_, v) => return mismatch(&v, "a record"),
        };
    }
    Ok(acc)
}

fn from_list(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let values = list(a, 0)?;
    let names = match arg(a, 1) {
        Value::Type(crate::value::Type::Record(c)) => c.fields.iter().map(|f| f.name.clone()).collect(),
        v => names(v)?,
    };
    if names.len() != values.len() {
        return raise("The list of values and the list of field names have different lengths.");
    }
    Ok(build(names, values.items.clone()))
}

fn from_table(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let (Some(n), Some(v)) = (t.column_index("Name"), t.column_index("Value")) else {
        return raise("The table needs the columns Name and Value.");
    };
    let mut names: Vec<Rc<str>> = Vec::with_capacity(t.rows.len());
    let mut values = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        match row.get(n).map(Slot::get).transpose()? {
            Some(Value::Text(name)) => {
                if names.contains(&name) {
                    return raise(format!("The field '{name}' appears more than once."));
                }
                names.push(name);
            }
            _ => return raise("Each Name must be text."),
        }
        values.push(row.get(v).cloned().unwrap_or(Slot::NULL));
    }
    Ok(build(names, values))
}

fn to_table(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let r = record(a, 0)?;
    let rows = r.iter().map(|(n, s)| [Slot::Val(Value::Text(n.clone())), s.clone()].into_iter().collect()).collect();
    Ok(Value::table(Table::untyped([Rc::from("Name"), Rc::from("Value")].into_iter().collect(), rows)))
}
