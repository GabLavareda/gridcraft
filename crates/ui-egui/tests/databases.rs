//! Data › From Database: connecting to a SQLite file, picking a table and loading it; and asking
//! for a login when a query names a server the session has none for.
//!
//! Ribbon buttons are painted without accessible labels, so these tests do what the button does
//! (`open_dialog("fromDatabase", …)`) and click inside the dialog, made of ordinary widgets.

use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::{Session, core::CellRef, core::Value};
use gridcraft_ui_egui::{Services, SheetApp};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness() -> Harness {
    harness_at(egui::vec2(1400.0, 900.0))
}

fn harness_at(size: egui::Vec2) -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    let mut h = egui_kittest::Harness::builder().with_size(size).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Services::default()),
    );
    h.state_mut().ui.ribbon_tab = "Data".into();
    h.run_steps(4);
    h
}

/// A SQLite file in the temporary folder that goes away with the value.
struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new() -> TempDb {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path =
            std::env::temp_dir().join(format!("gridcraft-ui-{}-{}.db", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_file(&path);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE pedidos (id INTEGER, cliente TEXT, valor REAL);
             INSERT INTO pedidos VALUES (1, 'Ana', 120.5), (2, 'Bruno', 75), (3, 'Célia', 310.99);
             CREATE TABLE clientes (nome TEXT);
             CREATE VIEW pedidos_ativos AS SELECT * FROM pedidos WHERE valor > 100;",
        )
        .unwrap();
        TempDb(path)
    }
}

#[test]
fn editing_an_existing_database_query_restores_sql_and_saves_without_duplicates() {
    let f = TempDb::new();
    let mut h = harness();
    let path = f.0.to_string_lossy().into_owned();
    let source =
        h.state_mut().session.run("data.fromDatabase", json!({"kind":"sqlite","path":path,"query":"SELECT * FROM pedidos","name":"Vendas"})).unwrap()
            ["formula"]
            .clone();
    let formula =
        h.state_mut().session.run("data.query.addStep", json!({"formula":source,"op":"keepTopRows","params":{"count":1}})).unwrap()["formula"]
            .clone();
    h.state_mut().session.run("data.query.set", json!({"name":"Vendas","formula":formula,"description":"Descrição preservada"})).unwrap();
    let original = h.state().session.active().unwrap().wb.queries[0].clone();
    h.state_mut().grid.pane = Some("queries".into());
    h.run_steps(4);
    h.get_by_label("Edit").click();
    h.run_steps(4);
    let dialog = h.state().dialog.as_ref().unwrap();
    assert_eq!(dialog.name(), "fromDatabase");
    assert_eq!(dialog.values["original"], json!("Vendas"));
    assert_eq!(dialog.values["sql"], json!("SELECT * FROM pedidos"));
    assert!(h.query_by_label("SQL statement (optional)").is_none(), "saved SQL stays hidden until requested");
    let sql = "SELECT * FROM pedidos WHERE id > 1 ORDER BY id";
    // Type into the actual SQL widget, with no M editing.
    replace_sql(&mut h, sql);
    h.run_steps(4);
    h.get_by_label("Save").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    let wb = &h.state().session.active().unwrap().wb;
    assert_eq!(wb.queries.len(), 1);
    let query = &wb.queries[0];
    assert_eq!(query.name, original.name);
    assert_eq!(query.description, original.description);
    assert_eq!(query.load_to, original.load_to);
    assert!(query.formula.contains("Table.FirstN"), "{}", query.formula);
    assert!(query.formula.contains(sql));
    let sheet = wb.sheet(wb.sheet_index("Vendas").unwrap()).unwrap();
    assert_eq!(sheet.value(CellRef::parse("B2").unwrap()), Value::text("Bruno"));
    assert_eq!(sheet.value(CellRef::parse("B3").unwrap()), Value::Empty);
    h.state_mut().session.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.active().unwrap().wb.queries[0].formula, original.formula);
    // Cancel keeps the saved query and its cells unchanged.
    h.state_mut().open_dialog("fromDatabase", json!({"name":"Vendas"}));
    h.run_steps(4);
    replace_sql(&mut h, "SELECT 999");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert_eq!(h.state().session.active().unwrap().wb.queries[0].formula, original.formula);
}

