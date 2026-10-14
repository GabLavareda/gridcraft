//! `Splitter.*` and `Combiner.*`: functions that make the functions which split a text into a
//! list and combine a list into a text. Also `Table.FromList` and `Table.ToList`, which use them.

use std::rc::Rc;

use super::table::{check_size, default_names, make};
use super::*;
use crate::error::{EResult, raise};
use crate::interp::{Interp, MAX_TEXT, check_len, too_large};
use crate::value::{Builtin, Function, Row, Slot, Type, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Splitter.SplitTextByDelimiter", 1, 3, |_, a| { text(a, 0)?; bind(&BY_DELIMITER, &[arg(a, 0), arg(a, 1)]) };
    "Splitter.SplitTextByAnyDelimiter", 1, 3, |_, a| { names(arg(a, 0))?; bind(&BY_ANY_DELIMITER, &[arg(a, 0), arg(a, 1), arg(a, 2)]) };
    "Splitter.SplitTextByEachDelimiter", 1, 3, |_, a| { names(arg(a, 0))?; bind(&BY_EACH_DELIMITER, &[arg(a, 0), arg(a, 1), arg(a, 2)]) };
    "Splitter.SplitTextByPositions", 1, 2, |_, a| { list(a, 0)?; bind(&BY_POSITIONS, &[arg(a, 0), arg(a, 1)]) };
    "Splitter.SplitTextByLengths", 1, 2, |_, a| { list(a, 0)?; bind(&BY_LENGTHS, &[arg(a, 0), arg(a, 1)]) };
    "Splitter.SplitTextByRepeatedLengths", 1, 2, |_, a| { count(a, 0)?; bind(&BY_REPEATED_LENGTHS, &[arg(a, 0), arg(a, 1)]) };
    "Splitter.SplitTextByWhitespace", 0, 1, |_, a| bind(&BY_WHITESPACE, &[arg(a, 0)]);
    "Splitter.SplitTextByCharacterTransition", 2, 2, |_, a| bind(&BY_TRANSITION, &[arg(a, 0), arg(a, 1)]);
    "Splitter.SplitByNothing", 0, 0, |_, _| bind(&BY_NOTHING, &[]);
    "Combiner.CombineTextByDelimiter", 1, 2, |_, a| { text(a, 0)?; bind(&COMBINE_BY_DELIMITER, &[arg(a, 0), arg(a, 1)]) };
    "Combiner.CombineTextByEachDelimiter", 1, 2, |_, a| { names(arg(a, 0))?; bind(&COMBINE_BY_EACH_DELIMITER, &[arg(a, 0), arg(a, 1)]) };
    "Combiner.CombineTextByLengths", 1, 2, |_, a| { list(a, 0)?; bind(&COMBINE_BY_LENGTHS, &[arg(a, 0), arg(a, 1)]) };
    "Table.FromList", 1, 5, from_list;
    "Table.ToList", 1, 2, to_list;
];

/// The function `f` with its first arguments fixed.
fn bind(f: &'static Builtin, fixed: &[&Value]) -> EResult<Value> {
    Ok(Value::Function(Rc::new(Function::Bound(f, fixed.iter().map(|v| (*v).clone()).collect()))))
}

/// A static function that takes `fixed` configuration values, then the value to work on.
const fn bound(name: &'static str, fixed: u8, f: crate::value::BuiltinFn) -> Builtin {
    Builtin { name, min: fixed + 1, max: fixed + 1, f }
}

static BY_DELIMITER: Builtin = bound("Splitter.SplitTextByDelimiter", 2, |_, a| {
    let Some(s) = opt_text(a, 2)? else { return Ok(Value::list(Vec::new())) };
    texts(split_by(&s, &[text(a, 0)?], quoted(arg(a, 1)), false))
});

static BY_ANY_DELIMITER: Builtin = bound("Splitter.SplitTextByAnyDelimiter", 3, |_, a| {
    let Some(s) = opt_text(a, 3)? else { return Ok(Value::list(Vec::new())) };
    texts(split_by(&s, &names(arg(a, 0))?, quoted(arg(a, 1)), false))
});

