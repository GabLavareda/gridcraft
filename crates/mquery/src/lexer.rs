//! Lexical analysis: M source text to tokens.

use std::rc::Rc;

use crate::error::ParseError;

/// Most tokens accepted in one document.
const MAX_TOKENS: usize = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    And,
    As,
    Each,
    Else,
    Error,
    False,
    If,
    In,
    Is,
    Let,
    Meta,
    Not,
    Null,
    Or,
    Otherwise,
    Section,
    Shared,
    Then,
    True,
    Try,
    Type,
}

fn keyword(s: &str) -> Option<Kw> {
    Some(match s {
        "and" => Kw::And,
        "as" => Kw::As,
        "each" => Kw::Each,
        "else" => Kw::Else,
        "error" => Kw::Error,
        "false" => Kw::False,
        "if" => Kw::If,
        "in" => Kw::In,
        "is" => Kw::Is,
        "let" => Kw::Let,
        "meta" => Kw::Meta,
        "not" => Kw::Not,
        "null" => Kw::Null,
        "or" => Kw::Or,
        "otherwise" => Kw::Otherwise,
        "section" => Kw::Section,
        "shared" => Kw::Shared,
        "then" => Kw::Then,
        "true" => Kw::True,
        "try" => Kw::Try,
        "type" => Kw::Type,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// A regular identifier (`Table.SelectRows`), a quoted one (`#"Changed Type"`) or one of the
    /// `#name` intrinsics (`#date`, `#table`, `#shared`), kept with its `#`.
    Ident(Rc<str>),
    Number(f64),
    Text(Rc<str>),
    Kw(Kw),
    Comma,
    Semi,
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
    Plus,
    Minus,
    Star,
    Slash,
    Amp,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    At,
    Bang,
    Question,
    Coalesce,
    Arrow,
    DotDot,
    Ellipsis,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
}

pub(crate) fn char_at(src: &str, i: usize) -> Option<char> {
    src.get(i..)?.chars().next()
}

pub(crate) fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

pub(crate) fn is_ident_part(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Skips whitespace and comments from `i`.
fn skip_trivia(src: &str, mut i: usize) -> Result<usize, ParseError> {
    loop {
        match char_at(src, i) {
            Some(c) if c.is_whitespace() || c == '\u{feff}' => i += c.len_utf8(),
            Some('/') if char_at(src, i + 1) == Some('/') => {
                i = src.get(i..).and_then(|s| s.find('\n')).map_or(src.len(), |n| i + n + 1);
            }
            Some('/') if char_at(src, i + 1) == Some('*') => {
                let close = src.get(i + 2..).and_then(|s| s.find("*/"));
                match close {
                    Some(n) => i = i + 2 + n + 2,
                    None => return Err(ParseError::new("This comment is never closed: add */.", i)),
                }
            }
            _ => return Ok(i),
        }
    }
}

/// Reads the body of a text literal or quoted identifier; `i` is just past the opening quote.
/// Returns the text and the offset just past the closing quote.
fn read_quoted(src: &str, mut i: usize, open: usize) -> Result<(String, usize), ParseError> {
    let mut out = String::new();
    loop {
        match char_at(src, i) {
            None => return Err(ParseError::new("This text is never closed: add a closing quote.", open)),
            Some('"') => {
                if char_at(src, i + 1) == Some('"') {
                    out.push('"');
                    i += 2;
                } else {
                    return Ok((out, i + 1));
                }
            }
            Some('#') if char_at(src, i + 1) == Some('(') => {
                let close = src.get(i..).and_then(|s| s.find(')')).map(|n| i + n);
                let Some(close) = close else { return Err(ParseError::new("This escape sequence is never closed: add ).", i)) };
                let body = src.get(i + 2..close).unwrap_or("");
                for part in body.split(',') {
                    let part = part.trim();
                    match part {
                        "cr" => out.push('\r'),
                        "lf" => out.push('\n'),
                        "tab" => out.push('\t'),
                        "#" => out.push('#'),
                        hex if (hex.len() == 4 || hex.len() == 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                            let c = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
                            match c {
                                Some(c) => out.push(c),
                                None => return Err(ParseError::new("This escape sequence isn't a valid character.", i)),
                            }
                        }
                        _ => {
                            return Err(ParseError::new(
                                "This escape sequence isn't recognized. Use #(cr), #(lf), #(tab), #(#) or a hexadecimal code.",
                                i,
                            ));
                        }
                    }
                }
                i = close + 1;
            }
            Some(c) => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
}

fn read_number(src: &str, start: usize) -> Result<(f64, usize), ParseError> {
    let b = src.as_bytes();
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let mut i = start;
    if at(i) == b'0' && (at(i + 1) == b'x' || at(i + 1) == b'X') && at(i + 2).is_ascii_hexdigit() {
        i += 2;
        let digits = i;
        while at(i).is_ascii_hexdigit() {
            i += 1;
        }
        let text = src.get(digits..i).unwrap_or("");
        return match u64::from_str_radix(text, 16) {
            Ok(n) => Ok((n as f64, i)),
            Err(_) => Err(ParseError::new("This hexadecimal number is too large.", start)),
        };
    }
    while at(i).is_ascii_digit() {
        i += 1;
    }
    if at(i) == b'.' && at(i + 1).is_ascii_digit() {
        i += 1;
        while at(i).is_ascii_digit() {
            i += 1;
        }
    }
    if at(i) == b'e' || at(i) == b'E' {
        let mut j = i + 1;
        if at(j) == b'+' || at(j) == b'-' {
            j += 1;
        }
        if at(j).is_ascii_digit() {
            while at(j).is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let text = src.get(start..i).unwrap_or("");
    match text.parse::<f64>() {
        Ok(n) => Ok((n, i)),
        Err(_) => Err(ParseError::new("This isn't a valid number.", start)),
    }
}

/// Reads a regular identifier, dots included (`Table.SelectRows`), from `start`.
fn read_identifier(src: &str, start: usize) -> usize {
    let mut i = start;
    loop {
        while let Some(c) = char_at(src, i).filter(|c| is_ident_part(*c)) {
            i += c.len_utf8();
        }
        match (char_at(src, i), char_at(src, i + 1)) {
            (Some('.'), Some(c)) if is_ident_start(c) => i += 1,
            _ => return i,
        }
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        i = skip_trivia(src, i)?;
        let start = i;
        let Some(c) = char_at(src, i) else {
            out.push(Token { tok: Tok::Eof, start, end: start });
            return Ok(out);
        };
        if out.len() >= MAX_TOKENS {
            return Err(ParseError::new("The query is too long.", start));
        }
        let next = char_at(src, i + c.len_utf8());
        let (tok, end) = match c {
            '0'..='9' => {
                let (n, end) = read_number(src, i)?;
                (Tok::Number(n), end)
            }
            '.' if next.is_some_and(|d| d.is_ascii_digit()) => {
                let (n, end) = read_number(src, i)?;
                (Tok::Number(n), end)
            }
            '.' => {
                if src.get(i..).is_some_and(|s| s.starts_with("...")) {
                    (Tok::Ellipsis, i + 3)
                } else if next == Some('.') {
                    (Tok::DotDot, i + 2)
                } else {
                    return Err(ParseError::new("A period isn't expected here.", i));
                }
            }
            '"' => {
                let (s, end) = read_quoted(src, i + 1, i)?;
                (Tok::Text(Rc::from(s)), end)
            }
            '#' if next == Some('"') => {
                let (s, end) = read_quoted(src, i + 2, i)?;
                (Tok::Ident(Rc::from(s)), end)
            }
            '#' if next.is_some_and(is_ident_start) => {
                let end = read_identifier(src, i + 1);
                let word = src.get(i..end).unwrap_or("");
                match word {
                    "#infinity" => (Tok::Number(f64::INFINITY), end),
                    "#nan" => (Tok::Number(f64::NAN), end),
                    "#binary" | "#date" | "#datetime" | "#datetimezone" | "#duration" | "#sections" | "#shared" | "#table" | "#time" => {
                        (Tok::Ident(Rc::from(word)), end)
                    }
                    _ => return Err(ParseError::new(format!("{word} isn't a keyword. Quote names that need it: #\"name\"."), i)),
                }
            }
            c if is_ident_start(c) => {
                let end = read_identifier(src, i);
                let word = src.get(i..end).unwrap_or("");
                match keyword(word) {
                    Some(k) => (Tok::Kw(k), end),
                    None => (Tok::Ident(Rc::from(word)), end),
                }
            }
            ',' => (Tok::Comma, i + 1),
            ';' => (Tok::Semi, i + 1),
            '=' if next == Some('>') => (Tok::Arrow, i + 2),
            '=' => (Tok::Eq, i + 1),
            '<' if next == Some('=') => (Tok::Le, i + 2),
            '<' if next == Some('>') => (Tok::Ne, i + 2),
            '<' => (Tok::Lt, i + 1),
            '>' if next == Some('=') => (Tok::Ge, i + 2),
            '>' => (Tok::Gt, i + 1),
            '+' => (Tok::Plus, i + 1),
            '-' => (Tok::Minus, i + 1),
            '*' => (Tok::Star, i + 1),
            '/' => (Tok::Slash, i + 1),
            '&' => (Tok::Amp, i + 1),
            '(' => (Tok::LParen, i + 1),
            ')' => (Tok::RParen, i + 1),
            '[' => (Tok::LBracket, i + 1),
            ']' => (Tok::RBracket, i + 1),
            '{' => (Tok::LBrace, i + 1),
            '}' => (Tok::RBrace, i + 1),
            '@' => (Tok::At, i + 1),
            '!' => (Tok::Bang, i + 1),
            '?' if next == Some('?') => (Tok::Coalesce, i + 2),
            '?' => (Tok::Question, i + 1),
            other => return Err(ParseError::new(format!("The character '{other}' isn't expected here."), i)),
        };
        out.push(Token { tok, start, end });
        i = end;
    }
}

/// Reads a generalized identifier (a field name such as `Unit Price` or `2024 Sales`) from
/// `start`. Returns the name and the offset just past it; `None` when there is no name there.
pub(crate) fn read_generalized_identifier(src: &str, start: usize) -> Option<(&str, usize)> {
    let mut i = start;
    let mut end = start;
    while let Some(c) = char_at(src, i) {
        if is_ident_part(c) || c == '.' {
            i += c.len_utf8();
            end = i;
        } else if c == ' ' {
            i += 1;
        } else {
            break;
        }
    }
    let name = src.get(start..end)?;
    if name.is_empty() { None } else { Some((name, end)) }
}

/// Whether `name` can be written without quoting as an identifier.
pub fn is_regular_identifier(name: &str) -> bool {
    if keyword(name).is_some() || name.is_empty() {
        return false;
    }
    name.split('.').all(|part| {
        let mut cs = part.chars();
        cs.next().is_some_and(is_ident_start) && cs.all(is_ident_part)
    })
}

/// `name` as M source: bare when it can be, otherwise `#"quoted"`.
pub fn quote_identifier(name: &str) -> String {
    if is_regular_identifier(name) { name.to_string() } else { format!("#{}", quote_text(name)) }
}

/// `text` as an M text literal.
pub fn quote_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\"\""),
            '\r' => out.push_str("#(cr)"),
            '\n' => out.push_str("#(lf)"),
            '\t' => out.push_str("#(tab)"),
            // Only a `#` that would start an escape sequence needs escaping.
            '#' if chars.peek() == Some(&'(') => out.push_str("#(#)"),
            c if c.is_control() => out.push_str(&format!("#({:04X})", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
