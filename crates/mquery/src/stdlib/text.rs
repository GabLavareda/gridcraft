//! `Text.*` and `Character.*`. Positions and lengths count characters (Unicode scalar values).

use std::rc::Rc;

use super::keys::Comparer;
use super::*;
use crate::error::{EResult, raise};
use crate::interp::{Interp, MAX_TEXT, check_len, too_large};
use crate::value::{Builtin, Slot, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Text.From", 1, 2, from;
    "Text.Length", 1, 1, length;
    "Text.Upper", 1, 2, upper;
    "Text.Lower", 1, 2, lower;
    "Text.Proper", 1, 2, proper;
    "Text.Trim", 1, 2, |_, a| trim(a, true, true);
    "Text.TrimStart", 1, 2, |_, a| trim(a, true, false);
    "Text.TrimEnd", 1, 2, |_, a| trim(a, false, true);
    "Text.Clean", 1, 1, clean;
    "Text.Start", 2, 2, start;
    "Text.End", 2, 2, end;
    "Text.Middle", 2, 3, middle;
    "Text.Range", 2, 3, range;
    "Text.At", 2, 2, at;
    "Text.Contains", 2, 3, contains;
    "Text.StartsWith", 2, 3, starts_with;
    "Text.EndsWith", 2, 3, ends_with;
    "Text.PositionOf", 2, 4, position_of;
    "Text.PositionOfAny", 2, 3, position_of_any;
    "Text.Replace", 3, 3, replace;
    "Text.ReplaceRange", 4, 4, replace_range;
    "Text.RemoveRange", 2, 3, remove_range;
    "Text.Insert", 3, 3, insert;
    "Text.Remove", 2, 2, |_, a| keep_chars(a, false);
    "Text.Select", 2, 2, |_, a| keep_chars(a, true);
    "Text.Split", 2, 2, split;
    "Text.SplitAny", 2, 2, split_any;
    "Text.Combine", 1, 2, combine;
    "Text.PadStart", 2, 3, |_, a| pad(a, true);
    "Text.PadEnd", 2, 3, |_, a| pad(a, false);
    "Text.Repeat", 2, 2, repeat;
    "Text.Reverse", 1, 1, reverse;
    "Text.ToList", 1, 1, to_list;
    "Text.BeforeDelimiter", 2, 3, before_delimiter;
    "Text.AfterDelimiter", 2, 3, after_delimiter;
    "Text.BetweenDelimiters", 3, 5, between_delimiters;
    "Text.Format", 2, 3, format;
    "Character.FromNumber", 1, 1, char_from_number;
    "Character.ToNumber", 1, 1, char_to_number;
];

/// The text argument, or `None` for null: most text functions give null for null.
macro_rules! text_or_null {
    ($a:expr, $i:expr) => {
        match opt_text($a, $i)? {
            Some(s) => s,
            None => return Ok(Value::Null),
        }
    };
}

fn out(s: impl AsRef<str>) -> EResult<Value> {
    if s.as_ref().len() > MAX_TEXT { too_large() } else { Ok(Value::text(s)) }
}

/// Byte offset of character position `n`, the end of the text when `n` is past it.
fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `s` folded for a comparison that may ignore case.
fn fold(s: &str, ignore_case: bool) -> std::borrow::Cow<'_, str> {
    if ignore_case { s.to_lowercase().into() } else { s.into() }
}

fn from(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let culture = it.culture(opt_text(a, 1)?.as_deref());
    convert::to_text(arg(a, 0), culture)
}

fn length(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    Ok(Value::Number(char_len(&text_or_null!(a, 0)) as f64))
}

fn upper(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    out(text_or_null!(a, 0).to_uppercase())
}

fn lower(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    out(text_or_null!(a, 0).to_lowercase())
}

fn proper(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let mut r = String::with_capacity(s.len());
    let mut word_start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if word_start {
                r.extend(c.to_uppercase());
            } else {
                r.extend(c.to_lowercase());
            }
            word_start = false;
        } else {
            r.push(c);
            word_start = true;
        }
    }
    out(r)
}

/// The characters to trim: whitespace by default, or the ones given as text or a list.
fn trim_set(v: &Value) -> EResult<Option<Vec<char>>> {
    match v {
        Value::Null => Ok(None),
        Value::Text(s) => Ok(Some(s.chars().collect())),
        Value::List(l) => {
            let mut set = Vec::new();
            for s in &l.items {
                match s.get()? {
                    Value::Text(s) => set.extend(s.chars()),
                    v => return mismatch(&v, "a character"),
                }
            }
            Ok(Some(set))
        }
        v => mismatch(v, "the characters to trim"),
    }
}

