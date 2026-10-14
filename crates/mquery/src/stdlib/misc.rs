//! `Logical.*`, `Value.*`, `Type.*`, `Function.*`, `Error.*`, `Expression.*`, `Comparer.*`,
//! `Replacer.*` and `Diagnostics.*`.

use std::cmp::Ordering;
use std::rc::Rc;

use super::*;
use crate::ast::BinOp;
use crate::error::{EResult, raise};
use crate::interp::{Interp, MAX_TEXT, too_large};
use crate::ops;
use crate::value::{Builtin, Field, Function, Slot, Table, Type, Value, compare_values, values_equal};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Logical.From", 1, 1, |_, a| convert::to_logical(arg(a, 0));
    "Logical.FromText", 1, 1, |_, a| match arg(a, 0) { Value::Null => Ok(Value::Null), v @ Value::Text(_) => convert::to_logical(v), v => mismatch(v, "text") };
    "Logical.ToText", 1, 1, |_, a| Ok(match opt_logical(a, 0)? { Some(b) => Value::text(if b { "true" } else { "false" }), None => Value::Null });
    "Value.Type", 1, 1, |_, a| Ok(Value::Type(arg(a, 0).type_of()));
    "Value.Is", 2, 2, |_, a| Ok(Value::Logical(type_arg(a, 1)?.accepts(arg(a, 0))));
    "Value.As", 2, 2, value_as;
    "Value.Equals", 2, 3, |_, a| Ok(Value::Logical(values_equal(arg(a, 0), arg(a, 1))?));
    "Value.NullableEquals", 2, 3, |_, a| Ok(if arg(a, 0).is_null() || arg(a, 1).is_null() { Value::Null } else { Value::Logical(values_equal(arg(a, 0), arg(a, 1))?) });
    "Value.Compare", 2, 3, |_, a| Ok(ordering(compare_values(arg(a, 0), arg(a, 1))));
    "Value.Add", 2, 3, |_, a| ops::binary(BinOp::Add, arg(a, 0), arg(a, 1));
    "Value.Subtract", 2, 3, |_, a| ops::binary(BinOp::Sub, arg(a, 0), arg(a, 1));
    "Value.Multiply", 2, 3, |_, a| ops::binary(BinOp::Mul, arg(a, 0), arg(a, 1));
    "Value.Divide", 2, 3, |_, a| ops::binary(BinOp::Div, arg(a, 0), arg(a, 1));
    "Value.FromText", 1, 2, value_from_text;
    "Value.Metadata", 1, 1, |_, _| Ok(Value::record_of(Vec::new()));
    "Value.RemoveMetadata", 1, 2, |_, a| Ok(arg(a, 0).clone());
    "Value.ReplaceMetadata", 2, 2, |_, a| Ok(arg(a, 0).clone());
    "Value.ReplaceType", 2, 2, replace_type;
    "Type.Is", 2, 2, |_, a| Ok(Value::Logical(type_arg(a, 0)?.is_compatible_with(&type_arg(a, 1)?)));
    "Type.IsNullable", 1, 1, |_, a| Ok(Value::Logical(type_arg(a, 0)?.is_nullable()));
    "Type.NonNullable", 1, 1, |_, a| Ok(Value::Type(type_arg(a, 0)?.non_nullable()));
    "Type.ListItem", 1, 1, |_, a| Ok(Value::Type(type_arg(a, 0)?.item()));
    "Type.RecordFields", 1, 1, record_fields;
    "Type.TableRow", 1, 1, table_row;
    "Type.TableColumn", 2, 2, table_column_type;
    "Type.ForRecord", 2, 2, for_record;
    "Type.IsOpenRecord", 1, 1, |_, a| Ok(Value::Logical(matches!(type_arg(a, 0)?, Type::Record(c) if c.open)));
    "Function.Invoke", 2, 2, |it, a| { let args = list(a, 1)?.values()?; it.call(function(a, 0)?, &args) };
    "Function.From", 2, 2, |_, a| Ok(function(a, 1)?.clone());
    "Function.IsDataSource", 1, 1, |_, _| Ok(Value::Logical(false));
    "Error.Record", 1, 4, |_, a| Ok(Value::record_of(vec![("Reason", arg(a, 0).clone()), ("Message", arg(a, 1).clone()), ("Detail", arg(a, 2).clone())]));
    "Diagnostics.Trace", 3, 4, trace;
    "Expression.Evaluate", 1, 2, |it, a| { let env = opt_record(a, 1)?; it.evaluate_text(&text(a, 0)?, env) };
    "Expression.Constant", 1, 1, |_, a| constant(arg(a, 0));
    "Expression.Identifier", 1, 1, |_, a| Ok(Value::text(crate::lexer::quote_identifier(&text(a, 0)?)));
    "Comparer.Ordinal", 2, 2, |_, a| Ok(ordering(compare_values(arg(a, 0), arg(a, 1))));
    "Comparer.OrdinalIgnoreCase", 2, 2, |_, a| Ok(ordering(compare_ignoring_case(arg(a, 0), arg(a, 1))));
    "Comparer.FromCulture", 1, 2, |_, a| { opt_text(a, 0)?; Ok(Value::Function(Rc::new(Function::Bound(&FROM_CULTURE, vec![arg(a, 0).clone(), Value::Logical(opt_logical(a, 1)?.unwrap_or(false))])))) };
    "Comparer.Equals", 3, 3, |it, a| Ok(Value::Logical(matches!(it.call(function(a, 0)?, &[arg(a, 1).clone(), arg(a, 2).clone()])?, Value::Number(n) if n == 0.0)));
    "Replacer.ReplaceText", 3, 3, replace_text;
    "Replacer.ReplaceValue", 3, 3, |_, a| Ok(if values_equal(arg(a, 0), arg(a, 1))? { arg(a, 2).clone() } else { arg(a, 0).clone() });
];

