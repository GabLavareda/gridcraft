//! Data › Get Data › From Database: logins for database servers, browsing a database's tables,
//! and queries that read a table or run SQL (SQL Server, PostgreSQL, MySQL/MariaDB, SQLite).
//!
//! A login is kept for the session only ([`crate::datasource`]); the query in the workbook names
//! the server and database, never the login. Passwords are secret params: never journaled.

use gridcraft_db as db;
use gridcraft_mquery as mq;
use serde_json::{Value as Json, json};

use super::*;
use crate::datasource::{Credential, Kind, Need};

/// Rows a table preview returns unless asked otherwise, and at most.
const PREVIEW_ROWS: usize = 50;
const MAX_PREVIEW_ROWS: usize = 1000;
/// Longest SQL accepted, in bytes.
const MAX_SQL: usize = 1 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "data.fromDatabase",
            "From Database",
            ["Data", "Get & Transform Data"],
            None,
            "{kind, server | host + port?, database, username?, password?, encrypt?, trustServerCertificate?, schema? + table | query (SQL, approved by this call), name?, description?, load?: true, at?, sheet?} → a query that reads the table or runs the SQL, loaded like data.query.add: {name, formula, table?, sheet?, range?, rows?, cols?, errors?}",
            has_doc,
            from_database
        ),
        cmd!(
            query "data.database.connect",
            "Connect",
            [],
            None,
            "{kind, server | host + port?, database?, username?, password?, encrypt?, trustServerCertificate?} → {kind, server, database, tables: [{schema, name, view}], databases?: [...] (without a database)}; a login the server accepts is kept for the session",
            always,
            connect
        ),
        cmd!(
            query "data.database.preview",
            "Preview",
            [],
            None,
            "{kind, server | host + port?, database, schema? + table | query, rows?: 50} → {columns: [{name, type}], rows, moreRows, errors} with the session's login",
            always,
            preview
        ),
        cmd!(
            query "data.credentials.set",
            "Data Source Settings",
            [],
            None,
            "{kind, server | host + port?, username, password, encrypt?: true, trustServerCertificate?: false} keeps a login for queries of this session → {kind, server, username}",
            always,
            credentials_set
        ),
        cmd!(
            query "data.credentials.clear",
            "Clear Permissions",
            [],
            None,
            "{kind?, server? | host + port?} forgets the logins of a server, of a kind, or all → {removed}",
            always,
            credentials_clear
        ),
        cmd!(
            query "data.credentials.list",
            "Data Source Settings",
            [],
            None,
            "{} → [{kind, server, username, encrypt, trustServerCertificate}] (never passwords)",
            always,
            credentials_list
        ),
        cmd!(
            query "data.database.approve",
            "Run Native Query",
            [],
            None,
            "{kind, server | host + port?, database?, query} lets queries run this SQL on this database in this session → {approved: true}",
            always,
            approve
        ),
        cmd!(
            query "data.database.needs",
            "Needs",
            [],
            None,
            "{} → what queries were refused for since the last call: [{need: \"credentials\", kind, server, database} | {need: \"approval\", kind, server, database, query}]",
            always,
            needs
        ),
    ]
}

fn other(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}

/// The database a call names: kind, the server as Power Query writes it, and the database.
fn source(cmd: &str, p: &Json) -> Result<(Kind, String, String)> {
    let kind = str_param(p, "kind").and_then(Kind::parse).ok_or_else(|| bad(cmd, "`kind` is sqlserver, postgresql, mysql or sqlite"))?;
    let server = if kind == Kind::Sqlite {
        str_param(p, "path").or_else(|| str_param(p, "server")).ok_or_else(|| bad(cmd, "missing `path` (the SQLite file)"))?.trim().to_string()
    } else if let Some(server) = str_param(p, "server").filter(|s| !s.trim().is_empty()) {
        kind.split_server(server).map_err(|e| bad(cmd, e.message))?;
        server.trim().to_string()
    } else {
        let host = str_param(p, "host").filter(|h| !h.trim().is_empty()).ok_or_else(|| bad(cmd, "missing `server` (or `host` and `port`)"))?;
        let port = match p.get("port") {
            None | Some(Json::Null) => None,
            Some(Json::String(s)) if s.trim().is_empty() => None,
            Some(v) => Some(
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    .and_then(|n| u16::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| bad(cmd, "`port` is a number from 1 to 65535"))?,
            ),
        };
        kind.join_server(host, port)
    };
    if server.is_empty() {
        return Err(bad(cmd, if kind == Kind::Sqlite { "missing `path` (the SQLite file)" } else { "missing `server`" }));
    }
    let database = str_param(p, "database").unwrap_or("").trim().to_string();
    Ok((kind, server, database))
}

