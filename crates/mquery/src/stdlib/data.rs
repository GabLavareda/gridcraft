//! Data access: the current workbook, files, CSV, JSON, text encodings and binary values.

use std::rc::Rc;

use super::table::{check_size, default_names, make};
use super::*;
use crate::error::{DATA_FORMAT_ERROR, DATA_SOURCE_ERROR, EResult, error, raise};
use crate::interp::{Interp, MAX_TEXT, check_len, too_large};
use crate::value::{Builtin, Row, Slot, Table, Type, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Excel.CurrentWorkbook", 0, 0, current_workbook;
    "Excel.Workbook", 1, 3, excel_workbook;
    "File.Contents", 1, 2, |it, a| source(it.host.file_contents(&text(a, 0)?));
    "Csv.Document", 1, 5, csv_document;
    "Json.Document", 1, 2, |_, a| json::parse(&text_of(arg(a, 0), opt_number(a, 1)?)?);
    "Json.FromValue", 1, 2, json_from_value;
    "Lines.FromText", 1, 3, |_, a| Ok(lines(&text(a, 0)?));
    "Lines.FromBinary", 1, 4, |_, a| Ok(lines(&text_of(arg(a, 0), opt_number(a, 3)?)?));
    "Lines.ToText", 1, 2, lines_to_text;
    "Text.FromBinary", 1, 2, |_, a| Ok(match arg(a, 0) { Value::Null => Value::Null, v => Value::text(text_of(v, opt_number(a, 1)?)?) });
    "Text.ToBinary", 1, 3, |_, a| Ok(match opt_text(a, 0)? { Some(s) => Value::Binary(encode(&s, opt_number(a, 1)?)?.into()), None => Value::Null });
    "Binary.FromText", 1, 2, binary_from_text;
    "Binary.ToText", 1, 2, binary_to_text;
    "Binary.Length", 1, 1, |_, a| Ok(match arg(a, 0) { Value::Null => Value::Null, Value::Binary(b) => Value::Number(b.len() as f64), v => return mismatch(v, "a binary value") });
    "Binary.Buffer", 1, 1, |_, a| Ok(arg(a, 0).clone());
    "Binary.ToList", 1, 1, |_, a| match arg(a, 0) { Value::Binary(b) => { check_len(b.len())?; Ok(Value::list_of(b.iter().map(|x| Value::Number(*x as f64)).collect())) } v => mismatch(v, "a binary value") };
    "Binary.FromList", 1, 1, |_, a| Ok(Value::Binary(bytes_of(&list(a, 0)?.values()?)?.into()));
    "Binary.Combine", 1, 1, binary_combine;
    "#binary", 1, 1, |_, a| match arg(a, 0) { Value::Text(s) => Ok(Value::Binary(base64_decode(s)?.into())), Value::List(l) => Ok(Value::Binary(bytes_of(&l.values()?)?.into())), v => mismatch(v, "base64 text or a list of bytes") };
];

fn source(r: Result<Vec<u8>, String>) -> EResult<Value> {
    match r {
        Ok(bytes) => Ok(Value::Binary(bytes.into())),
        Err(message) => Err(error(DATA_SOURCE_ERROR, message)),
    }
}

fn current_workbook(it: &mut Interp, _: &[Value]) -> EResult<Value> {
    let items = it.host.current_workbook().map_err(|m| error(DATA_SOURCE_ERROR, m))?;
    let rows = items.into_iter().map(|(name, t)| [Slot::Val(Value::table(t)), Slot::Val(Value::text(name))].into_iter().collect()).collect();
    make(vec![Rc::from("Content"), Rc::from("Name")], vec![Type::any_table(), Type::Text], rows)
}

fn excel_workbook(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Value::Binary(bytes) = arg(a, 0) else { return mismatch(arg(a, 0), "the binary contents of a workbook") };
    let use_headers = match arg(a, 1) {
        Value::Record(r) => matches!(r.get("UseHeaders")?, Some(Value::Logical(true))),
        _ => opt_logical(a, 1)?.unwrap_or(false),
    };
    let parts = it.host.excel_workbook(bytes, use_headers).map_err(|m| error(DATA_SOURCE_ERROR, m))?;
    let rows = parts
        .into_iter()
        .map(|p| {
            [Value::text(&p.name), Value::table(p.data), Value::text(&p.name), Value::text(p.kind), Value::Logical(p.hidden)]
                .into_iter()
                .map(Slot::Val)
                .collect()
        })
        .collect();
    let columns = ["Name", "Data", "Item", "Kind", "Hidden"].into_iter().map(Rc::from).collect();
    make(columns, vec![Type::Text, Type::any_table(), Type::Text, Type::Text, Type::Logical], rows)
}

// ---------------------------------------------------------------- text encodings

/// The characters Windows-1252 has where Latin-1 has control codes (0x80 to 0x9F).
const CP1252: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}',
    '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

