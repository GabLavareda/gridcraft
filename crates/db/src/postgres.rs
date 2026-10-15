//! PostgreSQL (and servers that speak its protocol), with tokio-postgres.
//!
//! Rows are read in text form (the simple query protocol), which every type has; the column
//! types come from preparing the statement first. The session is set to write dates in ISO form,
//! intervals in ISO 8601 and binary data in hex, which is what the conversions read.

use futures_util::TryStreamExt as _;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, SimpleQueryMessage};

use crate::text::{parse_date, parse_datetime, parse_duration, parse_time};
use crate::{Cell, ColType, Collector, Column, Connect, Error, ErrorKind, Limits, Rows};

pub(crate) struct Conn {
    rt: tokio::runtime::Runtime,
    client: Client,
    command_timeout: std::time::Duration,
}

/// The message and its causes, which tokio-postgres keeps apart.
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        let m = c.to_string();
        if !s.contains(&m) {
            s.push_str(": ");
            s.push_str(&m);
        }
        cur = c.source();
    }
    s
}

fn error(e: tokio_postgres::Error) -> Error {
    match e.as_db_error() {
        Some(db) if *db.code() == SqlState::INVALID_PASSWORD || *db.code() == SqlState::INVALID_AUTHORIZATION_SPECIFICATION => {
            Error::new(ErrorKind::Login, format!("The server refused the user name or password: {}", db.message()))
        }
        Some(db) => Error::new(ErrorKind::Query, db.message().to_string()),
        None if e.is_closed() => Error::new(ErrorKind::Connect, "The connection to the server closed."),
        None => Error::new(ErrorKind::Connect, chain(&e)),
    }
}

impl Conn {
    pub(crate) fn open(c: &Connect) -> Result<Conn, Error> {
        let rt = crate::runtime()?;
        let mut config = tokio_postgres::Config::new();
        config
            .host(c.host.trim())
            .port(c.port_or_default())
            .user(&c.username)
            .password(&c.password)
            .application_name("GridCraft")
            .connect_timeout(c.connect_timeout);
        if !c.database.is_empty() {
            config.dbname(&c.database);
        }
        let client = crate::block_on(&rt, c.connect_timeout, "Connecting to the server", async {
            if c.encrypt {
                config.ssl_mode(SslMode::Require);
                let tls = native_tls::TlsConnector::builder()
                    .danger_accept_invalid_certs(c.trust_server_certificate)
                    .danger_accept_invalid_hostnames(c.trust_server_certificate)
                    .build()
                    .map_err(|e| Error::new(ErrorKind::Connect, format!("Couldn't set up encryption: {e}")))?;
                let (client, conn) = config.connect(postgres_native_tls::MakeTlsConnector::new(tls)).await.map_err(|e| {
                    let mut err = error(e);
                    if err.kind == ErrorKind::Connect {
                        err.message.push_str(
                            ". If the server doesn't encrypt connections, turn encryption off; if it uses a certificate of its own, \
                             choose to trust the server certificate.",
                        );
                    }
                    err
                })?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                Ok(client)
            } else {
                config.ssl_mode(SslMode::Disable);
                let (client, conn) = config.connect(tokio_postgres::NoTls).await.map_err(error)?;
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                Ok(client)
            }
        })?;
        let conn = Conn { rt, client, command_timeout: c.command_timeout };
        let client = &conn.client;
        crate::block_on(&conn.rt, c.connect_timeout, "Setting up the session", async {
            client
                .batch_execute("SET DateStyle = ISO, YMD; SET IntervalStyle = iso_8601; SET bytea_output = hex; SET extra_float_digits = 3")
                .await
                .map_err(error)
        })?;
        Ok(conn)
    }