static BY_EACH_DELIMITER: Builtin = bound("Splitter.SplitTextByEachDelimiter", 3, |_, a| {
    let Some(s) = opt_text(a, 3)? else { return Ok(Value::list(Vec::new())) };
    let delimiters = names(arg(a, 0))?;
    let from_end = matches!(arg(a, 2), Value::Logical(true));
    texts(split_each(&s, &delimiters, quoted(arg(a, 1)), from_end))
});

static BY_POSITIONS: Builtin = bound("Splitter.SplitTextByPositions", 2, |_, a| {
    let Some(s) = opt_text(a, 2)? else { return Ok(Value::list(Vec::new())) };
    let chars: Vec<char> = s.chars().collect();
    let from_end = matches!(arg(a, 1), Value::Logical(true));
    let mut cuts = Vec::new();
    for v in list(a, 0)?.values()? {
        let p = count(std::slice::from_ref(&v), 0)?;
        cuts.push(if from_end { chars.len().saturating_sub(p) } else { p }.min(chars.len()));
    }
    cuts.sort_unstable();
    let mut parts = Vec::with_capacity(cuts.len());
    for (i, start) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).copied().unwrap_or(chars.len());
        parts.push(chars.get(*start..end).unwrap_or(&[]).iter().collect::<String>());
    }
    texts(parts)
});

static BY_LENGTHS: Builtin = bound("Splitter.SplitTextByLengths", 2, |_, a| {
    let Some(s) = opt_text(a, 2)? else { return Ok(Value::list(Vec::new())) };
    let mut chars: Vec<char> = s.chars().collect();
    let from_end = matches!(arg(a, 1), Value::Logical(true));
    if from_end {
        chars.reverse();
    }
    let mut parts = Vec::new();
    let mut at = 0usize;
    for v in list(a, 0)?.values()? {
        let n = count(std::slice::from_ref(&v), 0)?;
        let end = at.saturating_add(n).min(chars.len());
        let mut piece: Vec<char> = chars.get(at..end).unwrap_or(&[]).to_vec();
        if from_end {
            piece.reverse();
        }
        parts.push(piece.into_iter().collect::<String>());
        at = end;
    }
    if from_end {
        parts.reverse();
    }
    texts(parts)
});

static BY_REPEATED_LENGTHS: Builtin = bound("Splitter.SplitTextByRepeatedLengths", 2, |_, a| {
    let Some(s) = opt_text(a, 2)? else { return Ok(Value::list(Vec::new())) };
    let n = count(a, 0)?;
    if n == 0 {
        return raise("The length must be at least 1.");
    }
    let chars: Vec<char> = s.chars().collect();
    if matches!(arg(a, 1), Value::Logical(true)) {
        // From the end: the short piece, if any, comes first.
        let lead = chars.len() % n;
        let (head, tail): (&[char], &[char]) = (chars.get(..lead).unwrap_or(&[]), chars.get(lead..).unwrap_or(&[]));
        let first = (!head.is_empty()).then(|| head.iter().collect::<String>());
        return texts(first.into_iter().chain(tail.chunks(n).map(|c| c.iter().collect())).collect());
    }
    texts(chars.chunks(n).map(|c| c.iter().collect()).collect())
});

static BY_WHITESPACE: Builtin = bound("Splitter.SplitTextByWhitespace", 1, |_, a| {
    let Some(s) = opt_text(a, 1)? else { return Ok(Value::list(Vec::new())) };
    texts(s.split_whitespace().map(String::from).collect())
});

static BY_TRANSITION: Builtin = bound("Splitter.SplitTextByCharacterTransition", 2, |it, a| {
    let Some(s) = opt_text(a, 2)? else { return Ok(Value::list(Vec::new())) };
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if let Some(p) = prev
            && in_class(it, arg(a, 0), p)?
            && in_class(it, arg(a, 1), c)?
        {
            parts.push(std::mem::take(&mut current));
        }
        current.push(c);
        prev = Some(c);
    }
    parts.push(current);
    texts(parts)
});

static BY_NOTHING: Builtin = bound("Splitter.SplitByNothing", 0, |_, a| Ok(Value::list(vec![Slot::Val(arg(a, 0).clone())])));

static COMBINE_BY_DELIMITER: Builtin = bound("Combiner.CombineTextByDelimiter", 2, |it, a| {
    let delimiter = text(a, 0)?;
    let parts = parts_of(it, arg(a, 2), quoted(arg(a, 1)).then_some(&*delimiter))?;
    joined(parts.iter().enumerate().flat_map(|(i, p)| [if i > 0 { &*delimiter } else { "" }, p.as_str()]))
});

