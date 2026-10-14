//! Errors: parse errors, M error values (what `error` raises and `try` catches) and the fatal
//! conditions that stop an evaluation (limits, cancellation).

use std::fmt;
use std::rc::Rc;

use crate::value::Value;

/// A syntax error, with the byte offset it was found at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub pos: usize,
}

impl ParseError {
    pub fn new(message: impl Into<String>, pos: usize) -> ParseError {
        ParseError { message: message.into(), pos }
    }
    /// 1-based line and column of the error in `src`.
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        let upto = src.get(..self.pos.min(src.len())).unwrap_or(src);
        let line = upto.bytes().filter(|b| *b == b'\n').count() + 1;
        let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, col)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// An M error value: `[Reason, Message, Detail]`.
#[derive(Clone)]
pub struct MError {
    pub reason: Rc<str>,
    pub message: Rc<str>,
    pub detail: Value,
}

impl fmt::Display for MError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.reason, self.message)
    }
}

impl fmt::Debug for MError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// Conditions that end an evaluation and that `try` can't catch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fatal {
    /// Expressions or calls nested deeper than the evaluator allows.
    TooDeep,
    /// A value grew past the size limit (list items, table cells, text length).
    TooLarge,
    /// The host asked to stop.
    Cancelled,
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Fatal::TooDeep => "The query nests expressions or calls too deeply to evaluate.",
            Fatal::TooLarge => "The query produced a value that is too large.",
            Fatal::Cancelled => "The evaluation was cancelled.",
        })
    }
}

/// Why an evaluation stopped.
#[derive(Clone, Debug)]
pub enum EvalError {
    /// An M error: `try` catches it, and containers keep it per item or cell.
    Raised(Rc<MError>),
    Fatal(Fatal),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::Raised(e) => write!(f, "{e}"),
            EvalError::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl std::error::Error for EvalError {}

pub type EResult<T> = Result<T, EvalError>;

/// Either kind of failure of `Interp::eval_text`.
#[derive(Clone, Debug)]
pub enum Error {
    Parse(ParseError),
    Eval(EvalError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "{e}"),
            Error::Eval(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Parse(e)
    }
}
impl From<EvalError> for Error {
    fn from(e: EvalError) -> Self {
        Error::Eval(e)
    }
}

pub const EXPRESSION_ERROR: &str = "Expression.Error";
pub const DATA_FORMAT_ERROR: &str = "DataFormat.Error";
pub const DATA_SOURCE_ERROR: &str = "DataSource.Error";

pub fn error(reason: &str, message: impl Into<String>) -> EvalError {
    EvalError::Raised(Rc::new(MError { reason: Rc::from(reason), message: Rc::from(message.into()), detail: Value::Null }))
}

pub fn error_with(reason: &str, message: impl Into<String>, detail: Value) -> EvalError {
    EvalError::Raised(Rc::new(MError { reason: Rc::from(reason), message: Rc::from(message.into()), detail }))
}

/// `Err` with an `Expression.Error`.
pub fn raise<T>(message: impl Into<String>) -> EResult<T> {
    Err(error(EXPRESSION_ERROR, message))
}

/// `Err` with a `DataFormat.Error` (a value that can't be read as the requested type).
pub fn raise_format<T>(message: impl Into<String>, detail: Value) -> EResult<T> {
    Err(error_with(DATA_FORMAT_ERROR, message, detail))
}
