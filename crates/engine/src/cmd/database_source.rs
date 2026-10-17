//! Locate a database source without evaluating M. Only replace its expressions: downstream
//! transformations, step names and query metadata belong to the existing query.

use gridcraft_mquery::{self as mq, ast::Expr, lexer::Tok};
use serde_json::{Value as Json, json};

use crate::datasource::Kind;

type Span = (usize, usize);

pub(super) struct Source {
    pub kind: Kind,
    pub server: String,
    pub database: String,
    pub schema: String,
    pub table: Option<String>,
    pub sql: Option<String>,
    span: Span,
    consumer: Option<(Span, String)>,
    options: Vec<(String, String)>,
    folding: bool,
    native: bool,
}

fn ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

fn text(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Text(s) => Some(s),
        _ => None,
    }
}

fn kind(name: &str) -> Option<Kind> {
    match name {
        "Sql.Database" => Some(Kind::SqlServer),
        "PostgreSQL.Database" => Some(Kind::PostgreSql),
        "MySQL.Database" => Some(Kind::MySql),
        "SQLite.Database" => Some(Kind::Sqlite),
        _ => None,
    }
}

fn database(expr: &Expr, formula: &str, span: Span) -> Option<Source> {
    let Expr::Invoke(function, args) = expr else { return None };
    let kind = kind(ident(function)?)?;
    let server = text(args.first()?)?.to_string();
    let count = if kind == Kind::Sqlite { 1 } else { 2 };
    let database = if count == 2 { text(args.get(1)?)?.to_string() } else { String::new() };
    if args.len() > count + 1 {
        return None;
    }
    let mut sql = None;
    let mut options = Vec::new();
    if let Some(opts) = args.get(count) {
        let Expr::Record(bindings) = opts else { return None };
        for (i, name) in bindings.names.iter().enumerate() {
            let expr = bindings.exprs.get(i)?;
            if &**name == "Query" {
                sql = Some(text(expr)?.to_string());
            } else {
                let (start, end) = *bindings.spans.get(i)?;
                options.push((name.to_string(), formula.get(start..end)?.to_string()));
            }
        }
    }
    Some(Source { kind, server, database, schema: String::new(), table: None, sql, span, consumer: None, options, folding: true, native: false })
}

fn navigation(expr: &Expr) -> Option<(&Expr, String, String)> {
    let Expr::Field { target: Some(target), name, optional: false } = expr else { return None };
    if &**name != "Data" {
        return None;
    }
    let Expr::Item { target, index, optional: false } = &**target else { return None };
    let Expr::Record(fields) = &**index else { return None };
    // Other selector fields could change which table is selected; do not discard them.
    if fields.names.iter().any(|n| !matches!(&**n, "Schema" | "Item")) {
        return None;
    }
    let item = text(fields.exprs.get(fields.index_of("Item")?)?)?.to_string();
    let schema = match fields.index_of("Schema") {
        Some(i) => text(fields.exprs.get(i)?)?.to_string(),
        None => String::new(),
    };
    Some((target, schema, item))
}

fn native(expr: &Expr) -> Option<(&Expr, String, bool)> {
    let Expr::Invoke(function, args) = expr else { return None };
    if ident(function)? != "Value.NativeQuery" || args.len() < 2 || args.len() > 4 {
        return None;
    }
    if args.get(2).is_some_and(|e| !matches!(e, Expr::Null)) {
        return None; // Parameterized SQL must keep its parameters in the advanced editor.
    }
    let mut folding = true;
    if let Some(options) = args.get(3) {
        let Expr::Record(fields) = options else { return None };
        for (name, expr) in fields.names.iter().zip(&fields.exprs) {
            match (&**name, &**expr) {
                ("EnableFolding", Expr::Logical(b)) => folding = *b,
                _ => return None,
            }
        }
    }
    Some((args.first()?, text(args.get(1)?)?.to_string(), folding))
}

