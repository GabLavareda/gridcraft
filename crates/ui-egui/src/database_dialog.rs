//! Data › Get Data › From Database: the kind of database, its host and port, the database and
//! the login; then its tables, to pick one and see its first rows, or SQL to run instead. The
//! engine does the work: `data.database.connect` checks the login and lists the tables,
//! `data.database.preview` shows rows and `data.fromDatabase` makes the query.
//!
//! The same dialog asks for a login when a query needs one it doesn't have (refreshing a
//! workbook from another session), and a second dialog asks before SQL written in a query runs.
//! Logins live in the engine for the session only; the dialog's copy goes with it.

use egui::{Color32, RichText, Ui};
use gridcraft_engine::datasource::Kind;
use serde_json::{Map, Value as Json, json};

use crate::SheetApp;
use crate::dialogs::Dialog;
use crate::i18n;

/// Rows a table preview shows.
const PREVIEW_ROWS: u64 = 50;
/// Height of the band with the tables, the SQL and the preview.
const BAND_HEIGHT: f32 = 300.0;
const ERROR_RED: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);

fn tr<'a>(ui: &Ui, text: &'a str) -> &'a str {
    i18n::current(ui.ctx()).tr(text)
}

fn text(values: &Map<String, Json>, key: &str) -> String {
    values.get(key).and_then(Json::as_str).unwrap_or("").to_string()
}

fn flag(values: &Map<String, Json>, key: &str, default: bool) -> bool {
    values.get(key).and_then(Json::as_bool).unwrap_or(default)
}

fn kind_of(values: &Map<String, Json>) -> Kind {
    Kind::parse(&text(values, "kind")).unwrap_or(Kind::SqlServer)
}

/// The dialog's starting values. `{}` starts empty (SQL Server); `{need: "credentials", kind,
/// server, database, retry?}` asks for the login of that server, then runs `retry` (`{id, params}`).
pub fn defaults(app: &mut SheetApp, params: &Json) -> Json {
    let kind = params.get("kind").and_then(Json::as_str).and_then(Kind::parse).unwrap_or(Kind::SqlServer);
    let mut v = json!({
        "kind": kind.id(), "host": "", "port": "", "database": "", "username": "", "password": "",
        "encrypt": true, "trust": false, "path": "", "sql": "",
    });
    if let Some(server) = params.get("server").and_then(Json::as_str) {
        if kind == Kind::Sqlite {
            v["path"] = json!(server);
        } else if let Ok((host, port)) = kind.split_server(server) {
            v["host"] = json!(host);
            v["port"] = json!(port.map(|p| p.to_string()).unwrap_or_default());
        }
    }
    for key in ["database", "host", "port", "username", "path"] {
        if let Some(x) = params.get(key).and_then(Json::as_str) {
            v[key] = json!(x);
        }
    }
    // Opened connected (with the session's login), on a table: from scripts and the control channel.
    if params.get("connect").and_then(Json::as_bool) == Some(true) {
        v["autoConnect"] = json!(true);
        v["autoTable"] = params.get("table").cloned().unwrap_or(Json::Null);
    }
    if params.get("need").and_then(Json::as_str) == Some("credentials") {
        v["mode"] = json!("credentials");
        v["retry"] = params.get("retry").cloned().unwrap_or(Json::Null);
    }
    refresh_login(app, &mut v);
    v
}

/// The login kept for the dialog's server, if any, minus its password; and whether logins can be
/// remembered at all.
fn refresh_login(app: &mut SheetApp, v: &mut Json) {
    let kind = v.as_object().map_or(Kind::SqlServer, kind_of);
    let server = server_of(v.as_object().map_or(&Map::new(), |m| m));
    v["stored"] = json!(false);
    if let Ok(list) = app.session.run("data.credentials.list", json!({})) {
        v["canRemember"] = list["canRemember"].clone();
        for c in list["logins"].as_array().into_iter().flatten() {
            if c["kind"] == json!(kind.id()) && c["server"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(&server)) {
                v["username"] = c["username"].clone();
                v["encrypt"] = c["encrypt"].clone();
                v["trust"] = c["trustServerCertificate"].clone();
                v["remember"] = c["remembered"].clone();
                v["stored"] = json!(true);
            }
        }
    }
}

/// The server as the engine and the query write it.
fn server_of(v: &Map<String, Json>) -> String {
    let kind = kind_of(v);
    if kind == Kind::Sqlite {
        return text(v, "path").trim().to_string();
    }
    let port = text(v, "port").trim().parse::<u16>().ok();
    kind.join_server(text(v, "host").trim(), port)
}

