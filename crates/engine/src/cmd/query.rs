//! Data › Get & Transform Data: queries written in the Power Query M language. A query is a
//! named formula kept in the workbook; loading it puts its result in a table on a sheet, and
//! refreshing it runs it again.

use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Query, Sheet};
use gridcraft_mquery as mq;
use gridcraft_numfmt::NumberFormat;
use serde_json::{Value as Json, json};

use super::*;
use crate::query::{self, Grid, Scalar, Target};

/// Longest formula accepted, in bytes.
const MAX_FORMULA: usize = 1 << 20;
/// Rows a preview returns unless asked otherwise, and at most.
const PREVIEW_ROWS: usize = 200;
const MAX_PREVIEW_ROWS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "data.query.add",
            "New Query",
            [],
            None,
            "{formula: Power Query M text, name?, description?, load?: true, at?: \"B2\" (on `sheet` or the active sheet; default: a new sheet named `sheet` or after the query)} → {name, table?, sheet?, range?, rows?, cols?, errors?}; load: false keeps a connection only",
            has_doc,
            add
        ),
        cmd!(
            "data.query.set",
            "Edit Query",
            [],
            None,
            "{name, formula?, rename?, description?, refresh?: true (a loaded query runs again when its formula changes)} → {name, rows?, cols?, errors?}",
            has_doc,
            set
        ),
        cmd!(
            "data.query.delete",
            "Delete Query",
            [],
            None,
            "{name, keepTable?: true (the loaded table stays as an ordinary table)} → {name}",
            has_doc,
            delete
        ),
        cmd!(
            "data.query.load",
            "Load To…",
            [],
            None,
            "{name, at?: \"B2\" (on `sheet` or the active sheet; default: a new sheet)} loads a connection-only query to a table → {name, table, sheet, range, rows, cols, errors}",
            has_doc,
            load
        ),
        cmd!(
            "data.query.refresh",
            "Refresh",
            [],
            None,
            "{name?} runs the query again and replaces its table; without a name, every loaded query → {refreshed: [{name, rows, cols, errors}], errors: [{name, error}]}",
            has_doc,
            refresh
        ),
        cmd!(
            query "data.query.list",
            "Queries",
            [],
            None,
            "{} → [{name, formula, description, loadTo?, sheet?, range?, steps: [{name, formula}], result?: step name, error?}]",
            has_doc,
            list
        ),
        cmd!(
            query "data.query.preview",
            "Preview",
            [],
            None,
            "{name? (a query of the workbook), formula? (instead of the stored one), step? (a step of the formula's let), rows?: 200} → {kind, columns: [{name, type}], rows: [[…]], moreRows, errors}; a cell that failed is {error}",
            has_doc,
            preview
        ),
        cmd!(query "data.query.functions", "Functions", [], None, "{prefix?} → the Power Query M library: names of functions and constants", always, functions),
        cmd!(
            query "data.query.parse",
            "Check Syntax",
            [],
            None,
            "{formula} → {steps: [{name, formula}], result?: step name} when the formula parses, else {error, line, column}; nothing runs",
            always,
            parse
        ),
    ]
}

/// Parses a formula without running it: its steps for an editor, or where its syntax fails.
fn parse(_: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.query.parse";
    let formula = str_param(p, "formula").ok_or_else(|| bad(C, "missing `formula`"))?;
    if formula.len() > MAX_FORMULA {
        return Err(bad(C, "the formula is too long"));
    }
    Ok(match mq::steps(formula) {
        Ok(steps) => {
            let list: Vec<Json> =
                steps.steps.iter().map(|st| json!({"name": st.name, "formula": formula.get(st.span.0..st.span.1).unwrap_or("")})).collect();
            match steps.result {
                Some(r) => json!({"steps": list, "result": r}),
                None => json!({"steps": list}),
            }
        }
        Err(e) => {
            let (line, column) = e.line_col(formula);
            json!({"error": e.message, "line": line, "column": column})
        }
    })
}

fn other(message: String) -> EngineError {
    EngineError::Other(message)
}

/// The formula of a call, checked for size and syntax.
fn checked_formula<'a>(cmd: &str, p: &'a Json) -> Result<Option<&'a str>> {
    let Some(formula) = str_param(p, "formula") else { return Ok(None) };
    if formula.len() > MAX_FORMULA {
        return Err(bad(cmd, "the formula is too long"));
    }
    if let Err(e) = mq::parse(formula) {
        let (line, col) = e.line_col(formula);
        return Err(other(format!("The formula has a syntax error at line {line}, column {col}: {}", e.message)));
    }
    Ok(Some(formula))
}

/// A query name: not empty, at most 80 characters, no control characters or double quotes.
fn check_name(cmd: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() || name.trim() != name {
        return Err(bad(cmd, "a query name can't be blank or start or end with a space"));
    }
    if name.chars().count() > 80 || name.chars().any(|c| c.is_control() || c == '"') {
        return Err(bad(cmd, "a query name is at most 80 characters, without quotes"));
    }
    Ok(())
}