fn replace_sql(h: &mut Harness, sql: &str) {
    if h.query_by_label("SQL statement (optional)").is_none() {
        h.get_by_label("Edit SQL").click();
        h.run_steps(4);
    }
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).type_text(sql);
    h.run_steps(4);
}

#[test]
fn a_loaded_table_can_be_changed_to_sql_and_failed_saves_keep_its_data() {
    let f = TempDb::new();
    let mut h = harness();
    let path = f.0.to_string_lossy().into_owned();
    h.state_mut().session.run("data.fromDatabase", json!({"kind":"sqlite","path":path,"table":"pedidos","name":"Pedidos"})).unwrap();
    h.state_mut().open_dialog("fromDatabase", json!({"name":"Pedidos"}));
    h.run_steps(4);
    assert!(h.query_by_label("SQL statement (optional)").is_none());
    replace_sql(&mut h, "SELECT * FROM pedidos WHERE id = 3");
    h.get_by_label("Save").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    let wb = h.state().session.active().unwrap().wb.clone();
    assert_eq!(wb.queries.len(), 1);
    assert_eq!(wb.sheet(wb.sheet_index("Pedidos").unwrap()).unwrap().value(CellRef::parse("B2").unwrap()), Value::text("Célia"));
    let original = wb.queries[0].formula.clone();
    h.state_mut().open_dialog("fromDatabase", json!({"name":"Pedidos"}));
    h.run_steps(4);
    assert_eq!(h.state().dialog.as_ref().unwrap().values["sql"], json!("SELECT * FROM pedidos WHERE id = 3"));
    replace_sql(&mut h, "SELECT invalid_column FROM pedidos");
    h.get_by_label("Save").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_some());
    assert_eq!(h.state().session.active().unwrap().wb.queries[0].formula, original);
    assert_eq!(
        h.state().session.active().unwrap().wb.sheet(wb.sheet_index("Pedidos").unwrap()).unwrap().value(CellRef::parse("B2").unwrap()),
        Value::text("Célia")
    );
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn from_database_loads_a_table() {
    let f = TempDb::new();
    let mut h = harness();
    let path = f.0.to_string_lossy().into_owned();
    h.state_mut().open_dialog("fromDatabase", json!({"kind": "sqlite", "path": path}));
    h.run_steps(4);
    h.get_by_label("Connect").click();
    h.run_steps(4);
    // The tables are listed; picking one shows its first rows.
    h.get_by_label("clientes");
    h.get_by_label("pedidos").click();
    h.run_steps(4);
    h.get_by_label("Célia");
    h.get_by_label("Load").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "loading closes the dialog");
    let wb = h.state().session.active().unwrap().wb.clone();
    assert_eq!(wb.queries.len(), 1);
    assert!(wb.queries[0].formula.contains("SQLite.Database("), "{}", wb.queries[0].formula);
    let sheet = wb.sheet(wb.sheet_index("pedidos").unwrap()).unwrap();
    assert_eq!(sheet.value(CellRef::parse("B4").unwrap()), Value::text("Célia"));
}

#[test]
fn a_query_without_a_login_asks_for_one() {
    let mut h = harness();
    let formula = "PostgreSQL.Database(\"127.0.0.1:1\", \"shop\")";
    let r = h.state_mut().run("data.query.add", json!({"name": "Shop", "formula": formula}));
    assert!(r.is_err());
    h.run_steps(4);
    let d = h.state().dialog.as_ref().expect("the login dialog opens");
    assert_eq!(d.name(), "fromDatabase");
    assert_eq!(d.values.get("mode"), Some(&json!("credentials")));
    assert_eq!((d.values.get("host"), d.values.get("port")), (Some(&json!("127.0.0.1")), Some(&json!("1"))));
    assert_eq!(d.values["retry"]["id"], json!("data.query.add"));
    h.get_by_label("This query needs a login for its database. (PostgreSQL: 127.0.0.1:1;shop)");
}

