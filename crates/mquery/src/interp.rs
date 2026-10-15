//! The evaluator.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::ast::*;
use crate::culture::Culture;
use crate::env::{Env, Frame, Thunk};
use crate::error::{EResult, EXPRESSION_ERROR, Error, EvalError, Fatal, MError, error, raise};
use crate::host::{Database, Host};
use crate::ops;
use crate::parser;
use crate::value::{Closure, Facet, Field, Function, Names, Record, Row, Slot, Table, Type, Value, values_equal};

// Sizes are capped per value. The evaluator computes eagerly, so a runaway query would otherwise
// take memory until the allocator gives up, which ends the process. The web build has a 32-bit
// address space and gets smaller caps.

/// Most items in a list and most rows in a table.
#[cfg(not(target_pointer_width = "32"))]
pub const MAX_ITEMS: usize = 1 << 24;
#[cfg(target_pointer_width = "32")]
pub const MAX_ITEMS: usize = 1 << 22;
/// Most cells in a table.
#[cfg(not(target_pointer_width = "32"))]
pub const MAX_CELLS: usize = 1 << 26;
#[cfg(target_pointer_width = "32")]
pub const MAX_CELLS: usize = 1 << 24;
/// Longest text, in bytes.
#[cfg(not(target_pointer_width = "32"))]
pub const MAX_TEXT: usize = 128 << 20;
#[cfg(target_pointer_width = "32")]
pub const MAX_TEXT: usize = 32 << 20;
/// Deepest nesting of evaluations (expressions inside expressions, calls inside calls) unless
/// the host raises it. Each level takes about 2 KiB of stack in a development build, so this
/// fits a 1 MiB stack (the main thread on Windows, the whole stack on the web) with room for the
/// caller. A recursive function gets about one call per four or five levels.
pub const DEFAULT_MAX_DEPTH: usize = 200;

pub struct Interp<'h> {
    pub(crate) host: &'h dyn Host,
    globals: HashMap<&'static str, Value>,
    /// The scope of the queries, which every expression is evaluated in.
    root: Rc<Env>,
    depth: usize,
    max_depth: usize,
    ticks: u32,
    /// Scopes that functions captured: reset when the evaluator goes, since a scope and a
    /// function in it can own each other.
    captured: Vec<Weak<Env>>,
    captured_cap: usize,
    /// State of the generator behind `Number.Random`; zero until first used.
    rng: u64,
    /// The navigation tables database functions returned, with their databases, for
    /// `Value.NativeQuery`.
    databases: Vec<(Weak<Table>, Rc<dyn Database>)>,
}

pub(crate) fn too_large<T>() -> EResult<T> {
    Err(EvalError::Fatal(Fatal::TooLarge))
}

/// Fails when a list or table would have more than the allowed number of items.
pub(crate) fn check_len(n: usize) -> EResult<()> {
    if n > MAX_ITEMS { too_large() } else { Ok(()) }
}

pub(crate) fn prim_type(p: Prim) -> Type {
    match p {
        Prim::Any => Type::Any,
        Prim::AnyNonNull => Type::AnyNonNull,
        Prim::Binary => Type::Binary,
        Prim::Date => Type::Date,
        Prim::DateTime => Type::DateTime,
        Prim::DateTimeZone => Type::DateTimeZone,
        Prim::Duration => Type::Duration,
        Prim::Function => Type::Function,
        Prim::List => Type::any_list(),
        Prim::Logical => Type::Logical,
        Prim::None => Type::None,
        Prim::Null => Type::Null,
        Prim::Number => Type::Number(Facet::Double),
        Prim::Record => Type::any_record(),
        Prim::Table => Type::any_table(),
        Prim::Text => Type::Text,
        Prim::Time => Type::Time,
        Prim::Type => Type::Type,
    }
}

/// Whether `v` fits a parameter or return type, when that can be told without evaluating the
/// type expression.
fn fits(t: &TypeExpr, v: &Value) -> Option<bool> {
    match t {
        TypeExpr::Prim(p) => Some(prim_type(*p).accepts(v)),
        TypeExpr::Nullable(inner) => {
            if v.is_null() {
                Some(true)
            } else {
                fits(inner, v)
            }
        }
        TypeExpr::List(_) => Some(matches!(v, Value::List(_))),
        TypeExpr::Record(..) => Some(matches!(v, Value::Record(_))),
        TypeExpr::Table(_) | TypeExpr::TableOf(_) => Some(matches!(v, Value::Table(_))),
        TypeExpr::Function => Some(matches!(v, Value::Function(_))),
        TypeExpr::Expr(_) => None,
    }
}