/// Where the table of a loaded query is: sheet index and the table.
fn loaded_table<'a>(wb: &'a Workbook, q: &Query) -> Option<(usize, &'a gridcraft_model::Table)> {
    let (si, ti) = wb.table(q.load_to.as_deref()?)?;
    Some((si, wb.sheet(si)?.tables.get(ti)?))
}

fn summary(name: &str, l: &query::Loaded) -> Json {
    json!({"name": name, "range": l.range.a1(), "rows": l.rows, "cols": l.cols, "errors": l.errors})
}

/// Evaluates to a full grid.
fn run(s: &Session, wb: &Workbook, target: &Target) -> Result<Grid> {
    let sys = wb.date_system;
    query::evaluate(wb, &s.data_sources, target, move |v| query::grid(sys, v, usize::MAX)).map_err(other)
}

/// Loads `grid` as the result of the query `name`: at `at` on an existing sheet, or on a new
/// sheet named `new_sheet`. Returns the sheet index, whether it is new, the table name and what
/// was written.
fn load_new(cx: &mut Ctx, name: &str, grid: &Grid, at: Option<(usize, CellRef)>, new_sheet: &str) -> Result<(usize, bool, String, query::Loaded)> {
    let table = query::table_name_for(&cx.wb, name);
    let (sheet, cell, created) = match at {
        Some((si, c)) => (si, c, false),
        None => {
            let i = (cx.wb.active_sheet + 1).min(cx.wb.sheets.len());
            cx.wb.sheets.insert(i, Arc::new(Sheet::new(new_sheet)));
            // Names scoped to later sheets shift.
            for n in cx.wb.names.iter_mut() {
                if let Some(sc) = n.scope.as_mut()
                    && *sc >= i
                {
                    *sc += 1;
                }
            }
            cx.wb.active_sheet = i;
            *cx.sel = crate::Selection::default();
            (i, CellRef::default(), true)
        }
    };
    let loaded = query::load(cx, sheet, cell, &table, grid).map_err(other)?;
    if created {
        query::fit_columns(cx, sheet, loaded.range);
    }
    Ok((sheet, created, table, loaded))
}

/// The destination a call asks for: `at` on `sheet` or the active sheet, else a new sheet with a
/// valid name.
fn destination(s: &Session, cmd: &str, p: &Json, query_name: &str) -> Result<(Option<(usize, CellRef)>, String)> {
    let d = s.doc()?;
    if p.get("at").is_some_and(|v| !v.is_null()) {
        let sheet = target_sheet(s, p)?;
        let cell = cell_param_on(s, p, "at", sheet)?.unwrap_or_default();
        return Ok((Some((sheet, cell)), String::new()));
    }
    if d.wb.protected_structure {
        return Err(other("The workbook is protected, so a sheet can't be added for the query. Give a cell with `at` instead.".into()));
    }
    let name = match str_param(p, "sheet") {
        Some(n) => {
            d.wb.check_sheet_name(n, None).map_err(|e| bad(cmd, e))?;
            n.to_string()
        }
        None => query::sheet_name_for(&d.wb, query_name),
    };
    Ok((None, name))
}

/// Records the new sheet's selection slot, as inserting a sheet does.
fn note_new_sheet(s: &mut Session, sheet: usize, created: bool) -> Result<()> {
    if created {
        let d = s.doc_mut()?;
        d.sheet_selections.insert(sheet.min(d.sheet_selections.len()), crate::Selection::default());
    }
    Ok(())
}

pub(crate) fn add(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.query.add";
    let formula = checked_formula(C, p)?.ok_or_else(|| bad(C, "missing `formula`"))?.to_string();
    let d = s.doc()?;
    let name = match str_param(p, "name") {
        Some(n) => {
            check_name(C, n)?;
            if d.wb.query(n).is_some() {
                return Err(other(format!("A query named '{n}' already exists.")));
            }
            n.to_string()
        }
        None => (1..).map(|k| format!("Query{k}")).find(|n| d.wb.query(n).is_none()).unwrap_or_else(|| "Query".into()),
    };
    let description = str_param(p, "description").unwrap_or("").to_string();
    if !bool_param(p, "load").unwrap_or(true) {
        return edit(s, |cx| {
            cx.wb.queries.push(Query { name: name.clone(), formula, description, load_to: None });
            // Nothing a formula can see changed, but the workbook did.
            cx.structural = true;
            Ok(json!({"name": name}))
        });
    }
    let (at, new_sheet) = destination(s, C, p, &name)?;
    let grid = run(s, &s.doc()?.wb, &Target::Formula { name: Some(&name), formula: &formula })?;
    let (sheet, created, table, loaded) = edit(s, |cx| {
        let r = load_new(cx, &name, &grid, at, &new_sheet)?;
        cx.wb.queries.push(Query { name: name.clone(), formula, description, load_to: Some(r.2.clone()) });
        Ok(r)
    })?;
    note_new_sheet(s, sheet, created)?;
    let sheet_name = s.doc()?.wb.sheet(sheet).map(|sh| sh.name.clone()).unwrap_or_default();
    let mut out = summary(&name, &loaded);
    out["table"] = json!(table);
    out["sheet"] = json!(sheet_name);
    Ok(out)
}