/// The login a call gives, if it gives one.
fn login(cmd: &str, p: &Json, kind: Kind, server: &str) -> Result<Option<Credential>> {
    if kind == Kind::Sqlite {
        return Ok(None);
    }
    let (username, password) = (str_param(p, "username"), str_param(p, "password"));
    if username.is_none() && password.is_none() {
        return Ok(None);
    }
    let (host, port) = kind.split_server(server).map_err(|e| bad(cmd, e.message))?;
    Ok(Some(Credential {
        kind,
        host,
        port,
        username: username.unwrap_or("").to_string(),
        password: password.unwrap_or("").to_string(),
        encrypt: bool_param(p, "encrypt").unwrap_or(true),
        trust_server_certificate: bool_param(p, "trustServerCertificate").unwrap_or(false),
    }))
}

fn connect_with(c: &Credential, database: &str) -> db::Connect {
    let mut x = db::Connect::new(c.kind, c.host.clone(), database);
    x.port = c.port;
    x.username = c.username.clone();
    x.password = c.password.clone();
    x.encrypt = c.encrypt;
    x.trust_server_certificate = c.trust_server_certificate;
    x
}

/// Connects with the login the call gives (kept once the server accepts it) or the session's.
fn open(s: &mut Session, cmd: &str, p: &Json) -> Result<(Kind, String, String, db::Connection)> {
    let (kind, server, database) = source(cmd, p)?;
    let given = login(cmd, p, kind, &server)?;
    let connect = match &given {
        Some(c) => connect_with(c, &database),
        None => s.data_sources.connect_for(kind, &server, &database).map_err(other)?,
    };
    let conn = db::Connection::open(&connect).map_err(|e| match e.kind {
        db::ErrorKind::Login => other(format!("The {kind} server at {server} refused the login: {}", e.message)),
        db::ErrorKind::Connect | db::ErrorKind::Timeout => other(format!("Couldn't connect to the {kind} server at {server}: {}", e.message)),
        _ => other(e.message),
    })?;
    if let Some(c) = given {
        s.data_sources.set(c);
    }
    Ok((kind, server, database, conn))
}

fn connect(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.database.connect";
    let (kind, server, database, mut conn) = open(s, C, p)?;
    let tables = conn.tables().map_err(|e| other(e.message))?;
    let list: Vec<Json> = tables.iter().map(|t| json!({"schema": t.schema, "name": t.name, "view": t.view})).collect();
    let mut out = json!({"kind": kind.id(), "server": server, "database": database, "tables": list});
    if database.is_empty() && kind != Kind::Sqlite {
        out["databases"] = json!(conn.databases().map_err(|e| other(e.message))?);
    }
    Ok(out)
}

/// The SQL of a call, checked for size.
fn sql_param<'a>(cmd: &str, p: &'a Json) -> Result<Option<&'a str>> {
    match str_param(p, "query").map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) if q.len() > MAX_SQL => Err(bad(cmd, "the query is too long")),
        q => Ok(q),
    }
}