fn type_expr_name(t: &TypeExpr) -> &'static str {
    match t {
        TypeExpr::Prim(p) => prim_type(*p).name(),
        TypeExpr::Nullable(inner) => type_expr_name(inner),
        TypeExpr::List(_) => "List",
        TypeExpr::Record(..) => "Record",
        TypeExpr::Table(_) | TypeExpr::TableOf(_) => "Table",
        TypeExpr::Function => "Function",
        TypeExpr::Expr(_) => "the declared type",
    }
}

pub(crate) fn cannot_convert<T>(v: &Value, to: &str) -> EResult<T> {
    raise(format!("A value of type {} can't be converted to type {to}.", v.kind()))
}

/// The record `try` gives for an error, and `catch` receives.
pub(crate) fn error_record(e: &MError) -> Value {
    Value::record_of(vec![
        ("Reason", Value::Text(e.reason.clone())),
        ("Message", Value::Text(e.message.clone())),
        ("Detail", e.detail.clone()),
        ("Message.Format", Value::Null),
        ("Message.Parameters", Value::Null),
    ])
}

/// The error that `error value` raises.
pub(crate) fn error_from_value(v: Value) -> EvalError {
    match &v {
        Value::Text(s) => error(EXPRESSION_ERROR, s.to_string()),
        Value::Record(r) => {
            let text = |name: &str| match r.slot(name) {
                Some(Slot::Val(Value::Text(s))) => Some(s.clone()),
                _ => None,
            };
            let detail = match r.slot("Detail") {
                Some(Slot::Val(d)) => d.clone(),
                _ => Value::Null,
            };
            EvalError::Raised(Rc::new(MError {
                reason: text("Reason").unwrap_or_else(|| Rc::from(EXPRESSION_ERROR)),
                message: text("Message").unwrap_or_else(|| Rc::from("")),
                detail,
            }))
        }
        _ => EvalError::Raised(Rc::new(MError { reason: Rc::from(EXPRESSION_ERROR), message: Rc::from("An error was raised."), detail: v })),
    }
}