/// The params naming the database, plus the login when one was typed.
fn source_params(v: &Map<String, Json>, with_login: bool) -> Json {
    let kind = kind_of(v);
    if kind == Kind::Sqlite {
        return json!({"kind": kind.id(), "path": text(v, "path").trim()});
    }
    let mut p = json!({"kind": kind.id(), "host": text(v, "host").trim(), "port": text(v, "port").trim(), "database": text(v, "database").trim()});
    let (user, password) = (text(v, "username"), text(v, "password"));
    // An empty password with a login kept for the server means: use the kept one.
    let typed = !password.is_empty() || (!user.trim().is_empty() && !flag(v, "stored", false));
    if with_login && typed {
        p["username"] = json!(user.trim());
        p["password"] = json!(password);
        p["encrypt"] = json!(flag(v, "encrypt", true));
        p["trustServerCertificate"] = json!(flag(v, "trust", false));
        p["remember"] = json!(flag(v, "remember", false));
    }
    p
}

/// What queries were refused for, cleared: the dialog shows its own errors.
fn forget_needs(app: &mut SheetApp) {
    let _ = app.session.run("data.database.needs", json!({}));
}

fn connect(app: &mut SheetApp, d: &mut Dialog) -> bool {
    let r = app.session.run("data.database.connect", source_params(&d.values, true));
    forget_needs(app);
    for key in ["tables", "databases", "selected", "preview", "previewError"] {
        d.values.remove(key);
    }
    match r {
        Ok(r) => {
            d.values.insert("tables".into(), r["tables"].clone());
            if let Some(dbs) = r.get("databases") {
                d.values.insert("databases".into(), dbs.clone());
            }
            d.values.remove("error");
            true
        }
        Err(e) => {
            d.values.insert("error".into(), json!(crate::clean_error(&e)));
            false
        }
    }
}

fn run_preview(app: &mut SheetApp, d: &mut Dialog, what: Json) {
    let mut p = source_params(&d.values, false);
    for (k, v) in what.as_object().into_iter().flatten() {
        p[k] = v.clone();
    }
    p["rows"] = json!(PREVIEW_ROWS);
    match app.session.run("data.database.preview", p) {
        Ok(r) => {
            d.values.insert("preview".into(), r);
            d.values.remove("previewError");
        }
        Err(e) => {
            d.values.remove("preview");
            d.values.insert("previewError".into(), json!(crate::clean_error(&e)));
        }
    }
    forget_needs(app);
}

/// The table picked in the list: schema and name.
fn selected(d: &Dialog) -> Option<(String, String)> {
    let i = d.values.get("selected")?.as_u64()? as usize;
    let t = d.values.get("tables")?.as_array()?.get(i)?;
    Some((t["schema"].as_str()?.to_string(), t["name"].as_str()?.to_string()))
}

/// Makes the query: loaded to a new sheet, or opened in the Query Editor.
fn finish(app: &mut SheetApp, d: &mut Dialog, load: bool) -> bool {
    let mut p = source_params(&d.values, true);
    let sql = text(&d.values, "sql");
    if !sql.trim().is_empty() {
        p["query"] = json!(sql.trim());
    } else if let Some((schema, table)) = selected(d) {
        p["schema"] = json!(schema);
        p["table"] = json!(table);
    } else {
        d.values.insert("error".into(), json!("Choose a table, or write a SQL statement."));
        return false;
    }
    p["load"] = json!(load);
    let r = app.session.run("data.fromDatabase", p);
    app.after_engine();
    forget_needs(app);
    match r {
        Ok(r) => {
            let name = r["name"].as_str().unwrap_or("").to_string();
            if load {
                app.toast = Some((format!("{name}: {} row(s) loaded.", r["rows"]), crate::now_ms()));
            } else {
                app.open_dialog("queryEditor", json!({"name": name}));
            }
            true
        }
        Err(e) => {
            d.values.insert("error".into(), json!(crate::clean_error(&e)));
            false
        }
    }
}

fn edit(ui: &mut Ui, d: &mut Dialog, key: &str, hint: &str, width: f32, password: bool) {
    let mut v = text(&d.values, key);
    let edit = egui::TextEdit::singleline(&mut v).hint_text(hint).desired_width(width).password(password);
    if ui.add(edit).changed() {
        d.values.insert(key.into(), json!(v));
    }
}

