//! Thin transformation controls: each action calls a command that returns the edited M formula.
use crate::{SheetApp, dialogs::Dialog, i18n};
use egui::Ui;
use serde_json::{Value as Json, json};

const OPS: [(&str, &str); 16] = [
    ("removeColumns", "Remove Columns"),
    ("keepColumns", "Keep Columns"),
    ("changeType", "Change Type"),
    ("filter", "Filter Rows"),
    ("sort", "Sort Rows"),
    ("rename", "Rename Column"),
    ("groupBy", "Group By"),
    ("removeDuplicates", "Remove Duplicates"),
    ("replaceValues", "Replace Values"),
    ("splitColumn", "Split Column"),
    ("addColumn", "Custom Column"),
    ("keepTopRows", "Keep Top Rows"),
    ("promoteHeaders", "Use First Row as Headers"),
    ("removeBlankRows", "Remove Blank Rows"),
    ("fillDown", "Fill Down"),
    ("unpivotOthers", "Unpivot Other Columns"),
];

fn text(d: &Dialog, key: &str) -> String {
    d.values.get(key).and_then(Json::as_str).unwrap_or("").to_string()
}
fn columns(d: &Dialog) -> Vec<String> {
    d.values.get("selectedColumns").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_str).map(str::to_string).collect()
}
fn error(d: &mut Dialog, e: String) {
    d.values.insert("transformError".into(), json!(crate::clean_error(&e)));
}

pub fn edit_step(app: &mut SheetApp, d: &mut Dialog, id: &str, mut params: Json) {
    params["formula"] = json!(text(d, "formula"));
    match app.session.run(id, params) {
        Ok(r) => {
            d.values.insert("formula".into(), r["formula"].clone());
            if let Some(step) = r.get("step") {
                d.values.insert("step".into(), step.clone());
            } else {
                d.values.remove("step");
            }
            for key in ["transformError", "transform", "renameStep", "selectedColumns"] {
                d.values.remove(key);
            }
            crate::query_editor::run_preview(app, d);
        }
        Err(e) => {
            error(d, e);
            crate::database_dialog::ask_for_needs(app, None);
        }
    }
}

fn apply(app: &mut SheetApp, d: &mut Dialog, op: &str, params: Json) {
    let name = d.values.get("original").and_then(Json::as_str).map(str::to_string).unwrap_or_else(|| text(d, "name"));
    edit_step(app, d, "data.query.addStep", json!({"name":name,"op":op,"params":params}));
}

fn edit(ui: &mut Ui, d: &mut Dialog, key: &str, label: &str) {
    let language = i18n::current(ui.ctx());
    ui.label(language.tr(label));
    let mut value = text(d, key);
    if ui.add(egui::TextEdit::singleline(&mut value).id_salt(key).desired_width(190.0)).changed() {
        d.values.insert(key.into(), json!(value));
    }
}
fn choice(ui: &mut Ui, d: &mut Dialog, key: &str, label: &str, options: &[(&str, &str)], default: &str) {
    let language = i18n::current(ui.ctx());
    ui.label(language.tr(label));
    let mut value = d.values.get(key).and_then(Json::as_str).unwrap_or(default).to_string();
    egui::ComboBox::from_id_salt(key).selected_text(language.tr(options.iter().find(|(k, _)| *k == value).map_or(default, |(_, l)| *l))).show_ui(
        ui,
        |ui| {
            for (key, label) in options {
                ui.selectable_value(&mut value, key.to_string(), language.tr(label));
            }
        },
    );
    d.values.insert(key.into(), json!(value));
}

fn typed_value(d: &Dialog, key: &str) -> Result<Json, String> {
    let value = text(d, key);
    match text(d, "valueType").as_str() {
        "number" => value.replace(',', ".").parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| json!(n)).ok_or("Enter a valid number.".into()),
        "logical" => match value.to_lowercase().as_str() {
            "true" | "verdadeiro" => Ok(json!(true)),
            "false" | "falso" => Ok(json!(false)),
            _ => Err("Enter true or false.".into()),
        },
        "null" => Ok(Json::Null),
        _ => Ok(json!(value)),
    }
}

