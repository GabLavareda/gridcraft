//! Databases for queries: the logins the user gives for each server, the native queries they
//! approved, and the connections behind `Sql.Database`, `PostgreSQL.Database`, `MySQL.Database`
//! and `SQLite.Database`.
//!
//! Logins are kept by the session. They are never written to the workbook, the journal (their
//! `password` param is secret, [`crate::cmd::is_secret_param`]) or any file of GridCraft's, and
//! they never show in command results. A login the user asks to remember goes to the
//! application's [`LoginStore`] (the system's credential store on the desktop) and comes back in
//! the next session. A query names a server and a database, never a login: a workbook from
//! elsewhere reaches only servers the user gave a login for.
//!
//! SQL written into a query (`[Query = "…"]`, `Value.NativeQuery`) can change a database, so it
//! runs only once the user approved that exact text for that database, as Excel asks. Reading
//! tables through a navigation table needs no approval: it only selects.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Mutex;

use gridcraft_db as db;
use gridcraft_mquery as mq;

pub use gridcraft_db::Kind;

/// A login for one server.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub kind: Kind,
    pub host: String,
    /// `None` for the kind's default port.
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub encrypt: bool,
    pub trust_server_certificate: bool,
}

impl std::fmt::Debug for Credential {
    // Never prints the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("encrypt", &self.encrypt)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .finish_non_exhaustive()
    }
}

impl Credential {
    fn port(&self) -> Option<u16> {
        self.port.or(self.kind.default_port())
    }

    fn matches(&self, kind: Kind, host: &str, port: Option<u16>) -> bool {
        self.kind == kind && self.host.eq_ignore_ascii_case(host.trim()) && self.port() == port.or(kind.default_port())
    }

    /// The server as Power Query writes it.
    pub fn server(&self) -> String {
        self.kind.join_server(&self.host, self.port)
    }
}

/// Something a query needed that the session didn't have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    /// A login for this server (none was given, or the server refused it).
    Credentials { kind: Kind, server: String, database: String },
    /// The user's approval to run this SQL on this database.
    Approval { kind: Kind, server: String, database: String, query: String },
}

/// Where remembered logins live between sessions: the system's credential store, which the
/// application provides (Windows Credential Manager, the macOS keychain). The engine keeps
/// nothing on disk itself.
pub trait LoginStore: Send + Sync {
    /// Every login kept.
    fn load(&self) -> Result<Vec<Credential>, String>;
    /// Keeps `c`, replacing the login kept for the same server.
    fn save(&self, c: &Credential) -> Result<(), String>;
    /// Forgets the login kept for this server.
    fn remove(&self, kind: Kind, host: &str, port: Option<u16>) -> Result<(), String>;
}

/// The logins and approvals of a session.
#[derive(Default)]
pub struct DataSources {
    credentials: Vec<Credential>,
    approved: HashSet<String>,
    /// What evaluations were refused for since it was last taken.
    needs: Mutex<Vec<Need>>,
    store: Option<Box<dyn LoginStore>>,
    /// The servers whose login the store keeps (`kind:host:port`).
    remembered: HashSet<String>,
}

/// The key a remembered login goes by: kind, host (lower case) and port.
pub fn login_key(kind: Kind, host: &str, port: Option<u16>) -> String {
    format!("{}:{}:{}", kind.id(), host.trim().to_lowercase(), port.or(kind.default_port()).unwrap_or(0))
}

/// The key of an approval: the database (server normalized) and the SQL.
fn approval_key(kind: Kind, server: &str, database: &str, sql: &str) -> String {
    let server = match kind.split_server(server) {
        Ok((host, port)) => format!("{}:{}", host.to_lowercase(), port.or(kind.default_port()).unwrap_or(0)),
        Err(_) => server.trim().to_lowercase(),
    };
    format!("{}\u{1}{server}\u{1}{}\u{1}{sql}", kind.id(), database.to_lowercase())
}

impl DataSources {
    /// Gives the session a place to remember logins in, and takes the logins it keeps (a login
    /// given in this session already wins). Fails only when the store can't be read.
    pub fn set_store(&mut self, store: Box<dyn LoginStore>) -> Result<usize, String> {
        let kept = store.load();
        self.store = Some(store);
        let kept = kept?;
        let n = kept.len();
        for c in kept {
            self.remembered.insert(login_key(c.kind, &c.host, c.port));
            if self.find(c.kind, &c.host, c.port).is_none() {
                self.credentials.push(c);
            }
        }
        Ok(n)
    }

