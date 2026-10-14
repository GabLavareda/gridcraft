//! M values.
//!
//! Containers (lists, records, table rows) hold [`Slot`]s: a value, or the error raised while
//! computing it, so that one bad cell doesn't fail the whole table until it is read.
//!
//! Values nest without limit (a list in a record in a list…), so the containers drop their
//! children through a queue instead of recursively: dropping a value a million levels deep must
//! not overflow the stack.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::rc::Rc;

use crate::ast::FuncDecl;
use crate::datetime;
use crate::env::Env;
use crate::error::{EResult, EvalError, Fatal, MError, raise};
use crate::interp::Interp;

/// Deepest nesting followed when comparing or printing values.
pub(crate) const MAX_VALUE_DEPTH: usize = 128;
/// Deepest nesting of a type inside a type.
const MAX_TYPE_DEPTH: u16 = 32;

pub type Row = Rc<[Slot]>;
pub type Names = Rc<[Rc<str>]>;

#[derive(Clone)]
pub enum Value {
    Null,
    Logical(bool),
    Number(f64),
    Text(Rc<str>),
    /// Days since 1970-01-01.
    Date(i32),
    /// Ticks (100 ns) since midnight.
    Time(i64),
    /// Ticks since 1970-01-01T00:00:00.
    DateTime(i64),
    /// Local ticks since 1970-01-01T00:00:00 and the offset from UTC in minutes.
    DateTimeZone(i64, i32),
    /// Ticks.
    Duration(i64),
    Binary(Rc<[u8]>),
    List(Rc<ListData>),
    Record(Rc<Record>),
    Table(Rc<Table>),
    Function(Rc<Function>),
    Type(Type),
}

/// An item of a list, a field of a record or a cell of a table.
#[derive(Clone)]
pub enum Slot {
    Val(Value),
    Err(Rc<MError>),
}

pub struct ListData {
    pub items: Vec<Slot>,
}

pub struct Record {
    pub names: Names,
    pub values: Row,
}

pub struct Table {
    pub columns: Names,
    /// One type per column.
    pub types: Rc<[Type]>,
    /// Every row has one slot per column.
    pub rows: Vec<Row>,
}

pub type BuiltinFn = fn(&mut Interp, &[Value]) -> EResult<Value>;

/// A library function.
pub struct Builtin {
    pub name: &'static str,
    /// Fewest and most arguments; `u8::MAX` for no upper bound.
    pub min: u8,
    pub max: u8,
    pub f: BuiltinFn,
}

pub struct Closure {
    pub decl: Rc<FuncDecl>,
    pub env: Rc<Env>,
}

pub enum Function {
    Closure(Closure),
    Builtin(&'static Builtin),
    /// A library function with its first arguments already given (what `Splitter.…` and
    /// `Combiner.…` return).
    Bound(&'static Builtin, Vec<Value>),
}

impl Function {
    /// How many arguments the function takes at least and at most.
    pub fn arity(&self) -> (usize, usize) {
        match self {
            Function::Closure(c) => (c.decl.params.iter().filter(|p| !p.optional).count(), c.decl.params.len()),
            Function::Builtin(b) => (b.min as usize, if b.max == u8::MAX { usize::MAX } else { b.max as usize }),
            Function::Bound(b, bound) => {
                let n = bound.len();
                ((b.min as usize).saturating_sub(n), if b.max == u8::MAX { usize::MAX } else { (b.max as usize).saturating_sub(n) })
            }
        }
    }
    pub fn builtin_name(&self) -> Option<&'static str> {
        match self {
            Function::Builtin(b) | Function::Bound(b, _) => Some(b.name),
            Function::Closure(_) => None,
        }
    }
}

// ---------------------------------------------------------------- types

/// Number types: what `Int64.Type`, `Currency.Type` and the like add to `number`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facet {
    Double,
    Decimal,
    Currency,
    Percentage,
    Single,
    Int64,
    Int32,
    Int16,
    Int8,
    Byte,
}

#[derive(Clone)]
pub struct Field {
    pub name: Rc<str>,
    pub ty: Type,
    pub optional: bool,
}

/// The parts of a compound type. `depth` bounds how far types nest.
pub struct Compound {
    depth: u16,
    pub fields: Vec<Field>,
    /// Open record, or a table/record type that says nothing about its fields.
    pub open: bool,
}

