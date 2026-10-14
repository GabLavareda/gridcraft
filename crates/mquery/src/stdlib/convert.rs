//! Conversions between kinds of values: what `Number.From`, `Text.From`, `Date.From` and
//! `Table.TransformColumnTypes` do.

use crate::culture::Culture;
use crate::datetime::{self, OLE_EPOCH_DAY, TICKS_PER_DAY};
use crate::error::{EResult, raise, raise_format};
use crate::interp::cannot_convert;
use crate::value::{Facet, Type, Value};

fn in_range(ticks: f64) -> Option<i64> {
    let lo = (datetime::MIN_DAY * TICKS_PER_DAY) as f64;
    let hi = ((datetime::MAX_DAY + 1) * TICKS_PER_DAY) as f64;
    (ticks.is_finite() && ticks >= lo && ticks < hi).then_some(ticks.round() as i64)
}

/// Ticks since 1970 for a number counted in days from 1899-12-30, as spreadsheets count dates.
fn ticks_from_serial(n: f64) -> Option<i64> {
    in_range((n + OLE_EPOCH_DAY as f64) * TICKS_PER_DAY as f64)
}

fn serial_from_ticks(t: i64) -> f64 {
    t as f64 / TICKS_PER_DAY as f64 - OLE_EPOCH_DAY as f64
}

pub(crate) fn to_text(v: &Value, c: &Culture) -> EResult<Value> {
    Ok(Value::text(match v {
        Value::Null => return Ok(Value::Null),
        Value::Text(_) => return Ok(v.clone()),
        Value::Number(n) => c.number_to_text(*n),
        Value::Logical(b) => (if *b { "true" } else { "false" }).to_string(),
        Value::Date(d) => datetime::date_to_text(*d as i64, c),
        Value::Time(t) => datetime::time_to_text(*t, c),
        Value::DateTime(t) => datetime::datetime_to_text(*t, c),
        Value::DateTimeZone(t, off) => datetime::datetimezone_to_text(*t, *off, c),
        Value::Duration(t) => datetime::duration_to_text(*t),
        v => return cannot_convert(v, "Text"),
    }))
}

pub(crate) fn to_number(v: &Value, c: &Culture) -> EResult<Value> {
    Ok(Value::Number(match v {
        Value::Null => return Ok(Value::Null),
        Value::Number(n) => *n,
        Value::Logical(b) => *b as i64 as f64,
        Value::Text(s) => match c.parse_number(s) {
            Some(n) => n,
            None => return raise_format("The text can't be read as a number.", v.clone()),
        },
        Value::Date(d) => (*d as i64 - OLE_EPOCH_DAY) as f64,
        Value::DateTime(t) | Value::DateTimeZone(t, _) => serial_from_ticks(*t),
        Value::Time(t) => *t as f64 / TICKS_PER_DAY as f64,
        Value::Duration(t) => *t as f64 / TICKS_PER_DAY as f64,
        v => return cannot_convert(v, "Number"),
    }))
}

pub(crate) fn to_logical(v: &Value) -> EResult<Value> {
    Ok(Value::Logical(match v {
        Value::Null => return Ok(Value::Null),
        Value::Logical(b) => *b,
        Value::Number(n) => *n != 0.0,
        Value::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            _ => return raise_format("The text can't be read as true or false.", v.clone()),
        },
        v => return cannot_convert(v, "Logical"),
    }))
}

pub(crate) fn to_date(v: &Value, c: &Culture) -> EResult<Value> {
    let days = match v {
        Value::Null => return Ok(Value::Null),
        Value::Date(_) => return Ok(v.clone()),
        Value::DateTime(t) | Value::DateTimeZone(t, _) => datetime::split_ticks(*t).0,
        Value::Number(n) => match ticks_from_serial(n.floor()) {
            Some(t) => datetime::split_ticks(t).0,
            None => return raise("The number is outside the range of dates."),
        },
        Value::Text(s) => match datetime::parse_datetime(s, c) {
            Some((t, _, _)) => datetime::split_ticks(t).0,
            None => return raise_format("The text can't be read as a date.", v.clone()),
        },
        v => return cannot_convert(v, "Date"),
    };
    Ok(Value::Date(days as i32))
}

pub(crate) fn to_datetime(v: &Value, c: &Culture) -> EResult<Value> {
    Ok(Value::DateTime(match v {
        Value::Null => return Ok(Value::Null),
        Value::DateTime(_) => return Ok(v.clone()),
        Value::DateTimeZone(t, _) => *t,
        Value::Date(d) => datetime::join_ticks(*d as i64, 0),
        Value::Time(t) => datetime::join_ticks(OLE_EPOCH_DAY, *t),
        Value::Number(n) => match ticks_from_serial(*n) {
            Some(t) => t,
            None => return raise("The number is outside the range of dates."),
        },
        Value::Text(s) => match datetime::parse_datetime(s, c) {
            Some((t, _, _)) => t,
            None => match datetime::parse_time(s) {
                Some(tod) => datetime::join_ticks(OLE_EPOCH_DAY, tod),
                None => return raise_format("The text can't be read as a date and time.", v.clone()),
            },
        },
        v => return cannot_convert(v, "DateTime"),
    }))
}