    /// Whether logins can be remembered from one session to the next.
    pub fn can_remember(&self) -> bool {
        self.store.is_some()
    }

    /// Whether the login for this server is remembered.
    pub fn is_remembered(&self, kind: Kind, host: &str, port: Option<u16>) -> bool {
        self.remembered.contains(&login_key(kind, host, port))
    }

    /// Keeps a login for this session, replacing the one for the same server. Given again
    /// without being remembered, a login that was remembered is forgotten by the store too, so
    /// an old password doesn't come back next time.
    pub fn set(&mut self, c: Credential) {
        let key = login_key(c.kind, &c.host, c.port);
        if self.remembered.remove(&key)
            && let Some(store) = &self.store
        {
            let _ = store.remove(c.kind, &c.host, c.port);
        }
        self.credentials.retain(|x| !x.matches(c.kind, &c.host, c.port));
        self.credentials.push(c);
    }

    /// Keeps a login for this session and in the store, for the next ones.
    pub fn remember(&mut self, c: Credential) -> Result<(), String> {
        let Some(store) = &self.store else { return Err("This version of GridCraft can't remember logins.".into()) };
        store.save(&c)?;
        let key = login_key(c.kind, &c.host, c.port);
        self.credentials.retain(|x| !x.matches(c.kind, &c.host, c.port));
        self.credentials.push(c);
        self.remembered.insert(key);
        Ok(())
    }

    /// Forgets the logins of one kind, of one server of it, or all, remembered ones included;
    /// returns how many.
    pub fn remove(&mut self, kind: Option<Kind>, server: Option<(&str, Option<u16>)>) -> usize {
        let before = self.credentials.len();
        let mut gone = Vec::new();
        self.credentials.retain(|c| {
            let same_kind = kind.is_none_or(|k| c.kind == k);
            let same_server = server.is_none_or(|(host, port)| c.matches(c.kind, host, port));
            if same_kind && same_server {
                gone.push((c.kind, c.host.clone(), c.port));
            }
            !(same_kind && same_server)
        });
        for (k, host, port) in gone {
            if self.remembered.remove(&login_key(k, &host, port))
                && let Some(store) = &self.store
            {
                let _ = store.remove(k, &host, port);
            }
        }
        before - self.credentials.len()
    }

    pub fn credentials(&self) -> &[Credential] {
        &self.credentials
    }

    pub fn find(&self, kind: Kind, host: &str, port: Option<u16>) -> Option<&Credential> {
        self.credentials.iter().find(|c| c.matches(kind, host, port))
    }

    /// Lets this SQL run on this database.
    pub fn approve(&mut self, kind: Kind, server: &str, database: &str, sql: &str) {
        self.approved.insert(approval_key(kind, server, database, sql));
    }

    pub fn is_approved(&self, kind: Kind, server: &str, database: &str, sql: &str) -> bool {
        self.approved.contains(&approval_key(kind, server, database, sql))
    }

    /// What evaluations were refused for since the last call, oldest first, each once.
    pub fn take_needs(&self) -> Vec<Need> {
        self.needs.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
    }

    fn note(&self, needs: Vec<Need>) {
        if let Ok(mut all) = self.needs.lock() {
            for n in needs {
                if !all.contains(&n) {
                    all.push(n);
                }
            }
        }
    }

    /// What it takes to connect to `server`: the login kept for it. Without one, the need is
    /// noted and the message says so.
    pub fn connect_for(&self, kind: Kind, server: &str, database: &str) -> Result<db::Connect, String> {
        if !db::supported(kind) {
            return Err(db::Connection::open(&db::Connect::new(kind, "-", ""))
                .err()
                .map_or_else(|| format!("{kind} isn't available."), |e| e.message));
        }
        if kind == Kind::Sqlite {
            return Ok(db::Connect::new(kind, server.trim(), ""));
        }
        let (host, port) = kind.split_server(server).map_err(|e| e.message)?;
        let Some(cred) = self.find(kind, &host, port) else {
            self.note(vec![Need::Credentials { kind, server: server.to_string(), database: database.to_string() }]);
            return Err(credentials_required(kind, server, database));
        };
        let mut c = db::Connect::new(kind, host, database);
        c.port = port;
        c.username = cred.username.clone();
        c.password = cred.password.clone();
        c.encrypt = cred.encrypt;
        c.trust_server_certificate = cred.trust_server_certificate;
        Ok(c)
    }
}

