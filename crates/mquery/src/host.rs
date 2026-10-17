//! What a query can reach outside itself. The evaluator does no I/O of its own: the application
//! that runs it decides what a query may read.

use std::rc::Rc;

use crate::value::Table;

/// One entry of a workbook, as `Excel.Workbook` lists it.
pub struct WorkbookPart {
    pub name: String,
    /// `"Sheet"`, `"Table"` or `"DefinedName"`.
    pub kind: &'static str,
    pub data: Table,
    pub hidden: bool,
}

/// One file under a folder, as `Folder.Files` lists it. Its content is read only when a query
/// reads it.
pub struct FileEntry {
    pub name: String,
    /// The folder it is in, ending with the path separator.
    pub folder: String,
    pub size: u64,
    /// Ticks since 1970-01-01T00:00, when the system tells.
    pub accessed: Option<i64>,
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub hidden: bool,
    pub read_only: bool,
    pub content: Rc<dyn Fn() -> Result<Vec<u8>, String>>,
}

/// Reads a web address (GET) with these headers: the body, or why not. `'static`, so that values
/// read later (the entity sets of an OData service) can keep it.
pub type Fetch = Rc<dyn Fn(&str, &[(String, String)], std::time::Duration) -> Result<Vec<u8>, String>>;

/// The current moment: local ticks since 1970-01-01T00:00 and the offset from UTC in minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub local_ticks: i64,
    pub offset_minutes: i32,
}

/// A database a query names: `Sql.Database("server", "db")` and its relatives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DataSource {
    /// `"SqlServer"`, `"PostgreSQL"`, `"MySQL"` or `"SQLite"`.
    pub kind: &'static str,
    /// The server as the query writes it (`host,port`, `host:port`); for SQLite, the file's path.
    pub server: String,
    /// The database; empty for the login's default.
    pub database: String,
}

/// A table or view of a database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
    pub view: bool,
}

/// A database the host connected to. Its methods fail with a message that becomes the query's
/// `DataSource.Error`. Values that come from it (the tables of a navigation table) keep it, so it
/// lives as long as they do.
pub trait Database {
    /// The tables and views.
    fn tables(&self) -> Result<Vec<TableRef>, String>;
    /// The rows of one table or view.
    fn table(&self, schema: &str, name: &str) -> Result<Table, String>;
    /// The rows a native query returns (`[Query = "…"]`, `Value.NativeQuery`).
    fn query(&self, sql: &str) -> Result<Table, String>;
    /// The databases on the same server (`Sql.Databases`).
    fn databases(&self) -> Result<Vec<String>, String>;
    /// Another database on the same server, with the same login.
    fn open(&self, database: &str) -> Result<Rc<dyn Database>, String>;
}

/// Each method fails with a message that becomes the query's `DataSource.Error`. The defaults
/// refuse, so a host only grants what it implements.
pub trait Host {
    /// `Excel.CurrentWorkbook()`: the tables and named ranges of the workbook the query is in.
    fn current_workbook(&self) -> Result<Vec<(String, Table)>, String> {
        Ok(Vec::new())
    }
    /// `File.Contents(path)`.
    fn file_contents(&self, path: &str) -> Result<Vec<u8>, String> {
        Err(format!("Reading files isn't available here ({path})."))
    }
    /// `Web.Contents`, `OData.Feed`: what reads the web, or `None` to refuse.
    fn web(&self) -> Option<Fetch> {
        None
    }
    /// `Folder.Files(path)`: the files under a folder, its subfolders included.
    fn folder_files(&self, path: &str) -> Result<Vec<FileEntry>, String> {
        Err(format!("Reading folders isn't available here ({path})."))
    }
    /// `Excel.Workbook(binary)`: the sheets, tables and named ranges of a workbook file.
    fn excel_workbook(&self, _bytes: &[u8], _use_headers: bool) -> Result<Vec<WorkbookPart>, String> {
        Err("Reading workbook files isn't available here.".into())
    }
    /// `Sql.Database`, `PostgreSQL.Database`, `MySQL.Database`, `SQLite.Database`: a connection to
    /// the database, with whatever credentials the host keeps for it.
    fn database(&self, source: &DataSource) -> Result<Rc<dyn Database>, String> {
        Err(format!("Databases aren't available here ({} at {}).", source.kind, source.server))
    }
    /// `DateTime.LocalNow()` and its relatives. `None` when there is no clock.
    fn now(&self) -> Option<Now> {
        None
    }
    /// Polled during long evaluations: `true` stops the query.
    fn cancelled(&self) -> bool {
        false
    }
    /// The culture used when a function isn't given one.
    fn culture(&self) -> &str {
        "en-US"
    }
}

/// A host that grants nothing: for pure expressions.
pub struct NoHost;
impl Host for NoHost {}
