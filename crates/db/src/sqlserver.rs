//! SQL Server (and Azure SQL) over TDS, with tiberius.

use futures_util::TryStreamExt as _;
use tiberius::{AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt as _};

use crate::{Cell, ColType, Collector, Column, Connect, Error, ErrorKind, Limits, Rows, TICKS_PER_DAY, TICKS_PER_SECOND};

/// 1900-01-01, where `datetime` counts days from, in days since 1970-01-01.
const DAY_1900: i64 = -25_567;
/// 0001-01-01, where `date` counts days from.
const DAY_1: i64 = -719_162;

pub(crate) struct Conn {
    rt: tokio::runtime::Runtime,
    client: Client<Compat<TcpStream>>,
    command_timeout: std::time::Duration,
}

fn error(e: tiberius::error::Error) -> Error {
    match &e {
        tiberius::error::Error::Server(t) if t.code() == 18456 => {
            Error::new(ErrorKind::Login, format!("The server refused the user name or password: {}", t.message()))
        }
        tiberius::error::Error::Server(t) => Error::new(ErrorKind::Query, t.message().to_string()),
        tiberius::error::Error::Io { .. } => Error::new(ErrorKind::Connect, e.to_string()),
        tiberius::error::Error::Tls(m) => Error::new(
            ErrorKind::Connect,
            format!(
                "The encrypted connection failed ({m}). If the server uses a certificate of its own, \
                 choose to trust the server certificate."
            ),
        ),
        _ => Error::new(ErrorKind::Query, e.to_string()),
    }
}

fn config(c: &Connect, host: &str, port: u16) -> Config {
    let mut config = Config::new();
    config.host(host);
    config.port(port);
    if !c.database.is_empty() {
        config.database(&c.database);
    }
    config.application_name("GridCraft");
    config.authentication(AuthMethod::sql_server(&c.username, &c.password));
    if c.encrypt {
        config.encryption(EncryptionLevel::Required);
        if c.trust_server_certificate {
            config.trust_cert();
        }
    } else {
        // The login is still encrypted, as other SQL Server clients do; the certificate isn't
        // checked when the rest of the connection isn't encrypted anyway.
        config.encryption(EncryptionLevel::Off);
        config.trust_cert();
    }
    config
}

async fn connect(config: Config) -> Result<Client<Compat<TcpStream>>, tiberius::error::Error> {
    let tcp = TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    Client::connect(config, tcp.compat_write()).await
}

impl Conn {
    pub(crate) fn open(c: &Connect) -> Result<Conn, Error> {
        let rt = crate::runtime()?;
        let host = c.host.trim().to_string();
        let port = c.port_or_default();
        let client = crate::block_on(&rt, c.connect_timeout, "Connecting to the server", async {
            match connect(config(c, &host, port)).await {
                // Azure asks to reconnect elsewhere, once.
                Err(tiberius::error::Error::Routing { host, port }) => connect(config(c, &host, port)).await.map_err(error),
                r => r.map_err(error),
            }
        })?;
        Ok(Conn { rt, client, command_timeout: c.command_timeout })
    }

    pub(crate) fn query(&mut self, sql: &str, limits: &Limits) -> Result<Rows, Error> {
        let client = &mut self.client;
        let limits = *limits;
        crate::block_on(&self.rt, self.command_timeout, "The statement", async move {
            let mut stream = client.simple_query(sql).await.map_err(error)?;
            // The first result set that has columns; later ones are read and dropped.
            let mut out: Option<(usize, Collector)> = None;
            while let Some(item) = stream.try_next().await.map_err(error)? {
                match item {
                    QueryItem::Metadata(meta) => {
                        if out.is_none() {
                            let columns =
                                meta.columns().iter().map(|c| Column { name: c.name().to_string(), ty: col_type(c.column_type()) }).collect();
                            out = Some((meta.result_index(), Collector::new(limits, columns)));
                        }
                    }
                    QueryItem::Row(row) => {
                        if let Some((index, coll)) = &mut out
                            && row.result_index() == *index
                            && !coll.full()
                        {
                            let cells = row.cells().map(|(_, d)| cell(d)).collect();
                            coll.push(cells)?;
                        }
                    }
                }
            }
            Ok(out.map(|(_, c)| c.out).unwrap_or_default())
        })
    }
}

