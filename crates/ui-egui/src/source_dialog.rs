//! Get Data sources and the session's permission to read a web site.
use egui::{RichText, Ui};
use serde_json::{Value as Json, json};

use crate::{SheetApp, dialogs::Dialog, i18n};

const SOURCES: [(&str, &str); 5] =
    [("folder", "From Folder"), ("xml", "From XML"), ("web", "From Web"), ("json", "JSON from Web"), ("odata", "From OData Feed")];

pub fn show(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog, open: &mut bool) {
    let language = i18n::current(ui.ctx());
    let mut kind = d.values.get("kind").and_then(Json::as_str).unwrap_or("web").to_string();
    egui::ComboBox::from_id_salt("query_source")
        .selected_text(language.tr(SOURCES.iter().find(|(k, _)| *k == kind).map_or("From Web", |(_, l)| *l)))
        .show_ui(ui, |ui| {
            for (key, label) in SOURCES {
                ui.selectable_value(&mut kind, key.to_string(), language.tr(label));
            }
        });
    d.values.insert("kind".into(), json!(kind));
    let local = matches!(kind.as_str(), "folder" | "xml");
    let key = if local { "path" } else { "url" };
    let mut address = d.values.get(key).and_then(Json::as_str).unwrap_or("").to_string();
    ui.label(language.tr(if local { "Path" } else { "URL" }));
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut address).desired_width(420.0));
        let picker = if kind == "folder" { &app.services.pick_folder } else { &app.services.pick_xml };
        if local
            && picker.is_some()
            && ui.button(language.tr("Browse…")).clicked()
            && let Some(path) = picker.as_ref().and_then(|pick| pick())
        {
            address = path;
        }
    });
    d.values.insert(key.into(), json!(address));
    if let Some(error) = d.values.get("error").and_then(Json::as_str) {
        ui.colored_label(egui::Color32::RED, error);
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(language.tr("Transform Data")).clicked() {
            let params = Json::Object(d.values.clone());
            match app.session.run("data.source.formula", params) {
                Ok(r) => {
                    // The user typed this address here: the site may be read, as SQL typed in
                    // From Database may run. A site that a query names on its own still asks.
                    if !local {
                        let _ = app.session.run("data.web.approve", json!({"site": address}));
                    }
                    *open = false;
                    app.open_dialog("queryEditor", json!({"formula": r["formula"], "autoPreview": true}));
                }
                Err(e) => {
                    d.values.insert("error".into(), json!(crate::clean_error(&e)));
                }
            }
        }
        if ui.button(language.tr("Cancel")).clicked() {
            *open = false;
        }
    });
}

pub fn approval(app: &mut SheetApp, ui: &mut Ui, d: &mut Dialog, open: &mut bool) {
    let language = i18n::current(ui.ctx());
    let site = d.values.get("site").and_then(Json::as_str).unwrap_or("").to_string();
    ui.label(language.tr("This query needs permission to read from this site. Allow it only if you trust the query."));
    ui.label(RichText::new(&site).strong());
    ui.label(language.tr("Permission lasts for this session."));
    if let Some(e) = d.values.get("error").and_then(Json::as_str) {
        ui.colored_label(egui::Color32::RED, e);
    }
    ui.horizontal(|ui| {
        if ui.button(language.tr("Allow Access")).clicked() {
            match app.session.run("data.web.approve", json!({"site": site})) {
                Ok(_) => {
                    *open = false;
                    if let Some(back) = d.back.as_mut()
                        && back.name() == "queryEditor"
                    {
                        back.values.insert("autoPreview".into(), json!(true));
                    }
                    let retry = d.values.get("retry").cloned().unwrap_or(Json::Null);
                    if let Some(id) = retry["id"].as_str() {
                        let _ = app.run(id, retry["params"].clone());
                    }
                }
                Err(e) => {
                    d.values.insert("error".into(), json!(crate::clean_error(&e)));
                }
            }
        }
        if ui.button(language.tr("Cancel")).clicked() {
            *open = false;
        }
    });
}
