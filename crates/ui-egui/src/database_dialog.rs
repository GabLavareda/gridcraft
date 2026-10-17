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
/// Wait until typing stops before issuing another SQL preview.
const SQL_PREVIEW_DELAY: f64 = 0.8;
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
    let mut params = params.clone();
    if let Some(name) = params.get("name").and_then(Json::as_str).map(str::to_string)
        && let Ok(source) = app.session.run("data.database.source", json!({"name":name}))
        && source["supported"] == true
    {
        params = source;
        params["original"] = json!(name);
        params["connect"] = json!(true);
    }
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
    v["sql"] = params.get("query").and_then(Json::as_str).map_or(json!(""), |s| json!(s));
    if let Some(name) = params.get("original") {
        v["original"] = name.clone();
    }
    // Opened connected (with the session's login), on a table: from scripts and the control channel.
    if params.get("connect").and_then(Json::as_bool) == Some(true) {
        v["autoConnect"] = json!(true);
        v["autoTable"] = params.get("table").cloned().unwrap_or(Json::Null);
        v["autoSchema"] = params.get("schema").cloned().unwrap_or(Json::Null);
    }
    if params.get("need").and_then(Json::as_str) == Some("credentials") {
        v["mode"] = json!("credentials");
        v["retry"] = params.get("retry").cloned().unwrap_or(Json::Null);
    }
    refresh_login(app, &mut v);
    v
}

/// Existing database queries use the same browser and SQL editor as new connections.
pub(crate) fn edit_query(app: &mut SheetApp, name: &str) {
    let database = app.session.run("data.database.source", json!({"name":name})).is_ok_and(|s| s["supported"] == true);
    app.open_dialog(if database { "fromDatabase" } else { "queryEditor" }, json!({"name":name}));
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
    for key in ["tables", "databases", "selected", "preview", "previewError", "sqlPreviewAt", "previewManual", "previewStale"] {
        d.values.remove(key);
    }
    match r {
        Ok(r) => {
            d.values.insert("tables".into(), r["tables"].clone());
            d.values.remove("editConnection");
            if let Some(dbs) = r.get("databases") {
                d.values.insert("databases".into(), dbs.clone());
            }
            d.values.remove("error");
            // Restore the saved table after an automatic connection or a manual login retry.
            let wanted = d.values.remove("autoTable");
            let schema = d.values.remove("autoSchema");
            let tables = d.values.get("tables").and_then(Json::as_array);
            let index = tables.and_then(|tables| {
                tables.iter().position(|t| {
                    wanted.as_ref().is_some_and(|w| t["name"] == *w) && schema.as_ref().is_none_or(|s| s.is_null() || t["schema"] == *s)
                })
            });
            if let Some(i) = index {
                d.values.insert("selected".into(), json!(i));
                let is_view = d.values.get("tables").and_then(Json::as_array).and_then(|t| t.get(i)).is_some_and(|t| t["view"] == true);
                d.values.insert("objectTab".into(), json!(if is_view { "views" } else { "tables" }));
                if text(&d.values, "sql").trim().is_empty()
                    && let Some((schema, table)) = selected(d)
                {
                    run_preview(app, d, json!({"schema":schema,"table":table}));
                }
            }
            true
        }
        Err(e) => {
            d.values.insert("error".into(), json!(crate::clean_error(&e)));
            false
        }
    }
}

