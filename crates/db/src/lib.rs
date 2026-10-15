//! Database connections for queries: SQL Server, PostgreSQL, MySQL/MariaDB and SQLite.
//!
//! A [`Connection`] reads tables, lists what a database holds and runs native SQL, and returns
//! rows as plain [`Cell`]s in the representation the M language uses (ticks, days since 1970), so
//! the engine copies them into query values without knowing any driver. The crate knows nothing
//! about workbooks, credentials storage or the M language.
//!
//! Connections need sockets: on the web every [`Connection::open`] fails with
//! [`ErrorKind::Unsupported`]. Each driver is a cargo feature (all on by default), so a build can
//! leave out what it doesn't want.
//!
//! Servers are hostile input like any file: every size read is capped by [`Limits`], identifiers
//! are quoted, and SQLite files are opened read-only.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::fmt;
use std::time::Duration;

mod text;
pub use text::{parse_date, parse_datetime, parse_duration, parse_time};

#[cfg(all(not(target_arch = "wasm32"), feature = "mysql"))]
mod mysql;
#[cfg(all(not(target_arch = "wasm32"), feature = "postgres"))]
mod postgres;
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
mod sqlite;
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlserver"))]
mod sqlserver;

/// Ticks (100 ns) in a day.
pub const TICKS_PER_DAY: i64 = 864_000_000_000;
/// Ticks in a second.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// The kind of database.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    SqlServer,
    PostgreSql,
    MySql,
    Sqlite,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::SqlServer, Kind::PostgreSql, Kind::MySql, Kind::Sqlite];

    /// The name used in commands: `sqlserver`, `postgresql`, `mysql`, `sqlite`.
    pub fn id(self) -> &'static str {
        match self {
            Kind::SqlServer => "sqlserver",
            Kind::PostgreSql => "postgresql",
            Kind::MySql => "mysql",
            Kind::Sqlite => "sqlite",
        }
    }

    /// The name people know it by.
    pub fn label(self) -> &'static str {
        match self {
            Kind::SqlServer => "SQL Server",
            Kind::PostgreSql => "PostgreSQL",
            Kind::MySql => "MySQL",
            Kind::Sqlite => "SQLite",
        }
    }

    /// Parses [`Kind::id`] or [`Kind::label`], ignoring case, plus a few common spellings.
    pub fn parse(s: &str) -> Option<Kind> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
        match s.as_str() {
            "sqlserver" | "mssql" | "sql" | "azuresql" => Some(Kind::SqlServer),
            "postgresql" | "postgres" | "pg" => Some(Kind::PostgreSql),
            "mysql" | "mariadb" => Some(Kind::MySql),
            "sqlite" | "sqlite3" => Some(Kind::Sqlite),
            _ => None,
        }
    }

    /// The port the server listens on unless told otherwise.
    pub fn default_port(self) -> Option<u16> {
        match self {
            Kind::SqlServer => Some(1433),
            Kind::PostgreSql => Some(5432),
            Kind::MySql => Some(3306),
            Kind::Sqlite => None,
        }
    }

    /// The schema a table is in when the database has no schemas of its own.
    pub fn default_schema(self) -> &'static str {
        match self {
            Kind::SqlServer => "dbo",
            Kind::PostgreSql => "public",
            Kind::MySql => "",
            Kind::Sqlite => "main",
        }
    }

    /// Whether this kind connects to a server (and takes a user name and password).
    pub fn has_server(self) -> bool {
        self != Kind::Sqlite
    }

    /// Quotes an identifier for this kind's SQL.
    pub fn quote(self, ident: &str) -> String {
        match self {
            Kind::SqlServer => format!("[{}]", ident.replace(']', "]]")),
            Kind::MySql => format!("`{}`", ident.replace('`', "``")),
            Kind::PostgreSql | Kind::Sqlite => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// `SELECT *` from a table, at most `take` rows of it.
    pub fn select_all(self, schema: &str, table: &str, take: Option<usize>) -> String {
        let name = if schema.is_empty() { self.quote(table) } else { format!("{}.{}", self.quote(schema), self.quote(table)) };
        match (self, take) {
            (Kind::SqlServer, Some(n)) => format!("SELECT TOP ({n}) * FROM {name}"),
            (_, Some(n)) => format!("SELECT * FROM {name} LIMIT {n}"),
            (_, None) => format!("SELECT * FROM {name}"),
        }
    }

    /// Splits a server as Power Query writes it into host and port: `host,port` for SQL Server
    /// (`host:port` too), `host:port` for the others, `[::1]:port` for IPv6 addresses.
    pub fn split_server(self, server: &str) -> Result<(String, Option<u16>), Error> {
        let s = server.trim();
        let s = s.strip_prefix("tcp:").unwrap_or(s);
        if s.is_empty() {
            return Err(Error::new(ErrorKind::Connect, "The server is missing."));
        }
        if self == Kind::SqlServer && s.contains('\\') {
            return Err(Error::new(
                ErrorKind::Connect,
                format!("Named instances ('{s}') aren't supported: give the instance's port instead, as 'host,port'."),
            ));
        }
        let port_of = |p: &str| -> Result<u16, Error> {
            p.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| Error::new(ErrorKind::Connect, format!("'{p}' isn't a port number.")))
        };
        // [v6]:port
        if let Some(rest) = s.strip_prefix('[') {
            let Some((host, after)) = rest.split_once(']') else {
                return Err(Error::new(ErrorKind::Connect, format!("'{s}' isn't a valid server.")));
            };
            let port = match after.strip_prefix(':').or_else(|| after.strip_prefix(',')) {
                Some(p) => Some(port_of(p)?),
                None if after.is_empty() => None,
                None => return Err(Error::new(ErrorKind::Connect, format!("'{s}' isn't a valid server."))),
            };
            return Ok((host.to_string(), port));
        }
        let split = if self == Kind::SqlServer { s.rsplit_once(',').or_else(|| s.rsplit_once(':')) } else { s.rsplit_once(':') };
        let (host, port) = match split {
            // A bare IPv6 address has more than one colon: no port.
            Some((h, _)) if h.contains(':') => (s, None),
            Some((h, p)) => (h, Some(port_of(p)?)),
            None => (s, None),
        };
        let host = match host.trim() {
            "." | "(local)" | "(localdb)" => "localhost",
            h => h,
        };
        Ok((host.to_string(), port))
    }

    /// Writes host and port back as a server the way Power Query does for this kind.
    pub fn join_server(self, host: &str, port: Option<u16>) -> String {
        let host = host.trim();
        let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
        match port {
            Some(p) if Some(p) != self.default_port() => match self {
                Kind::SqlServer => format!("{host},{p}"),
                _ => format!("{host}:{p}"),
            },
            _ => host,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What it takes to connect.
#[derive(Clone)]
pub struct Connect {
    pub kind: Kind,
    /// Host name or address; for SQLite, the database file's path.
    pub host: String,
    /// The port, or the kind's default.
    pub port: Option<u16>,
    /// The database; empty for the login's default.
    pub database: String,
    pub username: String,
    pub password: String,
    /// Encrypt the connection (TLS) and fail when the server can't.
    pub encrypt: bool,
    /// Accept the server's certificate without checking who signed it or whom it names.
    pub trust_server_certificate: bool,
    pub connect_timeout: Duration,
    /// How long one statement may run.
    pub command_timeout: Duration,
}

impl Connect {
    pub fn new(kind: Kind, host: impl Into<String>, database: impl Into<String>) -> Connect {
        Connect {
            kind,
            host: host.into(),
            port: None,
            database: database.into(),
            username: String::new(),
            password: String::new(),
            encrypt: true,
            trust_server_certificate: false,
            connect_timeout: Duration::from_secs(15),
            command_timeout: Duration::from_secs(600),
        }
    }

    pub fn port_or_default(&self) -> u16 {
        self.port.or(self.kind.default_port()).unwrap_or(0)
    }
}

impl fmt::Debug for Connect {
    // Never prints the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connect")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("encrypt", &self.encrypt)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .finish_non_exhaustive()
    }
}

/// What can go wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No driver for this kind in this build (or on the web).
    Unsupported,
    /// The server couldn't be reached, or the connection broke.
    Connect,
    /// The server refused the user name or password.
    Login,
    /// The server refused a statement.
    Query,
    /// The result is larger than [`Limits`] allow.
    TooLarge,
    /// A statement or the connection took longer than allowed.
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Caps on what one read takes in. Reaching one fails the read with [`ErrorKind::TooLarge`],
/// except `take`, which just stops.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_rows: usize,
    pub max_cells: usize,
    /// Text and binary bytes across the whole result.
    pub max_bytes: usize,
    /// Keep only the first rows (a preview).
    pub take: Option<usize>,
}

impl Default for Limits {
    fn default() -> Limits {
        #[cfg(not(target_pointer_width = "32"))]
        let (max_rows, max_cells, max_bytes) = (1 << 24, 1 << 26, 2 << 30);
        #[cfg(target_pointer_width = "32")]
        let (max_rows, max_cells, max_bytes) = (1 << 22, 1 << 24, 256 << 20);
        Limits { max_rows, max_cells, max_bytes, take: None }
    }
}

/// The type of a column, as M names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColType {
    Any,
    Logical,
    Int64,
    Number,
    Decimal,
    Currency,
    Text,
    Binary,
    Date,
    Time,
    DateTime,
    DateTimeZone,
    Duration,
}

