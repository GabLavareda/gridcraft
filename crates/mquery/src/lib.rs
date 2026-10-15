//! A clean-room implementation of the Power Query M formula language: lexer, parser, evaluator
//! and standard library, written from the public language specification and function reference.
//!
//! The crate does no I/O: a query reaches the workbook, files and the clock only through the
//! [`Host`] it is evaluated with. Evaluation is single-threaded and its values (`Rc`-based) stay
//! on the thread; hosts copy what they need out of a result before the [`Interp`] goes.
//!
//! ```
//! use gridcraft_mquery::{Interp, NoHost, render};
//! let mut interp = Interp::new(&NoHost);
//! let v = interp.eval_text("List.Sum(List.Transform({1..4}, each _ * _))").unwrap();
//! assert_eq!(render(&v), "30");
//! ```
//!
//! Never-crash rules of the workspace apply with force here, since queries are arbitrary
//! programs: syntax trees and evaluation depth are bounded, values drop without recursion,
//! container sizes are capped, and sorting tolerates inconsistent comparers.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod ast;
pub mod culture;
pub mod datetime;
mod env;
pub mod error;
pub mod host;
mod interp;
pub mod lexer;
mod ops;
pub mod parser;
mod stdlib;
pub mod value;

pub use ast::{SectionMember, Step, Steps};
pub use culture::Culture;
pub use error::{EResult, Error, EvalError, Fatal, MError, ParseError};
pub use host::{DataSource, Database, Host, NoHost, Now, TableRef, WorkbookPart};
pub use interp::{DEFAULT_MAX_DEPTH, Interp, MAX_CELLS, MAX_ITEMS, MAX_TEXT};
pub use lexer::{quote_identifier, quote_text};
pub use parser::{parse, parse_section, steps};
pub use stdlib::data::table_from_csv;
pub use value::{
    Facet, Field, Function, Lazy, ListData, Names, Record, Row, Slot, Table, Type, Value, compare_values, number_to_text, render, values_equal,
};

/// The names of every library function and constant, sorted.
pub fn library_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = stdlib::builtins().map(|b| b.name).chain(stdlib::constants().into_iter().map(|(n, _)| n)).collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests;
