//! Databases: `Sql.Database`, `Sql.Databases`, `PostgreSQL.Database`, `MySQL.Database`,
//! `SQLite.Database` and `Value.NativeQuery`.
//!
//! A database function returns a navigation table (`Name`, `Data`, `Schema`, `Item`, `Kind`)
//! whose `Data` cells are fetched only when read, so `Source{[Schema = "dbo", Item = "Orders"]}[Data]`
//! reads one table, not the database. With the `Query` option, or through `Value.NativeQuery`, the
//! database runs SQL instead. The host connects ([`crate::Host::database`]) with whatever
//! credentials it keeps: none of that is in the query.

use std::rc::Rc;

use super::*;
use crate::error::{DATA_SOURCE_ERROR, EResult, MError, error, raise};
use crate::host::{DataSource, Database, TableRef};
use crate::interp::Interp;
use crate::value::{Builtin, Lazy, Names, Row, Slot, Table, Type, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Sql.Database", 2, 3, |it, a| database(it, "SqlServer", text(a, 0)?, text(a, 1)?, a.get(2));
    "Sql.Databases", 1, 2, sql_databases;
    "PostgreSQL.Database", 2, 3, |it, a| database(it, "PostgreSQL", text(a, 0)?, text(a, 1)?, a.get(2));
    "MySQL.Database", 2, 3, |it, a| database(it, "MySQL", text(a, 0)?, text(a, 1)?, a.get(2));
    // Not in Power Query, which reaches SQLite only through ODBC drivers.
    "SQLite.Database", 1, 2, |it, a| database(it, "SQLite", text(a, 0)?, Rc::from(""), a.get(1));
    "Value.NativeQuery", 2, 4, native_query;
];

fn ds_error(message: String) -> Rc<MError> {
    Rc::new(MError { reason: Rc::from(DATA_SOURCE_ERROR), message: Rc::from(message), detail: Value::Null })
}

fn host_error(message: String) -> crate::error::EvalError {
    error(DATA_SOURCE_ERROR, message)
}

/// The options record of a database function: `Query` and `HierarchicalNavigation` are used;
/// the others Power Query documents (timeouts, navigation properties…) are accepted and have no
/// effect here.
struct Options {
    query: Option<Rc<str>>,
    hierarchical: bool,
}

fn options(v: Option<&Value>) -> EResult<Options> {
    let mut o = Options { query: None, hierarchical: false };
    match v {
        None | Some(Value::Null) => {}
        Some(Value::Record(r)) => {
            match r.get("Query")? {
                None | Some(Value::Null) => {}
                Some(Value::Text(q)) => o.query = Some(q),
                Some(v) => return mismatch(&v, "the text of a query"),
            }
            match r.get("HierarchicalNavigation")? {
                None | Some(Value::Null) => {}
                Some(Value::Logical(b)) => o.hierarchical = b,
                Some(v) => return mismatch(&v, "true or false"),
            }
        }
        Some(v) => return mismatch(v, "a record of options"),
    }
    Ok(o)
}

fn connect(it: &Interp, kind: &'static str, server: &str, database: &str) -> EResult<Rc<dyn Database>> {
    if server.trim().is_empty() {
        return raise(if kind == "SQLite" { "The database file is missing." } else { "The server is missing." });
    }
    let source = DataSource { kind, server: server.to_string(), database: database.to_string() };
    it.host.database(&source).map_err(host_error)
}

fn database(it: &mut Interp, kind: &'static str, server: Rc<str>, database: Rc<str>, opts: Option<&Value>) -> EResult<Value> {
    let o = options(opts)?;
    let db = connect(it, kind, &server, &database)?;
    if let Some(q) = &o.query {
        return Ok(Value::table(db.query(q).map_err(host_error)?));
    }
    let nav = Rc::new(navigation(&db, o.hierarchical).map_err(host_error)?);
    it.remember_database(&nav, db);
    Ok(Value::Table(nav))
}

fn text_slot(s: &str) -> Slot {
    Slot::Val(Value::text(s))
}