impl<'h> Interp<'h> {
    pub fn new(host: &'h dyn Host) -> Interp<'h> {
        let mut globals = HashMap::new();
        for b in crate::stdlib::builtins() {
            globals.insert(b.name, Value::Function(Rc::new(Function::Builtin(b))));
        }
        for (name, v) in crate::stdlib::constants() {
            globals.insert(name, v);
        }
        globals.insert("Culture.Current", Value::text(host.culture()));
        let root = Env::lazy(&Rc::new(Bindings { names: Rc::from(Vec::new()), exprs: Vec::new(), spans: Vec::new() }), None);
        Interp {
            host,
            globals,
            root,
            depth: 0,
            max_depth: DEFAULT_MAX_DEPTH,
            ticks: 0,
            captured: Vec::new(),
            captured_cap: 1024,
            rng: 0,
            databases: Vec::new(),
        }
    }

    /// Allows evaluations to nest `depth` levels deep instead of [`DEFAULT_MAX_DEPTH`]. Only for
    /// a caller that runs the evaluator on a thread with a stack to match: about 2 KiB a level
    /// in a development build, several times that without optimization.
    pub fn set_max_depth(&mut self, depth: usize) {
        self.max_depth = depth.max(16);
    }

    /// Sets the queries of the workbook, by name and formula. They can refer to each other; a
    /// query that doesn't parse fails only the queries that use it.
    pub fn set_queries(&mut self, queries: &[(&str, &str)]) {
        let mut names: Vec<Rc<str>> = Vec::new();
        let mut exprs = Vec::new();
        let mut spans = Vec::new();
        for (name, formula) in queries {
            if names.iter().any(|n| &**n == *name) {
                continue;
            }
            let e = match parser::parse(formula) {
                Ok(e) => e,
                Err(pe) => {
                    let (line, col) = pe.line_col(formula);
                    Expr::Failed(Rc::new(MError {
                        reason: Rc::from(EXPRESSION_ERROR),
                        message: Rc::from(format!("The query '{name}' has a syntax error at line {line}, column {col}: {}", pe.message)),
                        detail: Value::Null,
                    }))
                }
            };
            names.push(Rc::from(*name));
            exprs.push(Rc::new(e));
            spans.push((0, formula.len()));
        }
        self.retire_root();
        self.root = Env::lazy(&Rc::new(Bindings { names: names.into(), exprs, spans }), None);
    }

    fn retire_root(&mut self) {
        self.root.reset();
    }

    /// Evaluates the query `name` set with [`Interp::set_queries`].
    pub fn eval_query(&mut self, name: &str) -> EResult<Value> {
        let root = self.root.clone();
        let Frame::Lazy { decl, .. } = &root.frame else { return raise("There are no queries.") };
        match decl.index_of(name) {
            Some(i) => self.force(&root, i),
            None => raise(format!("There is no query named '{name}'.")),
        }
    }

    /// Parses and evaluates an expression; it can refer to the queries.
    pub fn eval_text(&mut self, src: &str) -> Result<Value, Error> {
        let e = parser::parse(src)?;
        let root = self.root.clone();
        Ok(self.eval(&e, &root)?)
    }

    /// Evaluates one step of a query: a member of its top-level `let`.
    pub fn eval_step(&mut self, src: &str, step: &str) -> Result<Value, Error> {
        let e = parser::parse(src)?;
        let root = self.root.clone();
        let Expr::Let(b, _) = &e else { return Err(Error::Eval(error(EXPRESSION_ERROR, "The query has no steps."))) };
        let Some(i) = b.index_of(step) else { return Err(Error::Eval(error(EXPRESSION_ERROR, format!("The query has no step named '{step}'.")))) };
        let scope = Env::lazy(b, Some(root));
        let r = self.force(&scope, i);
        self.retire(scope);
        Ok(r?)
    }

    /// The culture called `name`, or the host's when no name is given.
    pub fn culture(&self, name: Option<&str>) -> &'static Culture {
        Culture::find(name.unwrap_or_else(|| self.host.culture()))
    }

    /// The current moment, from the host.
    pub(crate) fn now(&self) -> EResult<crate::host::Now> {
        match self.host.now() {
            Some(n) => Ok(n),
            None => raise("The current date and time aren't available here."),
        }
    }

    /// The local offset from UTC in minutes; zero when the host has no clock.
    pub(crate) fn local_offset(&self) -> i32 {
        self.host.now().map_or(0, |n| n.offset_minutes)
    }

