//! The operators.

use std::cmp::Ordering;
use std::rc::Rc;

use crate::ast::{BinOp, UnOp};
use crate::datetime::{MAX_DAY, MIN_DAY, TICKS_PER_DAY, join_ticks, split_ticks, utc_ticks};
use crate::error::{EResult, EvalError, Fatal, raise};
use crate::interp::{MAX_ITEMS, MAX_TEXT};
use crate::value::{Record, Slot, Value, relational, values_equal};

fn no_operator<T>(symbol: &str, a: &Value, b: &Value) -> EResult<T> {
    raise(format!("The operator {symbol} can't be applied to values of type {} and {}.", a.kind(), b.kind()))
}

fn date_value(days: i64) -> EResult<Value> {
    if (MIN_DAY..=MAX_DAY).contains(&days) { Ok(Value::Date(days as i32)) } else { raise("The date is outside the years 1 to 9999.") }
}

fn datetime_ticks(t: Option<i64>) -> EResult<i64> {
    match t {
        Some(t) if (MIN_DAY * TICKS_PER_DAY..(MAX_DAY + 1) * TICKS_PER_DAY).contains(&t) => Ok(t),
        _ => raise("The date and time are outside the years 1 to 9999."),
    }
}

fn duration(t: Option<i64>) -> EResult<Value> {
    match t {
        Some(t) => Ok(Value::Duration(t)),
        None => raise("The duration is too long."),
    }
}

fn scale(t: i64, by: f64) -> EResult<Value> {
    let r = t as f64 * by;
    if r.is_finite() && r.abs() < 9.0e18 { Ok(Value::Duration(r.round() as i64)) } else { raise("The duration is too long.") }
}

pub(crate) fn unary(op: UnOp, v: Value) -> EResult<Value> {
    match (op, v) {
        (_, Value::Null) => Ok(Value::Null),
        (UnOp::Plus, v @ (Value::Number(_) | Value::Duration(_))) => Ok(v),
        (UnOp::Minus, Value::Number(n)) => Ok(Value::Number(-n)),
        (UnOp::Minus, Value::Duration(t)) => duration(t.checked_neg()),
        (UnOp::Not, Value::Logical(b)) => Ok(Value::Logical(!b)),
        (op, v) => {
            let symbol = match op {
                UnOp::Plus => "+",
                UnOp::Minus => "-",
                UnOp::Not => "not",
            };
            raise(format!("The operator {symbol} can't be applied to a value of type {}.", v.kind()))
        }
    }
}

/// `a & b` on records: the fields of both, those of `b` winning.
pub(crate) fn merge_records(a: &Record, b: &Record) -> Value {
    let mut names: Vec<Rc<str>> = a.names.to_vec();
    let mut values: Vec<Slot> = a.values.to_vec();
    for (name, slot) in b.iter() {
        match a.index_of(name) {
            Some(i) => {
                if let Some(v) = values.get_mut(i) {
                    *v = slot.clone();
                }
            }
            None => {
                names.push(name.clone());
                values.push(slot.clone());
            }
        }
    }
    Value::record(names.into(), values.into())
}

