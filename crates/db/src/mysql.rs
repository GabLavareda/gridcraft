//! MySQL and MariaDB, with mysql_async. Rows come in text form (the text protocol), read by the
//! column types the server reports.

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::prelude::Queryable as _;
use mysql_async::{OptsBuilder, SslOpts, Value};

use crate::text::{parse_date, parse_datetime, parse_duration};
use crate::{Cell, ColType, Collector, Column, Connect, Error, ErrorKind, Limits, Rows};

/// The character set number of binary strings.
const BINARY_CHARSET: u16 = 63;

pub(crate) struct Conn {
    rt: tokio::runtime::Runtime,
    conn: Option<mysql_async::Conn>,
    command_timeout: std::time::Duration,
}

fn error(e: mysql_async::Error) -> Error {
    match &e {
        // 1045: access denied; 1044: no access to the database.
        mysql_async::Error::Server(s) if s.code == 1045 || s.code == 1044 => {
            Error::new(ErrorKind::Login, format!("The server refused the user name or password: {}", s.message))
        }
        mysql_async::Error::Server(s) => Error::new(ErrorKind::Query, s.message.clone()),
        mysql_async::Error::Io(_) => Error::new(
            ErrorKind::Connect,
            format!(
                "{e}. If the server doesn't encrypt connections, turn encryption off; if it uses a certificate of its own, \
                 choose to trust the server certificate."
            ),
        ),
        _ => Error::new(ErrorKind::Connect, e.to_string()),
    }
}

impl Conn {
    pub(crate) fn open(c: &Connect) -> Result<Conn, Error> {
        let rt = crate::runtime()?;
        let mut opts = OptsBuilder::default()
            .ip_or_hostname(c.host.trim())
            .tcp_port(c.port_or_default())
            .user(Some(c.username.clone()))
            .pass(Some(c.password.clone()))
            .prefer_socket(false)
            .stmt_cache_size(0);
        if !c.database.is_empty() {
            opts = opts.db_name(Some(c.database.clone()));
        }
        if c.encrypt {
            opts = opts.ssl_opts(
                SslOpts::default()
                    .with_danger_accept_invalid_certs(c.trust_server_certificate)
                    .with_danger_skip_domain_validation(c.trust_server_certificate),
            );
        }
        let conn = crate::block_on(&rt, c.connect_timeout, "Connecting to the server", async { mysql_async::Conn::new(opts).await.map_err(error) })?;
        Ok(Conn { rt, conn: Some(conn), command_timeout: c.command_timeout })
    }

    pub(crate) fn query(&mut self, sql: &str, limits: &Limits) -> Result<Rows, Error> {
        let Some(conn) = self.conn.as_mut() else { return Err(Error::new(ErrorKind::Connect, "The connection is closed.")) };
        let limits = *limits;
        crate::block_on(&self.rt, self.command_timeout, "The statement", async move {
            let mut result = conn.query_iter(sql).await.map_err(error)?;
            let mut out: Option<(Vec<Kind>, Collector)> = None;
            // The first result set with columns; the others are read and dropped.
            for _ in 0..100_000 {
                if result.is_empty() {
                    break;
                }
                let columns = result.columns().unwrap_or_else(|| Vec::new().into());
                if out.is_none() && !columns.is_empty() {
                    let kinds: Vec<Kind> = columns.iter().map(kind).collect();
                    let cols = columns.iter().zip(&kinds).map(|(c, k)| Column { name: c.name_str().into_owned(), ty: k.col_type() }).collect();
                    let mut coll = Collector::new(limits, cols);
                    while let Some(mut row) = result.next().await.map_err(error)? {
                        if coll.full() {
                            continue;
                        }
                        let cells = kinds.iter().enumerate().map(|(i, k)| row.take::<Value, usize>(i).map_or(Cell::Null, |v| cell(*k, v))).collect();
                        coll.push(cells)?;
                    }
                    out = Some((kinds, coll));
                } else {
                    while result.next().await.map_err(error)?.is_some() {}
                }
            }
            result.drop_result().await.map_err(error)?;
            Ok(out.map(|(_, c)| c.out).unwrap_or_default())
        })
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // Says goodbye to the server while the runtime is still there.
        if let Some(conn) = self.conn.take() {
            let _ = crate::block_on(&self.rt, std::time::Duration::from_secs(2), "Disconnecting", async { conn.disconnect().await.map_err(error) });
        }
    }
}