fn preview(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.database.preview";
    let rows = u32_param(p, "rows").map_or(PREVIEW_ROWS, |n| (n as usize).clamp(1, MAX_PREVIEW_ROWS));
    let sql = sql_param(C, p)?.map(str::to_string);
    let table = str_param(p, "table").map(str::to_string);
    if sql.is_none() && table.is_none() {
        return Err(bad(C, "give a `table` or a `query`"));
    }
    let (kind, _, _, mut conn) = open(s, C, p)?;
    // One row more than shown tells whether there are more.
    let limits = db::Limits { take: Some(rows + 1), ..db::Limits::default() };
    let read = match (&sql, &table) {
        (Some(q), _) => conn.query(q, &limits),
        (None, Some(t)) => conn.table(str_param(p, "schema").unwrap_or(kind.default_schema()), t, &limits),
        (None, None) => return Err(bad(C, "give a `table` or a `query`")),
    }
    .map_err(|e| other(e.message))?;
    let sys = s.doc().map(|d| d.wb.date_system).unwrap_or_default();
    let value = mq::Value::table(crate::datasource::to_table(read));
    let grid = crate::query::grid(sys, &value, rows).map_err(other)?;
    Ok(super::query::grid_json(&grid, sys))
}

/// The formula of a query that reads `table` of the database, or runs `sql` on it, written as
/// Excel writes it.
pub fn database_formula(kind: Kind, server: &str, database: &str, schema: &str, table: Option<&str>, sql: Option<&str>) -> String {
    let (server, database) = (mq::quote_text(server), mq::quote_text(database));
    let source = match kind {
        Kind::SqlServer => format!("Sql.Database({server}, {database}"),
        Kind::PostgreSql => format!("PostgreSQL.Database({server}, {database}"),
        Kind::MySql => format!("MySQL.Database({server}, {database}"),
        Kind::Sqlite => format!("SQLite.Database({server}"),
    };
    if let Some(sql) = sql {
        let q = mq::quote_text(sql);
        return match kind {
            Kind::PostgreSql => format!("let\n    Source = Value.NativeQuery({source}), {q}, null, [EnableFolding=true])\nin\n    Source"),
            Kind::MySql => format!("let\n    Source = {source}, [ReturnSingleDatabase=true, Query={q}])\nin\n    Source"),
            _ => format!("let\n    Source = {source}, [Query={q}])\nin\n    Source"),
        };
    }
    let table = table.unwrap_or("");
    let source = if kind == Kind::MySql { format!("{source}, [ReturnSingleDatabase=true])") } else { format!("{source})") };
    let step = if schema.is_empty() || kind == Kind::Sqlite { table.to_string() } else { format!("{schema}_{table}") };
    let step = mq::quote_identifier(&step);
    format!(
        "let\n    Source = {source},\n    {step} = Source{{[Schema={},Item={}]}}[Data]\nin\n    {step}",
        mq::quote_text(schema),
        mq::quote_text(table)
    )
}

/// A query name from `base` that the workbook doesn't use yet.
fn free_name(wb: &gridcraft_model::Workbook, base: &str) -> String {
    let base: String = base.chars().filter(|c| !c.is_control() && *c != '"').take(70).collect::<String>().trim().to_string();
    let base = if base.is_empty() { "Query".to_string() } else { base };
    if wb.query(&base).is_none() {
        return base;
    }
    (2..).map(|k| format!("{base} ({k})")).find(|n| wb.query(n).is_none()).unwrap_or(base)
}

fn from_database(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.fromDatabase";
    let (kind, server, database) = source(C, p)?;
    let sql = sql_param(C, p)?.map(str::to_string);
    let table = str_param(p, "table").map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    if sql.is_none() && table.is_none() {
        return Err(bad(C, "give a `table` or a `query`"));
    }
    if kind != Kind::Sqlite && database.is_empty() {
        return Err(bad(C, "missing `database`"));
    }
    let schema = match str_param(p, "schema") {
        Some(sc) => sc.trim().to_string(),
        None if kind == Kind::MySql => database.clone(),
        None => kind.default_schema().to_string(),
    };
    if let Some(c) = login(C, p, kind, &server)? {
        s.data_sources.set(c);
    }
    if let Some(q) = &sql {
        // The user wrote this SQL here: it may run.
        s.data_sources.approve(kind, &server, &database, q);
    }
    let formula = database_formula(kind, &server, &database, &schema, table.as_deref(), sql.as_deref());
    let name = match str_param(p, "name") {
        Some(n) => n.to_string(),
        None => free_name(&s.doc()?.wb, table.as_deref().unwrap_or("Query")),
    };
    let mut params = json!({"name": name, "formula": formula});
    for key in ["description", "load", "at", "sheet"] {
        if let Some(v) = p.get(key) {
            params[key] = v.clone();
        }
    }
    let mut out = super::query::add(s, &params)?;
    out["formula"] = json!(formula);
    Ok(out)
}