fn utf16(bytes: &[u8], big_endian: bool) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|p| match p {
        [x, y] if big_endian => u16::from_be_bytes([*x, *y]),
        [x, y] => u16::from_le_bytes([*x, *y]),
    });
    char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect()
}

/// Decodes bytes as text. Without a code page: UTF-8, or UTF-16 when a byte-order mark says so.
pub(crate) fn decode(bytes: &[u8], codepage: Option<f64>) -> EResult<String> {
    if bytes.len() > MAX_TEXT {
        return too_large();
    }
    Ok(match codepage.map(|c| c as i64) {
        None | Some(65001) => match bytes {
            [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
            [0xff, 0xfe, rest @ ..] if codepage.is_none() => utf16(rest, false),
            [0xfe, 0xff, rest @ ..] if codepage.is_none() => utf16(rest, true),
            _ => String::from_utf8_lossy(bytes).into_owned(),
        },
        Some(1200) => utf16(bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes), false),
        Some(1201) => utf16(bytes.strip_prefix(&[0xfe, 0xff]).unwrap_or(bytes), true),
        Some(1252) => bytes
            .iter()
            .map(|b| if (0x80..0xa0).contains(b) { CP1252.get((*b - 0x80) as usize).copied().unwrap_or('\u{fffd}') } else { *b as char })
            .collect(),
        Some(28591) => bytes.iter().map(|b| *b as char).collect(),
        Some(20127) => bytes.iter().map(|b| if b.is_ascii() { *b as char } else { '?' }).collect(),
        Some(other) => {
            return raise(format!("The text encoding {other} isn't supported. Use 65001 (UTF-8), 1200 or 1201 (UTF-16), 1252, 28591 or 20127."));
        }
    })
}

fn encode(s: &str, codepage: Option<f64>) -> EResult<Vec<u8>> {
    Ok(match codepage.map(|c| c as i64) {
        None | Some(65001) => s.as_bytes().to_vec(),
        Some(1200) => s.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        Some(1201) => s.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        Some(1252) => s
            .chars()
            .map(|c| {
                if (c as u32) < 0x80 || (0xa0..0x100).contains(&(c as u32)) {
                    c as u8
                } else {
                    CP1252.iter().position(|x| *x == c).map_or(b'?', |i| 0x80 + i as u8)
                }
            })
            .collect(),
        Some(28591) => s.chars().map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' }).collect(),
        Some(20127) => s.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }).collect(),
        Some(other) => return raise(format!("The text encoding {other} isn't supported.")),
    })
}

/// The text of a text or binary source.
fn text_of(v: &Value, codepage: Option<f64>) -> EResult<String> {
    match v {
        Value::Text(s) => Ok(s.to_string()),
        Value::Binary(b) => decode(b, codepage),
        v => mismatch(v, "text or binary content"),
    }
}

fn lines(s: &str) -> Value {
    Value::list_of(s.lines().map(Value::text).collect())
}

fn lines_to_text(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let sep = opt_text(a, 1)?.unwrap_or_else(|| Rc::from("\r\n"));
    let mut out = String::new();
    for s in &*list(a, 0)?.items {
        match s.get()? {
            Value::Text(t) => out.push_str(&t),
            v => return mismatch(&v, "text"),
        }
        out.push_str(&sep);
        if out.len() > MAX_TEXT {
            return too_large();
        }
    }
    Ok(Value::text(out))
}

// ---------------------------------------------------------------- CSV

/// Splits delimited text into rows of fields. A field in double quotes can hold the delimiter,
/// doubled quotes and, when `quoted_breaks`, line breaks.
pub(crate) fn parse_csv(src: &str, delimiter: &str, quoted_breaks: bool) -> EResult<Vec<Vec<Slot>>> {
    let bytes = src.as_bytes();
    let delim = delimiter.as_bytes();
    let mut rows: Vec<Vec<Slot>> = Vec::new();
    let mut row: Vec<Slot> = Vec::new();
    let mut field: Vec<u8> = Vec::new();
    let mut i = 0;
    let mut in_quotes = false;
    // Whether the current row has any content yet (so a trailing line break adds no empty row).
    let mut pending = false;
    let end_field = |field: &mut Vec<u8>, row: &mut Vec<Slot>| {
        row.push(Slot::Val(Value::text(String::from_utf8_lossy(field))));
        field.clear();
    };
    while let Some(&b) = bytes.get(i) {
        if in_quotes {
            match b {
                b'"' if bytes.get(i + 1) == Some(&b'"') => {
                    field.push(b'"');
                    i += 2;
                }
                b'"' => {
                    in_quotes = false;
                    i += 1;
                }
                b'\r' | b'\n' if !quoted_breaks => in_quotes = false,
                _ => {
                    field.push(b);
                    i += 1;
                }
            }
            continue;
        }
        if b == b'"' && field.is_empty() {
            in_quotes = true;
            pending = true;
            i += 1;
        } else if !delim.is_empty() && bytes.get(i..i + delim.len()) == Some(delim) {
            end_field(&mut field, &mut row);
            pending = true;
            i += delim.len();
        } else if b == b'\r' || b == b'\n' {
            end_field(&mut field, &mut row);
            rows.push(std::mem::take(&mut row));
            check_len(rows.len())?;
            pending = false;
            i += if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
        } else {
            field.push(b);
            pending = true;
            i += 1;
        }
    }
    if pending {
        end_field(&mut field, &mut row);
        rows.push(row);
    }
    Ok(rows)
}

