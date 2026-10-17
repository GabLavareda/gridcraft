//! The standard library: the functions and constants every query can use.

use std::rc::Rc;

use crate::error::{EResult, raise};
use crate::value::{Builtin, Facet, ListData, Record, Table, Type, Value};

pub(crate) mod convert;
pub(crate) mod data;
pub(crate) mod database;
pub(crate) mod date;
pub(crate) mod json;
pub(crate) mod keys;
pub(crate) mod list;
pub(crate) mod misc;
pub(crate) mod number;
pub(crate) mod record;
pub(crate) mod sources;
pub(crate) mod splitter;
pub(crate) mod table;
pub(crate) mod table_shape;
pub(crate) mod text;

/// `&[Builtin]` from rows of `name, min args, max args, function;`.
macro_rules! builtin_table {
    ($($name:literal, $min:expr, $max:expr, $f:expr;)*) => {
        &[$($crate::value::Builtin { name: $name, min: $min, max: $max, f: $f }),*]
    };
}
pub(crate) use builtin_table;

pub(crate) fn builtins() -> impl Iterator<Item = &'static Builtin> {
    [
        text::FNS,
        number::FNS,
        list::FNS,
        record::FNS,
        table::FNS,
        table_shape::FNS,
        date::FNS,
        misc::FNS,
        data::FNS,
        database::FNS,
        sources::FNS,
        splitter::FNS,
    ]
    .into_iter()
    .flatten()
}

fn num(n: f64) -> Value {
    Value::Number(n)
}