static COMBINE_BY_EACH_DELIMITER: Builtin = bound("Combiner.CombineTextByEachDelimiter", 2, |it, a| {
    let delimiters = names(arg(a, 0))?;
    let parts = parts_of(it, arg(a, 2), None)?;
    joined(parts.iter().enumerate().flat_map(|(i, p)| [if i > 0 { delimiters.get(i - 1).map_or("", |d| &**d) } else { "" }, p.as_str()]))
});

static COMBINE_BY_LENGTHS: Builtin = bound("Combiner.CombineTextByLengths", 2, |it, a| {
    let lengths = list(a, 0)?.values()?;
    let parts = parts_of(it, arg(a, 2), None)?;
    let mut out = String::new();
    for (p, len) in parts.iter().zip(&lengths) {
        let n = count(std::slice::from_ref(len), 0)?;
        if n > MAX_TEXT {
            return too_large();
        }
        // Each part is cut or padded to its length.
        out.extend(p.chars().chain(std::iter::repeat(' ')).take(n));
    }
    Ok(Value::text(out))
});

fn texts(parts: Vec<String>) -> EResult<Value> {
    check_len(parts.len())?;
    Ok(Value::list(parts.into_iter().map(|p| Slot::Val(Value::text(p))).collect()))
}

fn joined<'s>(pieces: impl Iterator<Item = &'s str>) -> EResult<Value> {
    let mut out = String::new();
    for p in pieces {
        out.push_str(p);
        if out.len() > MAX_TEXT {
            return too_large();
        }
    }
    Ok(Value::text(out))
}

/// Whether a quote-style argument asks for CSV quoting. Without the argument it does.
fn quoted(v: &Value) -> bool {
    !matches!(v, Value::Number(n) if *n == 0.0)
}

/// The items of a list as text: null is empty, other values are written as `Text.From` would.
/// With `quote_for`, a part holding that delimiter or a quote is put in double quotes.
fn parts_of(it: &mut Interp, v: &Value, quote_for: Option<&str>) -> EResult<Vec<String>> {
    let Value::List(l) = v else { return mismatch(v, "a list of values to combine") };
    let culture = it.culture(None);
    let mut out = Vec::with_capacity(l.len());
    for s in &l.items {
        let part = match convert::to_text(&s.get()?, culture)? {
            Value::Text(t) => t.to_string(),
            _ => String::new(),
        };
        out.push(match quote_for {
            Some(d) if !d.is_empty() && (part.contains(d) || part.contains('"') || part.contains('\n')) => {
                format!("\"{}\"", part.replace('"', "\"\""))
            }
            _ => part,
        });
    }
    Ok(out)
}

/// Whether `c` belongs to a character class given as a list of characters or as a predicate.
fn in_class(it: &mut Interp, class: &Value, c: char) -> EResult<bool> {
    match class {
        Value::List(l) => Ok(l.items.iter().any(|s| matches!(s, Slot::Val(Value::Text(t)) if t.chars().eq(std::iter::once(c))))),
        f @ Value::Function(_) => it.test(f, Value::text(c.to_string())),
        v => mismatch(v, "a list of characters or a function"),
    }
}

/// Finds the earliest delimiter at or after byte `from`, outside quotes when `quotes`: its byte
/// offset and length.
fn next_delimiter(s: &str, from: usize, delimiters: &[Rc<str>], quotes: bool) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = from;
    let mut in_quotes = false;
    while let Some(&b) = bytes.get(i) {
        if quotes && b == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes && let Some(d) = delimiters.iter().find(|d| !d.is_empty() && bytes.get(i..i + d.len()) == Some(d.as_bytes())) {
            return Some((i, d.len()));
        }
        i += 1;
    }
    None
}

/// A piece of split text, with CSV quoting undone when `quotes`.
fn piece(s: &str, quotes: bool) -> String {
    if quotes && s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return s.get(1..s.len() - 1).unwrap_or("").replace("\"\"", "\"");
    }
    s.to_string()
}