fn col_type(t: ColumnType) -> ColType {
    match t {
        ColumnType::Bit | ColumnType::Bitn => ColType::Logical,
        ColumnType::Int1 | ColumnType::Int2 | ColumnType::Int4 | ColumnType::Int8 | ColumnType::Intn => ColType::Int64,
        ColumnType::Float4 | ColumnType::Float8 | ColumnType::Floatn => ColType::Number,
        ColumnType::Money | ColumnType::Money4 => ColType::Currency,
        ColumnType::Decimaln | ColumnType::Numericn => ColType::Decimal,
        ColumnType::Datetime4 | ColumnType::Datetime | ColumnType::Datetimen | ColumnType::Datetime2 => ColType::DateTime,
        ColumnType::Daten => ColType::Date,
        ColumnType::Timen => ColType::Time,
        ColumnType::DatetimeOffsetn => ColType::DateTimeZone,
        ColumnType::Guid
        | ColumnType::BigVarChar
        | ColumnType::BigChar
        | ColumnType::NVarchar
        | ColumnType::NChar
        | ColumnType::Xml
        | ColumnType::Text
        | ColumnType::NText => ColType::Text,
        ColumnType::BigVarBin | ColumnType::BigBinary | ColumnType::Image => ColType::Binary,
        _ => ColType::Any,
    }
}

/// Ticks of a `time` value kept as increments of 10^-scale seconds.
fn time_ticks(increments: u64, scale: u8) -> i64 {
    let t =
        if scale <= 7 { increments.saturating_mul(10u64.pow(u32::from(7 - scale))) } else { increments / 10u64.saturating_pow(u32::from(scale - 7)) };
    i64::try_from(t).unwrap_or(i64::MAX).min(TICKS_PER_DAY - 1)
}

fn datetime2_ticks(dt: tiberius::time::DateTime2) -> i64 {
    (DAY_1 + i64::from(dt.date().days())) * TICKS_PER_DAY + time_ticks(dt.time().increments(), dt.time().scale())
}

fn cell(d: &ColumnData<'static>) -> Cell {
    match d {
        ColumnData::U8(v) => v.map_or(Cell::Null, |v| Cell::Int(i64::from(v))),
        ColumnData::I16(v) => v.map_or(Cell::Null, |v| Cell::Int(i64::from(v))),
        ColumnData::I32(v) => v.map_or(Cell::Null, |v| Cell::Int(i64::from(v))),
        ColumnData::I64(v) => v.map_or(Cell::Null, Cell::Int),
        ColumnData::F32(v) => v.map_or(Cell::Null, |v| Cell::Float(f64::from(v))),
        ColumnData::F64(v) => v.map_or(Cell::Null, Cell::Float),
        ColumnData::Bit(v) => v.map_or(Cell::Null, Cell::Bool),
        ColumnData::String(v) => v.as_ref().map_or(Cell::Null, |s| Cell::Text(s.to_string())),
        ColumnData::Guid(v) => v.map_or(Cell::Null, |g| Cell::Text(g.to_string().to_ascii_uppercase())),
        ColumnData::Binary(v) => v.as_ref().map_or(Cell::Null, |b| Cell::Bytes(b.to_vec())),
        ColumnData::Numeric(v) => v.map_or(Cell::Null, |n| Cell::Decimal(n.to_string())),
        ColumnData::Xml(v) => v.as_ref().map_or(Cell::Null, |x| Cell::Text(x.to_string())),
        ColumnData::DateTime(v) => v.map_or(Cell::Null, |dt| {
            // Days since 1900 and 1/300 seconds since midnight.
            let frac = (i64::from(dt.seconds_fragments()) * TICKS_PER_SECOND + 150) / 300;
            Cell::DateTime((DAY_1900 + i64::from(dt.days())) * TICKS_PER_DAY + frac.min(TICKS_PER_DAY - 1))
        }),
        ColumnData::SmallDateTime(v) => v.map_or(Cell::Null, |dt| {
            // Days since 1900 and minutes since midnight.
            let t = i64::from(dt.seconds_fragments()) * 60 * TICKS_PER_SECOND;
            Cell::DateTime((DAY_1900 + i64::from(dt.days())) * TICKS_PER_DAY + t.min(TICKS_PER_DAY - 1))
        }),
        ColumnData::Time(v) => v.map_or(Cell::Null, |t| Cell::Time(time_ticks(t.increments(), t.scale()))),
        ColumnData::Date(v) => v.map_or(Cell::Null, |d| match i32::try_from(DAY_1 + i64::from(d.days())) {
            Ok(days) => Cell::Date(days),
            Err(_) => Cell::Null,
        }),
        ColumnData::DateTime2(v) => v.map_or(Cell::Null, |dt| Cell::DateTime(datetime2_ticks(dt))),
        ColumnData::DateTimeOffset(v) => v.map_or(Cell::Null, |dto| {
            // The server sends the moment in UTC and the offset of its local time.
            let offset = i32::from(dto.offset()).clamp(-14 * 60, 14 * 60);
            Cell::DateTimeZone(datetime2_ticks(dto.datetime2()) + i64::from(offset) * 60 * TICKS_PER_SECOND, offset)
        }),
    }
}