/// Conservative by design: ambiguous, dynamic or parameterized sources stay in the M editor.
pub(super) fn describe(formula: &str) -> Option<Source> {
    let tree = mq::parse(formula).ok()?;
    let tokens = mq::lexer::lex(formula).ok()?;
    if tokens.windows(2).filter(|t| matches!(&t[0].tok, Tok::Ident(n) if kind(n).is_some()) && t[1].tok == Tok::LParen).count() != 1 {
        return None;
    }
    let entries: Vec<(&Expr, Span, Option<&str>)> = match &tree {
        Expr::Let(bindings, _) => {
            // A locally redefined connector would not mean the database function.
            if bindings.names.iter().any(|n| kind(n).is_some() || &**n == "Value.NativeQuery") {
                return None;
            }
            bindings.exprs.iter().zip(&bindings.spans).zip(bindings.names.iter()).map(|((e, span), n)| (&**e, *span, Some(&**n))).collect()
        }
        expr => vec![(expr, (0, formula.len()), None)],
    };
    for (expr, span, name) in &entries {
        if let Some(mut source) = database(expr, formula, *span) {
            if source.sql.is_none() {
                let name = (*name)?;
                let mut found = false;
                for (candidate, consumer_span, _) in &entries {
                    if let Some((target, schema, table)) = navigation(candidate)
                        && ident(target) == Some(name)
                    {
                        if found {
                            return None;
                        }
                        source.schema = schema;
                        source.table = Some(table);
                        source.consumer = Some((*consumer_span, name.to_string()));
                        found = true;
                    } else if let Some((target, sql, folding)) = native(candidate)
                        && ident(target) == Some(name)
                    {
                        if found {
                            return None;
                        }
                        source.sql = Some(sql);
                        source.folding = folding;
                        source.native = true;
                        source.consumer = Some((*consumer_span, name.to_string()));
                        found = true;
                    }
                }
                if !found {
                    return None;
                }
            }
            return Some(source);
        }
        if let Some((target, schema, table)) = navigation(expr)
            && let Some(mut source) = database(target, formula, *span)
            && source.sql.is_none()
        {
            source.schema = schema;
            source.table = Some(table);
            return Some(source);
        }
        if let Some((target, sql, folding)) = native(expr)
            && let Some(mut source) = database(target, formula, *span)
            && source.sql.is_none()
        {
            source.sql = Some(sql);
            source.folding = folding;
            source.native = true;
            return Some(source);
        }
    }
    None
}

impl Source {
    pub fn json(&self) -> Json {
        json!({"supported":true,"kind":self.kind.id(),"server":self.server,"database":self.database,"schema":self.schema,"table":self.table,"query":self.sql})
    }

    pub fn replace(
        &self,
        formula: &str,
        kind: Kind,
        server: &str,
        database: &str,
        schema: &str,
        table: Option<&str>,
        sql: Option<&str>,
    ) -> Option<String> {
        // Leaving a connection unchanged also leaves its M formatting and source options intact.
        if self.kind == kind
            && self.server == server
            && self.database == database
            && self.sql.as_deref() == sql
            && (sql.is_some() || (self.schema == schema && self.table.as_deref() == table))
        {
            return Some(formula.to_string());
        }
        let function = match kind {
            Kind::SqlServer => "Sql.Database",
            Kind::PostgreSql => "PostgreSQL.Database",
            Kind::MySql => "MySQL.Database",
            Kind::Sqlite => "SQLite.Database",
        };
        let native = self.native || kind == Kind::PostgreSql || self.consumer.is_some();
        let mut options = if kind == self.kind { self.options.clone() } else { Vec::new() };
        if kind == Kind::MySql && !options.iter().any(|(n, _)| n == "ReturnSingleDatabase") {
            options.push(("ReturnSingleDatabase".into(), "true".into()));
        }
        if let Some(sql) = sql.filter(|_| !native) {
            options.push(("Query".into(), mq::quote_text(sql)));
        }
        let mut args = vec![mq::quote_text(server)];
        if kind != Kind::Sqlite {
            args.push(mq::quote_text(database));
        }
        if !options.is_empty() {
            args.push(format!("[{}]", options.iter().map(|(n, v)| format!("{}={v}", mq::quote_identifier(n))).collect::<Vec<_>>().join(", ")));
        }
        let base = format!("{function}({})", args.join(", "));
        let target = self.consumer.as_ref().map_or_else(|| base.clone(), |(_, name)| mq::quote_identifier(name));
        let result = if let Some(sql) = sql {
            if native { format!("Value.NativeQuery({target}, {}, null, [EnableFolding={}])", mq::quote_text(sql), self.folding) } else { target }
        } else {
            format!("{target}{{[Schema={},Item={}]}}[Data]", mq::quote_text(schema), mq::quote_text(table?))
        };
        let mut edits = match &self.consumer {
            Some((span, _)) => vec![(self.span, base), (*span, result)],
            None => vec![(self.span, result)],
        };
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.0));
        let mut out = formula.to_string();
        for ((start, end), replacement) in edits {
            out.get(start..end)?;
            out.replace_range(start..end, &replacement);
        }
        mq::parse(&out).ok()?;
        Some(out)
    }
}