#[derive(Clone)]
pub enum Type {
    Any,
    AnyNonNull,
    None,
    Null,
    Logical,
    Number(Facet),
    Time,
    Date,
    DateTime,
    DateTimeZone,
    Duration,
    Text,
    Binary,
    Type,
    Function,
    /// One unnamed field: the type made nullable.
    Nullable(Rc<Compound>),
    /// One unnamed field: the item type.
    List(Rc<Compound>),
    Record(Rc<Compound>),
    /// The fields are the columns.
    Table(Rc<Compound>),
}

impl Type {
    fn depth(&self) -> u16 {
        match self {
            Type::Nullable(c) | Type::List(c) | Type::Record(c) | Type::Table(c) => c.depth,
            _ => 0,
        }
    }
    fn compound(fields: Vec<Field>, open: bool) -> EResult<Rc<Compound>> {
        let depth = 1 + fields.iter().map(|f| f.ty.depth()).max().unwrap_or(0);
        if depth > MAX_TYPE_DEPTH {
            return raise("This type nests too deeply.");
        }
        Ok(Rc::new(Compound { depth, fields, open }))
    }
    fn single(ty: Type) -> EResult<Rc<Compound>> {
        Type::compound(vec![Field { name: Rc::from(""), ty, optional: false }], false)
    }
    pub fn nullable(ty: Type) -> EResult<Type> {
        Ok(match ty {
            Type::Any | Type::Null | Type::Nullable(_) => ty,
            Type::AnyNonNull => Type::Any,
            ty => Type::Nullable(Type::single(ty)?),
        })
    }
    pub fn non_nullable(&self) -> Type {
        match self {
            Type::Nullable(c) => c.fields.first().map_or(Type::Any, |f| f.ty.clone()),
            Type::Any => Type::AnyNonNull,
            Type::Null => Type::None,
            t => t.clone(),
        }
    }
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Any | Type::Null | Type::Nullable(_))
    }
    pub fn list(item: Type) -> EResult<Type> {
        Ok(Type::List(Type::single(item)?))
    }
    pub fn record(fields: Vec<Field>, open: bool) -> EResult<Type> {
        Ok(Type::Record(Type::compound(fields, open)?))
    }
    /// A table type with these columns.
    pub fn table(columns: Vec<Field>) -> EResult<Type> {
        Ok(Type::Table(Type::compound(columns, false)?))
    }
    /// `type table`, `type record`, `type list`: the kind without details.
    pub fn any_table() -> Type {
        Type::Table(Rc::new(Compound { depth: 1, fields: Vec::new(), open: true }))
    }
    pub fn any_record() -> Type {
        Type::Record(Rc::new(Compound { depth: 1, fields: Vec::new(), open: true }))
    }
    pub fn any_list() -> Type {
        Type::List(Rc::new(Compound { depth: 1, fields: vec![Field { name: Rc::from(""), ty: Type::Any, optional: false }], open: false }))
    }
    /// The item type of a list type.
    pub fn item(&self) -> Type {
        match self {
            Type::List(c) => c.fields.first().map_or(Type::Any, |f| f.ty.clone()),
            _ => Type::Any,
        }
    }

    /// The name of the type's kind, for messages.
    pub fn name(&self) -> &'static str {
        match self {
            Type::Any => "Any",
            Type::AnyNonNull => "AnyNonNull",
            Type::None => "None",
            Type::Null => "Null",
            Type::Logical => "Logical",
            Type::Number(_) => "Number",
            Type::Time => "Time",
            Type::Date => "Date",
            Type::DateTime => "DateTime",
            Type::DateTimeZone => "DateTimeZone",
            Type::Duration => "Duration",
            Type::Text => "Text",
            Type::Binary => "Binary",
            Type::Type => "Type",
            Type::Function => "Function",
            Type::Nullable(c) => c.fields.first().map_or("Any", |f| f.ty.name()),
            Type::List(_) => "List",
            Type::Record(_) => "Record",
            Type::Table(_) => "Table",
        }
    }

    /// Whether `v` conforms to the type. Item, field and column types aren't checked.
    pub fn accepts(&self, v: &Value) -> bool {
        match self {
            Type::Any => true,
            Type::AnyNonNull => !matches!(v, Value::Null),
            Type::None => false,
            Type::Null => matches!(v, Value::Null),
            Type::Nullable(c) => matches!(v, Value::Null) || c.fields.first().is_none_or(|f| f.ty.accepts(v)),
            Type::Logical => matches!(v, Value::Logical(_)),
            Type::Number(_) => matches!(v, Value::Number(_)),
            Type::Time => matches!(v, Value::Time(_)),
            Type::Date => matches!(v, Value::Date(_)),
            Type::DateTime => matches!(v, Value::DateTime(_)),
            Type::DateTimeZone => matches!(v, Value::DateTimeZone(..)),
            Type::Duration => matches!(v, Value::Duration(_)),
            Type::Text => matches!(v, Value::Text(_)),
            Type::Binary => matches!(v, Value::Binary(_)),
            Type::Type => matches!(v, Value::Type(_)),
            Type::Function => matches!(v, Value::Function(_)),
            Type::List(_) => matches!(v, Value::List(_)),
            Type::Record(_) => matches!(v, Value::Record(_)),
            Type::Table(_) => matches!(v, Value::Table(_)),
        }
    }

    /// Whether every value of `self` is a value of `other` (`Type.Is`), by kind.
    pub fn is_compatible_with(&self, other: &Type) -> bool {
        match (self, other) {
            (_, Type::Any) => true,
            (Type::None, _) => true,
            (Type::Any, _) => false,
            (Type::Nullable(a), Type::Nullable(b)) => match (a.fields.first(), b.fields.first()) {
                (Some(x), Some(y)) => x.ty.is_compatible_with(&y.ty),
                _ => false,
            },
            (Type::Null, Type::Nullable(_)) => true,
            (a, Type::Nullable(b)) => b.fields.first().is_some_and(|y| a.is_compatible_with(&y.ty)),
            (Type::Nullable(_), _) => false,
            (a, Type::AnyNonNull) => !matches!(a, Type::Null),
            (a, b) => a.same_kind(b),
        }
    }

    fn same_kind(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Number(_), Type::Number(_)) => true,
            (Type::Nullable(_), Type::Nullable(_)) => true,
            (Type::List(_), Type::List(_)) => true,
            (Type::Record(_), Type::Record(_)) => true,
            (Type::Table(_), Type::Table(_)) => true,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        }
    }

    pub(crate) fn equals(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Number(a), Type::Number(b)) => a == b,
            (Type::Nullable(a), Type::Nullable(b))
            | (Type::List(a), Type::List(b))
            | (Type::Record(a), Type::Record(b))
            | (Type::Table(a), Type::Table(b)) => {
                Rc::ptr_eq(a, b)
                    || (a.open == b.open
                        && a.fields.len() == b.fields.len()
                        && a.fields.iter().zip(&b.fields).all(|(x, y)| x.name == y.name && x.optional == y.optional && x.ty.equals(&y.ty)))
            }
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        }
    }
}