/// One value read from a database, in M's representation.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// An exact number as the server wrote it (`DECIMAL`, `NUMERIC`, `MONEY`).
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    /// Days since 1970-01-01.
    Date(i32),
    /// Ticks since midnight.
    Time(i64),
    /// Ticks since 1970-01-01T00:00.
    DateTime(i64),
    /// Local ticks since 1970-01-01T00:00 and the offset from UTC in minutes.
    DateTimeZone(i64, i32),
    /// Ticks.
    Duration(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: ColType,
}

/// The rows a read returned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub columns: Vec<Column>,
    /// Each row has one cell per column.
    pub rows: Vec<Vec<Cell>>,
}

/// A table or view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    /// Schema (SQL Server, PostgreSQL), database (MySQL) or `main` (SQLite).
    pub schema: String,
    pub name: String,
    pub view: bool,
}

/// Collects rows under [`Limits`].
pub(crate) struct Collector {
    limits: Limits,
    bytes: usize,
    pub(crate) out: Rows,
}

impl Collector {
    pub(crate) fn new(limits: Limits, columns: Vec<Column>) -> Collector {
        Collector { limits, bytes: 0, out: Rows { columns, rows: Vec::new() } }
    }

    /// Whether `take` rows are in: later rows are read and dropped.
    pub(crate) fn full(&self) -> bool {
        self.limits.take.is_some_and(|t| self.out.rows.len() >= t)
    }