fn credentials_required(kind: Kind, server: &str, database: &str) -> String {
    let at = if database.is_empty() { server.to_string() } else { format!("{server};{database}") };
    format!("Credentials are required to connect to the {kind} source. (Source at {at}.) Give them with Data › Get Data › From Database.")
}

/// A failed connection as a query reports it; a refused login is noted as a need.
fn describe(e: db::Error, kind: Kind, server: &str, database: &str, needs: &RefCell<Vec<Need>>) -> String {
    match e.kind {
        db::ErrorKind::Login => {
            needs.borrow_mut().push(Need::Credentials { kind, server: server.to_string(), database: database.to_string() });
            format!("The {kind} server at {server} refused the login: {}", e.message)
        }
        db::ErrorKind::Connect | db::ErrorKind::Timeout => format!("Couldn't connect to the {kind} server at {server}: {}", e.message),
        _ => e.message,
    }
}

// ---------------------------------------------------------------- rows to M values

fn col_type(t: db::ColType) -> mq::Type {
    match t {
        db::ColType::Any => mq::Type::Any,
        db::ColType::Logical => mq::Type::Logical,
        db::ColType::Int64 => mq::Type::Number(mq::Facet::Int64),
        db::ColType::Number => mq::Type::Number(mq::Facet::Double),
        db::ColType::Decimal => mq::Type::Number(mq::Facet::Decimal),
        db::ColType::Currency => mq::Type::Number(mq::Facet::Currency),
        db::ColType::Text => mq::Type::Text,
        db::ColType::Binary => mq::Type::Binary,
        db::ColType::Date => mq::Type::Date,
        db::ColType::Time => mq::Type::Time,
        db::ColType::DateTime => mq::Type::DateTime,
        db::ColType::DateTimeZone => mq::Type::DateTimeZone,
        db::ColType::Duration => mq::Type::Duration,
    }
}

fn cell(c: db::Cell) -> mq::Value {
    match c {
        db::Cell::Null => mq::Value::Null,
        db::Cell::Bool(b) => mq::Value::Logical(b),
        db::Cell::Int(i) => mq::Value::Number(i as f64),
        db::Cell::Float(f) => mq::Value::Number(f),
        db::Cell::Decimal(s) => match s.trim().parse::<f64>() {
            Ok(n) => mq::Value::Number(n),
            Err(_) => mq::Value::text(s),
        },
        db::Cell::Text(s) => mq::Value::text(s),
        db::Cell::Bytes(b) => mq::Value::Binary(b.into()),
        db::Cell::Date(d) => mq::Value::Date(d),
        db::Cell::Time(t) => mq::Value::Time(t),
        db::Cell::DateTime(t) => mq::Value::DateTime(t),
        db::Cell::DateTimeZone(t, o) => mq::Value::DateTimeZone(t, o),
        db::Cell::Duration(t) => mq::Value::Duration(t),
    }
}

/// The rows of a read as an M table. Columns without a name, or with a name taken, get one.
pub(crate) fn to_table(rows: db::Rows) -> mq::Table {
    let names = crate::query::unique_names(rows.columns.iter().map(|c| c.name.clone()), false);
    let types: Vec<mq::Type> = rows.columns.iter().map(|c| col_type(c.ty)).collect();
    let rows: Vec<mq::Row> = rows.rows.into_iter().map(|r| r.into_iter().map(|c| mq::Slot::Val(cell(c))).collect()).collect();
    mq::Table { columns: names.into_iter().map(Rc::from).collect(), types: types.into(), rows }
}

fn limits() -> db::Limits {
    db::Limits { max_rows: mq::MAX_ITEMS, max_cells: mq::MAX_CELLS, ..db::Limits::default() }
}

// ---------------------------------------------------------------- the host's side

/// A connection a query holds.
struct Connected {
    conn: RefCell<db::Connection>,
    connect: db::Connect,
    /// The server as the query wrote it.
    server: String,
    approved: Rc<HashSet<String>>,
    needs: Rc<RefCell<Vec<Need>>>,
}

impl Connected {
    fn run<T>(&self, f: impl FnOnce(&mut db::Connection) -> Result<T, db::Error>) -> Result<T, String> {
        let mut conn = self.conn.try_borrow_mut().map_err(|_| "The connection is busy.".to_string())?;
        f(&mut conn).map_err(|e| describe(e, self.connect.kind, &self.server, &self.connect.database, &self.needs))
    }
}

