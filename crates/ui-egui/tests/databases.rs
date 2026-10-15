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
    let mut session = Session::new();
    session.new_workbook();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
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
        let path = std::env::temp_dir().join(format!("gridcraft-ui-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE pedidos (id INTEGER, cliente TEXT, valor REAL);
             INSERT INTO pedidos VALUES (1, 'Ana', 120.5), (2, 'Bruno', 75), (3, 'Célia', 310.99);
             CREATE TABLE clientes (nome TEXT);",
        )
        .unwrap();
        TempDb(path)
    }
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
