//! Queries (Data › Get & Transform Data): evaluating Power Query M formulas against the
//! workbook, and loading their results into tables on a sheet.
//!
//! The M evaluator (`gridcraft-mquery`) does no I/O. [`WorkbookHost`] is what a query can reach:
//! the tables and named ranges of its own workbook (`Excel.CurrentWorkbook`), local files
//! (`File.Contents`, off the web build only), workbook files (`Excel.Workbook`) and the clock.
//! Nothing here opens the network.

use std::collections::HashMap;
use std::rc::Rc;

use gridcraft_core::date::days_from_civil;
use gridcraft_core::{CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{Cell, NumFmt, Sheet, StyleId, Table, TableColumn, TotalsFn, Workbook};
use gridcraft_mquery as mq;
use gridcraft_numfmt::NumberFormat;

use crate::cmd::Ctx;

/// 1899-12-30, the day serial 0 of the 1900 date system counts from, in days since 1970-01-01.
const DAY_1899_12_30: i64 = -25_569;
/// The last serial: 9999-12-31.
const MAX_SERIAL: i64 = 2_958_465;
/// The deepest nesting of evaluations on the evaluation thread, and the stack it gets. A level
/// takes about 2 KiB in a development build and several times that unoptimized.
#[cfg(all(not(target_arch = "wasm32"), target_pointer_width = "64"))]
const EVAL_THREAD: (usize, usize) = (10_000, 256 << 20);
#[cfg(all(not(target_arch = "wasm32"), not(target_pointer_width = "64")))]
const EVAL_THREAD: (usize, usize) = (2_000, 32 << 20);

/// Days since 1970-01-01 of the day with this serial.
fn unix_day(sys: DateSystem, serial: i64) -> i64 {
    match sys {
        // Serials below 60 are a day early: the 1900 system counts a 29 February 1900.
        DateSystem::D1900 => (if serial < 60 { serial + 1 } else { serial }) + DAY_1899_12_30,
        DateSystem::D1904 => serial + days_from_civil(1904, 1, 1),
    }
}

/// The serial of a day given in days since 1970-01-01; `None` before the date system starts.
fn serial_day(sys: DateSystem, day: i64) -> Option<i64> {
    let (s, first) = match sys {
        DateSystem::D1900 => {
            let s = day - DAY_1899_12_30;
            (if s <= 60 { s - 1 } else { s }, 1)
        }
        DateSystem::D1904 => (day - days_from_civil(1904, 1, 1), 0),
    };
    (first..=MAX_SERIAL).contains(&s).then_some(s)
}

// ---------------------------------------------------------------- cells to M values

/// Converts cells to M values, remembering per style whether its number format shows a date.
struct CellReader<'a> {
    wb: &'a Workbook,
    dates: HashMap<StyleId, bool>,
}

