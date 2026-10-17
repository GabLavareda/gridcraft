//! Recursive-descent parser for M expressions and section documents.
//!
//! Two limits keep hostile input from exhausting the stack: `MAX_NEST` bounds the parser's own
//! recursion, and `MAX_HEIGHT` bounds the height of the tree it builds, which bounds every later
//! walk of that tree (evaluation, dropping). Each parse function returns the height of its tree.

use std::rc::Rc;

use crate::ast::*;
use crate::error::ParseError;
use crate::lexer::{Kw, Tok, Token, lex, read_generalized_identifier};

/// Deepest nesting of expressions inside expressions.
pub(crate) const MAX_NEST: usize = 128;
/// Tallest syntax tree.
pub(crate) const MAX_HEIGHT: u32 = 256;

type PResult<T> = Result<T, ParseError>;
/// A tree and its height.
type Tree = (Expr, u32);

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
    eof: Token,
}

/// Parses one expression: the text of a query.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let mut p = Parser::new(src)?;
    let (e, _) = p.expression()?;
    p.expect_eof()?;
    Ok(e)
}

/// The steps of a query: the members of its top-level `let`, in order.
pub fn steps(src: &str) -> Result<Steps, ParseError> {
    Ok(match parse(src)? {
        Expr::Let(b, body) => Steps {
            steps: b.names.iter().zip(&b.spans).map(|(n, s)| Step { name: n.to_string(), span: *s }).collect(),
            result: match *body {
                Expr::Ident { name, .. } => Some(name.to_string()),
                _ => None,
            },
        },
        _ => Steps::default(),
    })
}

/// Parses a section document (`section Section1; shared Query1 = …;`) into its members.
pub fn parse_section(src: &str) -> Result<Vec<SectionMember>, ParseError> {
    let mut p = Parser::new(src)?;
    let mut out = Vec::new();
    p.skip_attributes()?;
    if p.eat(&Tok::Kw(Kw::Section)) {
        if matches!(p.peek(), Tok::Ident(_)) {
            p.bump();
        }
        p.expect(&Tok::Semi, "a semicolon after the section name")?;
    }
    while !matches!(p.peek(), Tok::Eof) {
        p.skip_attributes()?;
        let shared = p.eat(&Tok::Kw(Kw::Shared));
        let name = p.identifier("a query name")?;
        p.expect(&Tok::Eq, "= after the query name")?;
        let start = p.cur().start;
        p.expression()?;
        let end = p.prev_end();
        p.expect(&Tok::Semi, "a semicolon after the query")?;
        out.push(SectionMember { name: name.to_string(), shared, formula: src.get(start..end).unwrap_or("").to_string() });
    }
    Ok(out)
}

fn binop(t: &Tok) -> Option<(Option<BinOp>, u8)> {
    Some(match t {
        Tok::Coalesce => (Some(BinOp::Coalesce), 1),
        Tok::Kw(Kw::Or) => (Some(BinOp::Or), 2),
        Tok::Kw(Kw::And) => (Some(BinOp::And), 3),
        Tok::Kw(Kw::Is) => (None, 4),
        Tok::Kw(Kw::As) => (None, 5),
        Tok::Eq => (Some(BinOp::Eq), 6),
        Tok::Ne => (Some(BinOp::Ne), 6),
        Tok::Lt => (Some(BinOp::Lt), 7),
        Tok::Le => (Some(BinOp::Le), 7),
        Tok::Gt => (Some(BinOp::Gt), 7),
        Tok::Ge => (Some(BinOp::Ge), 7),
        Tok::Plus => (Some(BinOp::Add), 8),
        Tok::Minus => (Some(BinOp::Sub), 8),
        Tok::Amp => (Some(BinOp::Concat), 8),
        Tok::Star => (Some(BinOp::Mul), 9),
        Tok::Slash => (Some(BinOp::Div), 9),
        Tok::Kw(Kw::Meta) => (Some(BinOp::Meta), 10),
        _ => return None,
    })
}

