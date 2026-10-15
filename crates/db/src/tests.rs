//! Servers, names and text forms need no database; SQLite files are made here. The servers are
//! tested when the environment names one, each as `host=…;port=…;user=…;password=…;database=…`
//! (plus `encrypt=0` or `trust=1` when needed):
//! `GRIDCRAFT_TEST_SQLSERVER`, `GRIDCRAFT_TEST_POSTGRESQL`, `GRIDCRAFT_TEST_MYSQL`. Each server
//! needs the tables of `tests/seed/*.sql`; without the variable a server test passes untested.

use super::*;
use crate::text::days_from_civil;

#[test]
fn servers_split_as_power_query_writes_them() {
    let ok = |k: Kind, s: &str| k.split_server(s).ok();
    assert_eq!(ok(Kind::SqlServer, "db.example.com,1500"), Some(("db.example.com".into(), Some(1500))));
    assert_eq!(ok(Kind::SqlServer, "tcp:db,1433"), Some(("db".into(), Some(1433))));
    assert_eq!(ok(Kind::SqlServer, "(local)"), Some(("localhost".into(), None)));
    assert_eq!(ok(Kind::SqlServer, "localhost:11433"), Some(("localhost".into(), Some(11433))));
    assert_eq!(ok(Kind::PostgreSql, "pg:5433"), Some(("pg".into(), Some(5433))));
    assert_eq!(ok(Kind::PostgreSql, "[::1]:5433"), Some(("::1".into(), Some(5433))));
    assert_eq!(ok(Kind::MySql, "::1"), Some(("::1".into(), None)));
    assert_eq!(ok(Kind::MySql, "host"), Some(("host".into(), None)));
    for bad in ["", "host:x", "host:0", "host:70000", "[::1"] {
        assert!(Kind::MySql.split_server(bad).is_err(), "{bad}");
    }
    assert!(Kind::SqlServer.split_server("host\\SQLEXPRESS").is_err());
    assert_eq!(Kind::SqlServer.join_server("h", Some(1500)), "h,1500");
    assert_eq!(Kind::SqlServer.join_server("h", Some(1433)), "h");
    assert_eq!(Kind::PostgreSql.join_server("::1", Some(5433)), "[::1]:5433");
    for k in Kind::ALL {
        assert_eq!(Kind::parse(k.id()), Some(k));
        assert_eq!(Kind::parse(k.label()), Some(k));
    }
    assert_eq!(Kind::parse("MS SQL"), Some(Kind::SqlServer));
    assert_eq!(Kind::parse("oracle"), None);
}

#[test]
fn identifiers_are_quoted() {
    assert_eq!(Kind::SqlServer.select_all("dbo", "a]b", Some(5)), "SELECT TOP (5) * FROM [dbo].[a]]b]");
    assert_eq!(Kind::PostgreSql.select_all("public", "a\"b", None), "SELECT * FROM \"public\".\"a\"\"b\"");
    assert_eq!(Kind::MySql.select_all("", "a`b", Some(1)), "SELECT * FROM `a``b` LIMIT 1");
}

#[test]
fn text_forms_of_dates() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(parse_date("2026-01-15"), Some(20468));
    assert_eq!(parse_date("0001-01-01"), Some(-719_162));
    assert_eq!(parse_date("2026-02-29"), None);
    assert_eq!(parse_date("2024-02-29"), Some(19782));
    for bad in ["", "2026-13-01", "2026-1", "x-1-1", "0000-00-00", "2026-01-15 BC"] {
        assert_eq!(parse_date(bad), None, "{bad}");
    }
    assert_eq!(parse_time("10:30:00.5"), Some(((10 * 60 + 30) * 60) * TICKS_PER_SECOND + 5_000_000));
    assert_eq!(parse_time("24:00:00"), None);
    let day = 20468 * TICKS_PER_DAY;
    assert_eq!(parse_datetime("2026-01-15 10:30:00.123"), Some((day + 37_800 * TICKS_PER_SECOND + 1_230_000, None)));
    assert_eq!(parse_datetime("2026-01-15T10:30:00-03"), Some((day + 37_800 * TICKS_PER_SECOND, Some(-180))));
    assert_eq!(parse_datetime("2026-01-15 10:30:00+05:30"), Some((day + 37_800 * TICKS_PER_SECOND, Some(330))));
    assert_eq!(parse_datetime("2026-01-15"), Some((day, None)));
    assert_eq!(parse_datetime("infinity"), None);
    assert_eq!(parse_duration("-01:00:00"), Some(-3600 * TICKS_PER_SECOND));
    assert_eq!(parse_duration("838:59:59"), Some((838 * 3600 + 59 * 60 + 59) * TICKS_PER_SECOND));
    assert_eq!(parse_duration("P1DT2H3M4S"), Some(TICKS_PER_DAY + (2 * 3600 + 3 * 60 + 4) * TICKS_PER_SECOND));
    assert_eq!(parse_duration("PT1H30M"), Some(5400 * TICKS_PER_SECOND));
    assert_eq!(parse_duration("P1X"), None);
    assert_eq!(parse_duration("P9999999999999Y"), None);
}