    pub(crate) fn query(&mut self, sql: &str, limits: &Limits) -> Result<Rows, Error> {
        let client = &self.client;
        let limits = *limits;
        crate::block_on(&self.rt, self.command_timeout, "The statement", async move {
            // Preparing tells the column types; a batch of statements can't be prepared, and its
            // columns stay untyped. Running it reports what is really wrong with the SQL.
            let types: Option<Vec<Type>> = client.prepare(sql).await.ok().map(|st| st.columns().iter().map(|c| c.type_().clone()).collect());
            let stream = client.simple_query_raw(sql).await.map_err(error)?;
            futures_util::pin_mut!(stream);
            let mut out: Option<(Vec<Type>, Collector)> = None;
            let mut done = false;
            while let Some(msg) = stream.try_next().await.map_err(error)? {
                match msg {
                    SimpleQueryMessage::RowDescription(cols) if out.is_none() => {
                        let tys: Vec<Type> = match &types {
                            Some(t) if t.len() == cols.len() => t.clone(),
                            _ => vec![Type::UNKNOWN; cols.len()],
                        };
                        let columns = cols.iter().zip(&tys).map(|(c, t)| Column { name: c.name().to_string(), ty: col_type(t) }).collect();
                        out = Some((tys, Collector::new(limits, columns)));
                    }
                    SimpleQueryMessage::Row(row) if !done => {
                        if let Some((tys, coll)) = &mut out
                            && !coll.full()
                        {
                            let cells = tys.iter().enumerate().map(|(i, t)| row.get(i).map_or(Cell::Null, |s| cell(t, s))).collect();
                            coll.push(cells)?;
                        }
                    }
                    // Rows of later statements are dropped.
                    SimpleQueryMessage::CommandComplete(_) if out.is_some() => done = true,
                    _ => {}
                }
            }
            Ok(out.map(|(_, c)| c.out).unwrap_or_default())
        })
    }
}

fn col_type(t: &Type) -> ColType {
    match *t {
        Type::BOOL => ColType::Logical,
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => ColType::Int64,
        Type::FLOAT4 | Type::FLOAT8 => ColType::Number,
        Type::NUMERIC => ColType::Decimal,
        Type::DATE => ColType::Date,
        Type::TIME | Type::TIMETZ => ColType::Time,
        Type::TIMESTAMP => ColType::DateTime,
        Type::TIMESTAMPTZ => ColType::DateTimeZone,
        Type::INTERVAL => ColType::Duration,
        Type::BYTEA => ColType::Binary,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::CHAR | Type::UUID | Type::JSON | Type::JSONB | Type::XML => ColType::Text,
        _ => ColType::Any,
    }
}

fn hex(s: &str) -> Option<Vec<u8>> {
    let h = s.strip_prefix("\\x")?.as_bytes();
    if h.len() % 2 != 0 {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    h.chunks(2).map(|p| Some(digit(*p.first()?)? << 4 | digit(*p.get(1)?)?)).collect()
}

/// A value from its text form; text that doesn't read as its type stays text.
fn cell(t: &Type, s: &str) -> Cell {
    let text = || Cell::Text(s.to_string());
    match *t {
        Type::BOOL => match s {
            "t" => Cell::Bool(true),
            "f" => Cell::Bool(false),
            _ => text(),
        },
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => s.parse().map_or_else(|_| text(), Cell::Int),
        Type::FLOAT4 | Type::FLOAT8 => match s {
            "NaN" => Cell::Float(f64::NAN),
            "Infinity" => Cell::Float(f64::INFINITY),
            "-Infinity" => Cell::Float(f64::NEG_INFINITY),
            _ => s.parse().map_or_else(|_| text(), Cell::Float),
        },
        Type::NUMERIC => match s {
            "NaN" => Cell::Float(f64::NAN),
            "Infinity" => Cell::Float(f64::INFINITY),
            "-Infinity" => Cell::Float(f64::NEG_INFINITY),
            _ => Cell::Decimal(s.to_string()),
        },
        Type::DATE => parse_date(s).map_or_else(text, Cell::Date),
        // The offset of a time with a time zone is dropped: M's time has none.
        Type::TIME | Type::TIMETZ => {
            let clock = s.split(['+', '-']).next().unwrap_or(s);
            parse_time(clock).map_or_else(text, Cell::Time)
        }
        Type::TIMESTAMP => match parse_datetime(s) {
            Some((t, _)) => Cell::DateTime(t),
            None => text(),
        },
        Type::TIMESTAMPTZ => match parse_datetime(s) {
            Some((t, off)) => Cell::DateTimeZone(t, off.unwrap_or(0)),
            None => text(),
        },
        Type::INTERVAL => parse_duration(s).map_or_else(text, Cell::Duration),
        Type::BYTEA => hex(s).map_or_else(text, Cell::Bytes),
        _ => text(),
    }
}