/// How a column's text is read.
#[derive(Clone, Copy)]
enum Kind {
    Bool,
    Int,
    UInt,
    Float,
    Decimal,
    Date,
    DateTime,
    Time,
    Year,
    Bits,
    Bytes,
    Text,
}

impl Kind {
    fn col_type(self) -> ColType {
        match self {
            Kind::Bool => ColType::Logical,
            Kind::Int | Kind::Year => ColType::Int64,
            Kind::UInt | Kind::Float | Kind::Bits => ColType::Number,
            Kind::Decimal => ColType::Decimal,
            Kind::Date => ColType::Date,
            Kind::DateTime => ColType::DateTime,
            Kind::Time => ColType::Duration,
            Kind::Bytes => ColType::Binary,
            Kind::Text => ColType::Text,
        }
    }
}

fn kind(c: &mysql_async::Column) -> Kind {
    let binary = c.character_set() == BINARY_CHARSET;
    let unsigned = c.flags().contains(ColumnFlags::UNSIGNED_FLAG);
    match c.column_type() {
        // TINYINT(1) is how MySQL writes booleans.
        ColumnType::MYSQL_TYPE_TINY if c.column_length() == 1 => Kind::Bool,
        ColumnType::MYSQL_TYPE_LONGLONG if unsigned => Kind::UInt,
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_LONGLONG => Kind::Int,
        ColumnType::MYSQL_TYPE_YEAR => Kind::Year,
        ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => Kind::Float,
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => Kind::Decimal,
        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => Kind::Date,
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 | ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => {
            Kind::DateTime
        }
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => Kind::Time,
        ColumnType::MYSQL_TYPE_BIT => Kind::Bits,
        ColumnType::MYSQL_TYPE_GEOMETRY => Kind::Bytes,
        ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB
        | ColumnType::MYSQL_TYPE_VAR_STRING
        | ColumnType::MYSQL_TYPE_STRING
        | ColumnType::MYSQL_TYPE_VARCHAR
            if binary =>
        {
            Kind::Bytes
        }
        _ => Kind::Text,
    }
}

fn cell(k: Kind, v: Value) -> Cell {
    let bytes = match v {
        Value::NULL => return Cell::Null,
        Value::Bytes(b) => b,
        // The text protocol sends text; typed values only come from prepared statements.
        Value::Int(i) => return Cell::Int(i),
        Value::UInt(u) => return i64::try_from(u).map_or(Cell::Float(u as f64), Cell::Int),
        Value::Float(f) => return Cell::Float(f64::from(f)),
        Value::Double(f) => return Cell::Float(f),
        Value::Date(..) | Value::Time(..) => return Cell::Null,
    };
    if let Kind::Bytes = k {
        return Cell::Bytes(bytes);
    }
    if let Kind::Bits = k {
        // BIT(n) arrives as big-endian bytes.
        if bytes.len() <= 8 {
            let n = bytes.iter().fold(0u64, |acc, b| acc << 8 | u64::from(*b));
            return if bytes.len() == 1 && n <= 1 { Cell::Bool(n == 1) } else { Cell::Float(n as f64) };
        }
        return Cell::Bytes(bytes);
    }
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => return Cell::Bytes(e.into_bytes()),
    };
    let parsed = match k {
        Kind::Bool => match s.as_str() {
            "0" => Some(Cell::Bool(false)),
            "1" => Some(Cell::Bool(true)),
            _ => s.parse::<i64>().ok().map(Cell::Int),
        },
        Kind::Int | Kind::Year => s.parse::<i64>().ok().map(Cell::Int),
        Kind::UInt => s.parse::<u64>().ok().map(|u| i64::try_from(u).map_or(Cell::Float(u as f64), Cell::Int)),
        Kind::Float => s.parse::<f64>().ok().map(Cell::Float),
        Kind::Decimal => Some(Cell::Decimal(s.clone())),
        // Zero dates (0000-00-00) aren't dates: they read as null.
        Kind::Date if s.starts_with("0000-00-00") => Some(Cell::Null),
        Kind::Date => parse_date(&s).map(Cell::Date),
        Kind::DateTime if s.starts_with("0000-00-00") => Some(Cell::Null),
        Kind::DateTime => parse_datetime(&s).map(|(t, _)| Cell::DateTime(t)),
        // TIME is a duration in MySQL: it can pass 24 hours and go negative.
        Kind::Time => parse_duration(&s).map(Cell::Duration),
        Kind::Bits | Kind::Bytes | Kind::Text => None,
    };
    parsed.unwrap_or(Cell::Text(s))
}