#[test]
fn wide_preview_and_long_sql_keep_actions_on_screen() {
    for size in [egui::vec2(1440.0, 900.0), egui::vec2(1024.0, 700.0), egui::vec2(800.0, 600.0)] {
        let mut h = harness_at(size);
        h.state_mut().open_dialog("fromDatabase", json!({"kind":"sqlserver","host":"127.0.0.1","database":"LojaTeste"}));
        let d = h.state_mut().dialog.as_mut().unwrap();
        d.values.insert("tables".into(), json!([{"schema":"dbo","name":"Produtos","view":false}]));
        d.values.insert("selected".into(), json!(0));
        d.values.insert(
            "sql".into(),
            json!(format!("SELECT {}\nFROM Produtos\n{}", "CategoriaDescricao, Nome, ProdutoId, Preco", "-- long statement\n".repeat(80))),
        );
        let rows: Vec<_> =
            (0..50).map(|n| json!([n, format!("Acessório de informática {n}"), "Descrição da categoria sem quebras de linha"])).collect();
        d.values.insert("preview".into(),json!({"columns":[{"name":"ProdutoId","type":"int64"},{"name":"Nome","type":"text"},{"name":"CategoriaDescricao","type":"text"}],"rows":rows,"moreRows":150}));
        h.run_steps(6);
        assert!(h.query_by_label("SQL statement (optional)").is_none());
        h.get_by_label("Edit SQL").click();
        h.run_steps(4);
        let preview = h.get_by_label("Data Preview").rect();
        let header = h.get_by_role_and_label(egui::accesskit::Role::Label, "CategoriaDescricao").rect();
        assert!(header.height() < 30.0, "a column header stays on one line: {size:?} {header:?}");
        let sql = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect();
        assert!(preview.left() > h.get_by_label("Tables (1)").rect().right() + 100.0, "preview remains beside the sidebar: {size:?}");
        assert!((preview.left() - sql.left()).abs() < 10.0, "SQL stays inside the right panel: {size:?}");
        for label in ["Load", "Query Editor", "Cancel"] {
            let r = h.get_by_role_and_label(egui::accesskit::Role::Button, label).rect();
            assert!(r.left() >= 0.0 && r.top() >= 0.0 && r.right() <= size.x && r.bottom() <= size.y, "{label} must stay inside {size:?}: {r:?}");
            assert!(r.top() - preview.bottom() > 120.0, "preview retains useful vertical space at {size:?}");
        }
        h.get_by_label("Edit Connection").click();
        h.run_steps(4);
        h.get_by_label("Connect");
    }
}

#[test]
fn sql_is_only_shown_on_request_and_hiding_it_preserves_the_query_and_sidebar() {
    let f = TempDb::new();
    let mut h = harness();
    h.state_mut().open_dialog("fromDatabase", json!({"kind":"sqlite","path":f.0.to_string_lossy(),"connect":true,"table":"pedidos"}));
    h.run_steps(4);
    assert!(h.query_by_label("SQL statement (optional)").is_none());
    let sidebar_item = h.get_by_role_and_label(egui::accesskit::Role::Button, "pedidos").rect();
    let sql = "SELECT * FROM pedidos WHERE id = 2";
    replace_sql(&mut h, sql);
    h.get_by_label("Preview").click();
    h.run_steps(4);
    h.get_by_label("Bruno");
    let before = h.state().dialog.as_ref().unwrap().values.clone();
    h.get_by_label("Hide SQL").click();
    h.run_steps(4);
    assert!(h.query_by_label("SQL statement (optional)").is_none());
    assert_eq!(h.state().dialog.as_ref().unwrap().values["sql"], json!(sql));
    assert_eq!(h.state().dialog.as_ref().unwrap().values["selected"], before["selected"]);
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"], before["preview"]);
    assert_eq!(h.get_by_role_and_label(egui::accesskit::Role::Button, "pedidos").rect(), sidebar_item);
    // The stored SQL can be previewed and loaded while its editor is hidden.
    h.get_by_label("Preview").click();
    h.run_steps(4);
    h.get_by_label("Bruno");
    h.get_by_label("Edit SQL").click();
    h.run_steps(4);
    assert_eq!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).value().as_deref(), Some(sql));
    h.get_by_label("Hide SQL").click();
    h.run_steps(4);
    h.get_by_label("Load").click();
    h.run_steps(4);
    let query = h.state().session.active().unwrap().wb.queries[0].clone();
    assert!(query.formula.contains(sql));
    h.state_mut().open_dialog("fromDatabase", json!({"name":query.name}));
    h.run_steps(4);
    assert!(h.query_by_label("SQL statement (optional)").is_none(), "reopening starts compact even with saved SQL");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["sql"], json!(sql));
}