    /// A pseudo-random number from 0 up to, but not including, 1 (xorshift), seeded from the
    /// host's clock when it has one.
    pub(crate) fn random(&mut self) -> f64 {
        if self.rng == 0 {
            self.rng = self.host.now().map_or(0, |n| n.local_ticks as u64) ^ 0x9e37_79b9_7f4a_7c15;
            if self.rng == 0 {
                self.rng = 0x9e37_79b9_7f4a_7c15;
            }
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Remembers which database a navigation table came from.
    pub(crate) fn remember_database(&mut self, nav: &Rc<Table>, db: Rc<dyn Database>) {
        self.databases.retain(|(t, _)| t.strong_count() > 0);
        self.databases.push((Rc::downgrade(nav), db));
    }

    /// The database a navigation table came from.
    pub(crate) fn database_of(&self, nav: &Rc<Table>) -> Option<Rc<dyn Database>> {
        self.databases.iter().find(|(t, _)| std::ptr::eq(t.as_ptr(), Rc::as_ptr(nav))).map(|(_, db)| db.clone())
    }

    /// The library, as the record `#shared`.
    fn shared(&self) -> Value {
        let mut entries: Vec<(&&'static str, &Value)> = self.globals.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let names: Names = entries.iter().map(|(n, _)| Rc::from(**n)).collect();
        let values: Row = entries.iter().map(|(_, v)| Slot::Val((*v).clone())).collect();
        Value::record(names, values)
    }

    fn tick(&mut self) -> EResult<()> {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks & 0x3fff == 0 && self.host.cancelled() {
            return Err(EvalError::Fatal(Fatal::Cancelled));
        }
        Ok(())
    }

    /// Remembers a scope that outlives its expression (something captured it), to reset it at
    /// the end.
    fn retire(&mut self, scope: Rc<Env>) {
        if Rc::strong_count(&scope) > 1 {
            self.captured.push(Rc::downgrade(&scope));
            if self.captured.len() > self.captured_cap {
                self.captured.retain(|w| w.strong_count() > 0);
                self.captured_cap = (self.captured.len() * 2).max(1024);
            }
        }
    }

    // ------------------------------------------------------------ names

    fn lookup(&mut self, name: &str, mut skip: u8, env: &Rc<Env>) -> EResult<Value> {
        let mut cur = Some(env);
        while let Some(e) = cur {
            match &e.frame {
                Frame::Args { decl, values } => {
                    if let Some(i) = decl.params.iter().position(|p| &*p.name == name) {
                        if skip == 0 {
                            return Ok(values.get(i).cloned().unwrap_or(Value::Null));
                        }
                        skip -= 1;
                    }
                }
                Frame::Lazy { decl, .. } => {
                    if let Some(i) = decl.index_of(name) {
                        if skip == 0 {
                            return self.force(e, i);
                        }
                        skip -= 1;
                    }
                }
                Frame::Fields(r) => {
                    if let Some(s) = r.slot(name) {
                        if skip == 0 {
                            return s.get();
                        }
                        skip -= 1;
                    }
                }
                Frame::Barrier => return unknown_name(name),
            }
            cur = e.parent.as_ref();
        }
        match self.globals.get(name) {
            Some(v) => Ok(v.clone()),
            None if name == "#shared" => Ok(self.shared()),
            None if name == "#sections" => Ok(Value::record_of(Vec::new())),
            None => unknown_name(name),
        }
    }

    /// The value of member `i` of a lazy scope, evaluating it the first time.
    fn force(&mut self, env: &Rc<Env>, i: usize) -> EResult<Value> {
        let Frame::Lazy { decl, cells } = &env.frame else { return raise("This scope has no members.") };
        let Some(cell) = cells.get(i) else { return raise("This scope has no such member.") };
        match &*cell.borrow() {
            Thunk::Done(v) => return Ok(v.clone()),
            Thunk::Failed(e) => return Err(EvalError::Raised(e.clone())),
            Thunk::SelfFn(d) => return Ok(Value::Function(Rc::new(Function::Closure(Closure { decl: d.clone(), env: env.clone() })))),
            Thunk::Running => {
                let name = decl.names.get(i).map_or("", |n| &**n);
                return raise(format!("'{name}' refers to itself: a cyclic reference was found while evaluating it."));
            }
            Thunk::Pending => {}
        }
        let Some(expr) = decl.exprs.get(i).cloned() else { return raise("This scope has no such member.") };
        cell.replace(Thunk::Running);
        let r = self.eval(&expr, env);
        let state = match &r {
            Ok(Value::Function(f)) => match &**f {
                Function::Closure(c) if Rc::ptr_eq(&c.env, env) => Thunk::SelfFn(c.decl.clone()),
                _ => Thunk::Done(Value::Function(f.clone())),
            },
            Ok(v) => Thunk::Done(v.clone()),
            Err(EvalError::Raised(e)) => Thunk::Failed(e.clone()),
            Err(EvalError::Fatal(_)) => Thunk::Pending,
        };
        cell.replace(state);
        r
    }

    // ------------------------------------------------------------ expressions

    pub(crate) fn eval(&mut self, e: &Expr, env: &Rc<Env>) -> EResult<Value> {
        self.depth += 1;
        let r = if self.depth > self.max_depth { Err(EvalError::Fatal(Fatal::TooDeep)) } else { self.eval_inner(e, env) };
        self.depth -= 1;
        r
    }

    fn eval_inner(&mut self, e: &Expr, env: &Rc<Env>) -> EResult<Value> {
        match e {
            Expr::Null => Ok(Value::Null),
            Expr::Logical(b) => Ok(Value::Logical(*b)),
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Text(s) => Ok(Value::Text(s.clone())),
            Expr::Ident { name, skip, .. } => self.lookup(name, *skip, env),
            Expr::List(items) => self.eval_list(items, env),
            Expr::Record(b) => self.eval_record(b, env),
            Expr::Unary(op, a) => {
                let v = self.eval(a, env)?;
                ops::unary(*op, v)
            }
            Expr::Chain(first, rest) => self.eval_chain(first, rest, env),
            Expr::If(c, a, b) => match self.eval(c, env)? {
                Value::Logical(true) => self.eval(a, env),
                Value::Logical(false) => self.eval(b, env),
                v => cannot_convert(&v, "Logical"),
            },
            Expr::Let(b, body) => {
                let scope = Env::lazy(b, Some(env.clone()));
                let r = self.eval(body, &scope);
                self.retire(scope);
                r
            }
            Expr::Function(decl) => Ok(Value::Function(Rc::new(Function::Closure(Closure { decl: decl.clone(), env: env.clone() })))),
            Expr::Invoke(f, args) => self.eval_invoke(f, args, env),
            Expr::Field { target, name, optional } => {
                let t = self.eval_target(target.as_deref(), env)?;
                field(&t, name, *optional)
            }
            Expr::Project { target, names, optional } => {
                let t = self.eval_target(target.as_deref(), env)?;
                project(&t, names, *optional)
            }
            Expr::Item { target, index, optional } => {
                let t = self.eval(target, env)?;
                let i = self.eval(index, env)?;
                item(&t, &i, *optional)
            }
            Expr::Try(a, handler) => self.eval_try(a, handler, env),
            Expr::Raise(a) => {
                let v = self.eval(a, env)?;
                Err(error_from_value(v))
            }
            Expr::As(a, t) => {
                let v = self.eval(a, env)?;
                let ty = self.eval_type(t, env)?;
                if ty.accepts(&v) { Ok(v) } else { cannot_convert(&v, ty.name()) }
            }
            Expr::Is(a, t) => {
                let v = self.eval(a, env)?;
                let ty = self.eval_type(t, env)?;
                Ok(Value::Logical(ty.accepts(&v)))
            }
            Expr::Type(t) => Ok(Value::Type(self.eval_type(t, env)?)),
            Expr::NotImplemented => raise("This part of the query isn't written yet (...)."),
            Expr::Failed(e) => Err(EvalError::Raised(e.clone())),
        }
    }

    /// The target of a field access; none means the implicit `_`.
    fn eval_target(&mut self, target: Option<&Expr>, env: &Rc<Env>) -> EResult<Value> {
        match target {
            Some(t) => self.eval(t, env),
            None => self.lookup("_", 0, env),
        }
    }

    #[inline(never)]
    fn eval_list(&mut self, items: &[ListItem], env: &Rc<Env>) -> EResult<Value> {
        let mut out: Vec<Slot> = Vec::with_capacity(items.len());
        for it in items {
            match it {
                ListItem::One(e) => out.push(Slot::capture(self.eval(e, env))?),
                ListItem::Range(a, b) => {
                    let (a, b) = (self.eval(a, env)?, self.eval(b, env)?);
                    match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => {
                            if x.fract() != 0.0 || y.fract() != 0.0 || !x.is_finite() || !y.is_finite() {
                                return raise("A list range needs whole numbers.");
                            }
                            if y >= x {
                                check_len(out.len().saturating_add((y - x) as usize).saturating_add(1))?;
                                let mut n = *x;
                                while n <= *y {
                                    out.push(Slot::Val(Value::Number(n)));
                                    n += 1.0;
                                }
                            }
                        }
                        (Value::Text(x), Value::Text(y)) => {
                            let (mut xs, mut ys) = (x.chars(), y.chars());
                            let (Some(p), None, Some(q), None) = (xs.next(), xs.next(), ys.next(), ys.next()) else {
                                return raise("A list range of text needs single characters.");
                            };
                            for c in p..=q {
                                out.push(Slot::Val(Value::text(c.to_string())));
                            }
                        }
                        _ => return raise(format!("A list range can't go from a value of type {} to a value of type {}.", a.kind(), b.kind())),
                    }
                }
            }
        }
        Ok(Value::list(out))
    }

    #[inline(never)]
    fn eval_record(&mut self, b: &Rc<Bindings>, env: &Rc<Env>) -> EResult<Value> {
        let scope = Env::lazy(b, Some(env.clone()));
        let mut values = Vec::with_capacity(b.names.len());
        for i in 0..b.names.len() {
            values.push(Slot::capture(self.force(&scope, i))?);
        }
        self.retire(scope);
        Ok(Value::record(b.names.clone(), values.into()))
    }

    #[inline(never)]
    fn eval_chain(&mut self, first: &Expr, rest: &[(BinOp, Expr)], env: &Rc<Env>) -> EResult<Value> {
        let mut acc = self.eval(first, env)?;
        for (op, rhs) in rest {
            acc = match op {
                BinOp::And | BinOp::Or => {
                    let stop = *op == BinOp::Or;
                    match acc {
                        Value::Logical(b) if b == stop => Value::Logical(b),
                        Value::Logical(_) => match self.eval(rhs, env)? {
                            r @ (Value::Logical(_) | Value::Null) => r,
                            r => return cannot_convert(&r, "Logical"),
                        },
                        Value::Null => match self.eval(rhs, env)? {
                            Value::Logical(b) if b == stop => Value::Logical(b),
                            Value::Logical(_) | Value::Null => Value::Null,
                            r => return cannot_convert(&r, "Logical"),
                        },
                        l => return cannot_convert(&l, "Logical"),
                    }
                }
                BinOp::Coalesce => {
                    if acc.is_null() {
                        self.eval(rhs, env)?
                    } else {
                        acc
                    }
                }
                // Metadata isn't kept.
                BinOp::Meta => acc,
                _ => {
                    let r = self.eval(rhs, env)?;
                    ops::binary(*op, &acc, &r)?
                }
            };
        }
        Ok(acc)
    }

    #[inline(never)]
    fn eval_invoke(&mut self, f: &Expr, args: &[Expr], env: &Rc<Env>) -> EResult<Value> {
        let fv = self.eval(f, env)?;
        let mut argv = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval(a, env)?);
        }
        self.call(&fv, &argv)
    }