#[test]
fn reads_stop_at_the_limits() {
    let cols = vec![Column { name: "a".into(), ty: ColType::Text }];
    let mut c = Collector::new(Limits { max_rows: 2, ..Limits::default() }, cols.clone());
    assert!(c.push(vec![Cell::Int(1)]).is_ok() && c.push(vec![Cell::Int(2)]).is_ok());
    assert_eq!(c.push(vec![Cell::Int(3)]).map_err(|e| e.kind), Err(ErrorKind::TooLarge));
    let mut c = Collector::new(Limits { max_bytes: 5, ..Limits::default() }, cols.clone());
    assert_eq!(c.push(vec![Cell::Text("123456".into())]).map_err(|e| e.kind), Err(ErrorKind::TooLarge));
    let mut c = Collector::new(Limits { take: Some(1), ..Limits::default() }, cols);
    for i in 0..5 {
        assert!(c.push(vec![Cell::Int(i), Cell::Int(i)]).is_ok());
    }
    // Rows are cut or padded to the columns.
    assert_eq!(c.out.rows, vec![vec![Cell::Int(0)]]);
}

/// A file in the temporary folder that goes away with the value.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str) -> TempFile {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        TempFile(std::env::temp_dir().join(format!("gridcraft-db-{}-{n}-{name}", std::process::id())))
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
fn sqlite_file() -> TempFile {
    let f = TempFile::new("vendas.db");
    let db = rusqlite::Connection::open(&f.0).unwrap();
    db.execute_batch(
        "CREATE TABLE pedidos (id INTEGER PRIMARY KEY, cliente TEXT NOT NULL, valor DECIMAL(10,2), data DATE, criado DATETIME, ativo BOOLEAN, nota REAL, dados BLOB, livre);
         INSERT INTO pedidos VALUES (1, 'Ana', 120.5, '2026-01-15', '2026-01-15 10:30:00', 1, 4.5, x'0102', 'x'),
                                    (2, 'Bruno', 75, '2026-02-01', 'not a date', 0, NULL, NULL, 2),
                                    (3, 'Célia', 310.99, NULL, '2026-03-20T17:45:10Z', 'true', 3, x'', NULL);
         CREATE VIEW ativos AS SELECT id, cliente FROM pedidos WHERE ativo;
         CREATE TABLE \"odd \"\"name\" (a);",
    )
    .unwrap();
    f
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
#[test]
fn sqlite_files_read_with_their_declared_types() {
    let f = sqlite_file();
    let mut c = Connection::open(&Connect::new(Kind::Sqlite, f.path(), "")).unwrap();
    let tables = c.tables().unwrap();
    let names: Vec<(&str, &str, bool)> = tables.iter().map(|t| (t.schema.as_str(), t.name.as_str(), t.view)).collect();
    assert_eq!(names, vec![("main", "ativos", true), ("main", "odd \"name", false), ("main", "pedidos", false)]);
    assert_eq!(c.databases().unwrap(), vec!["main".to_string()]);

    let rows = c.table("main", "pedidos", &Limits::default()).unwrap();
    let types: Vec<ColType> = rows.columns.iter().map(|c| c.ty).collect();
    use ColType::*;
    assert_eq!(types, vec![Int64, Text, Decimal, Date, DateTime, Logical, Number, Binary, Any]);
    let day = |d: i32| i64::from(d) * TICKS_PER_DAY;
    assert_eq!(
        rows.rows[0],
        vec![
            Cell::Int(1),
            Cell::Text("Ana".into()),
            Cell::Float(120.5),
            Cell::Date(20468),
            Cell::DateTime(day(20468) + 37_800 * TICKS_PER_SECOND),
            Cell::Bool(true),
            Cell::Float(4.5),
            Cell::Bytes(vec![1, 2]),
            Cell::Text("x".into())
        ]
    );
    // Text that isn't a date stays text; an offset makes a datetimezone.
    assert_eq!(rows.rows[1][4], Cell::Text("not a date".into()));
    assert_eq!(rows.rows[2][4], Cell::DateTimeZone(day(20532) + (17 * 3600 + 45 * 60 + 10) * TICKS_PER_SECOND, 0));
    assert_eq!(rows.rows[2][5], Cell::Bool(true));
    assert_eq!(rows.rows[2][6], Cell::Float(3.0));
    assert_eq!(rows.rows[2][1], Cell::Text("Célia".into()));

    let odd = c.table("main", "odd \"name", &Limits::default()).unwrap();
    assert_eq!(odd.columns.len(), 1);
    let preview = c.table("main", "pedidos", &Limits { take: Some(2), ..Limits::default() }).unwrap();
    assert_eq!(preview.rows.len(), 2);
    let q = c.query("SELECT cliente, valor * 2 AS dobro FROM pedidos WHERE id = 3", &Limits::default()).unwrap();
    assert_eq!(q.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["cliente", "dobro"]);
    assert_eq!(q.rows, vec![vec![Cell::Text("Célia".into()), Cell::Float(621.98)]]);
    let e = c.query("SELEC nothing", &Limits::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Query);
}

#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
#[test]
fn sqlite_files_are_never_changed_or_made() {
    let f = sqlite_file();
    let mut c = Connection::open(&Connect::new(Kind::Sqlite, f.path(), "")).unwrap();
    assert!(c.query("DELETE FROM pedidos", &Limits::default()).is_err());
    assert!(c.query("CREATE TABLE x (a)", &Limits::default()).is_err());
    assert_eq!(c.table("main", "pedidos", &Limits::default()).unwrap().rows.len(), 3);

    let missing = TempFile::new("missing.db");
    let e = Connection::open(&Connect::new(Kind::Sqlite, missing.path(), "")).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Connect);
    assert!(!missing.0.exists(), "opening doesn't create the file");

    let not_db = TempFile::new("text.db");
    std::fs::write(&not_db.0, b"just some text, not a database at all").unwrap();
    let e = Connection::open(&Connect::new(Kind::Sqlite, not_db.path(), "")).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Connect);
}