fn trim(a: &[Value], at_start: bool, at_end: bool) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let set = trim_set(arg(a, 1))?;
    let is_trim = |c: char| match &set {
        Some(set) => set.contains(&c),
        None => c.is_whitespace(),
    };
    let mut r: &str = &s;
    if at_start {
        r = r.trim_start_matches(is_trim);
    }
    if at_end {
        r = r.trim_end_matches(is_trim);
    }
    out(r)
}

fn clean(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    out(text_or_null!(a, 0).chars().filter(|c| !c.is_control()).collect::<String>())
}

fn start(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    out(s.get(..byte_at(&s, count(a, 1)?)).unwrap_or(&s))
}

fn end(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let n = count(a, 1)?;
    let skip = char_len(&s).saturating_sub(n);
    out(s.get(byte_at(&s, skip)..).unwrap_or(&s))
}

fn middle(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let from = byte_at(&s, count(a, 1)?);
    let rest = s.get(from..).unwrap_or("");
    match opt_count(a, 2)? {
        Some(n) => out(rest.get(..byte_at(rest, n)).unwrap_or(rest)),
        None => out(rest),
    }
}

fn range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let (offset, len) = (count(a, 1)?, char_len(&s));
    let n = opt_count(a, 2)?.unwrap_or(len.saturating_sub(offset));
    if offset > len || n > len - offset {
        return raise("The range is outside the text.");
    }
    let rest = s.get(byte_at(&s, offset)..).unwrap_or("");
    out(rest.get(..byte_at(rest, n)).unwrap_or(rest))
}

fn at(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    match s.chars().nth(count(a, 1)?) {
        Some(c) => out(c.to_string()),
        None => raise("The position is outside the text."),
    }
}

fn contains(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (s, sub) = (text_or_null!(a, 0), text_or_null!(a, 1));
    let ic = Comparer::from(arg(a, 2))?.ignore_case();
    Ok(Value::Logical(fold(&s, ic).contains(&*fold(&sub, ic))))
}

fn starts_with(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (s, sub) = (text_or_null!(a, 0), text_or_null!(a, 1));
    let ic = Comparer::from(arg(a, 2))?.ignore_case();
    Ok(Value::Logical(fold(&s, ic).starts_with(&*fold(&sub, ic))))
}

fn ends_with(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (s, sub) = (text_or_null!(a, 0), text_or_null!(a, 1));
    let ic = Comparer::from(arg(a, 2))?.ignore_case();
    Ok(Value::Logical(fold(&s, ic).ends_with(&*fold(&sub, ic))))
}

/// Character positions of every (possibly overlapping) occurrence of `sub` in `s`.
fn occurrences(s: &str, sub: &str) -> Vec<usize> {
    let mut found = Vec::new();
    if sub.is_empty() {
        return found;
    }
    for (n, (i, _)) in s.char_indices().enumerate() {
        if s.get(i..).is_some_and(|rest| rest.starts_with(sub)) {
            found.push(n);
        }
    }
    found
}

/// Picks from positions by an occurrence argument: first (default), last, or all as a list.
fn pick_occurrence(found: Vec<usize>, occurrence: &Value) -> EResult<Value> {
    let pos = |p: Option<&usize>| Value::Number(p.map_or(-1.0, |p| *p as f64));
    match occurrence {
        Value::Null => Ok(pos(found.first())),
        Value::Number(n) if *n == 0.0 => Ok(pos(found.first())),
        Value::Number(n) if *n == 1.0 => Ok(pos(found.last())),
        Value::Number(n) if *n == 2.0 => Ok(Value::list_of(found.into_iter().map(|p| Value::Number(p as f64)).collect())),
        v => mismatch(v, "an occurrence (first, last or all)"),
    }
}

fn position_of(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (s, sub) = (text_or_null!(a, 0), text(a, 1)?);
    let ic = Comparer::from(arg(a, 3))?.ignore_case();
    pick_occurrence(occurrences(&fold(&s, ic), &fold(&sub, ic)), arg(a, 2))
}

