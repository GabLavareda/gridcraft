//! The syntax tree of M expressions.

use std::rc::Rc;

use crate::error::MError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Plus,
    Minus,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Coalesce,
    /// `x meta y`: metadata isn't kept, the value is `x`.
    Meta,
}

/// The primitive types, as named in type expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    Any,
    AnyNonNull,
    Binary,
    Date,
    DateTime,
    DateTimeZone,
    Duration,
    Function,
    List,
    Logical,
    None,
    Null,
    Number,
    Record,
    Table,
    Text,
    Time,
    Type,
}

impl Prim {
    pub fn from_name(s: &str) -> Option<Prim> {
        Some(match s {
            "any" => Prim::Any,
            "anynonnull" => Prim::AnyNonNull,
            "binary" => Prim::Binary,
            "date" => Prim::Date,
            "datetime" => Prim::DateTime,
            "datetimezone" => Prim::DateTimeZone,
            "duration" => Prim::Duration,
            "function" => Prim::Function,
            "list" => Prim::List,
            "logical" => Prim::Logical,
            "none" => Prim::None,
            "null" => Prim::Null,
            "number" => Prim::Number,
            "record" => Prim::Record,
            "table" => Prim::Table,
            "text" => Prim::Text,
            "time" => Prim::Time,
            "type" => Prim::Type,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FieldSpec {
    pub name: Rc<str>,
    pub optional: bool,
    pub ty: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    Prim(Prim),
    Nullable(Box<TypeExpr>),
    /// `{ item-type }`
    List(Box<TypeExpr>),
    /// `[ a = type, optional b = type, ... ]`; the flag is the open-record marker.
    Record(Vec<FieldSpec>, bool),
    /// `table [ a = type, ... ]`
    Table(Vec<FieldSpec>),
    /// `table expression`, where the expression gives the row type.
    TableOf(Box<Expr>),
    /// `function (params) as type`: only the kind is kept.
    Function,
    /// An expression that evaluates to a type value (`Int64.Type`).
    Expr(Box<Expr>),
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: Rc<str>,
    pub optional: bool,
    pub ty: Option<TypeExpr>,
}

#[derive(Debug)]
pub struct FuncDecl {
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Expr,
}

/// The members of a `let`, of a record literal or of a section: evaluated on demand, in any
/// order, and able to refer to each other.
#[derive(Debug)]
pub struct Bindings {
    pub names: Rc<[Rc<str>]>,
    pub exprs: Vec<Rc<Expr>>,
    /// Byte range of each member's expression in the source.
    pub spans: Vec<(usize, usize)>,
}

impl Bindings {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| &**n == name)
    }
}

#[derive(Clone, Debug)]
pub enum ListItem {
    One(Expr),
    /// `a..b`
    Range(Expr, Expr),
}

#[derive(Clone, Debug)]
pub enum TryHandler {
    /// `try x`: gives a `[HasError, Value | Error]` record.
    None,
    /// `try x otherwise y`
    Otherwise(Box<Expr>),
    /// `try x catch (e) => y` or `catch () => y`
    Catch(Rc<FuncDecl>),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Null,
    Logical(bool),
    Number(f64),
    Text(Rc<str>),
    /// `skip` is how many enclosing scopes that define `name` the lookup passes over: inside its
    /// own definition a member sees the outer name, unless it is written `@name` (`inclusive`).
    Ident {
        name: Rc<str>,
        skip: u8,
        inclusive: bool,
    },
    List(Vec<ListItem>),
    Record(Rc<Bindings>),
    Unary(UnOp, Box<Expr>),
    /// `a op b op c …` for operators of one precedence level, applied left to right. Kept flat so
    /// that a long chain (`a & b & c & …`) doesn't nest.
    Chain(Box<Expr>, Vec<(BinOp, Expr)>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Let(Rc<Bindings>, Box<Expr>),
    Function(Rc<FuncDecl>),
    Invoke(Box<Expr>, Vec<Expr>),
    /// `x[name]`, `x[name]?`; no target means the implicit `_`.
    Field {
        target: Option<Box<Expr>>,
        name: Rc<str>,
        optional: bool,
    },
    /// `x[[a],[b]]`
    Project {
        target: Option<Box<Expr>>,
        names: Vec<Rc<str>>,
        optional: bool,
    },
    /// `x{i}`, `x{i}?`
    Item {
        target: Box<Expr>,
        index: Box<Expr>,
        optional: bool,
    },
    Try(Box<Expr>, TryHandler),
    Raise(Box<Expr>),
    As(Box<Expr>, TypeExpr),
    Is(Box<Expr>, TypeExpr),
    Type(TypeExpr),
    /// `...`
    NotImplemented,
    /// A member that could not be parsed: evaluating it raises the error, so a broken query only
    /// fails the queries that use it.
    Failed(Rc<MError>),
}

/// One member of a section document (`shared Query1 = …;`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionMember {
    pub name: String,
    pub shared: bool,
    /// The member's expression, as written.
    pub formula: String,
}

/// A step of a query: one member of its top-level `let`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    /// Byte range of the step's expression in the query text.
    pub span: (usize, usize),
}

/// The steps of a query and the step its `in` clause returns.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Steps {
    pub steps: Vec<Step>,
    /// The step named by `in`, when the query ends with a step name.
    pub result: Option<String>,
}