impl<'a> CellReader<'a> {
    fn new(wb: &'a Workbook) -> CellReader<'a> {
        CellReader { wb, dates: HashMap::new() }
    }

    fn slot(&mut self, cell: &Cell) -> mq::Slot {
        mq::Slot::Val(match cell.value.scalar() {
            Value::Empty | Value::Array(_) => mq::Value::Null,
            Value::Bool(b) => mq::Value::Logical(b),
            Value::Text(s) => mq::Value::text(&*s),
            Value::Number(n) => {
                let wb = self.wb;
                let is_date =
                    *self.dates.entry(cell.style).or_insert_with(|| NumberFormat::parse(wb.styles.get(cell.style).num_fmt.as_str()).is_date());
                if is_date { date_value(wb.date_system, n) } else { mq::Value::Number(n) }
            }
            Value::Error(e) => {
                return mq::Slot::Err(Rc::new(mq::MError {
                    reason: Rc::from("Expression.Error"),
                    message: Rc::from(format!("The cell holds the error {e}.")),
                    detail: mq::Value::Null,
                }));
            }
        })
    }

    /// The cells of `data` as rows, `width` columns wide.
    fn rows(&mut self, sh: &Sheet, data: Option<RangeRef>, width: usize) -> Vec<mq::Row> {
        let Some(r) = data else { return Vec::new() };
        let mut rows: Vec<Vec<mq::Slot>> = vec![vec![mq::Slot::NULL; width]; r.height() as usize];
        for (c, cell) in sh.cells.iter_range(r) {
            if !r.contains(c) {
                continue;
            }
            let slot = self.slot(cell);
            if let Some(x) = rows.get_mut((c.row - r.start.row) as usize).and_then(|row| row.get_mut((c.col - r.start.col) as usize)) {
                *x = slot;
            }
        }
        // Values spilled by dynamic arrays live beside the cells.
        for (c, v) in sh.spill.range(r.start..=r.end) {
            if r.contains(*c)
                && let Some(x) = rows.get_mut((c.row - r.start.row) as usize).and_then(|row| row.get_mut((c.col - r.start.col) as usize))
                && x.is_null()
            {
                *x = self.slot(&Cell { value: v.clone(), formula: None, style: sh.style_id(*c) });
            }
        }
        rows.into_iter().map(Into::into).collect()
    }

    /// A table of the workbook as an M table.
    fn table(&mut self, sh: &Sheet, t: &Table) -> mq::Table {
        let columns: mq::Names = unique_names(t.columns.iter().map(|c| c.name.clone()), false).into_iter().map(Rc::from).collect();
        let width = columns.len();
        mq::Table::untyped(columns, self.rows(sh, t.data_range(), width))
    }

    /// A range as an M table with the columns `Column1`, `Column2`… (or, with `headers`, named by
    /// its first row).
    fn range(&mut self, sh: &Sheet, r: RangeRef, headers: bool) -> mq::Table {
        let width = r.width() as usize;
        let mut rows = self.rows(sh, Some(r), width);
        let names: Vec<String> = if headers && !rows.is_empty() {
            let first = rows.remove(0);
            first
                .iter()
                .enumerate()
                .map(|(i, s)| match s {
                    mq::Slot::Val(mq::Value::Text(t)) if !t.is_empty() => t.to_string(),
                    mq::Slot::Val(mq::Value::Number(n)) => mq::number_to_text(*n),
                    _ => format!("Column{}", i + 1),
                })
                .collect()
        } else {
            (1..=width).map(|i| format!("Column{i}")).collect()
        };
        mq::Table::untyped(unique_names(names.into_iter(), false).into_iter().map(Rc::from).collect(), rows)
    }
}

/// A number from a cell formatted as a date: a date, a time or a date and time.
fn date_value(sys: DateSystem, n: f64) -> mq::Value {
    const TICKS_PER_DAY: f64 = 864_000_000_000.0;
    if !n.is_finite() || n < 0.0 || n >= (MAX_SERIAL + 1) as f64 {
        return mq::Value::Number(n);
    }
    let day = n.floor();
    let tod = ((n - day) * TICKS_PER_DAY).round() as i64;
    if tod == 0 {
        mq::Value::Date(unix_day(sys, day as i64) as i32)
    } else if day == 0.0 {
        mq::Value::Time(tod)
    } else {
        mq::Value::DateTime(unix_day(sys, day as i64) * TICKS_PER_DAY as i64 + tod)
    }
}

/// Names made unique: a repeat gets a number after it. An Excel table tells its columns apart
/// without regard to case (`ignore_case`), M with regard to it.
fn unique_names(names: impl Iterator<Item = String>, ignore_case: bool) -> Vec<String> {
    let key = |s: &str| if ignore_case { s.to_lowercase() } else { s.to_string() };
    let mut taken = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (i, n) in names.enumerate() {
        let base = if n.is_empty() { format!("Column{}", i + 1) } else { n };
        let mut name = base.clone();
        let mut k = 2;
        while !taken.insert(key(&name)) {
            name = format!("{base}{k}");
            k += 1;
        }
        out.push(name);
    }
    out
}

/// The range a defined name refers to, when its formula is a plain reference such as
/// `Sheet1!$A$1:$C$9`.
fn name_range(wb: &Workbook, formula: &str, scope: Option<usize>) -> Option<(usize, RangeRef)> {
    let (sheet, body) = crate::cmd::split_sheet(formula.trim().trim_start_matches('='));
    let si = match sheet {
        Some(n) => wb.sheet_index(&n)?,
        None => scope?,
    };
    Some((si, RangeRef::parse(&body.replace('$', ""))?))
}

// ---------------------------------------------------------------- the host

/// What a query of `wb` can reach.
pub struct WorkbookHost<'a> {
    wb: &'a Workbook,
}