fn level(op: BinOp) -> u8 {
    match op {
        BinOp::Coalesce => 1,
        BinOp::Or => 2,
        BinOp::And => 3,
        BinOp::Eq | BinOp::Ne => 6,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 7,
        BinOp::Add | BinOp::Sub | BinOp::Concat => 8,
        BinOp::Mul | BinOp::Div => 9,
        BinOp::Meta => 10,
    }
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> PResult<Parser<'a>> {
        let toks = lex(src)?;
        Ok(Parser { src, toks, pos: 0, depth: 0, eof: Token { tok: Tok::Eof, start: src.len(), end: src.len() } })
    }

    fn at(&self, i: usize) -> &Token {
        self.toks.get(i).unwrap_or(&self.eof)
    }
    fn cur(&self) -> &Token {
        self.at(self.pos)
    }
    fn peek(&self) -> &Tok {
        &self.cur().tok
    }
    fn peek2(&self) -> &Tok {
        &self.at(self.pos + 1).tok
    }
    fn prev_end(&self) -> usize {
        self.pos.checked_sub(1).map_or(0, |i| self.at(i).end)
    }
    fn bump(&mut self) {
        if self.pos < self.toks.len() {
            self.pos += 1;
        }
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.bump();
            true
        } else {
            false
        }
    }
    fn err<T>(&self, message: impl Into<String>) -> PResult<T> {
        Err(ParseError::new(message, self.cur().start))
    }
    fn expect(&mut self, t: &Tok, what: &str) -> PResult<()> {
        if self.eat(t) { Ok(()) } else { self.err(format!("Expected {what}.")) }
    }
    fn expect_eof(&self) -> PResult<()> {
        if matches!(self.peek(), Tok::Eof) {
            Ok(())
        } else {
            self.err("This part of the query isn't expected here: an operator or a comma may be missing.")
        }
    }
    fn check(&self, h: u32) -> PResult<u32> {
        if h > MAX_HEIGHT { self.err("This expression is too complex. Split it into steps.") } else { Ok(h) }
    }

    fn identifier(&mut self, what: &str) -> PResult<Rc<str>> {
        match self.peek() {
            Tok::Ident(n) => {
                let n = n.clone();
                self.bump();
                Ok(n)
            }
            _ => self.err(format!("Expected {what}.")),
        }
    }

    /// A field name: a quoted identifier, or a generalized one (`Unit Price`, `2024 Sales`).
    fn field_name(&mut self) -> PResult<Rc<str>> {
        let start = self.cur().start;
        if self.src.get(start..).is_some_and(|s| s.starts_with("#\"")) {
            return self.identifier("a field name");
        }
        match read_generalized_identifier(self.src, start) {
            Some((name, end)) => {
                while self.cur().start < end && !matches!(self.peek(), Tok::Eof) {
                    self.bump();
                }
                Ok(Rc::from(name))
            }
            None => self.err("Expected a field name."),
        }
    }

    /// Literal attributes (`[ Description = "…" ]`) before a section or a member: read and dropped.
    fn skip_attributes(&mut self) -> PResult<()> {
        if matches!(self.peek(), Tok::LBracket) {
            self.primary()?;
        }
        Ok(())
    }

    fn expression(&mut self) -> PResult<Tree> {
        self.depth += 1;
        if self.depth > MAX_NEST {
            return self.err("This expression is nested too deeply. Split it into steps.");
        }
        let r = self.expression_inner();
        self.depth -= 1;
        r
    }

    fn expression_inner(&mut self) -> PResult<Tree> {
        match self.peek() {
            Tok::Kw(Kw::Each) => {
                self.bump();
                let (body, h) = self.expression()?;
                let decl = FuncDecl { params: vec![Param { name: Rc::from("_"), optional: false, ty: None }], ret: None, body };
                Ok((Expr::Function(Rc::new(decl)), self.check(h + 1)?))
            }
            Tok::Kw(Kw::Let) => self.let_expr(),
            Tok::Kw(Kw::If) => {
                self.bump();
                let (c, hc) = self.expression()?;
                self.expect(&Tok::Kw(Kw::Then), "then")?;
                let (a, ha) = self.expression()?;
                self.expect(&Tok::Kw(Kw::Else), "else")?;
                let (b, hb) = self.expression()?;
                Ok((Expr::If(Box::new(c), Box::new(a), Box::new(b)), self.check(1 + hc.max(ha).max(hb))?))
            }
            Tok::Kw(Kw::Error) => {
                self.bump();
                let (e, h) = self.expression()?;
                Ok((Expr::Raise(Box::new(e)), self.check(h + 1)?))
            }
            Tok::Kw(Kw::Try) => {
                self.bump();
                let (e, h) = self.expression()?;
                if self.eat(&Tok::Kw(Kw::Otherwise)) {
                    let (d, hd) = self.expression()?;
                    return Ok((Expr::Try(Box::new(e), TryHandler::Otherwise(Box::new(d))), self.check(1 + h.max(hd))?));
                }
                if matches!(self.peek(), Tok::Ident(n) if &**n == "catch") && matches!(self.peek2(), Tok::LParen) {
                    self.bump();
                    let (decl, hd) = self.function_literal()?;
                    if decl.params.len() > 1 {
                        return self.err("A catch handler takes zero or one parameter.");
                    }
                    return Ok((Expr::Try(Box::new(e), TryHandler::Catch(Rc::new(decl))), self.check(1 + h.max(hd))?));
                }
                Ok((Expr::Try(Box::new(e), TryHandler::None), self.check(h + 1)?))
            }
            _ => self.binary(),
        }
    }

    fn let_expr(&mut self) -> PResult<Tree> {
        self.bump();
        let mut members = Vec::new();
        let mut h = 0;
        loop {
            let name = self.identifier("a step name")?;
            if members.iter().any(|(n, _, _): &(Rc<str>, Expr, (usize, usize))| *n == name) {
                return self.err(format!("The name '{name}' is defined more than once."));
            }
            self.expect(&Tok::Eq, "= after the step name")?;
            let start = self.cur().start;
            let (e, he) = self.expression()?;
            h = h.max(he);
            members.push((name, e, (start, self.prev_end())));
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Kw(Kw::In), "a comma or in")?;
        let (body, hb) = self.expression()?;
        Ok((Expr::Let(bindings(members), Box::new(body)), self.check(1 + h.max(hb))?))
    }

    /// Binary operators by precedence, without recursion: an operand stack and an operator stack.
    fn binary(&mut self) -> PResult<Tree> {
        let mut operands: Vec<Tree> = vec![self.unary()?];
        let mut ops: Vec<BinOp> = Vec::new();
        while let Some((op, prec)) = binop(self.peek()) {
            while ops.last().is_some_and(|top| level(*top) >= prec) {
                self.reduce(&mut operands, &mut ops)?;
            }
            let is_kw = matches!(self.peek(), Tok::Kw(Kw::Is));
            self.bump();
            match op {
                Some(op) => {
                    ops.push(op);
                    operands.push(self.unary()?);
                }
                None => {
                    let (ty, ht) = self.type_spec()?;
                    let Some((e, h)) = operands.pop() else { return self.err("Expected an expression.") };
                    let h = self.check(1 + h.max(ht))?;
                    operands.push((if is_kw { Expr::Is(Box::new(e), ty) } else { Expr::As(Box::new(e), ty) }, h));
                }
            }
        }
        while !ops.is_empty() {
            self.reduce(&mut operands, &mut ops)?;
        }
        match operands.pop() {
            Some(t) => Ok(t),
            None => self.err("Expected an expression."),
        }
    }

    fn reduce(&self, operands: &mut Vec<Tree>, ops: &mut Vec<BinOp>) -> PResult<()> {
        let (Some(op), Some((r, hr)), Some((l, hl))) = (ops.pop(), operands.pop(), operands.pop()) else {
            return self.err("Expected an expression.");
        };
        let tree = match l {
            Expr::Chain(first, mut rest) if rest.first().is_some_and(|(o, _)| level(*o) == level(op)) => {
                rest.push((op, r));
                (Expr::Chain(first, rest), self.check(hl.max(hr + 1))?)
            }
            l => (Expr::Chain(Box::new(l), vec![(op, r)]), self.check(1 + hl.max(hr))?),
        };
        operands.push(tree);
        Ok(())
    }

    fn unary(&mut self) -> PResult<Tree> {
        let mut ops = Vec::new();
        loop {
            let op = match self.peek() {
                Tok::Plus => UnOp::Plus,
                Tok::Minus => UnOp::Minus,
                Tok::Kw(Kw::Not) => UnOp::Not,
                _ => break,
            };
            self.bump();
            ops.push(op);
            self.check(ops.len() as u32)?;
        }
        let (mut e, mut h) = if matches!(self.peek(), Tok::Kw(Kw::Type)) {
            self.bump();
            let (t, h) = self.type_spec()?;
            (Expr::Type(t), self.check(h + 1)?)
        } else {
            let p = self.primary()?;
            self.postfix(p)?
        };
        for op in ops.into_iter().rev() {
            // A sign on a number literal is part of the literal.
            e = match (op, e) {
                (UnOp::Minus, Expr::Number(n)) => Expr::Number(-n),
                (UnOp::Plus, Expr::Number(n)) => Expr::Number(n),
                (op, e) => {
                    h = self.check(h + 1)?;
                    Expr::Unary(op, Box::new(e))
                }
            };
        }
        Ok((e, h))
    }

    fn postfix(&mut self, (mut e, mut h): Tree) -> PResult<Tree> {
        loop {
            match self.peek() {
                Tok::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    let mut ha = 0;
                    if !self.eat(&Tok::RParen) {
                        loop {
                            let (a, x) = self.expression()?;
                            ha = ha.max(x);
                            args.push(a);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                        self.expect(&Tok::RParen, "a comma or )")?;
                    }
                    h = self.check(1 + h.max(ha))?;
                    e = Expr::Invoke(Box::new(e), args);
                }
                Tok::LBrace => {
                    self.bump();
                    let (index, hi) = self.expression()?;
                    self.expect(&Tok::RBrace, "}")?;
                    let optional = self.eat(&Tok::Question);
                    h = self.check(1 + h.max(hi))?;
                    e = Expr::Item { target: Box::new(e), index: Box::new(index), optional };
                }
                Tok::LBracket => {
                    self.bump();
                    h = self.check(h + 1)?;
                    e = self.field_access(Some(Box::new(e)))?;
                }
                _ => return Ok((e, h)),
            }
        }
    }

    /// The rest of `[name]` or `[[a],[b]]` after the opening bracket, with its optional `?`.
    fn field_access(&mut self, target: Option<Box<Expr>>) -> PResult<Expr> {
        if matches!(self.peek(), Tok::LBracket) {
            let mut names = Vec::new();
            loop {
                self.expect(&Tok::LBracket, "[")?;
                names.push(self.field_name()?);
                self.expect(&Tok::RBracket, "]")?;
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RBracket, "]")?;
            let optional = self.eat(&Tok::Question);
            return Ok(Expr::Project { target, names, optional });
        }
        let name = self.field_name()?;
        self.expect(&Tok::RBracket, "]")?;
        let optional = self.eat(&Tok::Question);
        Ok(Expr::Field { target, name, optional })
    }

    /// Whether the parenthesis at the cursor opens a function literal: `Some(true)` when it
    /// surely does (`(…) =>`), `Some(false)` when it may (`(…) as …`).
    fn function_ahead(&self) -> Option<bool> {
        let mut depth = 0usize;
        let mut i = self.pos;
        loop {
            match &self.at(i).tok {
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        break;
                    }
                }
                Tok::Eof => return None,
                _ => {}
            }
            i += 1;
        }
        match &self.at(i + 1).tok {
            Tok::Arrow => Some(true),
            Tok::Kw(Kw::As) => Some(false),
            _ => None,
        }
    }

    /// `(a, optional b as type) as type => body`, from the opening parenthesis.
    fn function_literal(&mut self) -> PResult<(FuncDecl, u32)> {
        self.expect(&Tok::LParen, "(")?;
        let mut params: Vec<Param> = Vec::new();
        let mut h = 0;
        if !self.eat(&Tok::RParen) {
            loop {
                let optional = matches!(self.peek(), Tok::Ident(n) if &**n == "optional") && matches!(self.peek2(), Tok::Ident(_));
                if optional {
                    self.bump();
                }
                let name = self.identifier("a parameter name")?;
                if params.iter().any(|p| p.name == name) {
                    return self.err(format!("The parameter '{name}' is defined more than once."));
                }
                if !optional && params.last().is_some_and(|p| p.optional) {
                    return self.err("A required parameter can't follow an optional one.");
                }
                let ty = if self.eat(&Tok::Kw(Kw::As)) {
                    let (t, ht) = self.type_spec()?;
                    h = h.max(ht);
                    Some(t)
                } else {
                    None
                };
                params.push(Param { name, optional, ty });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen, "a comma or )")?;
        }
        let ret = if self.eat(&Tok::Kw(Kw::As)) {
            let (t, ht) = self.type_spec()?;
            h = h.max(ht);
            Some(t)
        } else {
            None
        };
        self.expect(&Tok::Arrow, "=>")?;
        let (body, hb) = self.expression()?;
        Ok((FuncDecl { params, ret, body }, self.check(1 + h.max(hb))?))
    }

    fn primary(&mut self) -> PResult<Tree> {
        let t = self.cur().clone();
        match t.tok {
            Tok::Number(n) => {
                self.bump();
                Ok((Expr::Number(n), 1))
            }
            Tok::Text(s) => {
                self.bump();
                Ok((Expr::Text(s), 1))
            }
            Tok::Kw(Kw::Null) => {
                self.bump();
                Ok((Expr::Null, 1))
            }
            Tok::Kw(Kw::True) => {
                self.bump();
                Ok((Expr::Logical(true), 1))
            }
            Tok::Kw(Kw::False) => {
                self.bump();
                Ok((Expr::Logical(false), 1))
            }
            Tok::Ident(name) => {
                self.bump();
                // `Section1!Query1`: the section name is dropped, there is one section.
                if matches!(self.peek(), Tok::Bang) && matches!(self.peek2(), Tok::Ident(_)) {
                    self.bump();
                    let span = (self.cur().start, self.cur().end);
                    let member = self.identifier("a name")?;
                    return Ok((Expr::Ident { name: member, span, skip: 0, inclusive: false }, 1));
                }
                Ok((Expr::Ident { name, span: (t.start, t.end), skip: 0, inclusive: false }, 1))
            }
            Tok::At => {
                self.bump();
                let span = (self.cur().start, self.cur().end);
                let name = self.identifier("a name after @")?;
                Ok((Expr::Ident { name, span, skip: 0, inclusive: true }, 1))
            }
            Tok::Ellipsis => {
                self.bump();
                Ok((Expr::NotImplemented, 1))
            }
            Tok::LParen => {
                match self.function_ahead() {
                    Some(true) => {
                        let (decl, h) = self.function_literal()?;
                        return Ok((Expr::Function(Rc::new(decl)), h));
                    }
                    Some(false) => {
                        // `(x) as number => …` or `(x) as number`: try the function first.
                        let (pos, depth) = (self.pos, self.depth);
                        if let Ok((decl, h)) = self.function_literal() {
                            return Ok((Expr::Function(Rc::new(decl)), h));
                        }
                        self.pos = pos;
                        self.depth = depth;
                    }
                    None => {}
                }
                self.bump();
                let tree = self.expression()?;
                self.expect(&Tok::RParen, ")")?;
                Ok(tree)
            }
            Tok::LBrace => {
                self.bump();
                let mut items = Vec::new();
                let mut h = 0;
                if !self.eat(&Tok::RBrace) {
                    loop {
                        let (a, ha) = self.expression()?;
                        h = h.max(ha);
                        if self.eat(&Tok::DotDot) {
                            let (b, hb) = self.expression()?;
                            h = h.max(hb);
                            items.push(ListItem::Range(a, b));
                        } else {
                            items.push(ListItem::One(a));
                        }
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RBrace, "a comma or }")?;
                }
                Ok((Expr::List(items), self.check(h + 1)?))
            }
            Tok::LBracket => {
                self.bump();
                if self.eat(&Tok::RBracket) {
                    return Ok((Expr::Record(bindings(Vec::new())), 1));
                }
                if matches!(self.peek(), Tok::LBracket) {
                    return Ok((self.field_access(None)?, 1));
                }
                // `[name]` is a field of `_`; `[name = …]` starts a record.
                let start = self.pos;
                let name = self.field_name()?;
                if !matches!(self.peek(), Tok::Eq) {
                    self.pos = start;
                    return Ok((self.field_access(None)?, 1));
                }
                let mut members = Vec::new();
                let mut h = 0;
                let mut name = name;
                loop {
                    if members.iter().any(|(n, _, _): &(Rc<str>, Expr, (usize, usize))| *n == name) {
                        return self.err(format!("The field '{name}' is defined more than once."));
                    }
                    self.expect(&Tok::Eq, "= after the field name")?;
                    let from = self.cur().start;
                    let (e, he) = self.expression()?;
                    h = h.max(he);
                    members.push((name, e, (from, self.prev_end())));
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                    name = self.field_name()?;
                }
                self.expect(&Tok::RBracket, "a comma or ]")?;
                Ok((Expr::Record(bindings(members)), self.check(h + 1)?))
            }
            // Lenient: `1 + if …` without parentheses.
            Tok::Kw(Kw::If | Kw::Let | Kw::Each | Kw::Try | Kw::Error) => self.expression(),
            Tok::Eof => self.err("The query ends before the expression is complete."),
            _ => self.err("Expected an expression."),
        }
    }

    /// A type, in a type context (after `type`, `as` or `is`).
    fn type_spec(&mut self) -> PResult<(TypeExpr, u32)> {
        self.depth += 1;
        if self.depth > MAX_NEST {
            return self.err("This type is nested too deeply.");
        }
        let r = self.type_spec_inner();
        self.depth -= 1;
        r
    }

    fn type_spec_inner(&mut self) -> PResult<(TypeExpr, u32)> {
        match self.peek().clone() {
            Tok::Kw(Kw::Null) => {
                self.bump();
                Ok((TypeExpr::Prim(Prim::Null), 1))
            }
            Tok::Kw(Kw::Type) => {
                self.bump();
                Ok((TypeExpr::Prim(Prim::Type), 1))
            }
            Tok::Ident(n) if &*n == "nullable" => {
                self.bump();
                let (t, h) = self.type_spec()?;
                Ok((TypeExpr::Nullable(Box::new(t)), self.check(h + 1)?))
            }
            Tok::Ident(n) if &*n == "table" && matches!(self.peek2(), Tok::LBracket) => {
                self.bump();
                self.bump();
                let (fields, _, h) = self.field_specs()?;
                Ok((TypeExpr::Table(fields), self.check(h + 1)?))
            }
            Tok::Ident(n) if &*n == "table" && matches!(self.peek2(), Tok::Ident(_)) => {
                self.bump();
                let p = self.primary()?;
                let (e, h) = self.postfix(p)?;
                Ok((TypeExpr::TableOf(Box::new(e)), self.check(h + 1)?))
            }
            Tok::Ident(n) if &*n == "function" && matches!(self.peek2(), Tok::LParen) => {
                self.bump();
                self.bump();
                if !self.eat(&Tok::RParen) {
                    loop {
                        if matches!(self.peek(), Tok::Ident(n) if &**n == "optional") && matches!(self.peek2(), Tok::Ident(_)) {
                            self.bump();
                        }
                        self.identifier("a parameter name")?;
                        if self.eat(&Tok::Kw(Kw::As)) {
                            self.type_spec()?;
                        }
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen, "a comma or )")?;
                }
                self.expect(&Tok::Kw(Kw::As), "as and the return type")?;
                self.type_spec()?;
                Ok((TypeExpr::Function, 1))
            }
            Tok::Ident(n) if Prim::from_name(&n).is_some() => {
                self.bump();
                Ok((TypeExpr::Prim(Prim::from_name(&n).unwrap_or(Prim::Any)), 1))
            }
            Tok::LBrace => {
                self.bump();
                let (t, h) = self.type_spec()?;
                self.expect(&Tok::RBrace, "}")?;
                Ok((TypeExpr::List(Box::new(t)), self.check(h + 1)?))
            }
            Tok::LBracket => {
                self.bump();
                let (fields, open, h) = self.field_specs()?;
                Ok((TypeExpr::Record(fields, open), self.check(h + 1)?))
            }
            _ => {
                let p = self.primary()?;
                let (e, h) = self.postfix(p)?;
                Ok((TypeExpr::Expr(Box::new(e)), self.check(h + 1)?))
            }
        }
    }

    /// `a = type, optional b = type, ... ]` after the opening bracket: fields, open marker, height.
    fn field_specs(&mut self) -> PResult<(Vec<FieldSpec>, bool, u32)> {
        let mut fields: Vec<FieldSpec> = Vec::new();
        let mut open = false;
        let mut h = 0;
        if self.eat(&Tok::RBracket) {
            return Ok((fields, open, h));
        }
        loop {
            if self.eat(&Tok::Ellipsis) {
                open = true;
                break;
            }
            let optional =
                matches!(self.peek(), Tok::Ident(n) if &**n == "optional") && !matches!(self.peek2(), Tok::Eq | Tok::Comma | Tok::RBracket);
            if optional {
                self.bump();
            }
            let name = self.field_name()?;
            if fields.iter().any(|f| f.name == name) {
                return self.err(format!("The field '{name}' is defined more than once."));
            }
            let ty = if self.eat(&Tok::Eq) {
                let (t, ht) = self.type_spec()?;
                h = h.max(ht);
                Some(t)
            } else {
                None
            };
            fields.push(FieldSpec { name, optional, ty });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBracket, "a comma or ]")?;
        Ok((fields, open, h))
    }
}

