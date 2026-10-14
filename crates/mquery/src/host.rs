//! What a query can reach outside itself. The evaluator does no I/O of its own: the application
//! that runs it decides what a query may read.

use crate::value::Table;

/// One entry of a workbook, as `Excel.Workbook` lists it.
pub struct WorkbookPart {
    pub name: String,
    /// `"Sheet"`, `"Table"` or `"DefinedName"`.
    pub kind: &'static str,
    pub data: Table,
    pub hidden: bool,
}

/// The current moment: local ticks since 1970-01-01T00:00 and the offset from UTC in minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub local_ticks: i64,
    pub offset_minutes: i32,
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
    /// `Web.Contents(url)`.
    fn web_contents(&self, url: &str) -> Result<Vec<u8>, String> {
        Err(format!("Reading from the web isn't available here ({url})."))
    }
    /// `Excel.Workbook(binary)`: the sheets, tables and named ranges of a workbook file.
    fn excel_workbook(&self, _bytes: &[u8], _use_headers: bool) -> Result<Vec<WorkbookPart>, String> {
        Err("Reading workbook files isn't available here.".into())
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