/// What `Comparer.FromCulture(culture, ignoreCase)` returns: it takes the two values to compare
/// after the two it was made with.
static FROM_CULTURE: Builtin = Builtin {
    name: "Comparer.FromCulture",
    min: 4,
    max: 4,
    f: |_, a| {
        Ok(ordering(if matches!(arg(a, 1), Value::Logical(true)) {
            compare_ignoring_case(arg(a, 2), arg(a, 3))
        } else {
            compare_values(arg(a, 2), arg(a, 3))
        }))
    },
};

fn ordering(o: Ordering) -> Value {
    Value::Number(match o {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    })
}

fn compare_ignoring_case(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        _ => compare_values(a, b),
    }
}

fn value_as(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let ty = type_arg(a, 1)?;
    if ty.accepts(arg(a, 0)) { Ok(arg(a, 0).clone()) } else { crate::interp::cannot_convert(arg(a, 0), ty.name()) }
}

/// `Value.FromText`: the number, logical, date or time the text reads as; otherwise the text.
fn value_from_text(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let v = arg(a, 0);
    let Value::Text(s) = v else { return Ok(v.clone()) };
    let culture = it.culture(opt_text(a, 1)?.as_deref());
    if let Some(n) = culture.parse_number(s) {
        return Ok(Value::Number(n));
    }
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => return Ok(Value::Logical(true)),
        "false" => return Ok(Value::Logical(false)),
        "null" => return Ok(Value::Null),
        _ => {}
    }
    if let Some((t, offset, has_time)) = crate::datetime::parse_datetime(s, culture) {
        return Ok(match (offset, has_time) {
            (Some(off), _) => Value::DateTimeZone(t, off),
            (None, true) => Value::DateTime(t),
            (None, false) => Value::Date(crate::datetime::split_ticks(t).0 as i32),
        });
    }
    if let Some(t) = crate::datetime::parse_time(s) {
        return Ok(Value::Time(t % crate::datetime::TICKS_PER_DAY));
    }
    Ok(v.clone())
}