/// Every binary operator except the short-circuiting ones (`and`, `or`, `??`) and `meta`.
pub(crate) fn binary(op: BinOp, a: &Value, b: &Value) -> EResult<Value> {
    use Value::*;
    match op {
        BinOp::Eq => return Ok(Logical(values_equal(a, b)?)),
        BinOp::Ne => return Ok(Logical(!values_equal(a, b)?)),
        BinOp::And | BinOp::Or | BinOp::Coalesce | BinOp::Meta => return Ok(a.clone()),
        _ => {}
    }
    if a.is_null() || b.is_null() {
        // Still an error when the other side has no such operator at all.
        return Ok(Null);
    }
    match op {
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let o = relational(a, b)?;
            Ok(Logical(match (op, o) {
                (_, None) => false,
                (BinOp::Lt, Some(o)) => o == Ordering::Less,
                (BinOp::Le, Some(o)) => o != Ordering::Greater,
                (BinOp::Gt, Some(o)) => o == Ordering::Greater,
                (_, Some(o)) => o != Ordering::Less,
            }))
        }
        BinOp::Add => match (a, b) {
            (Number(x), Number(y)) => Ok(Number(x + y)),
            (Duration(x), Duration(y)) => duration(x.checked_add(*y)),
            (Date(d), Duration(t)) | (Duration(t), Date(d)) => {
                let ticks = datetime_ticks(join_ticks(*d as i64, 0).checked_add(*t))?;
                date_value(split_ticks(ticks).0)
            }
            (DateTime(x), Duration(t)) | (Duration(t), DateTime(x)) => Ok(DateTime(datetime_ticks(x.checked_add(*t))?)),
            (DateTimeZone(x, o), Duration(t)) | (Duration(t), DateTimeZone(x, o)) => Ok(DateTimeZone(datetime_ticks(x.checked_add(*t))?, *o)),
            (Time(x), Duration(t)) | (Duration(t), Time(x)) => Ok(Time(x.wrapping_add(*t).rem_euclid(TICKS_PER_DAY))),
            _ => no_operator("+", a, b),
        },
        BinOp::Sub => match (a, b) {
            (Number(x), Number(y)) => Ok(Number(x - y)),
            (Duration(x), Duration(y)) => duration(x.checked_sub(*y)),
            (Date(x), Date(y)) => Ok(Duration((*x as i64 - *y as i64) * TICKS_PER_DAY)),
            (Date(d), Duration(t)) => {
                let ticks = datetime_ticks(join_ticks(*d as i64, 0).checked_sub(*t))?;
                date_value(split_ticks(ticks).0)
            }
            (DateTime(x), DateTime(y)) => duration(x.checked_sub(*y)),
            (DateTime(x), Duration(t)) => Ok(DateTime(datetime_ticks(x.checked_sub(*t))?)),
            (DateTimeZone(x, ox), DateTimeZone(y, oy)) => duration(utc_ticks(*x, *ox).checked_sub(utc_ticks(*y, *oy))),
            (DateTimeZone(x, o), Duration(t)) => Ok(DateTimeZone(datetime_ticks(x.checked_sub(*t))?, *o)),
            (Time(x), Time(y)) => Ok(Duration(x - y)),
            (Time(x), Duration(t)) => Ok(Time(x.wrapping_sub(*t).rem_euclid(TICKS_PER_DAY))),
            _ => no_operator("-", a, b),
        },
        BinOp::Mul => match (a, b) {
            (Number(x), Number(y)) => Ok(Number(x * y)),
            (Duration(t), Number(n)) | (Number(n), Duration(t)) => scale(*t, *n),
            _ => no_operator("*", a, b),
        },
        BinOp::Div => match (a, b) {
            (Number(x), Number(y)) => Ok(Number(x / y)),
            (Duration(t), Number(n)) => scale(*t, 1.0 / n),
            (Duration(x), Duration(y)) => Ok(Number(*x as f64 / *y as f64)),
            _ => no_operator("/", a, b),
        },
        BinOp::Concat => match (a, b) {
            (Text(x), Text(y)) => {
                if x.len() + y.len() > MAX_TEXT {
                    return Err(EvalError::Fatal(Fatal::TooLarge));
                }
                let mut s = String::with_capacity(x.len() + y.len());
                s.push_str(x);
                s.push_str(y);
                Ok(Text(Rc::from(s)))
            }
            (List(x), List(y)) => {
                if x.len() + y.len() > MAX_ITEMS {
                    return Err(EvalError::Fatal(Fatal::TooLarge));
                }
                let mut items = Vec::with_capacity(x.len() + y.len());
                items.extend(x.items.iter().cloned());
                items.extend(y.items.iter().cloned());
                Ok(Value::list(items))
            }
            (Record(x), Record(y)) => Ok(merge_records(x, y)),
            (Table(x), Table(y)) => crate::stdlib::table::combine(&[x.clone(), y.clone()]),
            (Date(d), Time(t)) => Ok(DateTime(join_ticks(*d as i64, *t))),
            _ => no_operator("&", a, b),
        },
        BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or | BinOp::Coalesce | BinOp::Meta => Ok(a.clone()),
    }
}