/// Splits at every occurrence of any of the delimiters.
fn split_by(s: &str, delimiters: &[Rc<str>], quotes: bool, _from_end: bool) -> Vec<String> {
    let mut parts = Vec::new();
    let mut from = 0;
    while let Some((at, len)) = next_delimiter(s, from, delimiters, quotes) {
        parts.push(piece(s.get(from..at).unwrap_or(""), quotes));
        from = at + len;
    }
    parts.push(piece(s.get(from..).unwrap_or(""), quotes));
    parts
}

/// Splits once at each delimiter in turn, from the start or from the end.
fn split_each(s: &str, delimiters: &[Rc<str>], quotes: bool, from_end: bool) -> Vec<String> {
    let mut parts = Vec::new();
    if from_end {
        let mut end = s.len();
        for d in delimiters.iter().rev() {
            let head = s.get(..end).unwrap_or("");
            match (!d.is_empty()).then(|| head.rfind(&**d)).flatten() {
                Some(at) => {
                    parts.push(piece(head.get(at + d.len()..).unwrap_or(""), quotes));
                    end = at;
                }
                None => break,
            }
        }
        parts.push(piece(s.get(..end).unwrap_or(""), quotes));
        parts.reverse();
        return parts;
    }
    let mut from = 0;
    for d in delimiters {
        match next_delimiter(s, from, std::slice::from_ref(d), quotes) {
            Some((at, len)) => {
                parts.push(piece(s.get(from..at).unwrap_or(""), quotes));
                from = at + len;
            }
            None => break,
        }
    }
    parts.push(piece(s.get(from..).unwrap_or(""), quotes));
    parts
}

/// `Table.FromList(list, splitter?, columns?, default?, extraValues?)`.
fn from_list(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let l = list(a, 0)?;
    let default_splitter = bind(&BY_DELIMITER, &[&Value::text(","), &Value::Null])?;
    let splitter = opt_function(a, 1)?.unwrap_or(&default_splitter);
    let mut split: Vec<Vec<Slot>> = Vec::with_capacity(l.len());
    for s in &l.items {
        split.push(match it.call1(splitter, s.get()?)? {
            Value::List(parts) => parts.items.clone(),
            other => vec![Slot::Val(other)],
        });
    }
    let (columns, types): (Vec<Rc<str>>, Vec<Type>) = match arg(a, 2) {
        Value::Null => {
            let n = split.first().map_or(1, Vec::len).max(1);
            (default_names(n), vec![Type::Any; n])
        }
        Value::Number(_) => {
            let n = count(a, 2)?;
            (default_names(n), vec![Type::Any; n])
        }
        Value::Type(Type::Table(c)) => (c.fields.iter().map(|f| f.name.clone()).collect(), c.fields.iter().map(|f| f.ty.clone()).collect()),
        v => {
            let n = names(v)?;
            let types = vec![Type::Any; n.len()];
            (n, types)
        }
    };
    let width = columns.len();
    check_size(split.len(), width)?;
    let default = Slot::Val(arg(a, 3).clone());
    // Extra values: an error by default, dropped with ExtraValues.Ignore, or kept as a list in
    // the last column with ExtraValues.List.
    let extra = opt_int(a, 4)?.unwrap_or(1);
    let mut rows: Vec<Row> = Vec::with_capacity(split.len());
    for mut cells in split {
        if cells.len() > width {
            match extra {
                0 if width > 0 => {
                    let rest = cells.split_off(width - 1);
                    cells.push(Slot::Val(Value::list(rest)));
                }
                2 => cells.truncate(width),
                _ => return raise(format!("An item gives {} values, but the table has {width} column(s).", cells.len())),
            }
        }
        cells.resize(width, default.clone());
        rows.push(cells.into());
    }
    make(columns, types, rows)
}

/// `Table.ToList(table, combiner?)`: each row combined into one value, with commas by default.
fn to_list(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let t = table(a, 0)?;
    let default_combiner = bind(&COMBINE_BY_DELIMITER, &[&Value::text(","), &Value::Null])?;
    let combiner = opt_function(a, 1)?.unwrap_or(&default_combiner);
    let mut out = Vec::with_capacity(t.rows.len());
    for r in &t.rows {
        out.push(Slot::capture(it.call1(combiner, Value::list(r.to_vec())))?);
    }
    Ok(Value::list(out))
}