/// Builds the members of a `let` or a record, after pointing each member's references to its own
/// name at the enclosing scope.
fn bindings(members: Vec<(Rc<str>, Expr, (usize, usize))>) -> Rc<Bindings> {
    let mut names = Vec::with_capacity(members.len());
    let mut exprs = Vec::with_capacity(members.len());
    let mut spans = Vec::with_capacity(members.len());
    for (name, mut e, span) in members {
        mark_self_refs(&mut e, &name);
        names.push(name);
        exprs.push(Rc::new(e));
        spans.push(span);
    }
    Rc::new(Bindings { names: names.into(), exprs, spans })
}

/// Within the definition of the member `name`, a plain reference to `name` means the name in an
/// enclosing scope (`@name` means the member itself): bump `skip` on those references. Scopes in
/// between that define `name` again hide it, except in their own member of that name.
fn mark_self_refs(e: &mut Expr, name: &str) {
    match e {
        Expr::Ident { name: n, skip, inclusive, .. } => {
            if !*inclusive && &**n == name {
                *skip = skip.saturating_add(1);
            }
        }
        Expr::Null | Expr::Logical(_) | Expr::Number(_) | Expr::Text(_) | Expr::NotImplemented | Expr::Failed(_) => {}
        Expr::List(items) => {
            for item in items {
                match item {
                    ListItem::One(a) => mark_self_refs(a, name),
                    ListItem::Range(a, b) => {
                        mark_self_refs(a, name);
                        mark_self_refs(b, name);
                    }
                }
            }
        }
        Expr::Record(b) => mark_in_bindings(b, name),
        Expr::Let(b, body) => {
            if b.index_of(name).is_none() {
                mark_self_refs(body, name);
            }
            mark_in_bindings(b, name);
        }
        Expr::Unary(_, a) | Expr::Raise(a) => mark_self_refs(a, name),
        Expr::Chain(first, rest) => {
            mark_self_refs(first, name);
            for (_, r) in rest {
                mark_self_refs(r, name);
            }
        }
        Expr::If(c, a, b) => {
            mark_self_refs(c, name);
            mark_self_refs(a, name);
            mark_self_refs(b, name);
        }
        Expr::Function(decl) => mark_in_function(decl, name),
        Expr::Invoke(f, args) => {
            mark_self_refs(f, name);
            for a in args {
                mark_self_refs(a, name);
            }
        }
        Expr::Field { target, .. } | Expr::Project { target, .. } => {
            if let Some(t) = target {
                mark_self_refs(t, name);
            }
        }
        Expr::Item { target, index, .. } => {
            mark_self_refs(target, name);
            mark_self_refs(index, name);
        }
        Expr::Try(a, handler) => {
            mark_self_refs(a, name);
            match handler {
                TryHandler::None => {}
                TryHandler::Otherwise(d) => mark_self_refs(d, name),
                TryHandler::Catch(decl) => mark_in_function(decl, name),
            }
        }
        Expr::As(a, t) | Expr::Is(a, t) => {
            mark_self_refs(a, name);
            mark_in_type(t, name);
        }
        Expr::Type(t) => mark_in_type(t, name),
    }
}