#[test]
fn servers_must_be_named() {
    let e = Connection::open(&Connect::new(Kind::PostgreSql, " ", "db")).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Connect);
}

#[test]
fn passwords_are_never_printed() {
    let mut c = Connect::new(Kind::MySql, "h", "d");
    c.password = "s3cret-value".into();
    assert!(!format!("{c:?}").contains("s3cret"));
}

// ---------------------------------------------------------------- servers

/// The connection the environment names for `kind`, if any.
fn server(kind: Kind, var: &str) -> Option<Connect> {
    let spec = std::env::var(var).ok()?;
    let mut c = Connect::new(kind, "", "");
    for part in spec.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.trim() {
            "host" => c.host = v.to_string(),
            "port" => c.port = v.parse().ok(),
            "user" => c.username = v.to_string(),
            "password" => c.password = v.to_string(),
            "database" => c.database = v.to_string(),
            "encrypt" => c.encrypt = v != "0",
            "trust" => c.trust_server_certificate = v == "1",
            _ => {}
        }
    }
    Some(c)
}

/// What every seeded server holds, read the same way: three orders, clients, a view.
fn check_seeded(kind: Kind, c: &Connect) {
    let mut conn = Connection::open(c).unwrap_or_else(|e| panic!("{kind}: {e}"));
    let tables = conn.tables().unwrap();
    let schema = if kind == Kind::MySql { c.database.as_str() } else { kind.default_schema() };
    let find = |name: &str| tables.iter().find(|t| t.schema == schema && t.name.eq_ignore_ascii_case(name));
    assert!(find("pedidos").is_some_and(|t| !t.view), "{kind}: {tables:?}");
    assert!(find("pedidosativos").or(find("pedidos_ativos")).is_some_and(|t| t.view), "{kind}: {tables:?}");
    assert!(conn.databases().unwrap().iter().any(|d| d == &c.database));

    let name = tables.iter().find(|t| t.name.eq_ignore_ascii_case("pedidos")).map(|t| t.name.clone()).unwrap();
    let rows = conn.table(schema, &name, &Limits::default()).unwrap();
    assert_eq!(rows.rows.len(), 3);
    let col = |n: &str| rows.columns.iter().position(|c| c.name.eq_ignore_ascii_case(n)).unwrap();
    let (id, cliente, valor, data, criado, ativo, nota) =
        (col("id"), col("cliente"), col("valor"), col("data"), col("criado"), col("ativo"), col("nota"));
    assert_eq!(rows.columns[id].ty, ColType::Int64, "{kind}");
    assert_eq!(rows.columns[valor].ty, ColType::Decimal, "{kind}");
    assert_eq!(rows.columns[data].ty, ColType::Date, "{kind}");
    assert_eq!(rows.columns[criado].ty, ColType::DateTime, "{kind}");
    assert_eq!(rows.columns[ativo].ty, ColType::Logical, "{kind}");
    let r0 = &rows.rows[0];
    assert_eq!(r0[id], Cell::Int(1), "{kind}");
    assert_eq!(r0[cliente], Cell::Text("Ana".into()), "{kind}");
    match &r0[valor] {
        Cell::Decimal(s) => assert_eq!(s.parse::<f64>().ok(), Some(120.5), "{kind}"),
        other => panic!("{kind}: {other:?}"),
    }
    assert_eq!(r0[data], Cell::Date(20468), "{kind}");
    assert_eq!(r0[criado], Cell::DateTime(20468 * TICKS_PER_DAY + 37_800 * TICKS_PER_SECOND + 1_230_000), "{kind}");
    assert_eq!(r0[ativo], Cell::Bool(true), "{kind}");
    assert_eq!(r0[nota], Cell::Float(4.5), "{kind}");
    assert_eq!(rows.rows[1][nota], Cell::Null, "{kind}");
    assert_eq!(rows.rows[2][cliente], Cell::Text("Célia".into()), "{kind}");

    let preview = conn.table(schema, &name, &Limits { take: Some(1), ..Limits::default() }).unwrap();
    assert_eq!(preview.rows.len(), 1);
    let sum = conn.query("SELECT COUNT(*) AS n FROM clientes", &Limits::default()).unwrap();
    assert_eq!(sum.rows.first().and_then(|r| r.first()), Some(&Cell::Int(3)), "{kind}");
    assert_eq!(conn.query("SELECT * FROM no_such_table", &Limits::default()).unwrap_err().kind, ErrorKind::Query);
    // The connection is still good after an error.
    assert_eq!(conn.query("SELECT 1 AS one", &Limits::default()).unwrap().rows, vec![vec![Cell::Int(1)]]);

    let mut wrong = c.clone();
    wrong.password.push('x');
    assert_eq!(Connection::open(&wrong).err().map(|e| e.kind), Some(ErrorKind::Login), "{kind}");
}