    #[inline(never)]
    fn eval_try(&mut self, a: &Expr, handler: &TryHandler, env: &Rc<Env>) -> EResult<Value> {
        match self.eval(a, env) {
            Ok(v) => Ok(match handler {
                TryHandler::None => Value::record_of(vec![("HasError", Value::Logical(false)), ("Value", v)]),
                _ => v,
            }),
            Err(EvalError::Raised(e)) => match handler {
                TryHandler::None => Ok(Value::record_of(vec![("HasError", Value::Logical(true)), ("Error", error_record(&e))])),
                TryHandler::Otherwise(d) => self.eval(d, env),
                TryHandler::Catch(decl) => {
                    let args = if decl.params.is_empty() { Vec::new() } else { vec![error_record(&e)] };
                    self.call_closure(&Closure { decl: decl.clone(), env: env.clone() }, &args)
                }
            },
            Err(fatal) => Err(fatal),
        }
    }

    fn eval_fields(&mut self, fields: &[FieldSpec], env: &Rc<Env>) -> EResult<Vec<Field>> {
        let mut out = Vec::with_capacity(fields.len());
        for f in fields {
            let ty = match &f.ty {
                Some(t) => self.eval_type(t, env)?,
                None => Type::Any,
            };
            out.push(Field { name: f.name.clone(), ty, optional: f.optional });
        }
        Ok(out)
    }