fn credentials_set(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.credentials.set";
    let (kind, server, _) = source(C, p)?;
    if kind == Kind::Sqlite {
        return Err(bad(C, "SQLite files need no login"));
    }
    if str_param(p, "username").is_none_or(|u| u.trim().is_empty()) {
        return Err(bad(C, "missing `username`"));
    }
    let c = login(C, p, kind, &server)?.ok_or_else(|| bad(C, "missing `username`"))?;
    let out = json!({"kind": kind.id(), "server": c.server(), "username": c.username});
    s.data_sources.set(c);
    Ok(out)
}

fn credentials_clear(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.credentials.clear";
    let kind = match str_param(p, "kind") {
        Some(k) => Some(Kind::parse(k).ok_or_else(|| bad(C, "`kind` is sqlserver, postgresql, mysql or sqlite"))?),
        None => None,
    };
    let server = match (kind, str_param(p, "server").or_else(|| str_param(p, "host"))) {
        (Some(k), Some(_)) => {
            let (_, server, _) = source(C, p)?;
            Some(k.split_server(&server).map_err(|e| bad(C, e.message))?)
        }
        (None, Some(_)) => return Err(bad(C, "give the `kind` of the server too")),
        (_, None) => None,
    };
    let removed = s.data_sources.remove(kind, server.as_ref().map(|(h, p)| (h.as_str(), *p)));
    Ok(json!({"removed": removed}))
}

fn credentials_list(s: &mut Session, _: &Json) -> Result<Json> {
    Ok(s.data_sources
        .credentials()
        .iter()
        .map(|c| {
            json!({"kind": c.kind.id(), "server": c.server(), "username": c.username, "encrypt": c.encrypt, "trustServerCertificate": c.trust_server_certificate})
        })
        .collect())
}

fn approve(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "data.database.approve";
    let (kind, server, database) = source(C, p)?;
    let sql = str_param(p, "query").filter(|q| !q.trim().is_empty()).ok_or_else(|| bad(C, "missing `query`"))?;
    if sql.len() > MAX_SQL {
        return Err(bad(C, "the query is too long"));
    }
    s.data_sources.approve(kind, &server, &database, sql);
    Ok(json!({"approved": true}))
}