// ---------------------------------------------------------------- construction and access

impl From<Value> for Slot {
    fn from(v: Value) -> Slot {
        Slot::Val(v)
    }
}

impl Slot {
    pub const NULL: Slot = Slot::Val(Value::Null);

    /// The value, or the error kept in its place.
    pub fn get(&self) -> EResult<Value> {
        match self {
            Slot::Val(v) => Ok(v.clone()),
            Slot::Err(e) => Err(EvalError::Raised(e.clone())),
        }
    }
    pub fn value(&self) -> Option<&Value> {
        match self {
            Slot::Val(v) => Some(v),
            Slot::Err(_) => None,
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Slot::Err(_))
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Slot::Val(Value::Null))
    }
    /// Keeps an M error in the slot; fatal conditions still stop the evaluation.
    pub fn capture(r: EResult<Value>) -> EResult<Slot> {
        match r {
            Ok(v) => Ok(Slot::Val(v)),
            Err(EvalError::Raised(e)) => Ok(Slot::Err(e)),
            Err(e @ EvalError::Fatal(_)) => Err(e),
        }
    }
}

impl Value {
    pub fn text(s: impl AsRef<str>) -> Value {
        Value::Text(Rc::from(s.as_ref()))
    }
    pub fn list(items: Vec<Slot>) -> Value {
        Value::List(Rc::new(ListData { items }))
    }
    pub fn list_of(values: Vec<Value>) -> Value {
        Value::List(Rc::new(ListData { items: values.into_iter().map(Slot::Val).collect() }))
    }
    pub fn record(names: Names, values: Row) -> Value {
        Value::Record(Rc::new(Record { names, values }))
    }
    /// A record from name/value pairs.
    pub fn record_of(fields: Vec<(&str, Value)>) -> Value {
        let names: Names = fields.iter().map(|(n, _)| Rc::from(*n)).collect();
        let values: Row = fields.into_iter().map(|(_, v)| Slot::Val(v)).collect();
        Value::record(names, values)
    }
    pub fn table(t: Table) -> Value {
        Value::Table(Rc::new(t))
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The type of the value, as `Value.Type` reports it.
    pub fn type_of(&self) -> Type {
        match self {
            Value::Null => Type::Null,
            Value::Logical(_) => Type::Logical,
            Value::Number(_) => Type::Number(Facet::Double),
            Value::Text(_) => Type::Text,
            Value::Date(_) => Type::Date,
            Value::Time(_) => Type::Time,
            Value::DateTime(_) => Type::DateTime,
            Value::DateTimeZone(..) => Type::DateTimeZone,
            Value::Duration(_) => Type::Duration,
            Value::Binary(_) => Type::Binary,
            Value::List(_) => Type::any_list(),
            Value::Record(_) => Type::any_record(),
            Value::Table(t) => t.table_type(),
            Value::Function(_) => Type::Function,
            Value::Type(_) => Type::Type,
        }
    }
    /// The name of the value's kind, for messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "Null",
            Value::Logical(_) => "Logical",
            Value::Number(_) => "Number",
            Value::Text(_) => "Text",
            Value::Date(_) => "Date",
            Value::Time(_) => "Time",
            Value::DateTime(_) => "DateTime",
            Value::DateTimeZone(..) => "DateTimeZone",
            Value::Duration(_) => "Duration",
            Value::Binary(_) => "Binary",
            Value::List(_) => "List",
            Value::Record(_) => "Record",
            Value::Table(_) => "Table",
            Value::Function(_) => "Function",
            Value::Type(_) => "Type",
        }
    }
    /// Rank of the kind in the order used to sort values of different kinds.
    fn kind_rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Logical(_) => 1,
            Value::Number(_) => 2,
            Value::Time(_) => 3,
            Value::Date(_) => 4,
            Value::DateTime(_) => 5,
            Value::DateTimeZone(..) => 6,
            Value::Duration(_) => 7,
            Value::Text(_) => 8,
            Value::Binary(_) => 9,
            Value::List(_) => 10,
            Value::Record(_) => 11,
            Value::Table(_) => 12,
            Value::Function(_) => 13,
            Value::Type(_) => 14,
        }
    }
}