/// Enumerations, mathematical constants and the named types.
pub(crate) fn constants() -> Vec<(&'static str, Value)> {
    let number = |f| Value::Type(Type::Number(f));
    vec![
        ("Order.Ascending", num(0.0)),
        ("Order.Descending", num(1.0)),
        ("JoinKind.Inner", num(0.0)),
        ("JoinKind.LeftOuter", num(1.0)),
        ("JoinKind.RightOuter", num(2.0)),
        ("JoinKind.FullOuter", num(3.0)),
        ("JoinKind.LeftAnti", num(4.0)),
        ("JoinKind.RightAnti", num(5.0)),
        ("JoinKind.LeftSemi", num(6.0)),
        ("JoinKind.RightSemi", num(7.0)),
        ("MissingField.Error", num(0.0)),
        ("MissingField.Ignore", num(1.0)),
        ("MissingField.UseNull", num(2.0)),
        ("QuoteStyle.None", num(0.0)),
        ("QuoteStyle.Csv", num(1.0)),
        ("CsvStyle.QuoteAfterDelimiter", num(0.0)),
        ("CsvStyle.QuoteAlways", num(1.0)),
        ("ExtraValues.List", num(0.0)),
        ("ExtraValues.Error", num(1.0)),
        ("ExtraValues.Ignore", num(2.0)),
        ("Occurrence.First", num(0.0)),
        ("Occurrence.Last", num(1.0)),
        ("Occurrence.All", num(2.0)),
        ("Occurrence.Optional", num(0.0)),
        ("Occurrence.Required", num(1.0)),
        ("Occurrence.Repeating", num(2.0)),
        ("GroupKind.Local", num(0.0)),
        ("GroupKind.Global", num(1.0)),
        ("RoundingMode.Up", num(0.0)),
        ("RoundingMode.Down", num(1.0)),
        ("RoundingMode.AwayFromZero", num(2.0)),
        ("RoundingMode.TowardZero", num(3.0)),
        ("RoundingMode.ToEven", num(4.0)),
        ("Day.Sunday", num(0.0)),
        ("Day.Monday", num(1.0)),
        ("Day.Tuesday", num(2.0)),
        ("Day.Wednesday", num(3.0)),
        ("Day.Thursday", num(4.0)),
        ("Day.Friday", num(5.0)),
        ("Day.Saturday", num(6.0)),
        ("TextEncoding.Utf8", num(65001.0)),
        ("TextEncoding.Utf16", num(1200.0)),
        ("TextEncoding.Unicode", num(1200.0)),
        ("TextEncoding.BigEndianUnicode", num(1201.0)),
        ("TextEncoding.Ascii", num(20127.0)),
        ("TextEncoding.Windows", num(1252.0)),
        ("BinaryEncoding.Base64", num(0.0)),
        ("BinaryEncoding.Hex", num(1.0)),
        ("RelativePosition.FromStart", num(0.0)),
        ("RelativePosition.FromEnd", num(1.0)),
        ("JoinSide.Left", num(0.0)),
        ("JoinSide.Right", num(1.0)),
        ("Precision.Double", num(0.0)),
        ("Precision.Decimal", num(1.0)),
        ("BufferMode.Eager", num(1.0)),
        ("BufferMode.Delayed", num(2.0)),
        ("Number.PI", num(std::f64::consts::PI)),
        ("Number.E", num(std::f64::consts::E)),
        ("Number.Epsilon", num(4.94065645841247e-324)),
        ("Number.NaN", num(f64::NAN)),
        ("Number.PositiveInfinity", num(f64::INFINITY)),
        ("Number.NegativeInfinity", num(f64::NEG_INFINITY)),
        ("Any.Type", Value::Type(Type::Any)),
        ("None.Type", Value::Type(Type::None)),
        ("Null.Type", Value::Type(Type::Null)),
        ("Logical.Type", Value::Type(Type::Logical)),
        ("Number.Type", number(Facet::Double)),
        ("Double.Type", number(Facet::Double)),
        ("Decimal.Type", number(Facet::Decimal)),
        ("Currency.Type", number(Facet::Currency)),
        ("Percentage.Type", number(Facet::Percentage)),
        ("Single.Type", number(Facet::Single)),
        ("Int64.Type", number(Facet::Int64)),
        ("Int32.Type", number(Facet::Int32)),
        ("Int16.Type", number(Facet::Int16)),
        ("Int8.Type", number(Facet::Int8)),
        ("Byte.Type", number(Facet::Byte)),
        ("Text.Type", Value::Type(Type::Text)),
        ("Character.Type", Value::Type(Type::Text)),
        ("Guid.Type", Value::Type(Type::Text)),
        ("Uri.Type", Value::Type(Type::Text)),
        ("Date.Type", Value::Type(Type::Date)),
        ("Time.Type", Value::Type(Type::Time)),
        ("DateTime.Type", Value::Type(Type::DateTime)),
        ("DateTimeZone.Type", Value::Type(Type::DateTimeZone)),
        ("Duration.Type", Value::Type(Type::Duration)),
        ("Binary.Type", Value::Type(Type::Binary)),
        ("Function.Type", Value::Type(Type::Function)),
        ("Type.Type", Value::Type(Type::Type)),
        ("List.Type", Value::Type(Type::any_list())),
        ("Record.Type", Value::Type(Type::any_record())),
        ("Table.Type", Value::Type(Type::any_table())),
    ]
}

// ---------------------------------------------------------------- arguments

const NULL: &Value = &Value::Null;

/// Argument `i`, null when it wasn't given.
pub(crate) fn arg(a: &[Value], i: usize) -> &Value {
    a.get(i).unwrap_or(NULL)
}

pub(crate) fn mismatch<T>(v: &Value, want: &str) -> EResult<T> {
    raise(format!("A value of type {} was given where {want} is expected.", v.kind()))
}

pub(crate) fn text(a: &[Value], i: usize) -> EResult<Rc<str>> {
    match arg(a, i) {
        Value::Text(s) => Ok(s.clone()),
        v => mismatch(v, "text"),
    }
}

pub(crate) fn opt_text(a: &[Value], i: usize) -> EResult<Option<Rc<str>>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        Value::Text(s) => Ok(Some(s.clone())),
        v => mismatch(v, "text"),
    }
}

pub(crate) fn number(a: &[Value], i: usize) -> EResult<f64> {
    match arg(a, i) {
        Value::Number(n) => Ok(*n),
        v => mismatch(v, "a number"),
    }
}