impl<'a> WorkbookHost<'a> {
    pub fn new(wb: &'a Workbook) -> WorkbookHost<'a> {
        WorkbookHost { wb }
    }
}

impl mq::Host for WorkbookHost<'_> {
    fn current_workbook(&self) -> Result<Vec<(String, mq::Table)>, String> {
        let mut reader = CellReader::new(self.wb);
        let mut out = Vec::new();
        for sh in &self.wb.sheets {
            for t in &sh.tables {
                out.push((t.name.clone(), reader.table(sh, t)));
            }
        }
        for n in &self.wb.names {
            if n.hidden || out.iter().any(|(name, _)| name.eq_ignore_ascii_case(&n.name)) {
                continue;
            }
            if let Some((si, r)) = name_range(self.wb, &n.formula, n.scope)
                && let Some(sh) = self.wb.sheet(si)
                && r.count() <= mq::MAX_CELLS as u64
            {
                out.push((n.name.clone(), reader.range(sh, r, false)));
            }
        }
        Ok(out)
    }

    fn file_contents(&self, path: &str) -> Result<Vec<u8>, String> {
        crate::io::read_file(path).map_err(|e| format!("The file '{path}' can't be read: {e}"))
    }

    fn excel_workbook(&self, bytes: &[u8], use_headers: bool) -> Result<Vec<mq::WorkbookPart>, String> {
        let (wb, _) = crate::io::open_bytes("workbook.xlsx", bytes).map_err(|e| format!("The workbook can't be read: {e}"))?;
        let mut reader = CellReader::new(&wb);
        let mut parts = Vec::new();
        for sh in &wb.sheets {
            let data = match sh.used_range() {
                Some(r) => reader.range(sh, r, use_headers),
                None => mq::Table::empty(),
            };
            parts.push(mq::WorkbookPart {
                name: sh.name.clone(),
                kind: "Sheet",
                data,
                hidden: sh.visibility != gridcraft_model::Visibility::Visible,
            });
        }
        for sh in &wb.sheets {
            for t in &sh.tables {
                parts.push(mq::WorkbookPart { name: t.name.clone(), kind: "Table", data: reader.table(sh, t), hidden: false });
            }
        }
        for n in &wb.names {
            if let Some((si, r)) = name_range(&wb, &n.formula, n.scope)
                && let Some(sh) = wb.sheet(si)
                && r.count() <= mq::MAX_CELLS as u64
            {
                parts.push(mq::WorkbookPart { name: n.name.clone(), kind: "DefinedName", data: reader.range(sh, r, use_headers), hidden: n.hidden });
            }
        }
        Ok(parts)
    }

    /// The clock in UTC: like `NOW()`, since the local offset isn't known without platform calls.
    fn now(&self) -> Option<mq::Now> {
        let ms = gridcraft_core::date::unix_now_ms()?;
        Some(mq::Now { local_ticks: (ms * 10_000.0) as i64, offset_minutes: 0 })
    }
}

// ---------------------------------------------------------------- evaluating

