//! Transformations must produce the expected data, preserve source text semantics and leave the
//! workbook and its history alone until the user saves.
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

const DATA: &str = r##"#table({"A", "B"}, {{"x", 2}, {"x", 1}, {"y", 3}})"##;
fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}
fn transform(s: &mut Session, formula: &str, op: &str, params: Json) -> Json {
    s.run("data.query.addStep", json!({"formula":formula,"op":op,"params":params})).unwrap()
}
fn rows(s: &mut Session, formula: &str) -> Json {
    let mut rows = s.run("data.query.preview", json!({"formula":formula})).unwrap()["rows"].clone();
    for value in rows.as_array_mut().into_iter().flatten().flat_map(|r| r.as_array_mut().into_iter().flatten()) {
        if let Some(n) = value.as_f64()
            && n.fract() == 0.0
            && n.abs() < 1e15
        {
            *value = json!(n as i64);
        }
    }
    rows
}

#[test]
fn transformations_change_data_only_when_the_returned_formula_is_used() {
    let mut s = session();
    let cases = [
        (DATA, "removeColumns", json!({"columns":["B"]}), json!([["x"], ["x"], ["y"]])),
        (DATA, "keepColumns", json!({"columns":["B"]}), json!([[2], [1], [3]])),
        (DATA, "changeType", json!({"columns":["B"],"type":"text"}), json!([["x", "2"], ["x", "1"], ["y", "3"]])),
        (DATA, "filter", json!({"column":"B","operator":">","value":1}), json!([["x", 2], ["y", 3]])),
        (DATA, "sort", json!({"column":"B","order":"ascending"}), json!([["x", 1], ["x", 2], ["y", 3]])),
        (DATA, "rename", json!({"column":"A","name":"Label"}), json!([["x", 2], ["x", 1], ["y", 3]])),
        (DATA, "groupBy", json!({"columns":["A"],"column":"B","aggregation":"sum","name":"Total"}), json!([["x", 3], ["y", 3]])),
        (DATA, "removeDuplicates", json!({"columns":["A"]}), json!([["x", 2], ["y", 3]])),
        (DATA, "replaceValues", json!({"columns":["A"],"from":"x","to":"z"}), json!([["z", 2], ["z", 1], ["y", 3]])),
        (
            r##"#table({"A"}, {{"a,b"},{"c,d"}})"##,
            "splitColumn",
            json!({"column":"A","delimiter":",","names":["Left","Right"]}),
            json!([["a", "b"], ["c", "d"]]),
        ),
        (DATA, "addColumn", json!({"name":"Double","expression":"[B] * 2"}), json!([["x", 2, 4], ["x", 1, 2], ["y", 3, 6]])),
        (DATA, "keepTopRows", json!({"count":2}), json!([["x", 2], ["x", 1]])),
        (r##"#table({"Column1"}, {{"Title"},{"value"}})"##, "promoteHeaders", json!({}), json!([["value"]])),
        (r##"#table({"A"}, {{""},{null},{"yes"}})"##, "removeBlankRows", json!({}), json!([["yes"]])),
        (r##"#table({"A"}, {{"x"},{null},{"y"}})"##, "fillDown", json!({"columns":["A"]}), json!([["x"], ["x"], ["y"]])),
        (DATA, "unpivotOthers", json!({"columns":["A"]}), json!([["x", "B", 2], ["x", "B", 1], ["y", "B", 3]])),
    ];
    let before = s.active().unwrap().wb.clone();
    let journal = s.journal.len();
    for (formula, op, params, expected) in cases {
        let out = transform(&mut s, formula, op, params);
        let actual = rows(&mut s, out["formula"].as_str().unwrap());
        assert_eq!(actual, expected, "{op}: {}", out["formula"]);
    }
    assert_eq!(s.active().unwrap().wb, before);
    assert_eq!(s.journal.len(), journal);
}

#[test]
fn successive_steps_rename_and_remove_reconnect_the_pipeline() {
    let mut s = session();
    let filtered = transform(&mut s, DATA, "filter", json!({"column":"B","operator":">","value":1}));
    let sorted = transform(&mut s, filtered["formula"].as_str().unwrap(), "sort", json!({"column":"B","order":"descending"}));
    let renamed = s.run("data.query.renameStep", json!({"formula":sorted["formula"],"step":"Filtered Rows","to":"Com espaços e \"aspas\""})).unwrap();
    assert_eq!(rows(&mut s, renamed["formula"].as_str().unwrap()), json!([["y", 3], ["x", 2]]));
    let removed = s.run("data.query.removeStep", json!({"formula":renamed["formula"],"step":"Com espaços e \"aspas\""})).unwrap();
    assert_eq!(rows(&mut s, removed["formula"].as_str().unwrap()), json!([["y", 3], ["x", 2], ["x", 1]]));
    assert!(s.run("data.query.removeStep", json!({"formula":removed["formula"],"step":"Source"})).is_err());
    let again = transform(&mut s, sorted["formula"].as_str().unwrap(), "sort", json!({"column":"B"}));
    assert_eq!(again["step"], "Sorted Rows 1");
    assert_eq!(rows(&mut s, again["formula"].as_str().unwrap()), json!([["x", 2], ["y", 3]]));
}

#[test]
fn renaming_respects_fields_strings_nested_scopes_and_exclusive_self_references() {
    let mut s = session();
    let f = r##"let Source = 4, Data = [Source = Source, Text = "Source", Local = ((Source) => Source)(8), Nested = (let Source = Source + 1 in Source)] in Data"##;
    let original = rows(&mut s, f);
    let renamed = s.run("data.query.renameStep", json!({"formula":f,"step":"Source","to":"Input Data"})).unwrap();
    assert_eq!(rows(&mut s, renamed["formula"].as_str().unwrap()), original);
    assert!(renamed["formula"].as_str().unwrap().contains("Text = \"Source\""));
    assert!(renamed["formula"].as_str().unwrap().contains("(Source) => Source"));
    assert!(
        s.run("data.query.renameStep", json!({"formula":"let Source=4, Data=((Target)=>Source)(2) in Data","step":"Source","to":"Target"})).is_err()
    );
    assert!(s.run("data.query.renameStep", json!({"formula":"let Source=4, Data=Source + Target in Data","step":"Source","to":"Target"})).is_err());
}

#[test]
fn invalid_parameters_and_hostile_names_are_handled() {
    let mut s = session();
    for (op, params) in [
        ("removeColumns", json!({"columns":["missing"]})),
        ("keepColumns", json!({"columns":[]})),
        ("changeType", json!({"columns":["A"],"type":"broken"})),
        ("sort", json!({"column":"A","order":"oops"})),
        ("keepTopRows", json!({"count":-1})),
        ("rename", json!({"column":"A","name":"B"})),
        ("addColumn", json!({"name":"A","expression":"1"})),
        ("groupBy", json!({"columns":["A"],"aggregation":"count","name":"A"})),
    ] {
        assert!(s.run("data.query.addStep", json!({"formula":DATA,"op":op,"params":params})).is_err(), "{op}");
    }
    let f = format!("#table({{{}, \"B\"}}, {{{{1,2}}}}) // a trailing comment", gridcraft_mquery::quote_text("a] \" #("));
    let out = transform(&mut s, &f, "filter", json!({"column":"a] \" #(","operator":"=","value":1}));
    assert_eq!(rows(&mut s, out["formula"].as_str().unwrap()), json!([[1, 2]]));
    assert!(s.run("data.query.removeStep", json!({"formula":"let A=B, B=1 in A","step":"B"})).is_err());
}

#[test]
fn source_formulas_quote_paths_and_urls_without_reading_them() {
    let mut s = Session::new();
    for (kind, path, url) in [
        ("folder", "/tmp/a \"quoted\" #(name)", ""),
        ("xml", "C:\\data\\a.xml", ""),
        ("web", "", "https://example.invalid/tables"),
        ("json", "", "https://example.invalid/data.json?q=%22"),
        ("odata", "", "https://example.invalid/odata"),
    ] {
        let out = s.run("data.source.formula", json!({"kind":kind,"path":path,"url":url})).unwrap();
        gridcraft_mquery::parse(out["formula"].as_str().unwrap()).unwrap();
    }
    assert!(s.run("data.source.formula", json!({"kind":"json","url":"file:///tmp/private"})).is_err());
}