impl ListData {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// All items as values; the first error among them is raised.
    pub fn values(&self) -> EResult<Vec<Value>> {
        self.items.iter().map(Slot::get).collect()
    }
}

impl Record {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| &**n == name)
    }
    pub fn slot(&self, name: &str) -> Option<&Slot> {
        self.values.get(self.index_of(name)?)
    }
    /// The field's value: `None` when there is no such field.
    pub fn get(&self, name: &str) -> EResult<Option<Value>> {
        self.slot(name).map(Slot::get).transpose()
    }
    pub fn len(&self) -> usize {
        self.names.len()
    }
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &Slot)> {
        self.names.iter().zip(self.values.iter())
    }
}

impl Table {
    /// A table whose columns have no particular type.
    pub fn untyped(columns: Names, rows: Vec<Row>) -> Table {
        let types = columns.iter().map(|_| Type::Any).collect();
        Table { columns, types, rows }
    }
    pub fn empty() -> Table {
        Table { columns: Rc::from(Vec::new()), types: Rc::from(Vec::new()), rows: Vec::new() }
    }
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|n| &**n == name)
    }
    pub fn width(&self) -> usize {
        self.columns.len()
    }
    /// Row `i` as a record that shares the row's cells.
    pub fn row_record(&self, i: usize) -> Option<Value> {
        self.rows.get(i).map(|r| Value::record(self.columns.clone(), r.clone()))
    }
    pub fn table_type(&self) -> Type {
        let fields = self.columns.iter().zip(self.types.iter()).map(|(n, t)| Field { name: n.clone(), ty: t.clone(), optional: false }).collect();
        Type::table(fields).unwrap_or_else(|_| Type::any_table())
    }
}