pub(crate) fn to_datetimezone(v: &Value, c: &Culture, local_offset: i32) -> EResult<Value> {
    Ok(match v {
        Value::Null => Value::Null,
        Value::DateTimeZone(..) => v.clone(),
        Value::Text(s) => match datetime::parse_datetime(s, c) {
            Some((t, off, _)) => Value::DateTimeZone(t, off.unwrap_or(local_offset)),
            None => return raise_format("The text can't be read as a date, time and time zone.", v.clone()),
        },
        other => match to_datetime(other, c)? {
            Value::DateTime(t) => Value::DateTimeZone(t, local_offset),
            x => x,
        },
    })
}

pub(crate) fn to_time(v: &Value, c: &Culture) -> EResult<Value> {
    Ok(Value::Time(match v {
        Value::Null => return Ok(Value::Null),
        Value::Time(_) => return Ok(v.clone()),
        Value::DateTime(t) | Value::DateTimeZone(t, _) => datetime::split_ticks(*t).1,
        Value::Number(n) => {
            if !n.is_finite() || *n < 0.0 || *n >= 1.0 {
                return raise("A time is a number from 0 up to, but not including, 1.");
            }
            (n * TICKS_PER_DAY as f64).round() as i64 % TICKS_PER_DAY
        }
        Value::Text(s) => match datetime::parse_time(s) {
            Some(t) => t % TICKS_PER_DAY,
            None => match datetime::parse_datetime(s, c) {
                Some((t, _, true)) => datetime::split_ticks(t).1,
                _ => return raise_format("The text can't be read as a time.", v.clone()),
            },
        },
        v => return cannot_convert(v, "Time"),
    }))
}

pub(crate) fn to_duration(v: &Value) -> EResult<Value> {
    Ok(Value::Duration(match v {
        Value::Null => return Ok(Value::Null),
        Value::Duration(_) => return Ok(v.clone()),
        Value::Number(n) => {
            let t = n * TICKS_PER_DAY as f64;
            if !t.is_finite() || t.abs() > 9.0e18 {
                return raise("The number is too large for a duration.");
            }
            t.round() as i64
        }
        Value::Text(s) => match datetime::parse_duration(s) {
            Some(t) => t,
            None => return raise_format("The text can't be read as a duration.", v.clone()),
        },
        v => return cannot_convert(v, "Duration"),
    }))
}

/// Rounds half to even, the default of the number conversions.
pub(crate) fn round_half_even(n: f64) -> f64 {
    let r = n.round();
    if (n - n.trunc()).abs() == 0.5 && r % 2.0 != 0.0 { r - n.signum() } else { r }
}

/// A number as one of the number types: whole numbers round and must fit, currency keeps four
/// decimals.
pub(crate) fn apply_facet(n: f64, facet: Facet) -> EResult<f64> {
    let (lo, hi) = match facet {
        Facet::Double | Facet::Decimal | Facet::Percentage => return Ok(n),
        Facet::Single => return Ok(n as f32 as f64),
        Facet::Currency => return Ok(round_half_even(n * 10_000.0) / 10_000.0),
        Facet::Int64 => (-9.223372036854776e18, 9.223372036854776e18),
        Facet::Int32 => (i32::MIN as f64, i32::MAX as f64),
        Facet::Int16 => (i16::MIN as f64, i16::MAX as f64),
        Facet::Int8 => (i8::MIN as f64, i8::MAX as f64),
        Facet::Byte => (0.0, 255.0),
    };
    if n.is_nan() {
        return raise("NaN can't be converted to a whole number.");
    }
    let r = round_half_even(n);
    if r < lo || r > hi { raise("The number is out of range for this whole-number type.") } else { Ok(r) }
}

/// Converts `v` to `ty`, as changing a column's type does.
pub(crate) fn convert(v: &Value, ty: &Type, c: &Culture, local_offset: i32) -> EResult<Value> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    match ty {
        Type::Any | Type::AnyNonNull => Ok(v.clone()),
        Type::Nullable(_) => convert(v, &ty.non_nullable(), c, local_offset),
        Type::Text => to_text(v, c),
        Type::Number(facet) => match to_number(v, c)? {
            Value::Number(n) => Ok(Value::Number(apply_facet(n, *facet)?)),
            other => Ok(other),
        },
        Type::Logical => to_logical(v),
        Type::Date => to_date(v, c),
        Type::DateTime => to_datetime(v, c),
        Type::DateTimeZone => to_datetimezone(v, c, local_offset),
        Type::Time => to_time(v, c),
        Type::Duration => to_duration(v),
        other => {
            if other.accepts(v) {
                Ok(v.clone())
            } else {
                cannot_convert(v, other.name())
            }
        }
    }
}