/// Draws the dialog; `open` turns false when it should close.
pub fn show(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog, open: &mut bool) {
    let credentials_only = text(&d.values, "mode") == "credentials";
    let kind = kind_of(&d.values);
    if d.values.remove("autoConnect").is_some_and(|v| v == json!(true)) && connect(app, d) {
        let wanted = d.values.remove("autoTable");
        let tables = d.values.get("tables").and_then(Json::as_array).cloned().unwrap_or_default();
        if let Some(i) = tables.iter().position(|t| wanted.as_ref().is_some_and(|w| t["name"] == *w)) {
            d.values.insert("selected".into(), json!(i));
            if let Some((schema, table)) = selected(d) {
                run_preview(app, d, json!({"schema": schema, "table": table}));
            }
        }
    }
    if credentials_only {
        let at = server_of(&d.values);
        let db = text(&d.values, "database");
        let source = if db.is_empty() { at } else { format!("{at};{db}") };
        ui.label(format!("{} ({kind}: {source})", tr(ui, "This query needs a login for its database.")));
        ui.add_space(4.0);
    }

    egui::Grid::new("database_fields").num_columns(2).spacing([8.0, 5.0]).show(ui, |ui| {
        ui.label(tr(ui, "Database type"));
        let mut chosen = kind;
        ui.add_enabled_ui(!credentials_only, |ui| {
            egui::ComboBox::from_id_salt("database_kind").selected_text(kind_label(kind)).width(170.0).show_ui(ui, |ui| {
                for k in Kind::ALL {
                    ui.selectable_value(&mut chosen, k, kind_label(k));
                }
            });
        });
        if chosen != kind {
            d.values.insert("kind".into(), json!(chosen.id()));
            for key in ["tables", "databases", "selected", "preview", "previewError", "error"] {
                d.values.remove(key);
            }
        }
        ui.end_row();
        if chosen == Kind::Sqlite {
            ui.label(tr(ui, "File"));
            ui.horizontal(|ui| {
                edit(ui, d, "path", "C:\\data\\vendas.db", 420.0, false);
                if app.services.pick_database.is_some()
                    && ui.button(tr(ui, "Browse…")).clicked()
                    && let Some(path) = app.services.pick_database.as_ref().and_then(|pick| pick())
                {
                    d.values.insert("path".into(), json!(path));
                }
            });
            ui.end_row();
        } else {
            let default_port = chosen.default_port().map(|p| p.to_string()).unwrap_or_default();
            ui.label(tr(ui, "Host"));
            ui.horizontal(|ui| {
                edit(ui, d, "host", "localhost", 260.0, false);
                ui.label(tr(ui, "Port"));
                edit(ui, d, "port", &default_port, 70.0, false);
            });
            ui.end_row();
            ui.label(tr(ui, "Database"));
            edit(ui, d, "database", "", 260.0, false);
            ui.end_row();
            ui.label(tr(ui, "Login"));
            ui.horizontal(|ui| {
                edit(ui, d, "username", "", 260.0, false);
                ui.label(tr(ui, "Password"));
                edit(ui, d, "password", "", 170.0, true);
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                let mut encrypt = flag(&d.values, "encrypt", true);
                if ui.checkbox(&mut encrypt, tr(ui, "Encrypt connection")).changed() {
                    d.values.insert("encrypt".into(), json!(encrypt));
                }
                let mut trust = flag(&d.values, "trust", false);
                ui.add_enabled_ui(encrypt, |ui| {
                    let hint = tr(ui, "Accept a certificate the server signed itself");
                    if ui.checkbox(&mut trust, tr(ui, "Trust server certificate")).on_hover_text(hint).changed() {
                        d.values.insert("trust".into(), json!(trust));
                    }
                });
            });
            ui.end_row();
            if flag(&d.values, "canRemember", false) {
                ui.label("");
                let mut remember = flag(&d.values, "remember", false);
                let hint = tr(ui, "Keeps the login in this computer's credential store, for the next times GridCraft opens");
                if ui.checkbox(&mut remember, tr(ui, "Remember the login on this computer")).on_hover_text(hint).changed() {
                    d.values.insert("remember".into(), json!(remember));
                }
                ui.end_row();
            }
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let connect_label = RichText::new(tr(ui, "Connect")).color(Color32::WHITE);
        let button = egui::Button::new(connect_label).fill(crate::theme::Tokens::get(ui.ctx()).accent).min_size(egui::vec2(80.0, 0.0));
        if ui.add(button).clicked() && connect(app, d) && credentials_only {
            *open = false;
            let retry = d.values.get("retry").cloned().unwrap_or(Json::Null);
            if let (Some(id), Some(params)) = (retry["id"].as_str(), retry.get("params")) {
                let _ = app.run(id, params.clone());
            } else {
                app.toast = Some(("Login kept for this session.".to_string(), crate::now_ms()));
            }
        }
        if !credentials_only && kind != Kind::Sqlite {
            let note = if flag(&d.values, "canRemember", false) {
                "Without Remember, the login lasts this session only. It never goes into the workbook."
            } else {
                "The login is kept for this session only, never in the workbook."
            };
            ui.label(RichText::new(tr(ui, note)).small().color(Color32::GRAY));
        }
    });
    if let Some(e) = d.values.get("error").and_then(Json::as_str) {
        ui.label(RichText::new(e).color(ERROR_RED));
    }

    if !credentials_only && d.values.contains_key("tables") {
        ui.separator();
        navigator(app, ui, d);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr(ui, "Cancel")).clicked() {
                    *open = false;
                }
                if ui.button(tr(ui, "Transform Data")).clicked() && finish(app, d, false) {
                    *open = false;
                }
                let load_label = RichText::new(tr(ui, "Load")).color(Color32::WHITE);
                let load = egui::Button::new(load_label).fill(crate::theme::Tokens::get(ui.ctx()).accent).min_size(egui::vec2(64.0, 0.0));
                if ui.add(load).clicked() && finish(app, d, true) {
                    *open = false;
                }
            });
        });
    } else if ui.button(tr(ui, "Cancel")).clicked() {
        *open = false;
    }
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::SqlServer => "SQL Server",
        Kind::PostgreSql => "PostgreSQL",
        Kind::MySql => "MySQL / MariaDB",
        Kind::Sqlite => "SQLite",
    }
}