fn csv_document(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let options = match arg(a, 1) {
        Value::Record(r) => Some(r.clone()),
        _ => None,
    };
    let opt = |name: &str| option(options.as_ref(), name);
    let columns_arg = if options.is_some() { opt("Columns")? } else { arg(a, 1).clone() };
    let delimiter = match (opt("Delimiter")?, arg(a, 2)) {
        (Value::Text(d), _) => d,
        (_, Value::Text(d)) => d.clone(),
        _ => Rc::from(","),
    };
    let encoding = match opt("Encoding")? {
        Value::Number(n) => Some(n),
        _ => opt_number(a, 4)?,
    };
    let quoted_breaks = !matches!(opt("QuoteStyle")?, Value::Number(n) if n == 0.0);
    let src = text_of(arg(a, 0), encoding)?;
    let rows = parse_csv(&src, &delimiter, quoted_breaks)?;
    let widest = rows.iter().map(Vec::len).max().unwrap_or(0);
    let (columns, types): (Vec<Rc<str>>, Vec<Type>) = match &columns_arg {
        Value::Null => (default_names(widest), vec![Type::Any; widest]),
        Value::Number(_) => {
            let n = count(std::slice::from_ref(&columns_arg), 0)?;
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
    check_size(rows.len(), width)?;
    let rows: Vec<Row> = rows
        .into_iter()
        .map(|mut r| {
            r.resize(width, Slot::NULL);
            r.into()
        })
        .collect();
    make(columns, types, rows)
}

fn json_from_value(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let mut out = String::new();
    json::write(arg(a, 0), &mut out, 0)?;
    Ok(Value::Binary(encode(&out, opt_number(a, 1)?)?.into()))
}

// ---------------------------------------------------------------- binary

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for (k, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if k <= chunk.len() {
                out.push(B64.get((n >> shift & 63) as usize).copied().unwrap_or(b'A') as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(s: &str) -> EResult<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return Err(error(DATA_FORMAT_ERROR, "The text isn't valid base64.")),
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    Ok(out)
}

fn bytes_of(values: &[Value]) -> EResult<Vec<u8>> {
    values
        .iter()
        .map(|v| match v {
            Value::Number(n) if n.fract() == 0.0 && (0.0..=255.0).contains(n) => Ok(*n as u8),
            v => mismatch(v, "a byte (a whole number from 0 to 255)"),
        })
        .collect()
}

fn binary_from_text(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(s) = opt_text(a, 0)? else { return Ok(Value::Null) };
    let bytes = match opt_number(a, 1)? {
        Some(1.0) => {
            let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
            if !digits.len().is_multiple_of(2) {
                return Err(error(DATA_FORMAT_ERROR, "Hexadecimal text needs two digits per byte."));
            }
            digits
                .chunks(2)
                .map(|p| {
                    std::str::from_utf8(p)
                        .ok()
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                        .ok_or_else(|| error(DATA_FORMAT_ERROR, "The text isn't valid hexadecimal."))
                })
                .collect::<EResult<Vec<u8>>>()?
        }
        _ => base64_decode(&s)?,
    };
    Ok(Value::Binary(bytes.into()))
}

fn binary_to_text(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let b = match arg(a, 0) {
        Value::Null => return Ok(Value::Null),
        Value::Binary(b) => b,
        v => return mismatch(v, "a binary value"),
    };
    if b.len() > MAX_TEXT / 2 {
        return too_large();
    }
    Ok(Value::text(match opt_number(a, 1)? {
        Some(1.0) => b.iter().map(|x| format!("{x:02x}")).collect::<String>(),
        _ => base64_encode(b),
    }))
}

fn binary_combine(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let mut out: Vec<u8> = Vec::new();
    for s in &*list(a, 0)?.items {
        match s.get()? {
            Value::Binary(b) => out.extend_from_slice(&b),
            v => return mismatch(&v, "a binary value"),
        }
        if out.len() > MAX_TEXT {
            return too_large();
        }
    }
    Ok(Value::Binary(out.into()))
}

/// A table of text cells, as `Csv.Document` gives it; for hosts that hold delimited text.
pub fn table_from_csv(src: &str, delimiter: &str) -> EResult<Table> {
    let rows = parse_csv(src, delimiter, true)?;
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    check_size(rows.len(), width)?;
    let rows = rows
        .into_iter()
        .map(|mut r| {
            r.resize(width, Slot::NULL);
            r.into()
        })
        .collect();
    Ok(Table::untyped(default_names(width).into(), rows))
}