/// What to evaluate.
pub enum Target<'a> {
    /// A query of the workbook, by name.
    Query(&'a str),
    /// A formula; `name` is the query it would replace or become, so that it can stand in for
    /// that query while the others refer to it.
    Formula { name: Option<&'a str>, formula: &'a str },
    /// One step (a member of the top-level `let`) of a formula.
    Step { name: Option<&'a str>, formula: &'a str, step: &'a str },
}

fn evaluate_in<R>(wb: &Workbook, target: &Target, max_depth: Option<usize>, done: impl FnOnce(&mq::Value) -> Result<R, String>) -> Result<R, String> {
    let host = WorkbookHost::new(wb);
    let mut it = mq::Interp::new(&host);
    if let Some(d) = max_depth {
        it.set_max_depth(d);
    }
    let (replaced, formula) = match target {
        Target::Query(_) => (None, None),
        Target::Formula { name, formula } | Target::Step { name, formula, .. } => (*name, Some(*formula)),
    };
    // The queries of the workbook, the one being edited standing in under its name.
    let mut queries: Vec<(&str, &str)> = Vec::with_capacity(wb.queries.len() + 1);
    for q in &wb.queries {
        match (replaced, formula) {
            (Some(n), Some(f)) if q.name.eq_ignore_ascii_case(n) => queries.push((q.name.as_str(), f)),
            _ => queries.push((q.name.as_str(), q.formula.as_str())),
        }
    }
    it.set_queries(&queries);
    let describe = |e: mq::Error, src: &str| match e {
        mq::Error::Parse(p) => {
            let (line, col) = p.line_col(src);
            format!("The formula has a syntax error at line {line}, column {col}: {}", p.message)
        }
        mq::Error::Eval(e) => e.to_string(),
    };
    let value = match target {
        Target::Query(name) => {
            let Some(q) = wb.query(name) else { return Err(format!("There is no query named '{name}'.")) };
            it.eval_query(&q.name).map_err(|e| e.to_string())?
        }
        Target::Formula { formula, .. } => it.eval_text(formula).map_err(|e| describe(e, formula))?,
        Target::Step { formula, step, .. } => it.eval_step(formula, step).map_err(|e| describe(e, formula))?,
    };
    done(&value)
}

/// Evaluates `target` in `wb` and hands the result to `done`, which turns it into plain data:
/// M values don't outlive the evaluation.
///
/// Off the web, the evaluation runs on a thread of its own with a large stack, so that deeply
/// recursive queries have room; a panic there becomes an error here.
pub fn evaluate<R: Send>(wb: &Workbook, target: &Target, done: impl FnOnce(&mq::Value) -> Result<R, String> + Send) -> Result<R, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // `Target` holds only shared references, but its lifetime keeps it off the thread:
        // rebuild it there from its parts.
        let parts: (u8, Option<&str>, &str, &str) = match target {
            Target::Query(name) => (0, None, name, ""),
            Target::Formula { name, formula } => (1, *name, formula, ""),
            Target::Step { name, formula, step } => (2, *name, formula, step),
        };
        let run = move |depth: Option<usize>, done: Box<dyn FnOnce(&mq::Value) -> Result<R, String> + Send + '_>| {
            let target = match parts {
                (0, _, name, _) => Target::Query(name),
                (1, name, formula, _) => Target::Formula { name, formula },
                (_, name, formula, step) => Target::Step { name, formula, step },
            };
            evaluate_in(wb, &target, depth, done)
        };
        let (depth, stack) = EVAL_THREAD;
        let mut done = Some(done);
        let outcome = std::thread::scope(|scope| {
            let done_on_thread = done.take();
            let spawned = std::thread::Builder::new().name("query".into()).stack_size(stack).spawn_scoped(scope, move || match done_on_thread {
                Some(d) => run(Some(depth), Box::new(d)),
                None => Err("The query couldn't be started.".to_string()),
            });
            match spawned {
                Ok(handle) => Some(handle.join().unwrap_or_else(|_| Err("The query stopped because of an internal error.".to_string()))),
                // No thread to be had: the caller's stack and the default depth will do.
                Err(_) => None,
            }
        });
        match outcome {
            Some(r) => r,
            None => Err("The query couldn't be started: no thread was available to run it.".to_string()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        evaluate_in(wb, target, None, done)
    }
}

// ---------------------------------------------------------------- M values to cells

/// The number format a loaded value needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Shown {
    Date,
    DateTime,
    Time,
    Duration,
    Percent,
}

impl Shown {
    fn code(self) -> &'static str {
        match self {
            Shown::Date => "m/d/yyyy",
            Shown::DateTime => "m/d/yyyy h:mm",
            Shown::Time => "h:mm:ss",
            Shown::Duration => "[h]:mm:ss",
            Shown::Percent => "0.00%",
        }
    }
}

/// A scalar result, ready for a cell.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Value(Value),
    /// A value that was an error in the query.
    Error(String),
}

/// What an M value is in a cell, and the number format it needs. Lists, records and tables show
/// as `[List]`, `[Record]` and `[Table]`.
fn cell_value(sys: DateSystem, v: &mq::Value) -> (Value, Option<Shown>) {
    const TICKS_PER_DAY: f64 = 864_000_000_000.0;
    let iso = |days: i64| {
        let (y, m, d) = gridcraft_core::date::civil_from_days(days);
        Value::text(format!("{y:04}-{m:02}-{d:02}"))
    };
    match v {
        mq::Value::Null => (Value::Empty, None),
        mq::Value::Logical(b) => (Value::Bool(*b), None),
        mq::Value::Number(n) if n.is_finite() => (Value::Number(*n), None),
        mq::Value::Number(_) => (Value::Error(gridcraft_core::CellError::Num), None),
        mq::Value::Text(s) => (Value::text(&**s), None),
        // A date before the date system starts has no serial: it loads as text.
        mq::Value::Date(d) => match serial_day(sys, *d as i64) {
            Some(s) => (Value::Number(s as f64), Some(Shown::Date)),
            None => (iso(*d as i64), None),
        },
        mq::Value::DateTime(t) | mq::Value::DateTimeZone(t, _) => {
            let day = t.div_euclid(TICKS_PER_DAY as i64);
            match serial_day(sys, day) {
                Some(s) => (Value::Number(s as f64 + t.rem_euclid(TICKS_PER_DAY as i64) as f64 / TICKS_PER_DAY), Some(Shown::DateTime)),
                None => (iso(day), None),
            }
        }
        mq::Value::Time(t) => (Value::Number(*t as f64 / TICKS_PER_DAY), Some(Shown::Time)),
        mq::Value::Duration(t) => (Value::Number(*t as f64 / TICKS_PER_DAY), Some(Shown::Duration)),
        mq::Value::Binary(_) => (Value::text("[Binary]"), None),
        mq::Value::List(_) => (Value::text("[List]"), None),
        mq::Value::Record(_) => (Value::text("[Record]"), None),
        mq::Value::Table(_) => (Value::text("[Table]"), None),
        mq::Value::Function(_) => (Value::text("[Function]"), None),
        mq::Value::Type(_) => (Value::text("[Type]"), None),
    }
}