pub(crate) fn opt_number(a: &[Value], i: usize) -> EResult<Option<f64>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(Some(*n)),
        v => mismatch(v, "a number"),
    }
}

fn whole(n: f64) -> EResult<i64> {
    if n.fract() != 0.0 || !n.is_finite() || n.abs() > 9.0e15 { raise("A whole number is expected.") } else { Ok(n as i64) }
}

pub(crate) fn int(a: &[Value], i: usize) -> EResult<i64> {
    whole(number(a, i)?)
}

pub(crate) fn opt_int(a: &[Value], i: usize) -> EResult<Option<i64>> {
    opt_number(a, i)?.map(whole).transpose()
}

/// A count or position: a whole number that isn't negative.
pub(crate) fn count(a: &[Value], i: usize) -> EResult<usize> {
    let n = int(a, i)?;
    if n < 0 { raise("A number that isn't negative is expected.") } else { Ok(n as usize) }
}

pub(crate) fn opt_count(a: &[Value], i: usize) -> EResult<Option<usize>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        _ => count(a, i).map(Some),
    }
}

pub(crate) fn opt_logical(a: &[Value], i: usize) -> EResult<Option<bool>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        Value::Logical(b) => Ok(Some(*b)),
        v => mismatch(v, "true or false"),
    }
}

pub(crate) fn list(a: &[Value], i: usize) -> EResult<Rc<ListData>> {
    match arg(a, i) {
        Value::List(l) => Ok(l.clone()),
        v => mismatch(v, "a list"),
    }
}

pub(crate) fn opt_list(a: &[Value], i: usize) -> EResult<Option<Rc<ListData>>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        Value::List(l) => Ok(Some(l.clone())),
        v => mismatch(v, "a list"),
    }
}

pub(crate) fn record(a: &[Value], i: usize) -> EResult<Rc<Record>> {
    match arg(a, i) {
        Value::Record(r) => Ok(r.clone()),
        v => mismatch(v, "a record"),
    }
}

pub(crate) fn opt_record(a: &[Value], i: usize) -> EResult<Option<Rc<Record>>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        Value::Record(r) => Ok(Some(r.clone())),
        v => mismatch(v, "a record"),
    }
}

pub(crate) fn table(a: &[Value], i: usize) -> EResult<Rc<Table>> {
    match arg(a, i) {
        Value::Table(t) => Ok(t.clone()),
        v => mismatch(v, "a table"),
    }
}

/// A function argument, as the value to call.
pub(crate) fn function(a: &[Value], i: usize) -> EResult<&Value> {
    match arg(a, i) {
        v @ Value::Function(_) => Ok(v),
        v => mismatch(v, "a function"),
    }
}

pub(crate) fn opt_function(a: &[Value], i: usize) -> EResult<Option<&Value>> {
    match arg(a, i) {
        Value::Null => Ok(None),
        v @ Value::Function(_) => Ok(Some(v)),
        v => mismatch(v, "a function"),
    }
}

pub(crate) fn type_arg(a: &[Value], i: usize) -> EResult<Type> {
    match arg(a, i) {
        Value::Type(t) => Ok(t.clone()),
        v => mismatch(v, "a type"),
    }
}

/// One text or a list of texts, as a list of names.
pub(crate) fn names(v: &Value) -> EResult<Vec<Rc<str>>> {
    match v {
        Value::Text(s) => Ok(vec![s.clone()]),
        Value::List(l) => l
            .items
            .iter()
            .map(|s| match s.get()? {
                Value::Text(s) => Ok(s),
                v => mismatch(&v, "a column name"),
            })
            .collect(),
        v => mismatch(v, "a column name or a list of column names"),
    }
}

/// An option of a record argument, null when the record or the field is missing.
pub(crate) fn option(r: Option<&Rc<Record>>, name: &str) -> EResult<Value> {
    match r {
        Some(r) => Ok(r.get(name)?.unwrap_or(Value::Null)),
        None => Ok(Value::Null),
    }
}