#[test]
fn object_tabs_and_search_keep_selection_and_sql_until_an_object_is_picked() {
    let f = TempDb::new();
    let mut h = harness();
    h.state_mut().open_dialog("fromDatabase", json!({"kind":"sqlite","path":f.0.to_string_lossy(),"connect":true}));
    h.run_steps(4);
    h.get_by_label("Tables (2)");
    h.get_by_label("Views (1)").click();
    h.run_steps(4);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "pedidos").is_none());
    // The filter matches schema and ignores case; the index remains that of the original list.
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search").click();
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search").type_text("MAIN.PEDIDOS_ATIV");
    h.run_steps(4);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "pedidos_ativos").click();
    h.run_steps(4);
    h.get_by_label("Célia");
    let picked = h.state().dialog.as_ref().unwrap().values["selected"].clone();
    replace_sql(&mut h, "SELECT * FROM pedidos WHERE id = 2");
    h.get_by_label("Tables (2)").click();
    h.run_steps(4);
    h.get_by_label("No matching tables or views.");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["selected"], picked);
    assert_eq!(h.state().dialog.as_ref().unwrap().values["sql"], json!("SELECT * FROM pedidos WHERE id = 2"));
    assert!(h.state().session.active().unwrap().wb.queries.is_empty());
    // Restore the view selection and load it, rather than the first filtered row's index.
    h.get_by_label("Views (1)").click();
    h.run_steps(4);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "pedidos_ativos").click();
    h.run_steps(4);
    h.get_by_label("Load").click();
    h.run_steps(4);
    let wb = &h.state().session.active().unwrap().wb;
    assert_eq!(wb.queries.len(), 1);
    assert!(wb.queries[0].formula.contains("pedidos_ativos"));
    h.state_mut().open_dialog("fromDatabase", json!({"name":"pedidos_ativos"}));
    h.run_steps(4);
    assert_eq!(h.state().dialog.as_ref().unwrap().values["objectTab"], json!("views"));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "pedidos_ativos");
}

#[test]
fn query_editor_button_opens_the_selected_source_and_reuses_existing_queries() {
    let f = TempDb::new();
    let mut h = harness();
    h.state_mut().open_dialog("fromDatabase", json!({"kind":"sqlite","path":f.0.to_string_lossy(),"connect":true,"table":"pedidos"}));
    h.run_steps(4);
    h.get_by_label("Query Editor").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog.as_ref().unwrap().name(), "queryEditor");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["original"], json!("pedidos"));
    h.get_by_label("Applied Steps");
    h.get_by_label("Célia");
    assert_eq!(h.state().session.active().unwrap().wb.queries.len(), 1);
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    h.state_mut().open_dialog("fromDatabase", json!({"name":"pedidos"}));
    h.run_steps(4);
    h.get_by_label("Query Editor").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog.as_ref().unwrap().name(), "queryEditor");
    assert_eq!(h.state().session.active().unwrap().wb.queries.len(), 1);
}