pub fn toolbar(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog) {
    let language = i18n::current(ui.ctx());
    if d.values.contains_key("renameStep") {
        ui.horizontal(|ui| {
            edit(ui, d, "renameStepTo", "New step name");
            if ui.button(language.tr("Apply Step Name")).clicked() {
                let step = text(d, "renameStep");
                let to = text(d, "renameStepTo");
                edit_step(app, d, "data.query.renameStep", json!({"step":step,"to":to}));
            }
            if ui.button(language.tr("Cancel Step Rename")).clicked() {
                d.values.remove("renameStep");
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        for (op, label) in OPS {
            if ui.button(language.tr(label)).clicked() {
                let columns = columns(d);
                if matches!(
                    op,
                    "removeColumns" | "keepColumns" | "removeDuplicates" | "promoteHeaders" | "removeBlankRows" | "fillDown" | "unpivotOthers"
                ) {
                    apply(app, d, op, json!({"columns":columns}));
                } else {
                    d.values.insert("transform".into(), json!(op));
                    for key in
                        ["value", "from", "to", "newName", "expression", "valueType", "type", "operator", "order", "aggregation", "aggregateColumn"]
                    {
                        d.values.remove(key);
                    }
                    d.values.insert("count".into(), json!(10));
                    d.values.insert("delimiter".into(), json!(","));
                }
            }
        }
    });
    let op = text(d, "transform");
    if !op.is_empty() {
        ui.group(|ui| {
            ui.label(language.tr(OPS.iter().find(|(k, _)| *k == op).map_or("Transform Data", |(_, label)| *label)));
            ui.horizontal_wrapped(|ui| options(ui, d, &op));
            ui.horizontal(|ui| {
                if ui.button(language.tr("Apply Transformation")).clicked() {
                    let culture = if app.ui.language == i18n::Language::PtBr { "pt-BR" } else { "en-US" };
                    match params(d, &op, culture) {
                        Ok(params) => apply(app, d, &op, params),
                        Err(e) => error(d, e),
                    }
                }
                if ui.button(language.tr("Cancel Transformation")).clicked() {
                    d.values.remove("transform");
                }
            });
        });
    }
    if let Some(error) = d.values.get("transformError").and_then(Json::as_str) {
        ui.colored_label(egui::Color32::from_rgb(196, 43, 28), language.tr(error));
    }
}

const TYPES: [(&str, &str); 8] = [
    ("text", "Text"),
    ("number", "Number"),
    ("integer", "Whole Number"),
    ("date", "Date"),
    ("datetime", "Date/Time"),
    ("time", "Time"),
    ("logical", "Logical"),
    ("percentage", "Percentage"),
];
const OPERATORS: [(&str, &str); 10] = [
    ("=", "Equals"),
    ("<>", "Does Not Equal"),
    (">", "Greater Than"),
    (">=", "Greater Than or Equal"),
    ("<", "Less Than"),
    ("<=", "Less Than or Equal"),
    ("contains", "Contains"),
    ("startsWith", "Begins With"),
    ("endsWith", "Ends With"),
    ("isNull", "Is Null"),
];
const ORDERS: [(&str, &str); 2] = [("ascending", "Ascending"), ("descending", "Descending")];
const AGGREGATIONS: [(&str, &str); 5] = [("sum", "Sum"), ("count", "Count Rows"), ("average", "Average"), ("min", "Minimum"), ("max", "Maximum")];
const VALUE_TYPES: [(&str, &str); 4] = [("text", "Text"), ("number", "Number"), ("logical", "Logical"), ("null", "Null")];

/// The options the chosen transformation asks for.
fn options(ui: &mut Ui, d: &mut Dialog, op: &str) {
    match op {
        "changeType" => choice(ui, d, "type", "Data type", &TYPES, "text"),
        "filter" => {
            choice(ui, d, "operator", "Operator", &OPERATORS, "=");
            if text(d, "operator") != "isNull" {
                edit(ui, d, "value", "Value");
            }
        }
        "sort" => choice(ui, d, "order", "Order", &ORDERS, "ascending"),
        "rename" | "addColumn" | "groupBy" => {
            edit(ui, d, "newName", "New column name");
            if op == "addColumn" {
                edit(ui, d, "expression", "Column formula");
            }
            if op == "groupBy" {
                choice(ui, d, "aggregation", "Aggregation", &AGGREGATIONS, "sum");
                if text(d, "aggregation") != "count" {
                    let available: Vec<String> = d
                        .values
                        .get("preview")
                        .and_then(|p| p["columns"].as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["name"].as_str().map(str::to_string))
                        .collect();
                    let columns: Vec<(&str, &str)> = available.iter().map(|n| (n.as_str(), n.as_str())).collect();
                    let first = self::columns(d).first().cloned().unwrap_or_default();
                    choice(ui, d, "aggregateColumn", "Aggregate column", &columns, &first);
                }
            }
        }
        "replaceValues" => {
            edit(ui, d, "from", "Value to find");
            edit(ui, d, "to", "Replace with");
        }
        "splitColumn" => edit(ui, d, "delimiter", "Delimiter"),
        "keepTopRows" => {
            ui.label(i18n::current(ui.ctx()).tr("Number of rows"));
            let mut count = d.values.get("count").and_then(Json::as_u64).unwrap_or(10);
            if ui.add(egui::DragValue::new(&mut count).range(0..=1_048_576)).changed() {
                d.values.insert("count".into(), json!(count));
            }
        }
        _ => {}
    }
    if matches!(op, "filter" | "replaceValues") && text(d, "operator") != "isNull" {
        choice(ui, d, "valueType", "Value type", &VALUE_TYPES, "text");
    }
}

/// The `data.query.addStep` params for the chosen transformation and its options, or why they
/// are not complete.
fn params(d: &Dialog, op: &str, culture: &str) -> Result<Json, String> {
    let selected = columns(d);
    let first = selected.first().cloned().unwrap_or_default();
    if matches!(op, "filter" | "sort" | "rename" | "splitColumn") && selected.len() != 1 {
        return Err("Select exactly one column.".into());
    }
    let mut params = json!({
        "columns": selected,
        "column": first,
        "name": text(d, "newName"),
        "type": text(d, "type"),
        "culture": culture,
        "operator": text(d, "operator"),
        "order": text(d, "order"),
        "aggregation": text(d, "aggregation"),
        "expression": text(d, "expression"),
        "delimiter": text(d, "delimiter"),
        "count": d.values.get("count"),
    });
    match op {
        "groupBy" if text(d, "aggregation") != "count" => params["column"] = json!(text(d, "aggregateColumn")),
        "splitColumn" => params["names"] = json!([format!("{first}.1"), format!("{first}.2")]),
        "replaceValues" => params["textOnly"] = json!(text(d, "valueType") == "text"),
        _ => {}
    }
    let values: &[&str] = match op {
        "replaceValues" => &["from", "to"],
        "filter" if text(d, "operator") != "isNull" => &["value"],
        _ => &[],
    };
    for key in values {
        params[*key] = typed_value(d, key)?;
    }
    Ok(params)
}