/// The tables to pick from, the SQL to run instead, and a preview of either.
fn navigator(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog) {
    let tables = d.values.get("tables").and_then(Json::as_array).cloned().unwrap_or_default();
    let picked = d.values.get("selected").and_then(Json::as_u64).map(|i| i as usize);
    let mut click = None;
    // A band of fixed height: a vertical separator would otherwise take every bit of height the
    // window has, and the window would grow past the screen.
    let band = egui::vec2(ui.available_width(), BAND_HEIGHT);
    ui.allocate_ui_with_layout(band, egui::Layout::left_to_right(egui::Align::TOP), |ui| {
        ui.set_height(BAND_HEIGHT);
        ui.vertical(|ui| {
            ui.set_width(220.0);
            ui.label(RichText::new(format!("{} ({})", tr(ui, "Tables"), tables.len())).strong());
            egui::ScrollArea::vertical().id_salt("database_tables").max_height(BAND_HEIGHT - 50.0).auto_shrink([false, false]).show(ui, |ui| {
                if tables.is_empty() {
                    ui.label(RichText::new(tr(ui, "The database has no tables this login can read.")).italics().color(Color32::GRAY));
                }
                for (i, t) in tables.iter().enumerate() {
                    let schema = t["schema"].as_str().unwrap_or("");
                    let name = t["name"].as_str().unwrap_or("");
                    let label = if schema.is_empty() || schema == "main" { name.to_string() } else { format!("{schema}.{name}") };
                    let label = if t["view"] == json!(true) { format!("{label}  ({})", tr(ui, "view")) } else { label };
                    if ui.selectable_label(picked == Some(i), label).clicked() {
                        click = Some(i);
                    }
                }
            });
            if let Some(dbs) = d.values.get("databases").and_then(Json::as_array).filter(|l| !l.is_empty()) {
                let names: Vec<&str> = dbs.iter().filter_map(Json::as_str).collect();
                ui.label(RichText::new(format!("{}: {}", tr(ui, "Databases"), names.join(", "))).small().color(Color32::GRAY));
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.label(RichText::new(tr(ui, "SQL statement (optional)")).strong());
            let mut sql = text(&d.values, "sql");
            let edit = egui::TextEdit::multiline(&mut sql).code_editor().desired_rows(3).desired_width(f32::INFINITY).hint_text("SELECT …");
            if ui.add(edit).changed() {
                d.values.insert("sql".into(), json!(sql));
            }
            if !sql.trim().is_empty() && ui.button(tr(ui, "Preview")).clicked() {
                run_preview(app, d, json!({"query": sql.trim()}));
            }
            ui.add_space(4.0);
            preview(ui, d);
        });
    });
    if let Some(i) = click {
        d.values.insert("selected".into(), json!(i));
        if let Some((schema, table)) = selected(d) {
            d.values.insert("sql".into(), json!(""));
            run_preview(app, d, json!({"schema": schema, "table": table}));
        }
    }
}

fn preview(ui: &mut Ui, d: &Dialog) {
    if let Some(e) = d.values.get("previewError").and_then(Json::as_str) {
        ui.label(RichText::new(e).color(ERROR_RED));
        return;
    }
    let Some(p) = d.values.get("preview") else {
        ui.label(RichText::new(tr(ui, "Choose a table to see its first rows.")).italics().color(Color32::GRAY));
        return;
    };
    let rows = p["rows"].as_array().cloned().unwrap_or_default();
    let columns = p["columns"].as_array().cloned().unwrap_or_default();
    egui::ScrollArea::both().id_salt("database_preview").max_height(BAND_HEIGHT - 120.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("database_preview_grid").striped(true).min_col_width(50.0).show(ui, |ui| {
            for c in &columns {
                ui.vertical(|ui| {
                    ui.label(RichText::new(c["name"].as_str().unwrap_or("")).strong());
                    ui.label(RichText::new(c["type"].as_str().unwrap_or("")).small().color(Color32::GRAY));
                });
            }
            ui.end_row();
            for row in &rows {
                for v in row.as_array().into_iter().flatten() {
                    match crate::query_editor::cell_text(v) {
                        (t, Some(why)) => ui.label(RichText::new(t).color(ERROR_RED)).on_hover_text(why),
                        (t, None) if v.is_null() => ui.label(RichText::new(t).italics().color(Color32::GRAY)),
                        (t, None) => ui.label(t),
                    };
                }
                ui.end_row();
            }
        });
    });
    if p["moreRows"].as_u64().is_some_and(|n| n > 0) {
        ui.label(RichText::new(format!("{} {} {}", tr(ui, "First"), rows.len(), tr(ui, "rows shown."))).small().color(Color32::GRAY));
    }
}