    pub(crate) fn eval_type(&mut self, t: &TypeExpr, env: &Rc<Env>) -> EResult<Type> {
        Ok(match t {
            TypeExpr::Prim(p) => prim_type(*p),
            TypeExpr::Nullable(t) => Type::nullable(self.eval_type(t, env)?)?,
            TypeExpr::List(t) => Type::list(self.eval_type(t, env)?)?,
            TypeExpr::Record(fields, open) => Type::record(self.eval_fields(fields, env)?, *open)?,
            TypeExpr::Table(fields) => Type::table(self.eval_fields(fields, env)?)?,
            TypeExpr::TableOf(e) => match self.eval(e, env)? {
                Value::Type(Type::Record(c)) => Type::table(c.fields.clone())?,
                v => return raise(format!("A table type needs a record type for its rows, not a value of type {}.", v.kind())),
            },
            TypeExpr::Function => Type::Function,
            TypeExpr::Expr(e) => match self.eval(e, env)? {
                Value::Type(t) => t,
                v => return raise(format!("A type was expected, not a value of type {}.", v.kind())),
            },
        })
    }

    // ------------------------------------------------------------ calls

    /// Calls the function `f`.
    pub fn call(&mut self, f: &Value, args: &[Value]) -> EResult<Value> {
        let Value::Function(func) = f else { return raise(format!("A value of type {} can't be called as a function.", f.kind())) };
        self.tick()?;
        match &**func {
            Function::Closure(c) => self.call_closure(c, args),
            Function::Builtin(b) => {
                check_arity(b.name, args.len(), b.min as usize, if b.max == u8::MAX { usize::MAX } else { b.max as usize })?;
                (b.f)(self, args)
            }
            Function::Bound(b, bound) => {
                let mut all = Vec::with_capacity(bound.len() + args.len());
                all.extend(bound.iter().cloned());
                all.extend(args.iter().cloned());
                check_arity(b.name, all.len(), b.min as usize, if b.max == u8::MAX { usize::MAX } else { b.max as usize })?;
                (b.f)(self, &all)
            }
        }
    }