fn needs(s: &mut Session, _: &Json) -> Result<Json> {
    Ok(s.data_sources
        .take_needs()
        .into_iter()
        .map(|n| match n {
            Need::Credentials { kind, server, database } => json!({"need": "credentials", "kind": kind.id(), "server": server, "database": database}),
            Need::Approval { kind, server, database, query } => {
                json!({"need": "approval", "kind": kind.id(), "server": server, "database": database, "query": query})
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value as Json, json};

    use crate::Session;
    use crate::core::{CellRef, Value};

    /// A SQLite file in the temporary folder that goes away with the value.
    struct TempDb(std::path::PathBuf);

    impl TempDb {
        fn new() -> TempDb {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("gridcraft-engine-{}-{n}.db", std::process::id()));
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE pedidos (id INTEGER, cliente TEXT, valor DECIMAL(10,2), data DATE);
                 INSERT INTO pedidos VALUES (1, 'Ana', 120.5, '2026-01-15'), (2, 'Bruno', 75, '2026-02-01'), (3, 'Célia', 310.99, '2026-03-20');",
            )
            .unwrap();
            TempDb(path)
        }
        fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
        fn run(&self, sql: &str) {
            rusqlite::Connection::open(&self.0).unwrap().execute_batch(sql).unwrap();
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({})).unwrap();
        s
    }

    fn cell(s: &Session, sheet: &str, a1: &str) -> Value {
        let wb = &s.doc().unwrap().wb;
        wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().value(CellRef::parse(a1).unwrap())
    }

    #[test]
    fn sqlite_tables_load_and_refresh() {
        let f = TempDb::new();
        let mut s = session();
        let tables = s.execute("data.database.connect", json!({"kind": "sqlite", "path": f.path()})).unwrap();
        assert_eq!(tables["tables"], json!([{"schema": "main", "name": "pedidos", "view": false}]));
        let preview = s.execute("data.database.preview", json!({"kind": "sqlite", "path": f.path(), "table": "pedidos", "rows": 2})).unwrap();
        assert_eq!(preview["rows"][1][1], json!("Bruno"));
        assert_eq!(preview["moreRows"], json!(1));

        let out = s.execute("data.fromDatabase", json!({"kind": "sqlite", "path": f.path(), "table": "pedidos"})).unwrap();
        assert_eq!(out["name"], json!("pedidos"));
        assert_eq!(out["rows"], json!(3));
        let formula = out["formula"].as_str().unwrap();
        assert!(formula.contains("SQLite.Database(") && formula.contains("Source{[Schema=\"main\",Item=\"pedidos\"]}[Data]"), "{formula}");
        assert_eq!(cell(&s, "pedidos", "B4"), Value::text("Célia"));
        assert_eq!(cell(&s, "pedidos", "C2"), Value::Number(120.5));
        // Dates load as dates.
        assert_eq!(cell(&s, "pedidos", "D2"), Value::Number(46037.0));

        f.run("INSERT INTO pedidos VALUES (4, 'Davi', 10, '2026-04-01')");
        let r = s.execute("data.query.refresh", json!({"name": "pedidos"})).unwrap();
        assert_eq!(r["refreshed"][0]["rows"], json!(4));
        assert_eq!(cell(&s, "pedidos", "B5"), Value::text("Davi"));
        // A second query of the same table gets a name of its own.
        let again = s.execute("data.fromDatabase", json!({"kind": "sqlite", "path": f.path(), "table": "pedidos", "load": false})).unwrap();
        assert_eq!(again["name"], json!("pedidos (2)"));
    }

    #[test]
    fn native_queries_run_once_approved() {
        let f = TempDb::new();
        let mut s = session();
        // SQL given to From Database is the user's own: it runs.
        let out = s
            .execute(
                "data.fromDatabase",
                json!({"kind": "sqlite", "path": f.path(), "query": "SELECT cliente FROM pedidos WHERE valor > 100", "name": "Big"}),
            )
            .unwrap();
        assert_eq!(out["rows"], json!(2));
        // SQL that came in a workbook waits for approval.
        let formula = format!("SQLite.Database({}, [Query = \"SELECT count(*) AS n FROM pedidos\"])", gridcraft_mquery::quote_text(&f.path()));
        let e = s.execute("data.query.add", json!({"name": "Count", "formula": formula})).unwrap_err().to_string();
        assert!(e.contains("needs your approval"), "{e}");
        let needs = s.execute("data.database.needs", json!({})).unwrap();
        assert_eq!(
            needs,
            json!([{"need": "approval", "kind": "sqlite", "server": f.path(), "database": "", "query": "SELECT count(*) AS n FROM pedidos"}])
        );
        assert_eq!(s.execute("data.database.needs", json!({})).unwrap(), json!([]));
        s.execute("data.database.approve", json!({"kind": "sqlite", "path": f.path(), "query": "SELECT count(*) AS n FROM pedidos"})).unwrap();
        let out = s.execute("data.query.add", json!({"name": "Count", "formula": formula})).unwrap();
        assert_eq!(cell(&s, out["sheet"].as_str().unwrap(), "A2"), Value::Number(3.0));
        // SQLite files are only read.
        let write = format!("SQLite.Database({}, [Query = \"DELETE FROM pedidos\"])", gridcraft_mquery::quote_text(&f.path()));
        s.execute("data.database.approve", json!({"kind": "sqlite", "path": f.path(), "query": "DELETE FROM pedidos"})).unwrap();
        assert!(s.execute("data.query.add", json!({"name": "Gone", "formula": write})).is_err());
        assert_eq!(
            s.execute("data.database.preview", json!({"kind": "sqlite", "path": f.path(), "table": "pedidos"})).unwrap()["rows"]
                .as_array()
                .map(Vec::len),
            Some(3)
        );
    }

    #[test]
    fn logins_stay_in_the_session() {
        let mut s = session();
        let set = json!({"kind": "postgresql", "host": "db.example", "port": 5433, "username": "ana", "password": "s3cret-pw", "encrypt": false});
        assert_eq!(s.execute("data.credentials.set", set).unwrap(), json!({"kind": "postgresql", "server": "db.example:5433", "username": "ana"}));
        let list = s.execute("data.credentials.list", json!({})).unwrap();
        assert_eq!(
            list,
            json!([{"kind": "postgresql", "server": "db.example:5433", "username": "ana", "encrypt": false, "trustServerCertificate": false}])
        );
        // A query names the server, not the login; without a login there is nothing to connect with.
        let e =
            s.execute("data.query.add", json!({"name": "Q", "formula": "PostgreSQL.Database(\"other:5432\", \"shop\")"})).unwrap_err().to_string();
        assert!(e.contains("Credentials are required to connect to the PostgreSQL source. (Source at other:5432;shop.)"), "{e}");
        assert_eq!(s.execute("data.database.needs", json!({})).unwrap()[0]["need"], json!("credentials"));
        // A query from From Database holds no login either, and passwords never reach the journal.
        let _ = s.execute(
            "data.fromDatabase",
            json!({"kind": "mysql", "server": "127.0.0.1:1", "database": "d", "table": "t", "username": "bob", "password": "other-s3cret", "load": false}),
        );
        let q = s.doc().unwrap().wb.query("t").map(|q| q.formula.clone()).unwrap_or_default();
        assert!(q.contains("MySQL.Database(\"127.0.0.1:1\", \"d\", [ReturnSingleDatabase=true])") && !q.contains("bob"), "{q}");
        let journal = serde_json::to_string(&s.journal).unwrap();
        assert!(!journal.contains("s3cret"), "{journal}");
        assert_eq!(s.execute("data.credentials.clear", json!({"kind": "postgresql", "server": "db.example:5433"})).unwrap(), json!({"removed": 1}));
        assert_eq!(s.execute("data.credentials.clear", json!({})).unwrap(), json!({"removed": 1}));
        assert_eq!(s.execute("data.credentials.list", json!({})).unwrap(), json!([]));
    }

    #[test]
    fn calls_are_checked() {
        let mut s = session();
        for p in [
            json!({}),
            json!({"kind": "oracle", "server": "x", "table": "t"}),
            json!({"kind": "sqlserver", "table": "t", "database": "d"}),
            json!({"kind": "sqlserver", "server": "h\\SQLEXPRESS", "database": "d", "table": "t"}),
            json!({"kind": "sqlserver", "host": "h", "port": 70000, "database": "d", "table": "t"}),
            json!({"kind": "sqlserver", "server": "h", "table": "t"}),
            json!({"kind": "sqlserver", "server": "h", "database": "d"}),
        ] {
            assert!(s.execute("data.fromDatabase", p.clone()).is_err(), "{p}");
        }
        assert!(s.execute("data.credentials.set", json!({"kind": "sqlite", "path": "x.db", "username": "a"})).is_err());
        let missing = s.execute("data.database.connect", json!({"kind": "sqlite", "path": "C:/no/such/file.db"})).unwrap_err().to_string();
        assert!(missing.contains("There is no file"), "{missing}");
    }

    #[test]
    fn formulas_are_written_as_excel_writes_them() {
        use crate::datasource::Kind;
        let f = super::database_formula;
        assert_eq!(
            f(Kind::SqlServer, "srv,1500", "Vendas", "dbo", Some("Pedidos"), None),
            "let\n    Source = Sql.Database(\"srv,1500\", \"Vendas\"),\n    dbo_Pedidos = Source{[Schema=\"dbo\",Item=\"Pedidos\"]}[Data]\nin\n    dbo_Pedidos"
        );
        assert_eq!(
            f(Kind::PostgreSql, "pg", "vendas", "public", None, Some("select 1")),
            "let\n    Source = Value.NativeQuery(PostgreSQL.Database(\"pg\", \"vendas\"), \"select 1\", null, [EnableFolding=true])\nin\n    Source"
        );
        assert!(f(Kind::SqlServer, "s", "d", "dbo", Some("Order Details"), None).contains("#\"dbo_Order Details\" = Source"));
        // Whatever the SQL holds, the formula parses.
        let odd = f(Kind::MySql, "m", "d", "d", None, Some("select \"x\" -- \r\n from t"));
        assert!(gridcraft_mquery::parse(&odd).is_ok(), "{odd}");
    }

    // ------------------------------------------------------------ servers

    /// The server the environment names for a kind (see gridcraft-db's tests), as call params.
    fn server(var: &str, kind: &str) -> Option<Json> {
        let spec = std::env::var(var).ok()?;
        let mut p = json!({"kind": kind});
        for part in spec.split(';') {
            let (k, v) = part.split_once('=')?;
            match k {
                "host" => p["host"] = json!(v),
                "port" => p["port"] = json!(v),
                "user" => p["username"] = json!(v),
                "password" => p["password"] = json!(v),
                "database" => p["database"] = json!(v),
                "encrypt" => p["encrypt"] = json!(v != "0"),
                "trust" => p["trustServerCertificate"] = json!(v == "1"),
                _ => {}
            }
        }
        Some(p)
    }

    fn with(p: &Json, extra: Json) -> Json {
        let mut p = p.clone();
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        p
    }

    /// Browses, loads, refreshes and runs SQL on a seeded server (gridcraft-db `tests/seed`).
    fn check_server(var: &str, kind: &str, table: &str) {
        let Some(p) = server(var, kind) else { return };
        let mut s = session();
        let tables = s.execute("data.database.connect", p.clone()).unwrap();
        assert!(tables["tables"].as_array().unwrap().iter().any(|t| t["name"] == json!(table)), "{tables}");
        // The login is kept: what follows doesn't give it again.
        let mut bare = p.clone();
        bare.as_object_mut().unwrap().retain(|k, _| !matches!(k.as_str(), "username" | "password" | "encrypt" | "trustServerCertificate"));
        let out = s.execute("data.fromDatabase", with(&bare, json!({"table": table, "name": "Pedidos"}))).unwrap();
        assert_eq!(out["rows"], json!(3), "{out}");
        assert_eq!(cell(&s, "Pedidos", "B4"), Value::text("Célia"));
        s.execute("data.query.refresh", json!({"name": "Pedidos"})).unwrap();
        let sql = format!("SELECT COUNT(*) AS n FROM {table}");
        let out = s.execute("data.fromDatabase", with(&bare, json!({"query": sql, "name": "Total"}))).unwrap();
        assert_eq!(cell(&s, out["sheet"].as_str().unwrap(), "A2"), Value::Number(3.0));
        let wrong = s.execute("data.database.connect", with(&p, json!({"password": "wrong-password"}))).unwrap_err().to_string();
        assert!(wrong.contains("refused the login"), "{wrong}");
        // The refused login didn't replace the good one.
        s.execute("data.query.refresh", json!({})).unwrap();
        assert!(s.execute("data.query.refresh", json!({})).unwrap()["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn sql_server() {
        check_server("GRIDCRAFT_TEST_SQLSERVER", "sqlserver", "Pedidos");
    }

    #[test]
    fn postgresql() {
        check_server("GRIDCRAFT_TEST_POSTGRESQL", "postgresql", "pedidos");
    }

    #[test]
    fn mysql() {
        check_server("GRIDCRAFT_TEST_MYSQL", "mysql", "pedidos");
    }
}