#[test]
fn typing_preserves_rows_until_the_debounce_expires_and_errors_keep_previous_results() {
    let f = TempDb::new();
    let mut h = harness();
    h.state_mut().open_dialog("fromDatabase", json!({"kind":"sqlite","path":f.0.to_string_lossy(),"connect":true,"table":"pedidos"}));
    h.run_steps(4);
    let before = h.state().dialog.as_ref().unwrap().values["preview"].clone();
    replace_sql(&mut h, "SELECT * FROM pedidos WHERE id = 2");
    let deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"], before);
    h.get_by_label("Ana");
    h.input_mut().time = Some(deadline - 0.1);
    h.step();
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"], before);
    // Another keystroke resets the timer; the earlier deadline must not execute the query.
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).type_text(" OR id = 3");
    h.step();
    let next_deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    assert!(next_deadline > deadline);
    h.input_mut().time = Some(deadline + 0.01);
    h.step();
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"], before);
    h.input_mut().time = Some(next_deadline + 0.01);
    h.step();
    let result = h.state().dialog.as_ref().unwrap().values["preview"].clone();
    assert_eq!(result["rows"].as_array().unwrap().len(), 2);
    assert_eq!(result["rows"][0][1], "Bruno");
    assert_eq!(result["rows"][1][1], "Célia");
    assert!(!h.state().dialog.as_ref().unwrap().values.contains_key("sqlPreviewAt"));
    // A partial/invalid statement reports its error without making the previous table vanish.
    replace_sql(&mut h, "SELECT invalid_column FROM pedidos");
    let deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    h.input_mut().time = Some(deadline + 0.01);
    h.step();
    assert!(h.state().dialog.as_ref().unwrap().values["previewError"].is_string());
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"], result);
    h.get_by_label("Bruno");
    replace_sql(&mut h, "SELECT * FROM pedidos WHERE id = 1");
    let deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    h.input_mut().time = Some(deadline + 0.01);
    h.step();
    assert!(!h.state().dialog.as_ref().unwrap().values.contains_key("previewError"));
    assert!(!h.state().dialog.as_ref().unwrap().values.contains_key("previewStale"));
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"][0][1], "Ana");
    assert!(h.state().session.active().unwrap().wb.queries.is_empty());
}

#[test]
fn sql_with_side_effects_requires_manual_preview_and_picking_a_table_cancels_pending_sql() {
    let f = TempDb::new();
    let mut h = harness();
    h.state_mut().open_dialog("fromDatabase", json!({"kind":"sqlite","path":f.0.to_string_lossy(),"connect":true,"table":"pedidos"}));
    h.run_steps(4);
    let before = h.state().dialog.as_ref().unwrap().values["preview"].clone();
    replace_sql(&mut h, "DROP TABLE pedidos");
    let deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    h.input_mut().time = Some(deadline + 0.01);
    h.step();
    h.get_by_label("This SQL requires the Preview button.");
    assert!(!h.state().dialog.as_ref().unwrap().values.contains_key("previewError"));
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"], before);
    replace_sql(&mut h, "SELECT COUNT(*) AS total FROM pedidos");
    let deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    h.input_mut().time = Some(deadline + 0.01);
    h.step();
    assert_eq!(h.state().dialog.as_ref().unwrap().values["previewManual"], true);
    h.get_by_label("Preview").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"][0][0].as_f64(), Some(3.0));
    assert!(!h.state().dialog.as_ref().unwrap().values.contains_key("previewManual"));
    replace_sql(&mut h, "SELECT * FROM pedidos WHERE id = 2");
    let deadline = h.state().dialog.as_ref().unwrap().values["sqlPreviewAt"].as_f64().unwrap();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "clientes").click();
    h.run_steps(4);
    h.input_mut().time = Some(deadline + 0.01);
    h.step();
    let d = h.state().dialog.as_ref().unwrap();
    assert!(!d.values.contains_key("sqlPreviewAt"));
    assert_eq!(d.values["sql"], "");
    assert_eq!(d.values["preview"]["columns"][0]["name"], "nome");
}