// ---------------------------------------------------------------- native query approval

/// Asks before SQL written in a query runs: `{kind, server, database, query, retry?}`.
pub fn approval(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog, open: &mut bool) {
    let kind = kind_of(&d.values);
    let (server, database, sql) = (text(&d.values, "server"), text(&d.values, "database"), text(&d.values, "query"));
    ui.label(tr(ui, "A query is about to run this SQL. SQL can change the database: run it only if you trust where the query came from."));
    ui.add_space(4.0);
    let at = if database.is_empty() { server.clone() } else { format!("{server};{database}") };
    ui.label(RichText::new(format!("{kind}: {at}")).strong());
    let mut shown = sql.clone();
    egui::ScrollArea::vertical().id_salt("native_query").max_height(220.0).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(&mut shown).code_editor().desired_rows(6).desired_width(f32::INFINITY).interactive(false));
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr(ui, "Cancel")).clicked() {
                *open = false;
            }
            let run_label = RichText::new(tr(ui, "Run")).color(Color32::WHITE);
            let run = egui::Button::new(run_label).fill(crate::theme::Tokens::get(ui.ctx()).accent).min_size(egui::vec2(64.0, 0.0));
            if ui.add(run).clicked() {
                *open = false;
                let approved = app
                    .session
                    .run("data.database.approve", json!({"kind": kind.id(), "server": server, "path": server, "database": database, "query": sql}));
                let retry = d.values.get("retry").cloned().unwrap_or(Json::Null);
                match (approved, retry["id"].as_str(), retry.get("params")) {
                    (Err(e), _, _) => app.message = Some(("Native Database Query".into(), crate::clean_error(&e))),
                    (Ok(_), Some(id), Some(params)) => {
                        let _ = app.run(id, params.clone());
                    }
                    (Ok(_), _, _) => app.toast = Some(("Approved for this session: run the query again.".to_string(), crate::now_ms())),
                }
            }
        });
    });
}

/// After a command that ran queries: asks for what they were refused for (a login, an
/// approval), then runs `retry` (`{id, params}`) when it is given. Returns whether it asked.
pub fn ask_for_needs(app: &mut SheetApp, retry: Option<Json>) -> bool {
    let Ok(Json::Array(needs)) = app.session.run("data.database.needs", json!({})) else { return false };
    let Some(first) = needs.into_iter().next() else { return false };
    let mut params = first.clone();
    params["retry"] = retry.unwrap_or(Json::Null);
    match first["need"].as_str() {
        Some("credentials") => app.open_dialog("fromDatabase", params),
        Some("approval") => app.open_dialog("nativeQuery", params),
        _ => return false,
    }
    true
}