/// A query result as plain data: what crosses from the evaluation to the workbook.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    pub columns: Vec<String>,
    /// The M type of each column (`number`, `text`, `date`…), for display.
    pub types: Vec<String>,
    /// Row-major cells with the number format each needs.
    cells: Vec<(Scalar, Option<Shown>)>,
    pub rows: usize,
    /// What the result was before it was laid out as a table: `table`, `list`, `record` or the
    /// kind of a single value.
    pub kind: &'static str,
    /// Rows the result had beyond those kept (previews keep only the first).
    pub more_rows: usize,
}

impl Grid {
    pub fn width(&self) -> usize {
        self.columns.len()
    }
    pub fn cell(&self, row: usize, col: usize) -> Option<&Scalar> {
        self.cells.get(row.checked_mul(self.width())?.checked_add(col)?).map(|c| &c.0)
    }
    /// The number format code the cell needs to show as what it is (a date, a time, a percentage).
    pub fn format_code(&self, row: usize, col: usize) -> Option<&'static str> {
        self.cells.get(row.checked_mul(self.width())?.checked_add(col)?)?.1.map(Shown::code)
    }
    pub fn error_count(&self) -> usize {
        self.cells.iter().filter(|c| matches!(c.0, Scalar::Error(_))).count()
    }
}

fn type_label(t: &mq::Type) -> String {
    match t {
        mq::Type::Number(mq::Facet::Double) => "number".into(),
        mq::Type::Number(f) => format!("{f:?}").to_lowercase(),
        mq::Type::Nullable(_) => type_label(&t.non_nullable()),
        other => other.name().to_lowercase(),
    }
}

/// Lays a result out as a grid: a table as it is, a list as one column, a record as names and
/// values, anything else as a single cell. At most `max_rows` rows are kept.
pub fn grid(sys: DateSystem, v: &mq::Value, max_rows: usize) -> Result<Grid, String> {
    let table;
    let (t, kind): (&mq::Table, &'static str) = match v {
        mq::Value::Table(t) => (t, "table"),
        mq::Value::List(l) => {
            table =
                mq::Table::untyped([Rc::from("Value")].into_iter().collect(), l.items.iter().map(|s| std::iter::once(s.clone()).collect()).collect());
            (&table, "list")
        }
        mq::Value::Record(r) => {
            let rows = r.iter().map(|(n, s)| [mq::Slot::Val(mq::Value::Text(n.clone())), s.clone()].into_iter().collect()).collect();
            table = mq::Table::untyped([Rc::from("Name"), Rc::from("Value")].into_iter().collect(), rows);
            (&table, "record")
        }
        other => {
            table = mq::Table::untyped([Rc::from("Value")].into_iter().collect(), vec![std::iter::once(mq::Slot::Val(other.clone())).collect()]);
            (&table, value_kind(other))
        }
    };
    let width = t.width();
    let kept = t.rows.len().min(max_rows);
    let mut cells = Vec::with_capacity(kept.saturating_mul(width));
    for row in t.rows.iter().take(kept) {
        for (i, slot) in row.iter().enumerate() {
            cells.push(match slot {
                mq::Slot::Val(v) => {
                    let (value, shown) = cell_value(sys, v);
                    let percent = matches!(t.types.get(i), Some(mq::Type::Number(mq::Facet::Percentage))) && matches!(value, Value::Number(_));
                    (Scalar::Value(value), if percent { Some(Shown::Percent) } else { shown })
                }
                mq::Slot::Err(e) => (Scalar::Error(e.to_string()), None),
            });
        }
    }
    Ok(Grid {
        columns: t.columns.iter().map(|c| c.to_string()).collect(),
        types: t.types.iter().map(type_label).collect(),
        cells,
        rows: kept,
        kind,
        more_rows: t.rows.len() - kept,
    })
}