fn load(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.query.load";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let d = s.doc()?;
    let q = d.wb.query(name).ok_or_else(|| other(format!("There is no query named '{name}'.")))?.clone();
    if loaded_table(&d.wb, &q).is_some() {
        return Err(other(format!("The query '{}' is already loaded: refresh it instead.", q.name)));
    }
    let (at, new_sheet) = destination(s, C, p, &q.name)?;
    let grid = run(s, &s.doc()?.wb, &Target::Query(&q.name))?;
    let (sheet, created, table, loaded) = edit(s, |cx| {
        let r = load_new(cx, &q.name, &grid, at, &new_sheet)?;
        if let Some(stored) = cx.wb.queries.iter_mut().find(|x| x.name == q.name) {
            stored.load_to = Some(r.2.clone());
        }
        Ok(r)
    })?;
    note_new_sheet(s, sheet, created)?;
    let sheet_name = s.doc()?.wb.sheet(sheet).map(|sh| sh.name.clone()).unwrap_or_default();
    let mut out = summary(&q.name, &loaded);
    out["table"] = json!(table);
    out["sheet"] = json!(sheet_name);
    Ok(out)
}

/// Runs a loaded query and replaces its table. `formula` stands in for the stored formula.
fn reload(s: &mut Session, q: &Query, formula: Option<&str>) -> Result<query::Loaded> {
    let d = s.doc()?;
    let Some(table) = q.load_to.clone() else { return Err(other(format!("The query '{}' isn't loaded to a sheet.", q.name))) };
    let Some((sheet, t)) = loaded_table(&d.wb, q) else {
        return Err(other(format!("The table '{table}' that the query '{}' loads to is gone. Load the query again.", q.name)));
    };
    let at = t.range.start;
    let target = match formula {
        Some(f) => Target::Formula { name: Some(&q.name), formula: f },
        None => Target::Query(&q.name),
    };
    let grid = run(s, &d.wb, &target)?;
    edit(s, |cx| query::load(cx, sheet, at, &table, &grid).map_err(other))
}

fn set(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.query.set";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let formula = checked_formula(C, p)?.map(str::to_string);
    let d = s.doc()?;
    let old = d.wb.query(name).ok_or_else(|| other(format!("There is no query named '{name}'.")))?.clone();
    let rename = match str_param(p, "rename") {
        Some(n) if n != old.name => {
            check_name(C, n)?;
            if d.wb.query(n).is_some_and(|q| q.name != old.name) {
                return Err(other(format!("A query named '{n}' already exists.")));
            }
            Some(n.to_string())
        }
        _ => None,
    };
    let description = str_param(p, "description").map(str::to_string);
    let changed = formula.as_ref().is_some_and(|f| *f != old.formula);
    // Run the new formula before anything is stored: a formula that fails changes nothing.
    let loaded =
        if changed && old.load_to.is_some() && bool_param(p, "refresh").unwrap_or(true) { Some(reload(s, &old, formula.as_deref())?) } else { None };
    let new_name = rename.clone().unwrap_or_else(|| old.name.clone());
    edit(s, |cx| {
        if let Some(q) = cx.wb.queries.iter_mut().find(|q| q.name == old.name) {
            if let Some(f) = formula {
                q.formula = f;
            }
            if let Some(d) = description {
                q.description = d;
            }
            if let Some(n) = rename {
                q.name = n;
            }
        }
        cx.structural = true;
        Ok(())
    })?;
    Ok(match loaded {
        Some(l) => summary(&new_name, &l),
        None => json!({"name": new_name}),
    })
}

fn delete(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.query.delete";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let d = s.doc()?;
    let q = d.wb.query(name).ok_or_else(|| other(format!("There is no query named '{name}'.")))?.clone();
    let drop_table = !bool_param(p, "keepTable").unwrap_or(true);
    let table = if drop_table { loaded_table(&d.wb, &q).map(|(si, t)| (si, t.name.clone(), t.range)) } else { None };
    edit(s, |cx| {
        cx.wb.queries.retain(|x| x.name != q.name);
        if let Some((si, table, range)) = table {
            let sh = cx.sheet_mut(si)?;
            sh.tables.retain(|t| t.name != table);
            let cells: Vec<CellRef> = sh.cells.iter_range(range).map(|(c, _)| c).filter(|c| range.contains(*c)).collect();
            for c in cells {
                sh.set_value(c, Value::Empty);
            }
        }
        cx.structural = true;
        Ok(json!({"name": q.name}))
    })
}