    fn call_closure(&mut self, c: &Closure, args: &[Value]) -> EResult<Value> {
        let params = &c.decl.params;
        let required = params.iter().filter(|p| !p.optional).count();
        check_arity("The function", args.len(), required, params.len())?;
        let mut values = Vec::with_capacity(params.len());
        for (i, p) in params.iter().enumerate() {
            let v = match args.get(i) {
                Some(v) => {
                    if let Some(t) = &p.ty
                        && fits(t, v) == Some(false)
                        && !(p.optional && v.is_null())
                    {
                        return raise(format!(
                            "The parameter '{}' expects {}, but a value of type {} was given.",
                            p.name,
                            type_expr_name(t),
                            v.kind()
                        ));
                    }
                    v.clone()
                }
                None => Value::Null,
            };
            values.push(v);
        }
        let scope = Env::new(Some(c.env.clone()), Frame::Args { decl: c.decl.clone(), values });
        let r = self.eval(&c.decl.body, &scope)?;
        if let Some(t) = &c.decl.ret
            && fits(t, &r) == Some(false)
        {
            return raise(format!("The function should return {}, but returned a value of type {}.", type_expr_name(t), r.kind()));
        }
        Ok(r)
    }

    /// Calls `f` with one argument.
    pub(crate) fn call1(&mut self, f: &Value, a: Value) -> EResult<Value> {
        self.call(f, &[a])
    }

    /// Calls a predicate: a null result counts as false.
    pub(crate) fn test(&mut self, f: &Value, a: Value) -> EResult<bool> {
        match self.call(f, &[a])? {
            Value::Logical(b) => Ok(b),
            Value::Null => Ok(false),
            v => cannot_convert(&v, "Logical"),
        }
    }

    /// Evaluates `Expression.Evaluate(text, environment)`.
    pub(crate) fn evaluate_text(&mut self, src: &str, environment: Option<Rc<Record>>) -> EResult<Value> {
        let e = match parser::parse(src) {
            Ok(e) => e,
            Err(pe) => return raise(format!("The expression has a syntax error: {}", pe.message)),
        };
        let barrier = Env::new(None, Frame::Barrier);
        let scope = match environment {
            Some(r) => Env::new(Some(barrier), Frame::Fields(r)),
            None => barrier,
        };
        self.eval(&e, &scope)
    }
}

impl Drop for Interp<'_> {
    fn drop(&mut self) {
        self.root.reset();
        for w in self.captured.drain(..) {
            if let Some(e) = w.upgrade() {
                e.reset();
            }
        }
    }
}

fn unknown_name<T>(name: &str) -> EResult<T> {
    raise(format!("The name '{name}' isn't recognized. Check its spelling."))
}

fn check_arity(what: &str, given: usize, min: usize, max: usize) -> EResult<()> {
    if given >= min && given <= max {
        return Ok(());
    }
    let expected = if max == usize::MAX {
        format!("at least {min}")
    } else if min == max {
        format!("{min}")
    } else {
        format!("{min} to {max}")
    };
    raise(format!("{what} takes {expected} argument(s), but {given} were given."))
}

/// A list index: a whole number that isn't negative.
pub(crate) fn index_of(n: f64) -> EResult<usize> {
    if n.fract() != 0.0 || !n.is_finite() || n < 0.0 || n > MAX_ITEMS as f64 {
        return raise("An index must be a whole number that isn't negative.");
    }
    Ok(n as usize)
}