#[test]
fn sql_server() {
    let Some(c) = server(Kind::SqlServer, "GRIDCRAFT_TEST_SQLSERVER") else { return };
    check_seeded(Kind::SqlServer, &c);
    let mut conn = Connection::open(&c).unwrap();
    let rows = conn.query("SELECT Preco, Fuso, Hora FROM dbo.Pedidos ORDER BY Id", &Limits::default()).unwrap();
    let types: Vec<ColType> = rows.columns.iter().map(|c| c.ty).collect();
    assert_eq!(types, vec![ColType::Currency, ColType::DateTimeZone, ColType::Time]);
    let day = 20468 * TICKS_PER_DAY;
    assert_eq!(rows.rows[0][1], Cell::DateTimeZone(day + 37_800 * TICKS_PER_SECOND, -180));
    assert_eq!(rows.rows[0][2], Cell::Time(37_800 * TICKS_PER_SECOND));
    assert_eq!(rows.rows[2][2], Cell::Time((17 * 3600 + 45 * 60 + 10) * TICKS_PER_SECOND + 5_000_000));
    let other = conn.table("estoque", "Itens", &Limits::default()).unwrap();
    assert_eq!(other.rows.len(), 2);
    // Statements before the rows (SET NOCOUNT) and after them don't hide the rows.
    let r = conn.query("SET NOCOUNT ON; SELECT 7 AS n; SELECT 8 AS m", &Limits::default()).unwrap();
    assert_eq!(r.rows, vec![vec![Cell::Int(7)]]);
}

