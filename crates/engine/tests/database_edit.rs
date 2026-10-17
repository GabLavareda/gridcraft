//! Editing the SQL source keeps downstream M steps and updates an existing query in place.
use gridcraft_engine::{Session, cmd::database::database_formula, datasource::Kind};
use serde_json::{Value as Json, json};

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn describe(s: &mut Session, formula: &str) -> Json {
    s.run("data.database.source", json!({"formula":formula})).unwrap()
}

fn edit(s: &mut Session, formula: &str, p: Json) -> String {
    let mut p = p;
    p["formula"] = json!(formula);
    s.run("data.database.formula", p).unwrap()["formula"].as_str().unwrap().to_string()
}

#[test]
fn every_database_source_round_trips_sql_and_table_with_transformations() {
    let mut s = session();
    let sql = "SELECT id, 'ação # \"' AS nome\nFROM pedidos WHERE id > 1";
    for kind in Kind::ALL {
        for initial_sql in [None, Some("SELECT * FROM pedidos")] {
            let formula = database_formula(kind, "localhost", if kind == Kind::Sqlite { "" } else { "loja" }, "dbo", Some("pedidos"), initial_sql);
            let result = gridcraft_mquery::steps(&formula).unwrap().result.unwrap();
            let end = formula.rfind("\nin\n").unwrap();
            let formula = format!("{},\n    Kept = Table.FirstN({}, 2)\nin\n    Kept", &formula[..end], gridcraft_mquery::quote_identifier(&result));
            let steps = s.run("data.query.parse", json!({"formula":formula})).unwrap()["steps"].as_array().unwrap().clone();
            let transform = steps.last().unwrap()["formula"].clone();
            assert_eq!(describe(&mut s, &formula)["supported"], true, "{formula}");
            let params = json!({"kind":kind.id(),"server":"other","database":if kind == Kind::Sqlite {""} else {"newdb"},"query":sql});
            let changed = edit(&mut s, &formula, params);
            let source = describe(&mut s, &changed);
            assert_eq!(source["query"], sql, "{changed}");
            assert_eq!(source["server"], "other");
            assert!(changed.contains(transform.as_str().unwrap()));
            let table = edit(
                &mut s,
                &changed,
                json!({"kind":kind.id(),"server":"other","database":if kind == Kind::Sqlite {""} else {"newdb"},"schema":"sales","table":"new pedidos"}),
            );
            let source = describe(&mut s, &table);
            assert_eq!(source["table"], "new pedidos", "{table}");
            assert!(source["query"].is_null());
            assert!(table.contains(transform.as_str().unwrap()));
        }
    }
    assert!(s.active().unwrap().wb.queries.is_empty(), "draft source editing never saves a query");
}

#[test]
fn inline_sources_options_comments_and_native_query_by_reference_are_preserved() {
    let mut s = session();
    let formula = "// keep comment\nlet\n Source = Sql.Database(\"host\", \"shop\", [CreateNavigationProperties=false]),\n Orders = Value.NativeQuery(Source, \"SELECT * FROM Orders\", null, [EnableFolding=false]),\n Kept = Table.FirstN(Orders, 3) // keep transformation\nin Kept";
    let updated = edit(&mut s, formula, json!({"kind":"sqlserver","server":"newhost","database":"shop","query":"SELECT id FROM Orders"}));
    assert!(updated.starts_with("// keep comment"));
    assert!(updated.contains("CreateNavigationProperties=false"));
    assert!(updated.contains("EnableFolding=false"));
    assert!(updated.contains("Kept = Table.FirstN(Orders, 3) // keep transformation\nin Kept"));
    assert_eq!(describe(&mut s, &updated)["query"], "SELECT id FROM Orders");
    for formula in [
        "Sql.Database(\"host\",\"shop\",[Query=\"SELECT 1\"])",
        "Sql.Database(\"host\",\"shop\"){[Schema=\"dbo\",Item=\"Orders\"]}[Data]",
        "Value.NativeQuery(PostgreSQL.Database(\"host\",\"shop\"),\"SELECT 1\")",
    ] {
        let changed = edit(&mut s, formula, json!({"kind":"sqlserver","server":"host","database":"shop","query":"SELECT 2"}));
        assert_eq!(describe(&mut s, &changed)["query"], "SELECT 2");
    }
}

#[test]
fn dynamic_parameterized_and_ambiguous_sources_stay_in_the_m_editor() {
    let mut s = session();
    for formula in [
        "let Source = Sql.Database(server, \"shop\", [Query=\"SELECT 1\"]) in Source",
        "Sql.Database(\"host\",\"shop\",[Query=queryText])",
        "Value.NativeQuery(Sql.Database(\"host\",\"shop\"),\"SELECT @id\",[id=1])",
        "let A=Sql.Database(\"a\",\"db\",[Query=\"SELECT 1\"]), B=Sql.Database(\"b\",\"db\",[Query=\"SELECT 2\"]) in Table.Combine({A,B})",
        "let S=Sql.Database(\"a\",\"db\"), A=S{[Schema=\"dbo\",Item=\"A\"]}[Data], B=S{[Schema=\"dbo\",Item=\"B\"]}[Data] in Table.Combine({A,B})",
        "let Sql.Database=(x,y,z)=>#table({},{}), S=Sql.Database(\"x\",\"y\",[Query=\"SELECT 1\"]) in S",
    ] {
        assert_eq!(describe(&mut s, formula)["supported"], false, "{formula}");
        assert!(s.run("data.database.formula", json!({"formula":formula,"kind":"sqlite","path":"unused.db","query":"SELECT 1"})).is_err());
    }
}

#[test]
fn automatic_preview_declines_scripts_and_functions_before_connecting() {
    let mut s = session();
    for sql in [
        "DELETE FROM pedidos",
        "UPDATE pedidos SET id=1",
        "DROP TABLE pedidos",
        "ALTER TABLE pedidos ADD x INT",
        "SELECT * INTO backup FROM pedidos",
        "SELECT 1; DROP TABLE pedidos",
        "SELECT dbo.effect()",
        "SELECT COUNT(*) FROM pedidos",
    ] {
        // This is deliberately not a usable server or login: classification must do no I/O.
        let out = s
            .run("data.database.preview", json!({"kind":"sqlserver","server":"not a server","database":"shop","query":sql,"automatic":true}))
            .unwrap();
        assert_eq!(out, json!({"manualRequired":true}), "{sql}");
    }
    assert!(s.active().unwrap().wb.queries.is_empty());
}
