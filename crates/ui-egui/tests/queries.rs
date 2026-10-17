//! Queries in the interface: Data › From Table/Range opens the Query Editor on a query that reads
//! the table; saving loads it; the Queries & Connections pane lists, refreshes and deletes it.
//!
//! Ribbon buttons are painted without accessible labels, so these tests do what the buttons do
//! (`open_dialog("queryEditor", {fromSelection})`, the pane flag) and click inside the editor and
//! the pane, which are ordinary widgets.

use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::{Session, core::CellRef, core::Value};
use gridcraft_ui_egui::{Services, SheetApp};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// A workbook with the table `Stock` (Item, Qty) at A1:B4, the Data tab showing.
fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    session.run("range.setValues", json!({"range": "A1", "values": [["Item", "Qty"], ["pen", 10], ["ink", 0], ["pad", 7]]})).unwrap();
    session.run("insert.table", json!({"range": "A1:B4", "header": true, "name": "Stock"})).unwrap();
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

fn click(h: &mut Harness, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(4);
}

/// What Data › From Table/Range does.
fn from_table_range(h: &mut Harness) {
    h.state_mut().open_dialog("queryEditor", json!({"fromSelection": true}));
    h.run_steps(4);
}

fn cell(h: &Harness, sheet: &str, a1: &str) -> Value {
    let wb = &h.state().session.active().unwrap().wb;
    wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().value(CellRef::parse(a1).unwrap())
}

#[test]
fn from_table_range_makes_a_query_and_saving_loads_it() {
    let mut h = harness();
    from_table_range(&mut h);
    let dialog = h.state().dialog.as_ref().map(|d| d.name().to_string());
    assert_eq!(dialog.as_deref(), Some("queryEditor"));
    // The editor previews the table it reads, and lists the query's step.
    h.get_by_label("Applied Steps");
    h.get_by_label("  Source");
    h.get_by_label("pad");
    click(&mut h, "Save");
    assert!(h.state().dialog.is_none(), "saving closes the editor");
    let wb = h.state().session.active().unwrap().wb.clone();
    assert_eq!(wb.queries.len(), 1);
    // The query is named after the table; its own table needs another name.
    assert_eq!((wb.queries[0].name.as_str(), wb.queries[0].load_to.as_deref()), ("Stock", Some("Stock_2")));
    assert_eq!(cell(&h, "Stock", "A4"), Value::text("pad"));
}

#[test]
fn the_pane_lists_refreshes_and_deletes_queries() {
    let mut h = harness();
    let formula = "Table.SelectRows(Excel.CurrentWorkbook(){[Name = \"Stock\"]}[Content], each [Qty] > 0)";
    h.state_mut().session.run("data.query.add", json!({"name": "In Stock", "formula": formula})).unwrap();
    h.state_mut().session.run("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    h.state_mut().grid.pane = Some("queries".into());
    h.run_steps(4);
    assert_eq!(h.state().grid.pane.as_deref(), Some("queries"));
    h.get_by_label("In Stock");
    h.get_by_label("Loaded to 'In Stock'!A1:B3");
    // New stock of ink, then Refresh from the pane.
    h.state_mut().session.run("range.setValues", json!({"sheet": "Sheet1", "range": "B3", "values": [[5]]})).unwrap();
    click(&mut h, "Refresh");
    assert_eq!(cell(&h, "In Stock", "A3"), Value::text("ink"));
    h.get_by_label("Loaded to 'In Stock'!A1:B4");
    // Edit opens the editor on the stored query.
    click(&mut h, "Edit");
    assert_eq!(h.state().dialog.as_ref().map(|d| d.values.get("original").cloned()), Some(Some(json!("In Stock"))));
    click(&mut h, "Cancel");
    click(&mut h, "Delete");
    assert!(h.state().session.active().unwrap().wb.queries.is_empty());
    h.get_by_label("This workbook has no queries. Choose New Query, or From Table/Range on the Data tab.");
}

#[test]
fn a_table_is_needed_for_from_table_range() {
    let mut h = harness();
    h.state_mut().session.run("selection.set", json!({"cell": "F10"})).unwrap();
    from_table_range(&mut h);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().message.as_ref().map(|m| m.0.as_str()), Some("From Table/Range"));
}