fn refresh(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    if let Some(name) = str_param(p, "name") {
        let q = d.wb.query(name).ok_or_else(|| other(format!("There is no query named '{name}'.")))?.clone();
        let loaded = reload(s, &q, None)?;
        return Ok(json!({"refreshed": [summary(&q.name, &loaded)], "errors": []}));
    }
    Ok(refresh_all(s))
}

/// Refreshes every loaded query, in the order of the Queries pane, so that a query reading the
/// table of an earlier one sees its new rows. A query that fails is reported and the others go on.
pub(crate) fn refresh_all(s: &mut Session) -> Json {
    let queries: Vec<Query> = s.active().map(|d| d.wb.queries.iter().filter(|q| q.load_to.is_some()).cloned().collect()).unwrap_or_default();
    let mut refreshed = Vec::new();
    let mut errors = Vec::new();
    for q in queries {
        match reload(s, &q, None) {
            Ok(l) => refreshed.push(summary(&q.name, &l)),
            Err(e) => errors.push(json!({"name": q.name, "error": e.to_string()})),
        }
    }
    json!({"refreshed": refreshed, "errors": errors})
}

fn list(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let out: Vec<Json> = d
        .wb
        .queries
        .iter()
        .map(|q| {
            let mut o = json!({"name": q.name, "formula": q.formula, "description": q.description});
            if let Some(table) = &q.load_to {
                o["loadTo"] = json!(table);
            }
            if let Some((si, t)) = loaded_table(&d.wb, q) {
                o["sheet"] = json!(d.wb.sheet(si).map(|sh| sh.name.as_str()));
                o["range"] = json!(t.range.a1());
            }
            match mq::steps(&q.formula) {
                Ok(steps) => {
                    o["steps"] =
                        steps.steps.iter().map(|st| json!({"name": st.name, "formula": q.formula.get(st.span.0..st.span.1).unwrap_or("")})).collect();
                    if let Some(r) = steps.result {
                        o["result"] = json!(r);
                    }
                }
                Err(e) => o["error"] = json!(e.message),
            }
            o
        })
        .collect();
    Ok(Json::Array(out))
}

fn preview(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.query.preview";
    let d = s.doc()?;
    let name = str_param(p, "name");
    let stored = match name {
        Some(n) => d.wb.query(n),
        None => None,
    };
    let formula = match (str_param(p, "formula"), stored) {
        (Some(f), _) if f.len() > MAX_FORMULA => return Err(bad(C, "the formula is too long")),
        (Some(f), _) => f,
        (None, Some(q)) => q.formula.as_str(),
        (None, None) => return Err(bad(C, "give the `name` of a query or a `formula`")),
    };
    let name = stored.map(|q| q.name.as_str()).or(name);
    let target = match str_param(p, "step") {
        Some(step) => Target::Step { name, formula, step },
        None => Target::Formula { name, formula },
    };
    let rows = u32_param(p, "rows").map_or(PREVIEW_ROWS, |n| (n as usize).clamp(1, MAX_PREVIEW_ROWS));
    let sys = d.wb.date_system;
    let grid = query::evaluate(&d.wb, &s.data_sources, &target, move |v| query::grid(sys, v, rows)).map_err(other)?;
    Ok(grid_json(&grid, sys))
}

/// A preview grid as JSON: dates, times and percentages as they will show on the sheet.
pub(crate) fn grid_json(grid: &Grid, sys: gridcraft_core::DateSystem) -> Json {
    let columns: Vec<Json> = grid.columns.iter().zip(&grid.types).map(|(n, t)| json!({"name": n, "type": t})).collect();
    let mut formats: std::collections::HashMap<&'static str, NumberFormat> = std::collections::HashMap::new();
    let mut out_rows = Vec::with_capacity(grid.rows);
    for r in 0..grid.rows {
        let mut row = Vec::with_capacity(grid.width());
        for c in 0..grid.width() {
            row.push(match grid.cell(r, c) {
                Some(Scalar::Error(e)) => json!({"error": e}),
                Some(Scalar::Value(v)) => match (v, grid.format_code(r, c)) {
                    // Dates, times and percentages as they will show on the sheet.
                    (Value::Number(_), Some(code)) => {
                        let fmt = formats.entry(code).or_insert_with(|| NumberFormat::parse(code));
                        json!(gridcraft_numfmt::format_value(v, fmt, sys).text)
                    }
                    (Value::Number(n), None) => json!(n),
                    (Value::Bool(b), _) => json!(b),
                    (Value::Empty, _) => Json::Null,
                    (v, _) => json!(v.display()),
                },
                None => Json::Null,
            });
        }
        out_rows.push(Json::Array(row));
    }
    json!({"kind": grid.kind, "columns": columns, "rows": out_rows, "moreRows": grid.more_rows, "errors": grid.error_count()})
}