impl mq::Database for Connected {
    fn tables(&self) -> Result<Vec<mq::TableRef>, String> {
        let tables = self.run(|c| c.tables())?;
        Ok(tables.into_iter().map(|t| mq::TableRef { schema: t.schema, name: t.name, view: t.view }).collect())
    }

    fn table(&self, schema: &str, name: &str) -> Result<mq::Table, String> {
        Ok(to_table(self.run(|c| c.table(schema, name, &limits()))?))
    }

    fn query(&self, sql: &str) -> Result<mq::Table, String> {
        let kind = self.connect.kind;
        if !self.approved.contains(&approval_key(kind, &self.server, &self.connect.database, sql)) {
            self.needs.borrow_mut().push(Need::Approval {
                kind,
                server: self.server.clone(),
                database: self.connect.database.clone(),
                query: sql.to_string(),
            });
            return Err(format!(
                "This native database query needs your approval before it runs on {kind} at {}: native queries can change the database.",
                self.server
            ));
        }
        Ok(to_table(self.run(|c| c.query(sql, &limits()))?))
    }

    fn databases(&self) -> Result<Vec<String>, String> {
        self.run(|c| c.databases())
    }

    fn open(&self, database: &str) -> Result<Rc<dyn mq::Database>, String> {
        let mut connect = self.connect.clone();
        connect.database = database.to_string();
        let conn = db::Connection::open(&connect).map_err(|e| describe(e, connect.kind, &self.server, database, &self.needs))?;
        Ok(Rc::new(Connected {
            conn: RefCell::new(conn),
            connect,
            server: self.server.clone(),
            approved: self.approved.clone(),
            needs: self.needs.clone(),
        }))
    }
}

/// The databases one evaluation reaches: each connection is opened once, and what was refused
/// goes back to the session's needs when the evaluation ends.
pub(crate) struct Connector<'a> {
    sources: &'a DataSources,
    approved: Rc<HashSet<String>>,
    open: RefCell<HashMap<mq::DataSource, Rc<dyn mq::Database>>>,
    needs: Rc<RefCell<Vec<Need>>>,
}