pub(crate) fn table_column(t: &Table, i: usize) -> Value {
    Value::list(t.rows.iter().map(|r| r.get(i).cloned().unwrap_or(Slot::NULL)).collect())
}

pub(crate) fn field(target: &Value, name: &str, optional: bool) -> EResult<Value> {
    match target {
        Value::Record(r) => match r.slot(name) {
            Some(s) => s.get(),
            None if optional => Ok(Value::Null),
            None => raise(format!("The record has no field named '{name}'.")),
        },
        Value::Table(t) => match t.column_index(name) {
            Some(i) => Ok(table_column(t, i)),
            None if optional => Ok(Value::Null),
            None => raise(format!("The table has no column named '{name}'.")),
        },
        Value::Null if optional => Ok(Value::Null),
        v => raise(format!("A field can't be read from a value of type {}.", v.kind())),
    }
}

fn project(target: &Value, names: &[Rc<str>], optional: bool) -> EResult<Value> {
    match target {
        Value::Record(r) => {
            let mut values = Vec::with_capacity(names.len());
            for n in names {
                match r.slot(n) {
                    Some(s) => values.push(s.clone()),
                    None if optional => values.push(Slot::NULL),
                    None => return raise(format!("The record has no field named '{n}'.")),
                }
            }
            Ok(Value::record(names.into(), values.into()))
        }
        Value::Table(t) => {
            let mut idx = Vec::with_capacity(names.len());
            for n in names {
                match t.column_index(n) {
                    Some(i) => idx.push(Some(i)),
                    None if optional => idx.push(None),
                    None => return raise(format!("The table has no column named '{n}'.")),
                }
            }
            let rows = t.rows.iter().map(|r| idx.iter().map(|i| i.and_then(|i| r.get(i).cloned()).unwrap_or(Slot::NULL)).collect()).collect();
            let types = idx.iter().map(|i| i.and_then(|i| t.types.get(i).cloned()).unwrap_or(Type::Any)).collect();
            Ok(Value::table(Table { columns: names.into(), types, rows }))
        }
        Value::Null if optional => Ok(Value::Null),
        v => raise(format!("Fields can't be read from a value of type {}.", v.kind())),
    }
}

fn item(target: &Value, index: &Value, optional: bool) -> EResult<Value> {
    match (target, index) {
        (Value::List(l), Value::Number(n)) => {
            let i = index_of(*n)?;
            match l.items.get(i) {
                Some(s) => s.get(),
                None if optional => Ok(Value::Null),
                None => raise(format!("The list has {} item(s), so there is no item {i}.", l.len())),
            }
        }
        (Value::Table(t), Value::Number(n)) => {
            let i = index_of(*n)?;
            match t.row_record(i) {
                Some(r) => Ok(r),
                None if optional => Ok(Value::Null),
                None => raise(format!("The table has {} row(s), so there is no row {i}.", t.rows.len())),
            }
        }
        (Value::Table(t), Value::Record(key)) => {
            let mut cols = Vec::with_capacity(key.len());
            for (name, slot) in key.iter() {
                let Some(c) = t.column_index(name) else { return raise(format!("The table has no column named '{name}'.")) };
                cols.push((c, slot.get()?));
            }
            let mut found = None;
            for (i, row) in t.rows.iter().enumerate() {
                let mut all = true;
                for (c, want) in &cols {
                    let same = match row.get(*c) {
                        Some(Slot::Val(v)) => values_equal(v, want)?,
                        _ => false,
                    };
                    if !same {
                        all = false;
                        break;
                    }
                }
                if all {
                    if found.is_some() {
                        return raise("The key matches more than one row of the table.");
                    }
                    found = Some(i);
                }
            }
            match found.and_then(|i| t.row_record(i)) {
                Some(r) => Ok(r),
                None if optional => Ok(Value::Null),
                None => raise("The key doesn't match any row of the table."),
            }
        }
        (Value::Null, _) if optional => Ok(Value::Null),
        (Value::List(_) | Value::Table(_), i) => raise(format!("A value of type {} can't be used as an index.", i.kind())),
        (v, _) => raise(format!("An item can't be read from a value of type {}.", v.kind())),
    }
}
