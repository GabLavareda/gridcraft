//! The Query Editor (Data › Get & Transform Data): a query's name, its formula in the Power Query
//! M language, its steps and a preview of what it gives. The engine does the work:
//! `data.query.parse` lists the steps, `data.query.preview` runs the formula or one step, and
//! `data.query.add` / `data.query.set` save it.

use egui::{Color32, RichText, Ui};
use serde_json::{Map, Value as Json, json};

use crate::SheetApp;
use crate::dialogs::Dialog;
use crate::i18n;

/// Rows a preview shows.
const PREVIEW_ROWS: u64 = 100;
/// Height of the band with the steps and the formula.
const BAND_HEIGHT: f32 = 230.0;

fn tr<'a>(ui: &Ui, text: &'a str) -> &'a str {
    i18n::current(ui.ctx()).tr(text)
}

fn text(values: &Map<String, Json>, key: &str) -> String {
    values.get(key).and_then(Json::as_str).unwrap_or("").to_string()
}

/// A name for a new query that the workbook doesn't use yet.
fn new_name(app: &SheetApp, base: &str) -> String {
    let taken = |n: &str| app.session.active().is_some_and(|d| d.wb.query(n).is_some());
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|k| format!("{base} ({k})")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

/// The editor's starting values. `{name}` edits that query; `{table}` starts a query that reads
/// the table; `{fromSelection: true}` one that reads the table at the selection (Data › From
/// Table/Range); `{formula}` starts one with that formula; `{}` starts an example. `None` when
/// there is nothing to edit: the reason is shown as a message.
pub fn defaults(app: &mut SheetApp, params: &Json) -> Option<Json> {
    let stored = params.get("name").and_then(Json::as_str).and_then(|n| app.session.active()?.wb.query(n).cloned());
    if let Some(q) = stored {
        return Some(
            json!({"original": q.name, "name": q.name, "formula": q.formula, "description": q.description, "load": q.load_to.is_some(), "autoPreview": true}),
        );
    }
    let mut table = params.get("table").and_then(Json::as_str).map(str::to_string);
    if params.get("fromSelection").and_then(Json::as_bool) == Some(true) {
        table = app.session.active().and_then(|d| d.wb.active().and_then(|sh| sh.table_at(d.selection.active)).map(|t| t.name.clone()));
        if table.is_none() {
            app.message =
                Some(("From Table/Range".into(), "Select a cell in a table first. To make one, select the data and choose Insert › Table.".into()));
            return None;
        }
    }
    let (name, formula) = match (table.as_deref(), params.get("formula").and_then(Json::as_str)) {
        (Some(table), _) => (
            new_name(app, table),
            format!("let\n    Source = Excel.CurrentWorkbook(){{[Name = \"{}\"]}}[Content]\nin\n    Source", table.replace('"', "\"\"")),
        ),
        (None, Some(f)) => (new_name(app, "Query1"), f.to_string()),
        (None, None) => (
            new_name(app, "Query1"),
            "let\n    Source = #table({\"Item\", \"Qty\"}, {{\"pen\", 10}, {\"ink\", 3}}),\n    Kept = Table.SelectRows(Source, each [Qty] > 5)\nin\n    Kept".to_string(),
        ),
    };
    Some(json!({"original": null, "name": name, "formula": formula, "description": "", "load": true, "autoPreview": table.is_some()}))
}

/// Runs the preview of the whole formula, or of the selected step.
fn run_preview(app: &mut SheetApp, d: &mut Dialog) {
    let mut params = json!({"formula": text(&d.values, "formula"), "rows": PREVIEW_ROWS});
    // The query being edited stands in for the stored one under its stored name.
    let original = d.values.get("original").and_then(Json::as_str).map(str::to_string);
    params["name"] = json!(original.unwrap_or_else(|| text(&d.values, "name")));
    if let Some(step) = d.values.get("step").and_then(Json::as_str) {
        params["step"] = json!(step);
    }
    match app.session.run("data.query.preview", params) {
        Ok(r) => {
            d.values.insert("preview".into(), r);
            d.values.remove("previewError");
        }
        Err(e) => {
            d.values.remove("preview");
            d.values.insert("previewError".into(), json!(crate::clean_error(&e)));
            // A database that wants a login or an approval asks for it over the editor.
            crate::database_dialog::ask_for_needs(app, None);
        }
    }
    d.values.insert("previewFor".into(), json!(text(&d.values, "formula")));
}

/// Saves the query: a new one is added (and loaded when asked), an existing one changed.
fn save(app: &mut SheetApp, d: &mut Dialog) -> bool {
    let name = text(&d.values, "name").trim().to_string();
    let formula = text(&d.values, "formula");
    let description = text(&d.values, "description");
    let result = match d.values.get("original").and_then(Json::as_str).map(str::to_string) {
        Some(original) => app.run("data.query.set", json!({"name": original, "rename": name, "formula": formula, "description": description})),
        None => {
            let load = d.values.get("load").and_then(Json::as_bool).unwrap_or(true);
            app.run("data.query.add", json!({"name": name, "formula": formula, "description": description, "load": load}))
        }
    };
    match result {
        Ok(r) => {
            let msg = match r.get("rows") {
                Some(rows) => format!("{name}: {rows} row(s) loaded."),
                None => format!("{name} saved."),
            };
            app.toast = Some((msg, crate::now_ms()));
            true
        }
        Err(e) => {
            d.values.insert("saveError".into(), json!(crate::clean_error(&e)));
            false
        }
    }
}

/// A preview cell as text, and whether it holds an error.
pub(crate) fn cell_text(v: &Json) -> (String, Option<String>) {
    match v {
        Json::Null => ("null".into(), None),
        Json::Bool(b) => ((if *b { "TRUE" } else { "FALSE" }).into(), None),
        Json::Number(n) => match n.as_f64() {
            Some(x) if x.fract() == 0.0 && x.abs() < 1e15 => (format!("{}", x as i64), None),
            Some(x) => (format!("{x}"), None),
            None => (n.to_string(), None),
        },
        Json::String(s) => (s.clone(), None),
        Json::Object(o) => ("Error".into(), Some(o.get("error").and_then(Json::as_str).unwrap_or("").to_string())),
        Json::Array(_) => ("[List]".into(), None),
    }
}

fn preview(ui: &mut Ui, d: &Dialog) {
    if let Some(e) = d.values.get("previewError").and_then(Json::as_str) {
        ui.label(RichText::new(e).color(Color32::from_rgb(0xC4, 0x2B, 0x1C)));
        return;
    }
    let Some(p) = d.values.get("preview") else {
        ui.label(RichText::new(tr(ui, "Choose Preview to run the query.")).italics().color(Color32::GRAY));
        return;
    };
    let rows = p["rows"].as_array().cloned().unwrap_or_default();
    let columns = p["columns"].as_array().cloned().unwrap_or_default();
    let more = p["moreRows"].as_u64().unwrap_or(0);
    let mut summary = format!("{} · {} row(s)", p["kind"].as_str().unwrap_or(""), rows.len() as u64 + more);
    if more > 0 {
        summary.push_str(&format!(", first {} shown", rows.len()));
    }
    if let Some(errors) = p["errors"].as_u64().filter(|n| *n > 0) {
        summary.push_str(&format!(" · {errors} error(s)"));
    }
    if d.values.get("previewFor").and_then(Json::as_str) != d.values.get("formula").and_then(Json::as_str) {
        summary.push_str(" · the formula changed since");
    }
    ui.label(RichText::new(summary).small().color(Color32::from_gray(110)));
    egui::ScrollArea::both().id_salt("query_preview").max_height(230.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("query_preview_grid").striped(true).min_col_width(60.0).show(ui, |ui| {
            for c in &columns {
                ui.vertical(|ui| {
                    ui.label(RichText::new(c["name"].as_str().unwrap_or("")).strong());
                    ui.label(RichText::new(c["type"].as_str().unwrap_or("")).small().color(Color32::GRAY));
                });
            }
            ui.end_row();
            for row in &rows {
                for v in row.as_array().into_iter().flatten() {
                    match cell_text(v) {
                        (t, Some(why)) => {
                            ui.label(RichText::new(t).color(Color32::from_rgb(0xC4, 0x2B, 0x1C))).on_hover_text(why);
                        }
                        (t, None) if v.is_null() => {
                            ui.label(RichText::new(t).italics().color(Color32::GRAY));
                        }
                        (t, None) => {
                            ui.label(t);
                        }
                    }
                }
                ui.end_row();
            }
        });
    });
}