impl<'a> Connector<'a> {
    pub(crate) fn new(sources: &'a DataSources) -> Connector<'a> {
        Connector { sources, approved: Rc::new(sources.approved.clone()), open: RefCell::default(), needs: Rc::default() }
    }

    pub(crate) fn database(&self, source: &mq::DataSource) -> Result<Rc<dyn mq::Database>, String> {
        if let Some(db) = self.open.borrow().get(source) {
            return Ok(db.clone());
        }
        let kind = Kind::parse(source.kind).ok_or_else(|| format!("{} databases aren't supported.", source.kind))?;
        let connect = self.sources.connect_for(kind, &source.server, &source.database)?;
        let conn = db::Connection::open(&connect).map_err(|e| describe(e, kind, &source.server, &source.database, &self.needs))?;
        let db: Rc<dyn mq::Database> = Rc::new(Connected {
            conn: RefCell::new(conn),
            connect,
            server: source.server.clone(),
            approved: self.approved.clone(),
            needs: self.needs.clone(),
        });
        self.open.borrow_mut().insert(source.clone(), db.clone());
        Ok(db)
    }
}

impl Drop for Connector<'_> {
    fn drop(&mut self) {
        let needs = std::mem::take(&mut *self.needs.borrow_mut());
        self.sources.note(needs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(kind: Kind, host: &str, port: Option<u16>, user: &str) -> Credential {
        Credential { kind, host: host.into(), port, username: user.into(), password: "pw".into(), encrypt: true, trust_server_certificate: false }
    }

    #[test]
    fn logins_are_found_by_server() {
        let mut s = DataSources::default();
        s.set(login(Kind::SqlServer, "Db.Example", None, "a"));
        s.set(login(Kind::SqlServer, "db.example", Some(1433), "b"));
        s.set(login(Kind::PostgreSql, "db.example", Some(5433), "c"));
        // The default port is the same server: the second login replaced the first.
        assert_eq!(s.credentials().len(), 2);
        assert_eq!(s.find(Kind::SqlServer, "DB.EXAMPLE", None).map(|c| c.username.as_str()), Some("b"));
        assert!(s.find(Kind::PostgreSql, "db.example", None).is_none());
        assert_eq!(s.connect_for(Kind::PostgreSql, "db.example:5433", "shop").map(|c| (c.username, c.port)), Ok(("c".into(), Some(5433))));
        assert!(!format!("{:?}", s.credentials()).contains("pw"));

        let e = s.connect_for(Kind::MySql, "other", "shop").unwrap_err();
        assert!(e.starts_with("Credentials are required to connect to the MySQL source. (Source at other;shop.)"), "{e}");
        assert_eq!(s.take_needs(), vec![Need::Credentials { kind: Kind::MySql, server: "other".into(), database: "shop".into() }]);
        assert!(s.take_needs().is_empty());

        assert_eq!(s.remove(Some(Kind::SqlServer), Some(("db.example", None))), 1);
        assert_eq!(s.remove(None, None), 1);
        assert!(s.credentials().is_empty());
    }

    /// A login store in memory that counts what it is asked.
    #[derive(Default, Clone)]
    struct MemoryStore(std::sync::Arc<Mutex<Vec<Credential>>>);

    impl LoginStore for MemoryStore {
        fn load(&self) -> Result<Vec<Credential>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, c: &Credential) -> Result<(), String> {
            let mut all = self.0.lock().unwrap();
            all.retain(|x| !x.matches(c.kind, &c.host, c.port));
            all.push(c.clone());
            Ok(())
        }
        fn remove(&self, kind: Kind, host: &str, port: Option<u16>) -> Result<(), String> {
            self.0.lock().unwrap().retain(|x| !x.matches(kind, host, port));
            Ok(())
        }
    }

    #[test]
    fn remembered_logins_come_back_in_the_next_session() {
        let store = MemoryStore::default();
        let mut first = DataSources::default();
        assert!(!first.can_remember());
        assert!(first.remember(login(Kind::SqlServer, "db", None, "a")).is_err());
        first.set_store(Box::new(store.clone())).unwrap();
        first.remember(login(Kind::SqlServer, "db", None, "a")).unwrap();
        first.set(login(Kind::PostgreSql, "pg", None, "b"));
        // The next session has the remembered login, not the other.
        let mut next = DataSources::default();
        assert_eq!(next.set_store(Box::new(store.clone())), Ok(1));
        assert_eq!(next.find(Kind::SqlServer, "DB", Some(1433)).map(|c| c.username.as_str()), Some("a"));
        assert!(next.is_remembered(Kind::SqlServer, "db", None));
        assert!(next.find(Kind::PostgreSql, "pg", None).is_none());
        // Given again without remembering, it is forgotten for good.
        next.set(login(Kind::SqlServer, "db", None, "c"));
        assert!(store.0.lock().unwrap().is_empty());
        // Forgetting removes it from the store too.
        next.remember(login(Kind::SqlServer, "db", None, "d")).unwrap();
        assert_eq!(next.remove(Some(Kind::SqlServer), None), 1);
        assert!(store.0.lock().unwrap().is_empty() && !next.is_remembered(Kind::SqlServer, "db", None));
    }

    #[test]
    fn approvals_are_for_one_text_on_one_database() {
        let mut s = DataSources::default();
        s.approve(Kind::SqlServer, "Srv,1433", "Shop", "select 1");
        assert!(s.is_approved(Kind::SqlServer, "srv", "shop", "select 1"));
        assert!(!s.is_approved(Kind::SqlServer, "srv", "shop", "select 1 "));
        assert!(!s.is_approved(Kind::SqlServer, "srv", "other", "select 1"));
        assert!(!s.is_approved(Kind::PostgreSql, "srv", "shop", "select 1"));
    }

    #[test]
    fn rows_become_typed_tables() {
        let rows = db::Rows {
            columns: vec![
                db::Column { name: "id".into(), ty: db::ColType::Int64 },
                db::Column { name: "".into(), ty: db::ColType::Decimal },
                db::Column { name: "id".into(), ty: db::ColType::Text },
            ],
            rows: vec![vec![db::Cell::Int(1), db::Cell::Decimal("2.50".into()), db::Cell::Bytes(vec![1])]],
        };
        let t = to_table(rows);
        assert_eq!(t.columns.iter().map(|c| c.to_string()).collect::<Vec<_>>(), vec!["id", "Column2", "id2"]);
        assert_eq!(mq::render(&mq::Value::table(t)), "#table({\"id\", \"Column2\", \"id2\"}, {{1, 2.5, #binary(1 bytes)}})");
    }
}