fn position_of_any(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let set = trim_set(arg(a, 1))?.unwrap_or_default();
    let found = s.chars().enumerate().filter(|(_, c)| set.contains(c)).map(|(i, _)| i).collect();
    pick_occurrence(found, arg(a, 2))
}

fn replace(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let (old, new) = (text(a, 1)?, text(a, 2)?);
    if old.is_empty() {
        return Ok(Value::Text(s));
    }
    if new.len() > old.len() && s.len() / old.len().max(1) * new.len() > MAX_TEXT {
        return too_large();
    }
    out(s.replace(&*old, &new))
}

/// The byte range of `n` characters from character position `offset`, when inside the text.
fn char_range(s: &str, offset: usize, n: usize) -> EResult<(usize, usize)> {
    let len = char_len(s);
    if offset > len || n > len - offset {
        return raise("The range is outside the text.");
    }
    let from = byte_at(s, offset);
    let rest = s.get(from..).unwrap_or("");
    Ok((from, from + byte_at(rest, n)))
}

fn replace_range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let (from, to) = char_range(&s, count(a, 1)?, count(a, 2)?)?;
    let new = text(a, 3)?;
    out(format!("{}{}{}", s.get(..from).unwrap_or(""), new, s.get(to..).unwrap_or("")))
}

fn remove_range(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let (from, to) = char_range(&s, count(a, 1)?, opt_count(a, 2)?.unwrap_or(1))?;
    out(format!("{}{}", s.get(..from).unwrap_or(""), s.get(to..).unwrap_or("")))
}

fn insert(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let (from, _) = char_range(&s, count(a, 1)?, 0)?;
    let new = text(a, 2)?;
    out(format!("{}{}{}", s.get(..from).unwrap_or(""), new, s.get(from..).unwrap_or("")))
}

fn keep_chars(a: &[Value], keep: bool) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let set = trim_set(arg(a, 1))?.unwrap_or_default();
    out(s.chars().filter(|c| set.contains(c) == keep).collect::<String>())
}

fn text_list(parts: impl Iterator<Item = impl AsRef<str>>) -> EResult<Value> {
    let items: Vec<Slot> = parts.map(|p| Slot::Val(Value::text(p))).collect();
    check_len(items.len())?;
    Ok(Value::list(items))
}

fn split(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let sep = text(a, 1)?;
    if sep.is_empty() {
        return text_list(std::iter::once(&*s));
    }
    text_list(s.split(&*sep))
}

fn split_any(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let seps: Vec<char> = text(a, 1)?.chars().collect();
    text_list(s.split(|c| seps.contains(&c)))
}

fn combine(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(l) = opt_list(a, 0)? else { return Ok(Value::Null) };
    let sep = opt_text(a, 1)?.unwrap_or_else(|| Rc::from(""));
    let mut r = String::new();
    let mut first = true;
    for s in &l.items {
        match s.get()? {
            Value::Null => continue,
            Value::Text(t) => {
                if !first {
                    r.push_str(&sep);
                }
                r.push_str(&t);
                first = false;
            }
            v => return mismatch(&v, "text"),
        }
        if r.len() > MAX_TEXT {
            return too_large();
        }
    }
    out(r)
}

fn pad(a: &[Value], at_start: bool) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let width = count(a, 1)?;
    let fill = match opt_text(a, 2)? {
        None => ' ',
        Some(c) => {
            let mut cs = c.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => c,
                _ => return raise("The padding must be a single character."),
            }
        }
    };
    let missing = width.saturating_sub(char_len(&s));
    if missing > MAX_TEXT {
        return too_large();
    }
    let padding: String = std::iter::repeat_n(fill, missing).collect();
    out(if at_start { format!("{padding}{s}") } else { format!("{s}{padding}") })
}

fn repeat(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let n = count(a, 1)?;
    if s.len().saturating_mul(n) > MAX_TEXT {
        return too_large();
    }
    out(s.repeat(n))
}

fn reverse(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    out(text_or_null!(a, 0).chars().rev().collect::<String>())
}

fn to_list(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    check_len(s.len())?;
    text_list(s.chars().map(String::from))
}

/// A delimiter index argument: which occurrence, and whether counted from the end.
fn delimiter_index(v: &Value) -> EResult<(usize, bool)> {
    match v {
        Value::Null => Ok((0, false)),
        Value::Number(_) => Ok((count(std::slice::from_ref(v), 0)?, false)),
        Value::List(l) => {
            let vals = l.values()?;
            let n = count(&vals, 0)?;
            let from_end = matches!(vals.get(1), Some(Value::Number(d)) if *d == 1.0);
            Ok((n, from_end))
        }
        v => mismatch(v, "a delimiter index"),
    }
}

