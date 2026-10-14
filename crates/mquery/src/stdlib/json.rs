//! JSON: a reader that keeps the order of object members, and a writer.

use std::rc::Rc;

use crate::error::{DATA_FORMAT_ERROR, EResult, error, raise};
use crate::interp::check_len;
use crate::value::{MAX_VALUE_DEPTH, Slot, Value, number_to_text};

struct Reader<'a> {
    src: &'a str,
    pos: usize,
}

fn bad<T>(message: &str, pos: usize) -> EResult<T> {
    Err(error(DATA_FORMAT_ERROR, format!("The JSON isn't valid at position {pos}: {message}")))
}

/// Parses a JSON document: objects become records, arrays lists, numbers numbers.
pub(crate) fn parse(src: &str) -> EResult<Value> {
    let mut r = Reader { src: src.trim_start_matches('\u{feff}'), pos: 0 };
    let v = r.value(0)?;
    r.skip_ws();
    if r.pos < r.src.len() {
        return bad("there is more after the end of the document.", r.pos);
    }
    Ok(v)
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }
    fn expect(&mut self, b: u8) -> EResult<()> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            bad(&format!("'{}' is expected.", b as char), self.pos)
        }
    }
    fn word(&mut self, word: &str, v: Value) -> EResult<Value> {
        if self.src.get(self.pos..).is_some_and(|s| s.starts_with(word)) {
            self.pos += word.len();
            Ok(v)
        } else {
            bad("a value is expected.", self.pos)
        }
    }

    fn value(&mut self, depth: usize) -> EResult<Value> {
        if depth > MAX_VALUE_DEPTH {
            return bad("it nests too deeply.", self.pos);
        }
        self.skip_ws();
        match self.peek() {
            Some(b'{') => {
                self.pos += 1;
                let mut names: Vec<Rc<str>> = Vec::new();
                let mut values: Vec<Slot> = Vec::new();
                self.skip_ws();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    return Ok(Value::record(names.into(), values.into()));
                }
                loop {
                    self.skip_ws();
                    let name: Rc<str> = Rc::from(self.string()?);
                    self.skip_ws();
                    self.expect(b':')?;
                    let v = self.value(depth + 1)?;
                    // A repeated member keeps its first position and its last value.
                    match names.iter().position(|n| *n == name) {
                        Some(i) => {
                            if let Some(slot) = values.get_mut(i) {
                                *slot = Slot::Val(v);
                            }
                        }
                        None => {
                            names.push(name);
                            values.push(Slot::Val(v));
                        }
                    }
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Value::record(names.into(), values.into()));
                        }
                        _ => return bad("',' or '}' is expected.", self.pos),
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_ws();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Ok(Value::list(items));
                }
                loop {
                    items.push(Slot::Val(self.value(depth + 1)?));
                    check_len(items.len())?;
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Value::list(items));
                        }
                        _ => return bad("',' or ']' is expected.", self.pos),
                    }
                }
            }
            Some(b'"') => Ok(Value::text(self.string()?)),
            Some(b't') => self.word("true", Value::Logical(true)),
            Some(b'f') => self.word("false", Value::Logical(false)),
            Some(b'n') => self.word("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.pos;
                while matches!(self.peek(), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.pos += 1;
                }
                match self.src.get(start..self.pos).and_then(|s| s.parse::<f64>().ok()) {
                    Some(n) => Ok(Value::Number(n)),
                    None => bad("this isn't a number.", start),
                }
            }
            _ => bad("a value is expected.", self.pos),
        }
    }

    fn hex4(&mut self) -> EResult<u32> {
        let h = self.src.get(self.pos..self.pos + 4).and_then(|s| u32::from_str_radix(s, 16).ok());
        match h {
            Some(h) => {
                self.pos += 4;
                Ok(h)
            }
            None => bad("four hexadecimal digits are expected after \\u.", self.pos),
        }
    }

    fn string(&mut self) -> EResult<String> {
        let open = self.pos;
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let rest = self.src.get(self.pos..).unwrap_or("");
            // Copy the run up to the next quote or escape in one go.
            let run = rest.find(['"', '\\']).unwrap_or(rest.len());
            out.push_str(rest.get(..run).unwrap_or(""));
            self.pos += run;
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let c = self.peek();
                    self.pos += 1;
                    match c {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            let hi = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&hi) && self.src.get(self.pos..).is_some_and(|s| s.starts_with("\\u")) {
                                self.pos += 2;
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xd800) << 10) + lo.wrapping_sub(0xdc00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return bad("this escape isn't recognized.", self.pos),
                    }
                }
                _ => return bad("the text is never closed.", open),
            }
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Writes a value as JSON: tables as arrays of objects, dates and times as ISO 8601 text.
pub(crate) fn write(v: &Value, out: &mut String, depth: usize) -> EResult<()> {
    if depth > MAX_VALUE_DEPTH {
        return raise("The value nests too deeply to write as JSON.");
    }
    let slot = |s: &Slot, out: &mut String| -> EResult<()> { write(&s.get()?, out, depth + 1) };
    match v {
        Value::Null => out.push_str("null"),
        Value::Logical(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if n.is_finite() => out.push_str(&number_to_text(*n)),
        Value::Number(n) => write_string(&number_to_text(*n), out),
        Value::Text(s) => write_string(s, out),
        Value::Date(_) | Value::Time(_) | Value::DateTime(_) | Value::DateTimeZone(..) => {
            use crate::datetime::{Moment, format, split_ticks};
            let c = &crate::culture::INVARIANT;
            let s = match v {
                Value::Date(d) => format(Moment { days: *d as i64, ..Default::default() }, "yyyy-MM-dd", c),
                Value::Time(t) => format(Moment { tod: *t, ..Default::default() }, "HH:mm:ss.FFFFFFF", c),
                Value::DateTime(t) => {
                    let (days, tod) = split_ticks(*t);
                    format(Moment { days, tod, offset: None }, "yyyy-MM-ddTHH:mm:ss.FFFFFFF", c)
                }
                Value::DateTimeZone(t, off) => {
                    let (days, tod) = split_ticks(*t);
                    format(Moment { days, tod, offset: Some(*off) }, "yyyy-MM-ddTHH:mm:ss.FFFFFFFK", c)
                }
                _ => String::new(),
            };
            write_string(&s, out);
        }
        Value::Duration(t) => write_string(&crate::datetime::duration_to_text(*t), out),
        Value::List(l) => {
            out.push('[');
            for (i, s) in l.items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                slot(s, out)?;
            }
            out.push(']');
        }
        Value::Record(r) => {
            out.push('{');
            for (i, (n, s)) in r.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(n, out);
                out.push(':');
                slot(s, out)?;
            }
            out.push('}');
        }
        Value::Table(t) => {
            out.push('[');
            for (i, row) in t.rows.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('{');
                for (j, (n, s)) in t.columns.iter().zip(row.iter()).enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    write_string(n, out);
                    out.push(':');
                    slot(s, out)?;
                }
                out.push('}');
            }
            out.push(']');
        }
        v => return raise(format!("A value of type {} can't be written as JSON.", v.kind())),
    }
    if out.len() > crate::interp::MAX_TEXT {
        return crate::interp::too_large();
    }
    Ok(())
}