fn value_kind(v: &mq::Value) -> &'static str {
    match v {
        mq::Value::Null => "null",
        mq::Value::Logical(_) => "logical",
        mq::Value::Number(_) => "number",
        mq::Value::Text(_) => "text",
        mq::Value::Date(_) => "date",
        mq::Value::Time(_) => "time",
        mq::Value::DateTime(_) => "datetime",
        mq::Value::DateTimeZone(..) => "datetimezone",
        mq::Value::Duration(_) => "duration",
        mq::Value::Binary(_) => "binary",
        mq::Value::List(_) => "list",
        mq::Value::Record(_) => "record",
        mq::Value::Table(_) => "table",
        mq::Value::Function(_) => "function",
        mq::Value::Type(_) => "type",
    }
}

// ---------------------------------------------------------------- loading to a sheet

/// Where a result went.
pub struct Loaded {
    pub range: RangeRef,
    pub rows: usize,
    pub cols: usize,
    /// Cells left empty because their value was an error in the query.
    pub errors: usize,
}

fn occupied(sh: &Sheet, r: RangeRef) -> bool {
    sh.cells.iter_range(r).any(|(c, x)| r.contains(c) && (x.formula.is_some() || !x.value.is_empty()))
}

/// The parts of `new` that `old` doesn't cover (the same top-left corner is assumed).
fn grown(old: RangeRef, new: RangeRef) -> Vec<RangeRef> {
    let mut parts = Vec::new();
    if new.end.row > old.end.row {
        parts.push(RangeRef::new(CellRef::new(old.end.row + 1, new.start.col), new.end));
    }
    if new.end.col > old.end.col {
        parts.push(RangeRef::new(CellRef::new(new.start.row, old.end.col + 1), CellRef::new(new.end.row.min(old.end.row), new.end.col)));
    }
    parts
}