#[test]
fn postgresql() {
    let Some(c) = server(Kind::PostgreSql, "GRIDCRAFT_TEST_POSTGRESQL") else { return };
    check_seeded(Kind::PostgreSql, &c);
    let mut conn = Connection::open(&c).unwrap();
    let rows = conn.query("SELECT fuso, hora, duracao, dados, codigo FROM pedidos ORDER BY id", &Limits::default()).unwrap();
    let types: Vec<ColType> = rows.columns.iter().map(|c| c.ty).collect();
    assert_eq!(types, vec![ColType::DateTimeZone, ColType::Time, ColType::Duration, ColType::Binary, ColType::Text]);
    let r0 = &rows.rows[0];
    // The server writes the moment in its own time zone (UTC in the test container).
    assert_eq!(r0[0], Cell::DateTimeZone(20468 * TICKS_PER_DAY + 48_600 * TICKS_PER_SECOND, 0));
    assert_eq!(r0[2], Cell::Duration(TICKS_PER_DAY + (2 * 3600 + 3 * 60 + 4) * TICKS_PER_SECOND));
    assert_eq!(r0[3], Cell::Bytes(vec![1, 2, 255]));
    assert_eq!(r0[4], Cell::Text("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".into()));
    assert_eq!(rows.rows[2][3], Cell::Bytes(Vec::new()));
    // Types without a reading of their own come as the server writes them.
    let tags = conn.table("estoque", "itens", &Limits::default()).unwrap();
    assert_eq!(tags.rows[0][2], Cell::Text("{a,b}".into()));
    // A batch can't be prepared: its rows still come, untyped.
    let r = conn.query("SELECT 1 AS n; SELECT 2", &Limits::default()).unwrap();
    assert_eq!(r.rows, vec![vec![Cell::Text("1".into())]]);
}

#[test]
fn mysql() {
    let Some(c) = server(Kind::MySql, "GRIDCRAFT_TEST_MYSQL") else { return };
    check_seeded(Kind::MySql, &c);
    let mut conn = Connection::open(&c).unwrap();
    let rows = conn.query("SELECT hora, dados, grande FROM pedidos ORDER BY id", &Limits::default()).unwrap();
    let types: Vec<ColType> = rows.columns.iter().map(|c| c.ty).collect();
    assert_eq!(types, vec![ColType::Duration, ColType::Binary, ColType::Number]);
    assert_eq!(rows.rows[0][0], Cell::Duration(37_800 * TICKS_PER_SECOND));
    assert_eq!(rows.rows[1][0], Cell::Duration(-3600 * TICKS_PER_SECOND));
    assert_eq!(rows.rows[2][0], Cell::Duration(30 * 3600 * TICKS_PER_SECOND));
    assert_eq!(rows.rows[0][1], Cell::Bytes(vec![1, 2, 255]));
    assert_eq!(rows.rows[0][2], Cell::Float(18_446_744_073_709_551_615.0));
    assert_eq!(rows.rows[1][2], Cell::Int(5));
    let r = conn.query("SELECT 7 AS n; SELECT 8 AS m", &Limits::default());
    // Multiple statements are refused unless the server allows them; either way, nothing breaks.
    if let Ok(r) = r {
        assert_eq!(r.rows, vec![vec![Cell::Int(7)]]);
    }
    assert_eq!(conn.query("SELECT 1 AS one", &Limits::default()).unwrap().rows, vec![vec![Cell::Int(1)]]);
}