    pub(crate) fn push(&mut self, mut row: Vec<Cell>) -> Result<(), Error> {
        if self.full() {
            return Ok(());
        }
        row.resize(self.out.columns.len(), Cell::Null);
        let too_large = |what: &str| Error::new(ErrorKind::TooLarge, format!("The result has more {what} than a query can take."));
        if self.out.rows.len() >= self.limits.max_rows {
            return Err(too_large("rows"));
        }
        if self.out.rows.len().saturating_add(1).saturating_mul(self.out.columns.len().max(1)) > self.limits.max_cells {
            return Err(too_large("cells"));
        }
        for c in &row {
            self.bytes = self.bytes.saturating_add(match c {
                Cell::Text(s) | Cell::Decimal(s) => s.len(),
                Cell::Bytes(b) => b.len(),
                _ => 0,
            });
        }
        if self.bytes > self.limits.max_bytes {
            return Err(too_large("data"));
        }
        self.out.rows.push(row);
        Ok(())
    }
}

/// An open connection.
pub struct Connection {
    kind: Kind,
    inner: Inner,
}

// Boxed: the drivers' clients differ a lot in size.
enum Inner {
    #[cfg(all(not(target_arch = "wasm32"), feature = "sqlserver"))]
    SqlServer(Box<sqlserver::Conn>),
    #[cfg(all(not(target_arch = "wasm32"), feature = "postgres"))]
    PostgreSql(Box<postgres::Conn>),
    #[cfg(all(not(target_arch = "wasm32"), feature = "mysql"))]
    MySql(Box<mysql::Conn>),
    #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
    Sqlite(Box<sqlite::Conn>),
    /// Never built: keeps the enum inhabited when every driver is left out.
    #[allow(dead_code)]
    None(std::convert::Infallible),
}

fn unsupported(kind: Kind) -> Error {
    if cfg!(target_arch = "wasm32") {
        Error::new(ErrorKind::Unsupported, format!("{kind} databases can't be reached from the web version."))
    } else {
        Error::new(ErrorKind::Unsupported, format!("This build has no {kind} driver."))
    }
}

/// Whether this build can connect to `kind`.
pub fn supported(kind: Kind) -> bool {
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    match kind {
        Kind::SqlServer => cfg!(feature = "sqlserver"),
        Kind::PostgreSql => cfg!(feature = "postgres"),
        Kind::MySql => cfg!(feature = "mysql"),
        Kind::Sqlite => cfg!(feature = "sqlite"),
    }
}