/// Writes `grid` to `sheet` as the table `table`: a header row at `at`, the rows under it. When
/// the table exists it is replaced in place, growing or shrinking; its totals row moves with it.
/// Cells keep their formatting; dates, times and percentages get a number format.
pub fn load(cx: &mut Ctx, sheet: usize, at: CellRef, table: &str, grid: &Grid) -> Result<Loaded, String> {
    let width = grid.width();
    if width == 0 {
        return Err("The query result has no columns to load.".into());
    }
    let existing = cx.wb.sheet(sheet).and_then(|sh| sh.tables.iter().position(|t| t.name.eq_ignore_ascii_case(table)));
    let (old, totals) = match existing.and_then(|i| cx.wb.sheet(sheet).and_then(|sh| sh.tables.get(i))) {
        Some(t) => (Some(t.range), t.totals_row),
        None => (None, false),
    };
    let height = grid.rows as u64 + 1 + totals as u64;
    if at.row as u64 + height > MAX_ROWS as u64 {
        return Err(format!("The query result has {} rows, more than fit on the sheet from row {}.", grid.rows, at.row + 1));
    }
    if at.col as u64 + width as u64 > MAX_COLS as u64 {
        return Err(format!("The query result has {width} columns, more than fit on the sheet."));
    }
    let new = RangeRef::new(at, CellRef::new(at.row + height as u32 - 1, at.col + width as u32 - 1));
    {
        let sh = cx.wb.sheet(sheet).ok_or("no such sheet")?;
        if sh.is_protected() {
            return Err(crate::cmd::PROTECTED.into());
        }
        // Whatever the result newly covers must be free.
        let extra = match old {
            Some(o) => grown(o, new),
            None => vec![new],
        };
        for part in &extra {
            if occupied(sh, *part) {
                return Err(format!("There isn't room to load '{table}': the cells it would cover ({}) aren't empty.", part.a1()));
            }
        }
        if sh.tables.iter().enumerate().any(|(i, t)| Some(i) != existing && t.range.intersects(&new)) {
            return Err(format!("There isn't room to load '{table}': it would overlap another table."));
        }
        if sh.merges.iter().any(|m| m.intersects(&new)) {
            return Err(format!("There isn't room to load '{table}': it would cover merged cells."));
        }
    }
    // Each cell's value and, where it needs a number format, the style it has now and the format.
    let headers = unique_names(grid.columns.iter().cloned(), true);
    let mut errors = 0;
    let mut writes: Vec<(CellRef, Value, Option<(StyleId, Shown)>)> = Vec::with_capacity(grid.cells.len());
    let mut styled: HashMap<(StyleId, Shown), StyleId> = HashMap::new();
    {
        let sh = cx.wb.sheet(sheet).ok_or("no such sheet")?;
        let mut shows_date: HashMap<StyleId, bool> = HashMap::new();
        for (i, (scalar, shown)) in grid.cells.iter().enumerate() {
            let c = CellRef::new(at.row + 1 + (i / width) as u32, at.col + (i % width) as u32);
            let value = match scalar {
                Scalar::Value(v) => v.clone(),
                Scalar::Error(_) => {
                    errors += 1;
                    Value::Empty
                }
            };
            let restyle = shown.and_then(|shown| {
                let current = sh.style_id(c);
                // A date or time the user already formatted keeps its format.
                let keeps = shown != Shown::Percent
                    && *shows_date.entry(current).or_insert_with(|| NumberFormat::parse(cx.wb.styles.get(current).num_fmt.as_str()).is_date());
                (!keeps).then_some((current, shown))
            });
            if let Some(key) = restyle {
                styled.entry(key).or_insert(key.0);
            }
            writes.push((c, value, restyle));
        }
    }
    // One new style per (style, format) pair in use.
    for ((current, shown), id) in styled.iter_mut() {
        let mut style = cx.wb.styles.get(*current).clone();
        style.num_fmt = NumFmt::new(shown.code());
        *id = cx.wb.styles.intern(style);
    }
    let sh = cx.wb.sheet_mut(sheet).ok_or("no such sheet")?;
    // The totals row travels with the table.
    let totals_cells = match (old, totals) {
        (Some(o), true) => sh.take_cells(RangeRef::new(CellRef::new(o.end.row, o.start.col), o.end)),
        _ => Vec::new(),
    };
    // Clear what the old table covered and the new one doesn't.
    if let Some(o) = old {
        let stale: Vec<CellRef> = sh.cells.iter_range(o).map(|(c, _)| c).filter(|c| o.contains(*c) && !new.contains(*c)).collect();
        for c in stale {
            sh.set_value(c, Value::Empty);
        }
    }
    for (i, name) in headers.iter().enumerate() {
        sh.set_value(CellRef::new(at.row, at.col + i as u32), Value::text(name.as_str()));
    }
    for (c, value, restyle) in writes {
        if let Some(id) = restyle.and_then(|key| styled.get(&key)) {
            sh.set_style(c, *id);
        }
        sh.set_value(c, value);
    }
    let last_data = at.row + grid.rows as u32;
    if totals {
        let row = last_data + 1;
        for c in at.col..=new.end.col {
            sh.set_value(CellRef::new(row, c), Value::Empty);
        }
        if let Some(o) = old {
            for (c, cell) in totals_cells {
                let col = at.col + (c.col - o.start.col);
                if col <= new.end.col {
                    sh.set_cell(CellRef::new(row, col), cell);
                }
            }
        }
    }
    let columns = |previous: &[TableColumn]| -> Vec<TableColumn> {
        headers
            .iter()
            .map(|n| match previous.iter().find(|p| p.name.eq_ignore_ascii_case(n)) {
                Some(p) => TableColumn { name: n.clone(), ..p.clone() },
                None => TableColumn { name: n.clone(), totals: TotalsFn::None, totals_label: None, formula: None },
            })
            .collect()
    };
    match existing.and_then(|i| sh.tables.get_mut(i)) {
        Some(t) => {
            t.columns = columns(&t.columns);
            t.range = new;
            // Filters named columns by position: they no longer apply.
            t.filter.clear();
        }
        None => {
            let id = cx.wb.next_object_id();
            let sh = cx.wb.sheet_mut(sheet).ok_or("no such sheet")?;
            sh.tables.push(Table {
                id,
                name: table.to_string(),
                range: new,
                header_row: true,
                totals_row: false,
                columns: columns(&[]),
                style: "TableStyleMedium2".into(),
                banded_rows: true,
                banded_cols: false,
                first_col: false,
                last_col: false,
                filter_button: true,
                filter: vec![],
            });
        }
    }
    cx.structural = true;
    Ok(Loaded { range: new, rows: grid.rows, cols: width, errors })
}