fn mark_in_function(decl: &mut Rc<FuncDecl>, name: &str) {
    let Some(d) = Rc::get_mut(decl) else { return };
    if d.params.iter().any(|p| &*p.name == name) {
        return;
    }
    mark_self_refs(&mut d.body, name);
}

fn mark_in_bindings(b: &mut Rc<Bindings>, name: &str) {
    let Some(b) = Rc::get_mut(b) else { return };
    match b.index_of(name) {
        // The scope defines the name again: only its own member of that name still sees ours.
        Some(i) => {
            if let Some(e) = b.exprs.get_mut(i).and_then(Rc::get_mut) {
                mark_self_refs(e, name);
            }
        }
        None => {
            for e in &mut b.exprs {
                if let Some(e) = Rc::get_mut(e) {
                    mark_self_refs(e, name);
                }
            }
        }
    }
}

fn mark_in_type(t: &mut TypeExpr, name: &str) {
    match t {
        TypeExpr::Prim(_) | TypeExpr::Function => {}
        TypeExpr::Nullable(t) | TypeExpr::List(t) => mark_in_type(t, name),
        TypeExpr::Record(fields, _) | TypeExpr::Table(fields) => {
            for f in fields {
                if let Some(t) = &mut f.ty {
                    mark_in_type(t, name);
                }
            }
        }
        TypeExpr::TableOf(e) | TypeExpr::Expr(e) => mark_self_refs(e, name),
    }
}