// ---------------------------------------------------------------- equality and order

fn slots_equal(a: &Slot, b: &Slot, depth: usize) -> EResult<bool> {
    values_equal_at(&a.get()?, &b.get()?, depth)
}

/// The `=` operator.
pub fn values_equal(a: &Value, b: &Value) -> EResult<bool> {
    values_equal_at(a, b, 0)
}

fn values_equal_at(a: &Value, b: &Value, depth: usize) -> EResult<bool> {
    if depth > MAX_VALUE_DEPTH {
        return Err(EvalError::Fatal(Fatal::TooDeep));
    }
    Ok(match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Logical(x), Value::Logical(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Date(x), Value::Date(y)) => x == y,
        (Value::Time(x), Value::Time(y)) => x == y,
        (Value::DateTime(x), Value::DateTime(y)) => x == y,
        (Value::DateTimeZone(x, ox), Value::DateTimeZone(y, oy)) => datetime::utc_ticks(*x, *ox) == datetime::utc_ticks(*y, *oy),
        (Value::Duration(x), Value::Duration(y)) => x == y,
        (Value::Binary(x), Value::Binary(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            if Rc::ptr_eq(x, y) {
                return Ok(true);
            }
            if x.len() != y.len() {
                return Ok(false);
            }
            for (p, q) in x.items.iter().zip(&y.items) {
                if !slots_equal(p, q, depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Record(x), Value::Record(y)) => {
            if Rc::ptr_eq(x, y) {
                return Ok(true);
            }
            if x.len() != y.len() {
                return Ok(false);
            }
            for (name, p) in x.iter() {
                let Some(q) = y.slot(name) else { return Ok(false) };
                if !slots_equal(p, q, depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Table(x), Value::Table(y)) => {
            if Rc::ptr_eq(x, y) {
                return Ok(true);
            }
            if x.width() != y.width() || x.rows.len() != y.rows.len() {
                return Ok(false);
            }
            let mut map = Vec::with_capacity(x.width());
            for name in x.columns.iter() {
                let Some(j) = y.column_index(name) else { return Ok(false) };
                map.push(j);
            }
            for (r, s) in x.rows.iter().zip(&y.rows) {
                for (i, j) in map.iter().enumerate() {
                    let (Some(p), Some(q)) = (r.get(i), s.get(*j)) else { return Ok(false) };
                    if !slots_equal(p, q, depth + 1)? {
                        return Ok(false);
                    }
                }
            }
            true
        }
        (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
        (Value::Type(x), Value::Type(y)) => x.equals(y),
        _ => false,
    })
}

/// A total order over all values, for sorting: by kind first, then by value. Numbers put NaN
/// first; text compares by code point.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    compare_at(a, b, 0)
}

fn compare_slots(a: &Slot, b: &Slot, depth: usize) -> Ordering {
    match (a, b) {
        (Slot::Val(x), Slot::Val(y)) => compare_at(x, y, depth),
        (Slot::Err(_), Slot::Err(_)) => Ordering::Equal,
        (Slot::Err(_), _) => Ordering::Greater,
        (_, Slot::Err(_)) => Ordering::Less,
    }
}

fn compare_at(a: &Value, b: &Value, depth: usize) -> Ordering {
    if depth > MAX_VALUE_DEPTH {
        return Ordering::Equal;
    }
    match (a, b) {
        (Value::Logical(x), Value::Logical(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y).unwrap_or_else(|| y.is_nan().cmp(&x.is_nan())),
        (Value::Text(x), Value::Text(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Time(x), Value::Time(y)) | (Value::DateTime(x), Value::DateTime(y)) | (Value::Duration(x), Value::Duration(y)) => x.cmp(y),
        (Value::DateTimeZone(x, ox), Value::DateTimeZone(y, oy)) => datetime::utc_ticks(*x, *ox).cmp(&datetime::utc_ticks(*y, *oy)),
        (Value::Binary(x), Value::Binary(y)) => x.cmp(y),
        (Value::List(x), Value::List(y)) => {
            for (p, q) in x.items.iter().zip(&y.items) {
                let o = compare_slots(p, q, depth + 1);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Value::Record(x), Value::Record(y)) => {
            for (p, q) in x.values.iter().zip(y.values.iter()) {
                let o = compare_slots(p, q, depth + 1);
                if o != Ordering::Equal {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => a.kind_rank().cmp(&b.kind_rank()),
    }
}

/// The order behind `<`, `<=`, `>` and `>=`: `None` when either side is null or a NaN, an error
/// when the kinds can't be compared.
pub fn relational(a: &Value, b: &Value) -> EResult<Option<Ordering>> {
    Ok(match (a, b) {
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y),
        (Value::Logical(_), Value::Logical(_))
        | (Value::Text(_), Value::Text(_))
        | (Value::Date(_), Value::Date(_))
        | (Value::Time(_), Value::Time(_))
        | (Value::DateTime(_), Value::DateTime(_))
        | (Value::DateTimeZone(..), Value::DateTimeZone(..))
        | (Value::Duration(_), Value::Duration(_))
        | (Value::Binary(_), Value::Binary(_)) => Some(compare_values(a, b)),
        _ => return raise(format!("Values of type {} and {} can't be compared.", a.kind(), b.kind())),
    })
}

// ---------------------------------------------------------------- text

/// A number as text, independent of culture: the shortest digits that read back as the same
/// number, in scientific notation outside 1E-5…1E15.
pub fn number_to_text(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "∞".into() } else { "-∞".into() };
    }
    if n == 0.0 {
        return "0".into();
    }
    let sci = format!("{n:e}");
    let Some((mantissa, exp)) = sci.split_once('e') else { return sci };
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-5..15).contains(&exp) {
        return format!("{n}");
    }
    format!("{mantissa}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
}

/// The value as M source where that is possible: the form the tests and the nested-value preview
/// use. Functions and types print as placeholders; errors inside containers as `error "…"`.
pub fn render(v: &Value) -> String {
    let mut out = String::new();
    render_into(v, &mut out, 0);
    out
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&render(self))
    }
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        render_slot(self, &mut out, 0);
        f.write_str(&out)
    }
}

fn render_slot(s: &Slot, out: &mut String, depth: usize) {
    match s {
        Slot::Val(v) => render_into(v, out, depth),
        Slot::Err(e) => {
            out.push_str("error ");
            out.push_str(&crate::lexer::quote_text(&e.message));
        }
    }
}

fn render_into(v: &Value, out: &mut String, depth: usize) {
    use std::fmt::Write as _;
    if depth > MAX_VALUE_DEPTH || out.len() > (1 << 24) {
        out.push('…');
        return;
    }
    match v {
        Value::Null => out.push_str("null"),
        Value::Logical(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_nan() {
                out.push_str("#nan");
            } else if n.is_infinite() {
                out.push_str(if *n > 0.0 { "#infinity" } else { "-#infinity" });
            } else {
                out.push_str(&number_to_text(*n));
            }
        }
        Value::Text(s) => out.push_str(&crate::lexer::quote_text(s)),
        Value::Date(d) => {
            let (y, m, dd) = datetime::civil_from_days(*d as i64);
            let _ = write!(out, "#date({y}, {m}, {dd})");
        }
        Value::Time(t) => {
            let (h, mi, s) = datetime::time_parts(*t);
            let _ = write!(out, "#time({h}, {mi}, {})", number_to_text(s));
        }
        Value::DateTime(t) => {
            let (days, tod) = datetime::split_ticks(*t);
            let (y, m, dd) = datetime::civil_from_days(days);
            let (h, mi, s) = datetime::time_parts(tod);
            let _ = write!(out, "#datetime({y}, {m}, {dd}, {h}, {mi}, {})", number_to_text(s));
        }
        Value::DateTimeZone(t, off) => {
            let (days, tod) = datetime::split_ticks(*t);
            let (y, m, dd) = datetime::civil_from_days(days);
            let (h, mi, s) = datetime::time_parts(tod);
            let _ = write!(out, "#datetimezone({y}, {m}, {dd}, {h}, {mi}, {}, {}, {})", number_to_text(s), off / 60, off % 60);
        }
        Value::Duration(t) => {
            let (d, h, mi, s) = datetime::duration_parts(*t);
            let _ = write!(out, "#duration({d}, {h}, {mi}, {})", number_to_text(s));
        }
        Value::Binary(b) => {
            let _ = write!(out, "#binary({} bytes)", b.len());
        }
        Value::List(l) => {
            out.push('{');
            for (i, s) in l.items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_slot(s, out, depth + 1);
            }
            out.push('}');
        }
        Value::Record(r) => {
            out.push('[');
            for (i, (n, s)) in r.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&crate::lexer::quote_identifier(n));
                out.push_str(" = ");
                render_slot(s, out, depth + 1);
            }
            out.push(']');
        }
        Value::Table(t) => {
            out.push_str("#table({");
            for (i, c) in t.columns.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&crate::lexer::quote_text(c));
            }
            out.push_str("}, {");
            for (i, r) in t.rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push('{');
                for (j, s) in r.iter().enumerate() {
                    if j > 0 {
                        out.push_str(", ");
                    }
                    render_slot(s, out, depth + 1);
                }
                out.push('}');
            }
            out.push_str("})");
        }
        Value::Function(_) => out.push_str("[Function]"),
        Value::Type(t) => {
            out.push_str("type ");
            out.push_str(&t.name().to_ascii_lowercase());
        }
    }
}

