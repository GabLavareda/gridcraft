//! SQLite files, with rusqlite and the SQLite it bundles. Files are opened read-only: a query
//! never changes one, and a file that isn't there isn't created.
//!
//! SQLite stores any value in any column; the declared type of a column decides how its values
//! are read, as SQLite's own type affinity does: dates and times written as ISO text in a column
//! declared `DATE`, `DATETIME` or `TIME` read as dates and times.

use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{OpenFlags, Statement};

use crate::text::{parse_date, parse_datetime, parse_time};
use crate::{Cell, ColType, Collector, Column, Connect, Error, ErrorKind, Limits, Rows};

pub(crate) struct Conn {
    conn: rusqlite::Connection,
    command_timeout: Duration,
}

fn error(e: rusqlite::Error) -> Error {
    match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => {
            Error::new(ErrorKind::Timeout, "The statement took too long.")
        }
        rusqlite::Error::SqliteFailure(f, msg) if matches!(f.code, rusqlite::ErrorCode::CannotOpen | rusqlite::ErrorCode::NotADatabase) => {
            Error::new(ErrorKind::Connect, msg.unwrap_or_else(|| f.to_string()))
        }
        e => Error::new(ErrorKind::Query, e.to_string()),
    }
}

impl Conn {
    pub(crate) fn open(c: &Connect) -> Result<Conn, Error> {
        let path = c.host.trim();
        if path.is_empty() {
            return Err(Error::new(ErrorKind::Connect, "The database file is missing."));
        }
        if !std::path::Path::new(path).is_file() {
            return Err(Error::new(ErrorKind::Connect, format!("There is no file at '{path}'.")));
        }
        let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(|e| Error::new(ErrorKind::Connect, format!("Couldn't open '{path}': {e}")))?;
        conn.busy_timeout(c.connect_timeout).map_err(error)?;
        // Reading the schema tells a SQLite database from any other file.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .map_err(|e| Error::new(ErrorKind::Connect, format!("'{path}' isn't a SQLite database: {e}")))?;
        Ok(Conn { conn, command_timeout: c.command_timeout })
    }

    // This module is built only off the web, where the clock is there.
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn query(&mut self, sql: &str, limits: &Limits) -> Result<Rows, Error> {
        let deadline = Instant::now() + self.command_timeout;
        self.conn.progress_handler(100_000, Some(move || Instant::now() > deadline)).map_err(error)?;
        let r = self.run(sql, limits);
        self.conn.progress_handler(0, None::<fn() -> bool>).map_err(error)?;
        r
    }

    fn run(&self, sql: &str, limits: &Limits) -> Result<Rows, Error> {
        let mut st: Statement<'_> = self.conn.prepare(sql).map_err(error)?;
        let kinds: Vec<Kind> = st.columns().iter().map(|c| kind(c.decl_type())).collect();
        let columns = st.columns().iter().zip(&kinds).map(|(c, k)| Column { name: c.name().to_string(), ty: k.col_type() }).collect();
        let mut coll = Collector::new(*limits, columns);
        let mut rows = st.query([]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            if coll.full() {
                break;
            }
            let mut cells = Vec::with_capacity(kinds.len());
            for (i, k) in kinds.iter().enumerate() {
                cells.push(cell(*k, row.get_ref(i).map_err(error)?));
            }
            coll.push(cells)?;
        }
        Ok(coll.out)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Any,
    Int,
    Real,
    Numeric,
    Bool,
    Text,
    Blob,
    Date,
    DateTime,
    Time,
}

impl Kind {
    fn col_type(self) -> ColType {
        match self {
            Kind::Any => ColType::Any,
            Kind::Int => ColType::Int64,
            Kind::Real => ColType::Number,
            Kind::Numeric => ColType::Decimal,
            Kind::Bool => ColType::Logical,
            Kind::Text => ColType::Text,
            Kind::Blob => ColType::Binary,
            Kind::Date => ColType::Date,
            Kind::DateTime => ColType::DateTime,
            Kind::Time => ColType::Time,
        }
    }
}

/// The declared type, read the way SQLite's affinity rules read it, plus dates and booleans.
fn kind(decl: Option<&str>) -> Kind {
    let Some(d) = decl else { return Kind::Any };
    let d = d.to_ascii_uppercase();
    if d.contains("BOOL") {
        Kind::Bool
    } else if d.contains("INT") {
        Kind::Int
    } else if d.contains("DATETIME") || d.contains("TIMESTAMP") {
        Kind::DateTime
    } else if d.contains("DATE") {
        Kind::Date
    } else if d.contains("TIME") {
        Kind::Time
    } else if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") {
        Kind::Text
    } else if d.contains("BLOB") {
        Kind::Blob
    } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
        Kind::Real
    } else if d.contains("DEC") || d.contains("NUMERIC") || d.contains("MONEY") {
        Kind::Numeric
    } else if d.is_empty() {
        Kind::Any
    } else {
        Kind::Numeric
    }
}

fn cell(k: Kind, v: ValueRef<'_>) -> Cell {
    match v {
        ValueRef::Null => Cell::Null,
        ValueRef::Integer(i) => match k {
            Kind::Bool => Cell::Bool(i != 0),
            Kind::Real => Cell::Float(i as f64),
            _ => Cell::Int(i),
        },
        ValueRef::Real(f) => Cell::Float(f),
        ValueRef::Blob(b) => Cell::Bytes(b.to_vec()),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            let parsed = match k {
                Kind::Date => parse_date(&s).map(Cell::Date).or_else(|| parse_datetime(&s).map(|(t, _)| Cell::DateTime(t))),
                Kind::DateTime => parse_datetime(&s).map(|(t, off)| match off {
                    Some(o) => Cell::DateTimeZone(t, o),
                    None => Cell::DateTime(t),
                }),
                Kind::Time => parse_time(&s).map(Cell::Time),
                Kind::Bool => match s.to_ascii_lowercase().as_str() {
                    "true" | "t" | "yes" => Some(Cell::Bool(true)),
                    "false" | "f" | "no" => Some(Cell::Bool(false)),
                    _ => None,
                },
                _ => None,
            };
            parsed.unwrap_or(Cell::Text(s))
        }
    }
}