fn functions(_: &mut Session, p: &Json) -> Result<Json> {
    let prefix = str_param(p, "prefix").unwrap_or("").to_lowercase();
    Ok(mq::library_names().into_iter().filter(|n| n.to_lowercase().starts_with(&prefix)).map(Json::from).collect())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;
    use crate::core::{CellRef, Value};

    /// A workbook with the table `Stock` (Item, Qty, Price) at A1:C4 of Sheet1.
    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({})).unwrap();
        let rows = json!([["Item", "Qty", "Price"], ["pen", 10, 1.5], ["ink", 0, 8], ["pad", 7, 3]]);
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s.execute("insert.table", json!({"range": "A1:C4", "header": true, "name": "Stock"})).unwrap();
        s
    }

    /// The values of a range of the sheet `sheet`, as JSON rows.
    fn values(s: &Session, sheet: &str, range: &str) -> serde_json::Value {
        let wb = &s.doc().unwrap().wb;
        let sh = wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap();
        let r = crate::core::RangeRef::parse(range).unwrap();
        let rows = (r.start.row..=r.end.row).map(|row| {
            (r.start.col..=r.end.col)
                .map(|col| match sh.value(CellRef::new(row, col)) {
                    Value::Number(n) => json!(n),
                    Value::Bool(b) => json!(b),
                    Value::Empty => serde_json::Value::Null,
                    v => json!(v.display()),
                })
                .collect::<Vec<_>>()
        });
        json!(rows.collect::<Vec<_>>())
    }

    /// Sets cells of Sheet1, whatever sheet is active.
    fn set(s: &mut Session, range: &str, rows: serde_json::Value) {
        s.execute("range.setValues", json!({"sheet": "Sheet1", "range": range, "values": rows})).unwrap();
    }

    const IN_STOCK: &str = "let\n    Source = Excel.CurrentWorkbook(){[Name=\"Stock\"]}[Content],\n    Kept = Table.SelectRows(Source, each [Qty] > 0),\n    Valued = Table.AddColumn(Kept, \"Value\", each [Qty] * [Price], type number)\nin\n    Valued";

    #[test]
    fn a_query_loads_to_a_table_on_a_new_sheet() {
        let mut s = session();
        let r = s.execute("data.query.add", json!({"name": "In Stock", "formula": IN_STOCK})).unwrap();
        assert_eq!(r, json!({"name": "In Stock", "table": "In_Stock", "sheet": "In Stock", "range": "A1:D3", "rows": 2, "cols": 4, "errors": 0}));
        let wb = s.doc().unwrap().wb.clone();
        assert_eq!(wb.active().unwrap().name, "In Stock");
        assert_eq!(wb.queries.len(), 1);
        assert_eq!(wb.queries[0].load_to.as_deref(), Some("In_Stock"));
        let t = &wb.active().unwrap().tables[0];
        assert_eq!((t.name.as_str(), t.range.a1()), ("In_Stock", "A1:D3".to_string()));
        assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Item", "Qty", "Price", "Value"]);
        assert_eq!(values(&s, "In Stock", "A1:D3"), json!([["Item", "Qty", "Price", "Value"], ["pen", 10.0, 1.5, 15.0], ["pad", 7.0, 3.0, 21.0]]));
        // One step back undoes the sheet, the table and the query together.
        s.execute("edit.undo", json!({})).unwrap();
        let wb = s.doc().unwrap().wb.clone();
        assert!(wb.queries.is_empty() && wb.sheets.len() == 1 && wb.table("In_Stock").is_none());
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().wb.queries.len(), 1);
    }

    #[test]
    fn refreshing_follows_the_source_table() {
        let mut s = session();
        s.execute("data.query.add", json!({"name": "In Stock", "formula": IN_STOCK})).unwrap();
        // Ink is back in stock and there is a new item: the result grows from two rows to four.
        set(&mut s, "B3", json!([[5]]));
        set(&mut s, "A5", json!([["cap", 2, 0.5]]));
        // The table takes the row under it when it isn't extended automatically.
        let stock = |s: &Session| {
            let wb = s.doc().unwrap().wb.clone();
            let (si, ti) = wb.table("Stock").unwrap();
            wb.sheets[si].tables[ti].range.a1()
        };
        if stock(&s) != "A1:C5" {
            s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
            s.execute("table.resize", json!({"table": "Stock", "range": "A1:C5"})).unwrap();
        }
        assert_eq!(stock(&s), "A1:C5");
        let r = s.execute("data.query.refresh", json!({"name": "In Stock"})).unwrap();
        assert_eq!(r["refreshed"][0], json!({"name": "In Stock", "range": "A1:D5", "rows": 4, "cols": 4, "errors": 0}));
        assert_eq!(values(&s, "In Stock", "A4:D5"), json!([["pad", 7.0, 3.0, 21.0], ["cap", 2.0, 0.5, 1.0]]));
        let wb = s.doc().unwrap().wb.clone();
        let (si, ti) = wb.table("In_Stock").unwrap();
        assert_eq!(wb.sheets[si].tables[ti].range.a1(), "A1:D5");
        // Nearly everything sells out: the result shrinks and the rows it leaves are cleared.
        set(&mut s, "B2", json!([[0], [0], [0], [4]]));
        let r = s.execute("data.query.refresh", json!({})).unwrap();
        assert_eq!(r["refreshed"][0]["range"], "A1:D2");
        let wb = s.doc().unwrap().wb.clone();
        assert_eq!(wb.sheets[si].tables[ti].range.a1(), "A1:D2");
        assert_eq!(
            values(&s, "In Stock", "A2:D5"),
            json!([["cap", 4.0, 0.5, 2.0], [null, null, null, null], [null, null, null, null], [null, null, null, null]])
        );
    }

    #[test]
    fn refresh_all_runs_queries() {
        let mut s = session();
        s.execute("data.query.add", json!({"name": "Q", "formula": IN_STOCK})).unwrap();
        set(&mut s, "B3", json!([[1]]));
        let r = s.execute("data.refreshAll", json!({})).unwrap();
        assert_eq!(r["queries"]["refreshed"][0]["rows"], 3);
        assert_eq!(r["queries"]["errors"], json!([]));
    }

    #[test]
    fn dates_types_and_errors_load_as_the_sheet_shows_them() {
        let mut s = session();
        let formula = "Table.TransformColumnTypes(#table({\"d\", \"n\", \"p\", \"t\"}, {{\"2024-01-31\", \"7\", 0.25, #time(6, 0, 0)}, {\"nope\", \"x\", 0.5, null}}), {{\"d\", type date}, {\"n\", Int64.Type}, {\"p\", Percentage.Type}})";
        let p = s.execute("data.query.preview", json!({"formula": formula})).unwrap();
        assert_eq!(p["kind"], "table");
        assert_eq!(
            p["columns"],
            json!([{"name": "d", "type": "date"}, {"name": "n", "type": "int64"}, {"name": "p", "type": "percentage"}, {"name": "t", "type": "any"}])
        );
        assert_eq!(p["rows"][0], json!(["1/31/2024", 7.0, "25.00%", "6:00:00"]));
        assert!(p["rows"][1][0]["error"].as_str().unwrap().contains("can't be read as a date"));
        assert_eq!(p["errors"], 2);
        let r = s.execute("data.query.add", json!({"name": "Typed", "formula": formula, "at": "F1"})).unwrap();
        assert_eq!((r["sheet"].as_str(), r["range"].as_str(), r["errors"].as_u64()), (Some("Sheet1"), Some("F1:I3"), Some(2)));
        let wb = s.doc().unwrap().wb.clone();
        let sh = wb.active().unwrap();
        let at = |a: &str| CellRef::parse(a).unwrap();
        let fmt = |a: &str| wb.styles.get(sh.style_id(at(a))).num_fmt.as_str().to_string();
        assert_eq!(sh.value(at("F2")), Value::Number(45322.0));
        assert_eq!((fmt("F2"), fmt("H2"), fmt("I2"), fmt("G2")), ("m/d/yyyy".into(), "0.00%".into(), "h:mm:ss".into(), "General".into()));
        // Cells whose value was an error load empty.
        assert!(sh.value(at("F3")).is_empty() && sh.value(at("G3")).is_empty());
        assert_eq!(sh.value(at("H3")), Value::Number(0.5));
    }

    #[test]
    fn dates_in_the_sheet_reach_the_query_as_dates() {
        let mut s = session();
        set(&mut s, "F1", json!([["When", "At"], ["2024-01-31", "6:30"], ["2024-02-29", "18:00"]]));
        s.execute("insert.table", json!({"range": "F1:G3", "header": true, "name": "Days"})).unwrap();
        let p = s
            .execute(
                "data.query.preview",
                json!({"formula": "Table.AddColumn(Excel.CurrentWorkbook(){[Name=\"Days\"]}[Content], \"Next\", each Date.AddDays([When], 1))"}),
            )
            .unwrap();
        assert_eq!(p["rows"], json!([["1/31/2024", "6:30:00", "2/1/2024"], ["2/29/2024", "18:00:00", "3/1/2024"]]));
        let p = s
            .execute("data.query.preview", json!({"formula": "Value.Type(Excel.CurrentWorkbook(){[Name=\"Days\"]}[Content]{0}[When]) = type date"}))
            .unwrap();
        assert_eq!(p["rows"], json!([[true]]));
    }

    #[test]
    fn connection_only_queries_feed_others() {
        let mut s = session();
        let r = s
            .execute("data.query.add", json!({"name": "Base", "formula": "Excel.CurrentWorkbook(){[Name=\"Stock\"]}[Content]", "load": false}))
            .unwrap();
        assert_eq!(r, json!({"name": "Base"}));
        assert_eq!(s.doc().unwrap().wb.sheets.len(), 1);
        let r = s.execute("data.query.add", json!({"name": "Total", "formula": "#table({\"Total\"}, {{List.Sum(Base[Qty])}})"})).unwrap();
        assert_eq!(r["rows"], 1);
        assert_eq!(values(&s, "Total", "A1:A2"), json!([["Total"], [17.0]]));
        let l = s.execute("data.query.list", json!({})).unwrap();
        assert_eq!(l[0]["name"], "Base");
        assert!(l[0].get("loadTo").is_none());
        assert_eq!((l[1]["loadTo"].as_str(), l[1]["sheet"].as_str(), l[1]["range"].as_str()), (Some("Total"), Some("Total"), Some("A1:A2")));
        // A formula being edited stands in for the stored one while it is previewed.
        let p = s.execute("data.query.preview", json!({"name": "Base", "formula": "#table({\"Qty\"}, {{100}})"})).unwrap();
        assert_eq!(p["rows"], json!([[100.0]]));
        let p = s.execute("data.query.preview", json!({"formula": "List.Sum(Base[Qty])"})).unwrap();
        assert_eq!((p["kind"].as_str(), &p["rows"]), (Some("number"), &json!([[17.0]])));
        // Loading it later, at a cell.
        let r = s.execute("data.query.load", json!({"name": "base", "sheet": "Sheet1", "at": "F1"})).unwrap();
        assert_eq!((r["table"].as_str(), r["range"].as_str(), r["sheet"].as_str()), (Some("Base"), Some("F1:H4"), Some("Sheet1")));
        assert!(s.execute("data.query.load", json!({"name": "Base"})).unwrap_err().to_string().contains("already loaded"));
    }

    #[test]
    fn steps_can_be_listed_and_previewed() {
        let mut s = session();
        s.execute("data.query.add", json!({"name": "Q", "formula": IN_STOCK, "load": false})).unwrap();
        let l = s.execute("data.query.list", json!({})).unwrap();
        assert_eq!(l[0]["steps"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Source", "Kept", "Valued"]);
        assert_eq!(l[0]["steps"][1]["formula"], "Table.SelectRows(Source, each [Qty] > 0)");
        assert_eq!(l[0]["result"], "Valued");
        let p = s.execute("data.query.preview", json!({"name": "Q", "step": "Source"})).unwrap();
        assert_eq!(p["rows"].as_array().unwrap().len(), 3);
        let p = s.execute("data.query.preview", json!({"name": "Q", "step": "Kept", "rows": 1})).unwrap();
        assert_eq!((p["rows"].as_array().unwrap().len(), p["moreRows"].as_u64()), (1, Some(1)));
        let f = s.execute("data.query.functions", json!({"prefix": "table.select"})).unwrap();
        assert_eq!(f, json!(["Table.SelectColumns", "Table.SelectRows", "Table.SelectRowsWithErrors"]));
        // Parsing alone, for an editor: steps, or where the syntax fails. Nothing runs.
        let p = s.execute("data.query.parse", json!({"formula": IN_STOCK})).unwrap();
        assert_eq!((p["steps"].as_array().unwrap().len(), p["result"].as_str()), (3, Some("Valued")));
        assert_eq!(p["steps"][0], json!({"name": "Source", "formula": "Excel.CurrentWorkbook(){[Name=\"Stock\"]}[Content]"}));
        let p = s.execute("data.query.parse", json!({"formula": "let\n  a = 1,\n  b = )\nin a"})).unwrap();
        assert_eq!((p["line"].as_u64(), p["column"].as_u64(), p["error"].as_str()), (Some(3), Some(7), Some("Expected an expression.")));
        assert_eq!(s.execute("data.query.parse", json!({"formula": "1 + 2"})).unwrap(), json!({"steps": []}));
        assert!(s.execute("data.query.parse", json!({})).is_err());
    }

    #[test]
    fn editing_and_deleting() {
        let mut s = session();
        s.execute("data.query.add", json!({"name": "Q", "formula": IN_STOCK})).unwrap();
        let before = s.doc().unwrap().wb.clone();
        // A formula that doesn't parse, or fails, leaves everything as it was.
        let e = s.execute("data.query.set", json!({"name": "Q", "formula": "let a = in"})).unwrap_err().to_string();
        assert!(e.contains("syntax error at line 1, column 9"), "{e}");
        let e = s.execute("data.query.set", json!({"name": "Q", "formula": "Table.SelectRows(Nope, each true)"})).unwrap_err().to_string();
        assert!(e.contains("'Nope' isn't recognized"), "{e}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.doc().unwrap().wb));
        // A new formula runs at once and replaces the table where it is.
        let r = s
            .execute(
                "data.query.set",
                json!({"name": "Q", "formula": "#table({\"a\"}, {{1}, {2}, {3}})", "rename": "Numbers", "description": "three rows"}),
            )
            .unwrap();
        assert_eq!(r, json!({"name": "Numbers", "range": "A1:A4", "rows": 3, "cols": 1, "errors": 0}));
        assert_eq!(values(&s, "Q", "A1:B4"), json!([["a", null], [1.0, null], [2.0, null], [3.0, null]]));
        let wb = s.doc().unwrap().wb.clone();
        assert_eq!(
            (wb.queries[0].name.as_str(), wb.queries[0].description.as_str(), wb.queries[0].load_to.as_deref()),
            ("Numbers", "three rows", Some("Q"))
        );
        assert!(s.execute("data.query.add", json!({"name": "numbers", "formula": "1"})).unwrap_err().to_string().contains("already exists"));
        // Deleting keeps the table unless told otherwise.
        s.execute("data.query.delete", json!({"name": "Numbers"})).unwrap();
        let wb = s.doc().unwrap().wb.clone();
        assert!(wb.queries.is_empty() && wb.table("Q").is_some());
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("data.query.delete", json!({"name": "Numbers", "keepTable": false})).unwrap();
        let wb = s.doc().unwrap().wb.clone();
        assert!(wb.queries.is_empty() && wb.table("Q").is_none() && wb.active().unwrap().used_range().is_none());
    }

    #[test]
    fn a_refresh_needs_room() {
        let mut s = session();
        s.execute("data.query.add", json!({"name": "Q", "formula": IN_STOCK})).unwrap();
        // Something under the table, where a third row would go.
        s.execute("cell.set", json!({"cell": "B4", "input": "note"})).unwrap();
        set(&mut s, "B3", json!([[1]]));
        let before = s.doc().unwrap().wb.clone();
        let e = s.execute("data.query.refresh", json!({"name": "Q"})).unwrap_err().to_string();
        assert!(e.contains("There isn't room to load 'Q'") && e.contains("A4:D4"), "{e}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.doc().unwrap().wb));
        // The table of a query can't be put over cells in use.
        let e = s
            .execute("data.query.add", json!({"name": "Over", "formula": "#table({\"a\"}, {{1}})", "sheet": "Sheet1", "at": "B2"}))
            .unwrap_err()
            .to_string();
        assert!(e.contains("aren't empty"), "{e}");
        assert!(s.execute("data.query.refresh", json!({"name": "Nope"})).is_err());
    }

    #[test]
    fn the_totals_row_moves_with_the_table() {
        let mut s = session();
        s.execute("data.query.add", json!({"name": "Q", "formula": IN_STOCK})).unwrap();
        s.execute("table.totalRow", json!({"table": "Q", "on": true})).unwrap();
        let wb = s.doc().unwrap().wb.clone();
        let (si, ti) = wb.table("Q").unwrap();
        if !wb.sheets[si].tables[ti].totals_row {
            return;
        }
        assert_eq!(wb.sheets[si].tables[ti].range.a1(), "A1:D4");
        set(&mut s, "B3", json!([[1]]));
        s.execute("data.query.refresh", json!({"name": "Q"})).unwrap();
        let wb = s.doc().unwrap().wb.clone();
        let t = &wb.sheets[si].tables[ti];
        assert_eq!((t.range.a1(), t.totals_row), ("A1:D5".to_string(), true));
        assert_eq!(values(&s, "Q", "A4:A4"), json!([["pad"]]));
    }

    #[test]
    fn hostile_queries_end_in_errors() {
        let mut s = session();
        for formula in [
            "let f = (n) => @f(n + 1) in f(0)",
            "{1..1e12}",
            "error \"boom\"",
            "File.Contents(\"Z:\\\\no\\\\such.file\")",
            "Web.Contents(\"https://example.com\")",
        ] {
            assert!(s.execute("data.query.preview", json!({"formula": formula})).is_err(), "{formula}");
            assert!(s.execute("data.query.add", json!({"formula": formula})).is_err(), "{formula}");
        }
        assert!(s.doc().unwrap().wb.queries.is_empty());
        // Deep recursion that the evaluation thread has room for.
        let p = s.execute("data.query.preview", json!({"formula": "let sum = (n) => if n = 0 then 0 else n + @sum(n - 1) in sum(1500)"})).unwrap();
        assert_eq!(p["rows"], json!([[1125750.0]]));
    }
}