fn run_preview(app: &mut SheetApp, d: &mut Dialog, what: Json) {
    d.values.remove("sqlPreviewAt");
    d.values.remove("previewManual");
    let mut p = source_params(&d.values, false);
    for (k, v) in what.as_object().into_iter().flatten() {
        p[k] = v.clone();
    }
    p["rows"] = json!(PREVIEW_ROWS);
    match app.session.run("data.database.preview", p) {
        Ok(r) if r["manualRequired"] == true => {
            d.values.insert("previewManual".into(), json!(true));
        }
        Ok(r) => {
            d.values.insert("preview".into(), r);
            d.values.remove("previewError");
            d.values.remove("previewStale");
        }
        Err(e) => {
            d.values.insert("previewStale".into(), json!(true));
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
    let editing = d.values.get("original").and_then(Json::as_str).map(str::to_string);
    if let Some(name) = &editing {
        p["edit"] = json!(name);
    }
    let r = app.session.run("data.fromDatabase", p);
    app.after_engine();
    forget_needs(app);
    match r {
        Ok(r) => {
            let name = r["name"].as_str().unwrap_or("").to_string();
            if load {
                let message = match r.get("rows") {
                    Some(rows) => format!("{name}: {rows} row(s) loaded."),
                    None => format!("{name} saved."),
                };
                app.toast = Some((message, crate::now_ms()));
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
    ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
    ui.spacing_mut().button_padding = egui::vec2(12.0, 5.0);
    let credentials_only = text(&d.values, "mode") == "credentials";
    let kind = kind_of(&d.values);
    if d.values.remove("autoConnect").is_some_and(|v| v == json!(true)) {
        connect(app, d);
    }
    if credentials_only {
        let at = server_of(&d.values);
        let db = text(&d.values, "database");
        let source = if db.is_empty() { at } else { format!("{at};{db}") };
        ui.label(format!("{} ({kind}: {source})", tr(ui, "This query needs a login for its database.")));
        ui.add_space(4.0);
    }

    let show_connection = credentials_only || !d.values.contains_key("tables") || flag(&d.values, "editConnection", false);
    if show_connection {
        egui::Grid::new("database_fields").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
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
                ui.horizontal_wrapped(|ui| {
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
        ui.horizontal_wrapped(|ui| {
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
    } else {
        let tokens = crate::theme::Tokens::get(ui.ctx());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let database = text(&d.values, "database");
                let server = server_of(&d.values);
                let label = if database.is_empty() { server } else { format!("{server} / {database}") };
                ui.label(RichText::new(label).strong().size(16.0));
                ui.label(RichText::new(kind_label(kind)).small().color(tokens.text_dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr(ui, "Edit Connection")).clicked() {
                    d.values.insert("editConnection".into(), json!(true));
                }
            });
        });
    }
    if let Some(e) = d.values.get("error").and_then(Json::as_str) {
        ui.label(RichText::new(e).color(ERROR_RED));
    }

    if !credentials_only && d.values.contains_key("tables") && !flag(&d.values, "editConnection", false) {
        ui.separator();
        navigator(app, ui, d);
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr(ui, "Cancel")).clicked() {
                    *open = false;
                }
                if ui.button(tr(ui, "Query Editor")).clicked() && finish(app, d, false) {
                    *open = false;
                }
                let label = if d.values.contains_key("original") { "Save" } else { "Load" };
                let load_label = RichText::new(tr(ui, label)).color(Color32::WHITE);
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
    // The object list uses the entire left side. SQL is hidden until explicitly requested;
    // its bounded editor shares the right preview panel without shortening the object list.
    let height = (ui.available_height() - 66.0).max(180.0);
    let band = egui::vec2(ui.available_width(), height);
    let tables_width = (band.x * 0.24).clamp(200.0, 290.0);
    ui.allocate_ui_with_layout(band, egui::Layout::left_to_right(egui::Align::TOP), |ui| {
        ui.set_height(height);
        ui.vertical(|ui| {
            ui.set_width(tables_width);
            ui.set_height(height);
            let view_count = tables.iter().filter(|t| t["view"] == true).count();
            let mut views = text(&d.values, "objectTab") == "views";
            let mut reset_scroll = false;
            ui.horizontal(|ui| {
                reset_scroll |= ui.selectable_value(&mut views, false, format!("{} ({})", tr(ui, "Tables"), tables.len() - view_count)).changed();
                reset_scroll |= ui.selectable_value(&mut views, true, format!("{} ({view_count})", tr(ui, "Views"))).changed();
            });
            d.values.insert("objectTab".into(), json!(if views { "views" } else { "tables" }));
            let mut search = text(&d.values, "objectFilter");
            let changed = ui
                .horizontal(|ui| {
                    let label = ui.label(tr(ui, "Search"));
                    ui.add(
                        egui::TextEdit::singleline(&mut search)
                            .id_salt("database_object_filter")
                            .hint_text(tr(ui, "Name or schema"))
                            .desired_width(ui.available_width()),
                    )
                    .labelled_by(label.id)
                    .changed()
                })
                .inner;
            if changed {
                d.values.insert("objectFilter".into(), json!(&search));
                reset_scroll = true;
            }
            let needle = search.trim().to_lowercase();
            let mut list = egui::ScrollArea::both().id_salt(("database_tables", views)).max_height(height - 82.0).auto_shrink([false, false]);
            if reset_scroll {
                list = list.scroll_offset(egui::Vec2::ZERO);
            }
            list.show(ui, |ui| {
                let mut visible = 0;
                for (i, t) in tables.iter().enumerate() {
                    if (t["view"] == true) != views {
                        continue;
                    }
                    let schema = t["schema"].as_str().unwrap_or("");
                    let name = t["name"].as_str().unwrap_or("");
                    let label = if schema.is_empty() || schema == "main" { name.to_string() } else { format!("{schema}.{name}") };
                    if !format!("{schema}.{name}").to_lowercase().contains(&needle) {
                        continue;
                    }
                    visible += 1;
                    if ui
                        .add(egui::Button::selectable(picked == Some(i), &label).wrap_mode(egui::TextWrapMode::Extend))
                        .on_hover_text(&label)
                        .clicked()
                    {
                        click = Some(i);
                    }
                }
                if visible == 0 {
                    ui.label(RichText::new(tr(ui, "No matching tables or views.")).italics().color(crate::theme::Tokens::get(ui.ctx()).text_dim));
                }
            });
            if let Some(dbs) = d.values.get("databases").and_then(Json::as_array).filter(|l| !l.is_empty()) {
                let names: Vec<&str> = dbs.iter().filter_map(Json::as_str).collect();
                let summary = format!("{}: {}", tr(ui, "Databases"), names.join(", "));
                ui.add(egui::Label::new(RichText::new(&summary).small().color(Color32::GRAY)).truncate()).on_hover_text(summary);
            }
        });
        ui.separator();
        if let Some(i) = click {
            d.values.insert("selected".into(), json!(i));
            if let Some((schema, table)) = selected(d) {
                d.values.insert("sql".into(), json!(""));
                run_preview(app, d, json!({"schema": schema, "table": table}));
            }
        }
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            preview_panel(app, ui, d, height);
        });
    });
}

fn preview_panel(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog, height: f32) {
    let tokens = crate::theme::Tokens::get(ui.ctx());
    egui::Frame::NONE.fill(tokens.input_bg).stroke(egui::Stroke::new(1.0, tokens.input_border)).corner_radius(6.0).inner_margin(12.0).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((height - 24.0).max(66.0));
            let mut sql = text(&d.values, "sql");
            let mut show_sql = flag(&d.values, "showSql", false);
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr(ui, "Data Preview")).strong());
                let source = if !sql.trim().is_empty() {
                    "SQL".to_string()
                } else {
                    selected(d).map_or_else(
                        String::new,
                        |(schema, name)| if schema.is_empty() || schema == "main" { name } else { format!("{schema}.{name}") },
                    )
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if show_sql { "Hide SQL" } else { "Edit SQL" };
                    if ui.button(tr(ui, label)).clicked() {
                        show_sql = !show_sql;
                        d.values.insert("showSql".into(), json!(show_sql));
                    }
                    if !sql.trim().is_empty() && ui.button(tr(ui, "Preview")).clicked() {
                        run_preview(app, d, json!({"query": sql.trim()}));
                    }
                    ui.add(egui::Label::new(RichText::new(&source).color(tokens.text_dim)).truncate()).on_hover_text(source);
                });
            });
            ui.separator();
            if show_sql {
                ui.label(RichText::new(tr(ui, "SQL statement (optional)")).strong());
                let editor_height = (height * 0.3).clamp(65.0, 180.0);
                egui::ScrollArea::both().id_salt("database_sql").max_height(editor_height).auto_shrink([false, false]).show(ui, |ui| {
                    let edit = egui::TextEdit::multiline(&mut sql).code_editor().desired_rows(4).desired_width(f32::INFINITY).hint_text("SELECT …");
                    if ui.add_sized(egui::vec2(ui.available_width(), editor_height), edit).changed() {
                        d.values.insert("sql".into(), json!(&sql));
                        d.values.remove("previewError");
                        d.values.remove("previewManual");
                        d.values.insert("previewStale".into(), json!(true));
                        if sql.trim().is_empty() {
                            d.values.remove("sqlPreviewAt");
                        } else {
                            d.values.insert("sqlPreviewAt".into(), json!(ui.input(|i| i.time) + SQL_PREVIEW_DELAY));
                        }
                    }
                });
                ui.separator();
            }
            if let Some(deadline) = d.values.get("sqlPreviewAt").and_then(Json::as_f64) {
                let remaining = deadline - ui.input(|i| i.time);
                if remaining <= 0.0 {
                    run_preview(app, d, json!({"query":sql.trim(),"automatic":true}));
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(remaining.min(SQL_PREVIEW_DELAY)));
                    ui.label(RichText::new(tr(ui, "Waiting for typing to stop…")).small().color(tokens.text_dim));
                }
            }
            if flag(&d.values, "previewManual", false) {
                ui.label(RichText::new(tr(ui, "This SQL requires the Preview button.")).small().color(tokens.text_dim));
            } else if !d.values.contains_key("sqlPreviewAt") && flag(&d.values, "previewStale", false) && d.values.contains_key("preview") {
                ui.label(RichText::new(tr(ui, "Showing the previous preview until the query updates.")).small().color(tokens.text_dim));
            }
            preview(ui, d, (ui.available_height() - 24.0).max(50.0));
        },
    );
}

fn preview(ui: &mut Ui, d: &Dialog, height: f32) {
    let mut height = height;
    if let Some(e) = d.values.get("previewError").and_then(Json::as_str) {
        let error = ui.label(RichText::new(e).color(ERROR_RED));
        height = (height - error.rect.height() - ui.spacing().item_spacing.y).max(20.0);
    }
    let Some(p) = d.values.get("preview") else {
        let hint =
            if text(&d.values, "sql").trim().is_empty() { "Choose a table to see its first rows." } else { "Choose Preview to run the query." };
        ui.label(RichText::new(tr(ui, hint)).italics().color(Color32::GRAY));
        return;
    };
    let rows = p["rows"].as_array().cloned().unwrap_or_default();
    let columns = p["columns"].as_array().cloned().unwrap_or_default();
    egui::ScrollArea::both().id_salt("database_preview").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        // Keep headers and values on one line; wide tables scroll instead of crushing columns.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        egui::Grid::new("database_preview_grid").striped(true).min_col_width(80.0).min_row_height(24.0).spacing([20.0, 8.0]).show(ui, |ui| {
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
        Some("web") => app.open_dialog("webAccess", params),
        _ => return false,
    }
    true
}