/// Draws the editor; `open` turns false when it should close.
pub fn show(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog, open: &mut bool) {
    let formula = text(&d.values, "formula");
    // The steps follow the formula as it is typed; nothing runs until Preview.
    if d.values.get("parsedFor").and_then(Json::as_str) != Some(formula.as_str()) {
        let parsed = app.session.run("data.query.parse", json!({"formula": formula})).unwrap_or(Json::Null);
        d.values.insert("parsed".into(), parsed);
        d.values.insert("parsedFor".into(), json!(formula));
    }
    if d.values.remove("autoPreview").is_some_and(|v| v == json!(true)) {
        run_preview(app, d);
    }

    egui::Grid::new("query_fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for (key, label) in [("name", "Name"), ("description", "Description")] {
            ui.label(tr(ui, label));
            let mut v = text(&d.values, key);
            if ui.add(egui::TextEdit::singleline(&mut v).desired_width(420.0)).changed() {
                d.values.insert(key.into(), json!(v));
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);

    let parsed = d.values.get("parsed").cloned().unwrap_or(Json::Null);
    let selected = d.values.get("step").and_then(Json::as_str).map(str::to_string);
    let mut clicked: Option<Option<String>> = None;
    // A band of fixed height: a vertical separator would otherwise take every bit of height the
    // window has, and the window would grow past the screen.
    let band = egui::vec2(ui.available_width(), BAND_HEIGHT);
    ui.allocate_ui_with_layout(band, egui::Layout::left_to_right(egui::Align::TOP), |ui| {
        ui.set_height(BAND_HEIGHT);
        // Applied steps.
        ui.vertical(|ui| {
            ui.set_width(170.0);
            ui.label(RichText::new(tr(ui, "Applied Steps")).strong());
            egui::ScrollArea::vertical().id_salt("query_steps").max_height(BAND_HEIGHT - 24.0).show(ui, |ui| {
                let steps = parsed["steps"].as_array().cloned().unwrap_or_default();
                if ui.selectable_label(selected.is_none(), tr(ui, "Result")).clicked() {
                    clicked = Some(None);
                }
                for st in &steps {
                    let name = st["name"].as_str().unwrap_or("").to_string();
                    let resp = ui.selectable_label(selected.as_deref() == Some(name.as_str()), format!("  {name}"));
                    if resp.on_hover_text(st["formula"].as_str().unwrap_or("")).clicked() {
                        clicked = Some(Some(name));
                    }
                }
            });
        });
        ui.separator();
        // The formula.
        ui.vertical(|ui| {
            let mut f = formula.clone();
            let error_room = if parsed["error"].is_string() { 22.0 } else { 0.0 };
            egui::ScrollArea::vertical().id_salt("query_formula").max_height(BAND_HEIGHT - error_room).show(ui, |ui| {
                let edit = egui::TextEdit::multiline(&mut f).code_editor().desired_rows(12).desired_width(f32::INFINITY).lock_focus(true);
                if ui.add(edit).changed() {
                    d.values.insert("formula".into(), json!(f));
                }
            });
            if let Some(e) = parsed["error"].as_str() {
                let at = format!("{} {}, {} {}: {e}", tr(ui, "At line"), parsed["line"], tr(ui, "column"), parsed["column"]);
                ui.label(RichText::new(at).color(Color32::from_rgb(0xC4, 0x2B, 0x1C)));
            }
        });
    });
    if let Some(step) = clicked {
        match step {
            Some(s) => d.values.insert("step".into(), json!(s)),
            None => d.values.remove("step"),
        };
        run_preview(app, d);
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(tr(ui, "Preview")).clicked() {
            run_preview(app, d);
        }
        if let Some(step) = d.values.get("step").and_then(Json::as_str) {
            ui.label(RichText::new(format!("{}: {step}", tr(ui, "Step"))).small().color(Color32::GRAY));
        }
    });
    preview(ui, d);

    ui.add_space(8.0);
    if let Some(e) = d.values.get("saveError").and_then(Json::as_str) {
        ui.label(RichText::new(e).color(Color32::from_rgb(0xC4, 0x2B, 0x1C)));
    }
    ui.horizontal(|ui| {
        if d.values.get("original").is_none_or(Json::is_null) {
            let mut load = d.values.get("load").and_then(Json::as_bool).unwrap_or(true);
            if ui.checkbox(&mut load, tr(ui, "Load to a new sheet")).changed() {
                d.values.insert("load".into(), json!(load));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save_label = RichText::new(tr(ui, "Save")).color(Color32::WHITE);
            let save_button = egui::Button::new(save_label).fill(crate::theme::Tokens::get(ui.ctx()).accent).min_size(egui::vec2(64.0, 0.0));
            if ui.add(save_button).clicked() && save(app, d) {
                *open = false;
            }
            if ui.button(tr(ui, "Cancel")).clicked() {
                *open = false;
            }
        });
    });
}