/// Byte offset of occurrence `index` of `delim` in `s`, from the start or from the end.
fn find_delimiter(s: &str, delim: &str, index: usize, from_end: bool) -> Option<usize> {
    if delim.is_empty() {
        return None;
    }
    if from_end { s.rmatch_indices(delim).nth(index).map(|(i, _)| i) } else { s.match_indices(delim).nth(index).map(|(i, _)| i) }
}

fn before_delimiter(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let delim = text(a, 1)?;
    let (index, from_end) = delimiter_index(arg(a, 2))?;
    match find_delimiter(&s, &delim, index, from_end) {
        Some(i) => out(s.get(..i).unwrap_or("")),
        None => Ok(Value::Text(s)),
    }
}

fn after_delimiter(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let delim = text(a, 1)?;
    let (index, from_end) = delimiter_index(arg(a, 2))?;
    match find_delimiter(&s, &delim, index, from_end) {
        Some(i) => out(s.get(i + delim.len()..).unwrap_or("")),
        None => out(""),
    }
}

fn between_delimiters(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    let (open, close) = (text(a, 1)?, text(a, 2)?);
    let (i1, end1) = delimiter_index(arg(a, 3))?;
    let (i2, end2) = delimiter_index(arg(a, 4))?;
    let Some(from) = find_delimiter(&s, &open, i1, end1).map(|i| i + open.len()) else { return out("") };
    let rest = s.get(from..).unwrap_or("");
    // The closing delimiter is counted from where the opening one ended (or back from it).
    let to = if end2 {
        find_delimiter(s.get(..from - open.len()).unwrap_or(""), &close, i2, true).map(|i| (i + close.len(), from - open.len()))
    } else {
        find_delimiter(rest, &close, i2, false).map(|i| (from, from + i))
    };
    match to {
        Some((x, y)) if x <= y => out(s.get(x..y).unwrap_or("")),
        Some(_) => out(""),
        None => out(rest),
    }
}

/// `Text.Format("#{0} of #[Total]", {1}, …)`: `#{n}` is item `n` of a list, `#[name]` a field
/// of a record.
fn format(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let template = text(a, 0)?;
    let culture = it.culture(opt_text(a, 2)?.as_deref());
    let mut r = String::new();
    let mut rest: &str = &template;
    while let Some(i) = rest.find('#') {
        r.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i + 1..).unwrap_or("");
        let (close, is_field) = match after.chars().next() {
            Some('{') => ('}', false),
            Some('[') => (']', true),
            _ => {
                r.push('#');
                rest = after;
                continue;
            }
        };
        let Some(j) = after.find(close) else { return raise("The format text has a placeholder that isn't closed.") };
        let key = after.get(1..j).unwrap_or("").trim();
        let v = if is_field {
            match arg(a, 1) {
                Value::Record(rec) => match rec.get(key)? {
                    Some(v) => v,
                    None => return raise(format!("The record has no field named '{key}'.")),
                },
                v => return mismatch(v, "a record"),
            }
        } else {
            let n: usize = match key.parse() {
                Ok(n) => n,
                Err(_) => return raise("A placeholder needs a number: #{0}."),
            };
            match arg(a, 1) {
                Value::List(l) => match l.items.get(n) {
                    Some(s) => s.get()?,
                    None => return raise(format!("The list has no item {n}.")),
                },
                v => return mismatch(v, "a list"),
            }
        };
        if let Value::Text(t) = convert::to_text(&v, culture)? {
            r.push_str(&t);
        }
        rest = after.get(j + 1..).unwrap_or("");
    }
    r.push_str(rest);
    out(r)
}

fn char_from_number(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(n) = opt_int(a, 0)? else { return Ok(Value::Null) };
    match u32::try_from(n).ok().and_then(char::from_u32) {
        Some(c) => out(c.to_string()),
        None => raise("The number isn't a character code."),
    }
}

fn char_to_number(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let s = text_or_null!(a, 0);
    match s.chars().next() {
        Some(c) => Ok(Value::Number(c as u32 as f64)),
        None => raise("The text is empty."),
    }
}