/// `Value.ReplaceType`: a table takes the column types of a table type; other values are kept.
fn replace_type(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match (arg(a, 0), type_arg(a, 1)?) {
        (Value::Table(t), Type::Table(c)) if !c.fields.is_empty() => {
            if c.fields.len() != t.width() {
                return raise("The table type has a different number of columns than the table.");
            }
            let columns = c.fields.iter().map(|f| f.name.clone()).collect();
            let types = c.fields.iter().map(|f| f.ty.clone()).collect();
            Ok(Value::table(Table { columns, types, rows: t.rows.clone() }))
        }
        (v, _) => Ok(v.clone()),
    }
}

fn field_record(f: &Field) -> Value {
    Value::record_of(vec![("Type", Value::Type(f.ty.clone())), ("Optional", Value::Logical(f.optional))])
}

fn record_fields(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match type_arg(a, 0)? {
        Type::Record(c) => {
            Ok(Value::record(c.fields.iter().map(|f| f.name.clone()).collect(), c.fields.iter().map(|f| Slot::Val(field_record(f))).collect()))
        }
        _ => raise("A record type is expected."),
    }
}

fn table_row(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match type_arg(a, 0)? {
        Type::Table(c) => Ok(Value::Type(Type::record(c.fields.clone(), false)?)),
        _ => raise("A table type is expected."),
    }
}

fn table_column_type(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let name = text(a, 1)?;
    match type_arg(a, 0)? {
        Type::Table(c) => match c.fields.iter().find(|f| f.name == name) {
            Some(f) => Ok(Value::Type(f.ty.clone())),
            None => raise(format!("The table type has no column named '{name}'.")),
        },
        _ => raise("A table type is expected."),
    }
}

/// `Type.ForRecord([a = [Type = type number, Optional = false], …], open)`.
fn for_record(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let fields = record(a, 0)?;
    let open = opt_logical(a, 1)?.unwrap_or(false);
    let mut out = Vec::with_capacity(fields.len());
    for (name, slot) in fields.iter() {
        let Value::Record(spec) = slot.get()? else { return raise("Each field is described by a record with Type and Optional.") };
        let ty = match spec.get("Type")? {
            Some(Value::Type(t)) => t,
            _ => Type::Any,
        };
        let optional = matches!(spec.get("Optional")?, Some(Value::Logical(true)));
        out.push(Field { name: name.clone(), ty, optional });
    }
    Ok(Value::Type(Type::record(out, open)?))
}

/// `Diagnostics.Trace(level, message, value, delayed?)`: gives `value`; nothing is traced.
fn trace(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    match (opt_logical(a, 3)?, arg(a, 2)) {
        (Some(true), f @ Value::Function(_)) => it.call(f, &[]),
        (_, v) => Ok(v.clone()),
    }
}

fn constant(v: &Value) -> EResult<Value> {
    match v {
        Value::Function(_) | Value::Type(_) | Value::Binary(_) => raise("This value can't be written as a constant."),
        v => {
            let s = crate::value::render(v);
            if s.len() > MAX_TEXT { too_large() } else { Ok(Value::text(s)) }
        }
    }
}

/// `Replacer.ReplaceText`: replaces within text; values that aren't text are left as they are.
fn replace_text(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Value::Text(s) = arg(a, 0) else { return Ok(arg(a, 0).clone()) };
    let (old, new) = (opt_text(a, 1)?.unwrap_or_else(|| Rc::from("")), opt_text(a, 2)?.unwrap_or_else(|| Rc::from("")));
    // Replacing the empty text fills in a cell that is empty.
    if old.is_empty() {
        return Ok(if s.is_empty() { Value::Text(new) } else { Value::Text(s.clone()) });
    }
    if new.len() > old.len() && s.len() / old.len() * new.len() > MAX_TEXT {
        return too_large();
    }
    Ok(Value::text(s.replace(&*old, &new)))
}