#[test]
fn transformations_edit_steps_then_save_the_result() {
    let mut h = harness();
    from_table_range(&mut h);
    click(&mut h, "Qty");
    click(&mut h, "Filter Rows");
    {
        let d = h.state_mut().dialog.as_mut().unwrap();
        d.values.insert("operator".into(), json!(">"));
        d.values.insert("value".into(), json!("5"));
        d.values.insert("valueType".into(), json!("number"));
    }
    click(&mut h, "Apply Transformation");
    assert!(h.state().session.active().unwrap().wb.queries.is_empty());
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"], json!([["pen", 10.0], ["pad", 7.0]]));
    click(&mut h, "Qty");
    click(&mut h, "Remove Columns");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"], json!([["pen"], ["pad"]]));
    click(&mut h, "  Filtered Rows");
    click(&mut h, "Rename Step");
    h.state_mut().dialog.as_mut().unwrap().values.insert("renameStepTo".into(), json!("In Stock"));
    click(&mut h, "Apply Step Name");
    click(&mut h, "  In Stock");
    click(&mut h, "Remove Step");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"], json!([["pen"], ["ink"], ["pad"]]));
    click(&mut h, "Save");
    assert!(h.state().dialog.is_none());
    assert_eq!(cell(&h, "Stock", "A3"), Value::text("ink"));
}

#[test]
fn xml_source_can_be_opened_and_loaded_without_writing_m() {
    let mut h = harness();
    let path = std::env::temp_dir().join(format!("gridcraft-source-ui-{}.xml", std::process::id()));
    std::fs::write(&path, "<root><row><a>one</a></row><row><a>two</a></row></root>").unwrap();
    h.state_mut().open_dialog("fromSource", json!({"kind":"xml","path":path.to_string_lossy()}));
    h.run_steps(4);
    click(&mut h, "Transform Data");
    assert_eq!(h.state().dialog.as_ref().unwrap().name(), "queryEditor");
    click(&mut h, "Open Table");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"], json!([["one"], ["two"]]));
    click(&mut h, "Save");
    assert_eq!(cell(&h, "Query1", "A2"), Value::text("one"));
    std::fs::remove_file(path).unwrap();
}

// The web tests use a closed port on this computer: nothing leaves the machine.

#[test]
fn a_site_typed_in_get_data_is_allowed() {
    let mut h = harness();
    h.state_mut().open_dialog("fromSource", json!({"kind":"json","url":"http://127.0.0.1:9/data.json"}));
    h.run_steps(4);
    click(&mut h, "Transform Data");
    let d = h.state().dialog.as_ref().unwrap();
    assert_eq!(d.name(), "queryEditor", "no permission asked for the site the user typed");
    assert!(d.values["formula"].as_str().unwrap().contains("Json.Document"));
    assert!(h.state().session.data_sources.is_web_approved("127.0.0.1:9"));
}

#[test]
fn a_site_named_by_a_query_asks_for_permission_and_cancel_returns_to_the_editor() {
    let mut h = harness();
    let formula = "Json.Document(Web.Contents(\"http://127.0.0.1:9/data.json\"))";
    h.state_mut().open_dialog("queryEditor", json!({"formula": formula, "autoPreview": true}));
    h.run_steps(4);
    assert_eq!(h.state().dialog.as_ref().unwrap().name(), "webAccess");
    h.get_by_label("127.0.0.1:9");
    click(&mut h, "Cancel");
    assert_eq!(h.state().dialog.as_ref().unwrap().name(), "queryEditor");
    assert!(!h.state().session.data_sources.is_web_approved("127.0.0.1:9"));
}

#[test]
fn ctrl_click_selects_multiple_columns_and_row_actions_need_no_selection() {
    let mut h = harness();
    from_table_range(&mut h);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Item").click();
    h.run_steps(4);
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::CTRL));
    h.run_steps(1);
    click(&mut h, "Qty");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["selectedColumns"], json!(["Item", "Qty"]));
    h.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.state_mut().dialog.as_mut().unwrap().values.remove("selectedColumns");
    h.run_steps(2);
    click(&mut h, "Keep Top Rows");
    h.state_mut().dialog.as_mut().unwrap().values.insert("count".into(), json!(1));
    click(&mut h, "Apply Transformation");
    assert_eq!(h.state().dialog.as_ref().unwrap().values["preview"]["rows"], json!([["pen", 10.0]]));
}