impl Connection {
    /// Connects and logs in.
    pub fn open(c: &Connect) -> Result<Connection, Error> {
        if c.kind.has_server() && c.host.trim().is_empty() {
            return Err(Error::new(ErrorKind::Connect, "The server is missing."));
        }
        let inner = match c.kind {
            #[cfg(all(not(target_arch = "wasm32"), feature = "sqlserver"))]
            Kind::SqlServer => Inner::SqlServer(Box::new(sqlserver::Conn::open(c)?)),
            #[cfg(all(not(target_arch = "wasm32"), feature = "postgres"))]
            Kind::PostgreSql => Inner::PostgreSql(Box::new(postgres::Conn::open(c)?)),
            #[cfg(all(not(target_arch = "wasm32"), feature = "mysql"))]
            Kind::MySql => Inner::MySql(Box::new(mysql::Conn::open(c)?)),
            #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
            Kind::Sqlite => Inner::Sqlite(Box::new(sqlite::Conn::open(c)?)),
            #[allow(unreachable_patterns)]
            k => return Err(unsupported(k)),
        };
        Ok(Connection { kind: c.kind, inner })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Runs SQL and returns the rows of its first result set (no rows for a statement that
    /// returns none).
    pub fn query(&mut self, sql: &str, limits: &Limits) -> Result<Rows, Error> {
        match &mut self.inner {
            #[cfg(all(not(target_arch = "wasm32"), feature = "sqlserver"))]
            Inner::SqlServer(c) => c.query(sql, limits),
            #[cfg(all(not(target_arch = "wasm32"), feature = "postgres"))]
            Inner::PostgreSql(c) => c.query(sql, limits),
            #[cfg(all(not(target_arch = "wasm32"), feature = "mysql"))]
            Inner::MySql(c) => c.query(sql, limits),
            #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
            Inner::Sqlite(c) => c.query(sql, limits),
            Inner::None(n) => match *n {},
        }
    }

    /// The rows of one table or view.
    pub fn table(&mut self, schema: &str, name: &str, limits: &Limits) -> Result<Rows, Error> {
        let sql = self.kind.select_all(schema, name, limits.take);
        self.query(&sql, limits)
    }

    /// The tables and views of the database, by schema and name.
    pub fn tables(&mut self) -> Result<Vec<TableInfo>, Error> {
        let sql = match self.kind {
            Kind::SqlServer => "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES ORDER BY TABLE_SCHEMA, TABLE_NAME",
            Kind::PostgreSql => {
                "SELECT table_schema::text, table_name::text, table_type::text FROM information_schema.tables \
                 WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY 1, 2"
            }
            Kind::MySql => {
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
                 WHERE (DATABASE() IS NULL AND TABLE_SCHEMA NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')) \
                 OR TABLE_SCHEMA = DATABASE() ORDER BY 1, 2"
            }
            Kind::Sqlite => {
                "SELECT 'main', name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name"
            }
        };
        let rows = self.query(sql, &Limits::default())?;
        Ok(rows
            .rows
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter().map(|c| match c {
                    Cell::Text(s) => s,
                    Cell::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                    _ => String::new(),
                });
                let (schema, name, ty) = (it.next()?, it.next()?, it.next().unwrap_or_default());
                Some(TableInfo { schema, name, view: ty.to_ascii_uppercase().contains("VIEW") })
            })
            .collect())
    }

    /// The databases on the server the login can see.
    pub fn databases(&mut self) -> Result<Vec<String>, Error> {
        let sql = match self.kind {
            Kind::SqlServer => "SELECT name FROM sys.databases WHERE HAS_DBACCESS(name) = 1 ORDER BY name",
            Kind::PostgreSql => "SELECT datname::text FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY 1",
            Kind::MySql => "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY 1",
            Kind::Sqlite => return Ok(vec!["main".to_string()]),
        };
        let rows = self.query(sql, &Limits::default())?;
        Ok(rows
            .rows
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(Cell::Text(s)) => Some(s),
                Some(Cell::Bytes(b)) => Some(String::from_utf8_lossy(&b).into_owned()),
                _ => None,
            })
            .collect())
    }
}

/// Runs `fut` on a runtime of its own, failing after `timeout`.
#[cfg(all(not(target_arch = "wasm32"), any(feature = "sqlserver", feature = "postgres", feature = "mysql")))]
pub(crate) fn block_on<T>(
    rt: &tokio::runtime::Runtime,
    timeout: Duration,
    what: &str,
    fut: impl std::future::Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    rt.block_on(async {
        match tokio::time::timeout(timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(Error::new(ErrorKind::Timeout, format!("{what} took longer than {} seconds.", timeout.as_secs()))),
        }
    })
}

#[cfg(all(not(target_arch = "wasm32"), any(feature = "sqlserver", feature = "postgres", feature = "mysql")))]
pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::new(ErrorKind::Connect, format!("Couldn't start the network runtime: {e}")))
}

#[cfg(test)]
mod tests;