/// The rows of one table, fetched when first read.
fn lazy_table(db: &Rc<dyn Database>, t: &TableRef) -> Slot {
    let (db, schema, name) = (db.clone(), t.schema.clone(), t.name.clone());
    Slot::Lazy(Lazy::new(move || db.table(&schema, &name).map(Value::table).map_err(ds_error)))
}

fn columns(names: &[&str]) -> Names {
    names.iter().map(|n| Rc::from(*n)).collect()
}

/// The navigation table of a database: one row per table and view (`Name` is `schema.table`), or,
/// hierarchical, one row per schema whose `Data` lists its tables.
fn navigation(db: &Rc<dyn Database>, hierarchical: bool) -> Result<Table, String> {
    let tables = db.tables()?;
    let kind = |t: &TableRef| if t.view { "View" } else { "Table" };
    if !hierarchical {
        let rows: Vec<Row> = tables
            .iter()
            .map(|t| {
                let name = if t.schema.is_empty() { t.name.clone() } else { format!("{}.{}", t.schema, t.name) };
                [text_slot(&name), lazy_table(db, t), text_slot(&t.schema), text_slot(&t.name), text_slot(kind(t))].into_iter().collect()
            })
            .collect();
        return Ok(Table {
            columns: columns(&["Name", "Data", "Schema", "Item", "Kind"]),
            types: vec![Type::Text, Type::any_table(), Type::Text, Type::Text, Type::Text].into(),
            rows,
        });
    }
    let mut schemas: Vec<&str> = tables.iter().map(|t| t.schema.as_str()).collect();
    schemas.sort_unstable();
    schemas.dedup();
    let rows: Vec<Row> = schemas
        .iter()
        .map(|s| {
            let inner: Vec<Row> = tables
                .iter()
                .filter(|t| t.schema == *s)
                .map(|t| [text_slot(&t.name), lazy_table(db, t), text_slot(&t.schema), text_slot(&t.name), text_slot(kind(t))].into_iter().collect())
                .collect();
            let inner = Table {
                columns: columns(&["Name", "Data", "Schema", "Item", "Kind"]),
                types: vec![Type::Text, Type::any_table(), Type::Text, Type::Text, Type::Text].into(),
                rows: inner,
            };
            [text_slot(s), Slot::Val(Value::table(inner)), text_slot("Schema")].into_iter().collect()
        })
        .collect();
    Ok(Table { columns: columns(&["Name", "Data", "Kind"]), types: vec![Type::Text, Type::any_table(), Type::Text].into(), rows })
}

/// `Sql.Databases(server)`: the databases of a server; each one's `Data` is its navigation table,
/// read when opened.
fn sql_databases(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let server = text(a, 0)?;
    options(a.get(1))?;
    let db = connect(it, "SqlServer", &server, "")?;
    let names = db.databases().map_err(host_error)?;
    let rows: Vec<Row> = names
        .iter()
        .map(|n| {
            let (db, name) = (db.clone(), n.clone());
            let data = Lazy::new(move || {
                let other = db.open(&name).map_err(ds_error)?;
                navigation(&other, false).map(Value::table).map_err(ds_error)
            });
            [text_slot(n), Slot::Lazy(data), text_slot("Database")].into_iter().collect()
        })
        .collect();
    Ok(Value::table(Table { columns: columns(&["Name", "Data", "Kind"]), types: vec![Type::Text, Type::any_table(), Type::Text].into(), rows }))
}

/// `Value.NativeQuery(database, query, parameters, options)`: SQL run by the database a database
/// function returned.
fn native_query(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Value::Table(target) = arg(a, 0) else { return mismatch(arg(a, 0), "a database") };
    let sql = text(a, 1)?;
    match arg(a, 2) {
        Value::Null => {}
        Value::Record(r) if r.is_empty() => {}
        Value::List(l) if l.is_empty() => {}
        _ => return raise("Parameters of native queries aren't supported yet: write the values into the query."),
    }
    match arg(a, 3) {
        Value::Null | Value::Record(_) => {}
        v => return mismatch(v, "a record of options"),
    }
    let Some(db) = it.database_of(target) else {
        return raise(
            "A native query runs on a database itself: the value Sql.Database, PostgreSQL.Database, MySQL.Database or SQLite.Database returns.",
        );
    };
    Ok(Value::table(db.query(&sql).map_err(host_error)?))
}