/// Widths that fit the header and the first rows of the loaded columns.
pub fn fit_columns(cx: &mut Ctx, sheet: usize, r: RangeRef) {
    let Some(sh) = cx.wb.sheet(sheet) else { return };
    let sample = RangeRef::new(r.start, CellRef::new(r.end.row.min(r.start.row + 200), r.end.col));
    let mut widths: HashMap<u32, f32> = HashMap::new();
    for (c, _) in sh.cells.iter_range(sample) {
        if !sample.contains(c) {
            continue;
        }
        let st = cx.wb.styles.get(sh.style_id(c));
        let text = crate::display::cell_text(&cx.wb, sh, c);
        let w = text.lines().map(|l| crate::cmd::format::approx_text_width(l, st.font.size, st.font.bold)).fold(0.0, f32::max)
            + 6.0
            + sh.filter_button_space(c);
        let e = widths.entry(c.col).or_insert(0.0);
        *e = e.max(w);
    }
    let Some(sh) = cx.wb.sheet_mut(sheet) else { return };
    for (col, w) in widths {
        let e = sh.cols.entry(col).or_default();
        e.size = Some(w.clamp(8.0, 400.0).ceil());
    }
}

/// A table name for a query: letters, digits and underscores, not starting with a digit, not a
/// cell reference, and not in use.
pub fn table_name_for(wb: &Workbook, query: &str) -> String {
    let mut base: String = query.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '.' { c } else { '_' }).collect();
    if base.is_empty() || base.chars().next().is_some_and(|c| c.is_ascii_digit()) || CellRef::parse(&base).is_some() {
        base.insert(0, '_');
    }
    let taken = |n: &str| wb.table(n).is_some() || wb.names.iter().any(|d| d.name.eq_ignore_ascii_case(n));
    if !taken(&base) {
        return base;
    }
    (2..).map(|k| format!("{base}_{k}")).find(|n| !taken(n)).unwrap_or(base)
}

/// A sheet name for a query: what the query is called, made valid and unique.
pub fn sheet_name_for(wb: &Workbook, query: &str) -> String {
    let clean: String = query.chars().filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')).take(31).collect();
    let clean = clean.trim_matches('\'').trim().to_string();
    let base = if clean.is_empty() { "Query".to_string() } else { clean };
    if wb.check_sheet_name(&base, None).is_ok() {
        return base;
    }
    (2..1000)
        .map(|k| {
            let suffix = format!(" ({k})");
            let head: String = base.chars().take(31 - suffix.chars().count()).collect();
            format!("{head}{suffix}")
        })
        .find(|n| wb.check_sheet_name(n, None).is_ok())
        .unwrap_or_else(|| wb.next_sheet_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serials_and_days_agree_with_the_date_systems() {
        let day = |y, m, d| days_from_civil(y, m, d);
        assert_eq!(serial_day(DateSystem::D1900, day(1900, 1, 1)), Some(1));
        assert_eq!(serial_day(DateSystem::D1900, day(1900, 2, 28)), Some(59));
        assert_eq!(serial_day(DateSystem::D1900, day(1900, 3, 1)), Some(61));
        assert_eq!(serial_day(DateSystem::D1900, day(2024, 1, 1)), Some(45292));
        assert_eq!(serial_day(DateSystem::D1900, day(9999, 12, 31)), Some(MAX_SERIAL));
        assert_eq!(serial_day(DateSystem::D1900, day(1899, 12, 31)), None);
        assert_eq!(serial_day(DateSystem::D1904, day(1904, 1, 1)), Some(0));
        assert_eq!(serial_day(DateSystem::D1904, day(2024, 1, 1)), Some(45292 - 1462));
        for sys in [DateSystem::D1900, DateSystem::D1904] {
            for s in [1, 2, 58, 59, 61, 62, 366, 45292, MAX_SERIAL - 1462] {
                assert_eq!(serial_day(sys, unix_day(sys, s)), Some(s), "{sys:?} {s}");
            }
        }
    }

    #[test]
    fn names_are_made_unique_and_valid() {
        assert_eq!(unique_names(["a".to_string(), "A".into(), "".into(), "a".into()].into_iter(), true), ["a", "A2", "Column3", "a3"]);
        assert_eq!(unique_names(["a".to_string(), "A".into()].into_iter(), false), ["a", "A"]);
        let wb = Workbook::new();
        assert_eq!(table_name_for(&wb, "Sales 2024"), "Sales_2024");
        assert_eq!(table_name_for(&wb, "2024"), "_2024");
        assert_eq!(table_name_for(&wb, "A1"), "_A1");
        assert_eq!(sheet_name_for(&wb, "Sheet1"), "Sheet1 (2)");
        assert_eq!(sheet_name_for(&wb, "a/b:c"), "abc");
        assert_eq!(sheet_name_for(&wb, &"x".repeat(40)).chars().count(), 31);
    }
}