// ---------------------------------------------------------------- dropping without recursion

/// Something waiting to be dropped: held only for that.
#[allow(dead_code)]
pub(crate) enum Deferred {
    Slot(Slot),
    Env(Rc<Env>),
}

thread_local! {
    static QUEUE: RefCell<Vec<Deferred>> = const { RefCell::new(Vec::new()) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

struct Draining;
impl Drop for Draining {
    fn drop(&mut self) {
        let _ = DRAINING.try_with(|f| f.set(false));
    }
}

/// Drops `d` from a queue rather than in place: its own children queue up behind it, so a deep
/// value unwinds in a loop. Outside the thread's lifetime the value drops in place.
pub(crate) fn defer(d: Deferred) {
    let queued = QUEUE.try_with(move |q| {
        if let Ok(mut q) = q.try_borrow_mut() {
            q.push(d);
        }
    });
    if queued.is_err() || DRAINING.try_with(|f| f.replace(true)).unwrap_or(true) {
        return;
    }
    let _guard = Draining;
    loop {
        let next = QUEUE.try_with(|q| q.try_borrow_mut().ok().and_then(|mut q| q.pop())).ok().flatten();
        match next {
            Some(item) => drop(item),
            None => break,
        }
    }
}

impl Value {
    /// Whether dropping this value frees a container that may hold more containers.
    pub(crate) fn needs_defer(&self) -> bool {
        match self {
            Value::List(r) => Rc::strong_count(r) == 1 && !r.items.is_empty(),
            Value::Record(r) => Rc::strong_count(r) == 1 && Rc::strong_count(&r.values) == 1 && !r.values.is_empty(),
            Value::Table(r) => Rc::strong_count(r) == 1 && !r.rows.is_empty(),
            Value::Function(r) => Rc::strong_count(r) == 1 && !matches!(**r, Function::Builtin(_)),
            _ => false,
        }
    }
    fn is_container(&self) -> bool {
        matches!(self, Value::List(_) | Value::Record(_) | Value::Table(_) | Value::Function(_))
    }
}

impl Slot {
    pub(crate) fn needs_defer(&self) -> bool {
        match self {
            Slot::Val(v) => v.needs_defer(),
            Slot::Err(e) => error_needs_defer(e),
        }
    }
    /// Moves a slot that owns a container to the drop queue, leaving null behind.
    fn unlink(&mut self) {
        if self.needs_defer() {
            defer(Deferred::Slot(std::mem::replace(self, Slot::NULL)));
        }
    }
}

/// Whether dropping this error frees a container kept as its detail.
pub(crate) fn error_needs_defer(e: &Rc<MError>) -> bool {
    Rc::strong_count(e) == 1 && e.detail.is_container()
}

impl Drop for ListData {
    fn drop(&mut self) {
        for s in &mut self.items {
            s.unlink();
        }
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if let Some(cells) = Rc::get_mut(&mut self.values) {
            for s in cells {
                s.unlink();
            }
        }
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        for row in &mut self.rows {
            if let Some(cells) = Rc::get_mut(row) {
                for s in cells {
                    s.unlink();
                }
            }
        }
    }
}
