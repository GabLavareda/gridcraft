//! Sources beyond single files: folders (`Folder.Files`), XML (`Xml.Document`, `Xml.Tables`), the
//! web (`Web.Contents`, `Web.Page` for HTML tables) and OData feeds (`OData.Feed`).
//!
//! The evaluator does no I/O: folders and the web come from the [`crate::Host`]. Files of a folder
//! and the entity sets of an OData service are read only when a query reads them (lazy cells).
//! XML and HTML are parsed here, with their nesting and sizes bounded.

use std::rc::Rc;

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

use super::*;
use crate::error::{DATA_FORMAT_ERROR, DATA_SOURCE_ERROR, EResult, EvalError, MError, error, raise};
use crate::host::Fetch;
use crate::interp::{Interp, MAX_CELLS, MAX_ITEMS, MAX_TEXT, check_len};
use crate::value::{Builtin, Lazy, Names, Row, Slot, Table, Type, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Folder.Files", 1, 2, folder_files;
    "Web.Contents", 1, 2, web_contents;
    "Web.Page", 1, 1, |_, a| web_page(&text_of(arg(a, 0))?);
    "Xml.Document", 1, 2, |_, a| xml_document(&text_of(arg(a, 0))?);
    "Xml.Tables", 1, 3, |_, a| xml_tables(&text_of(arg(a, 0))?);
    "OData.Feed", 1, 3, odata_feed;
];

/// Deepest XML or HTML nesting read.
const MAX_DEPTH: usize = 256;
/// Most pages of an OData feed followed.
const MAX_PAGES: usize = 10_000;

fn ds(message: String) -> EvalError {
    error(DATA_SOURCE_ERROR, message)
}

fn format_error(message: impl Into<String>) -> EvalError {
    error(DATA_FORMAT_ERROR, message.into())
}

fn names(list: &[&str]) -> Names {
    list.iter().map(|n| Rc::from(*n)).collect()
}

/// The text of text or binary content (UTF-8, a byte order mark dropped).
fn text_of(v: &Value) -> EResult<String> {
    match v {
        Value::Text(s) => Ok(s.to_string()),
        Value::Binary(b) => {
            if b.len() > MAX_TEXT {
                return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
            }
            let s = String::from_utf8_lossy(b);
            Ok(s.strip_prefix('\u{feff}').unwrap_or(&s).to_string())
        }
        v => mismatch(v, "text or binary content"),
    }
}

fn lazy_error(e: EvalError) -> Rc<MError> {
    match e {
        EvalError::Raised(e) => e,
        EvalError::Fatal(f) => Rc::new(MError { reason: Rc::from(DATA_SOURCE_ERROR), message: Rc::from(f.to_string()), detail: Value::Null }),
    }
}

// ---------------------------------------------------------------- folders

fn folder_files(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let path = text(a, 0)?;
    let files = it.host.folder_files(&path).map_err(ds)?;
    check_len(files.len())?;
    let rows: Vec<Row> = files
        .into_iter()
        .map(|f| {
            let extension = match f.name.rfind('.') {
                Some(i) if i > 0 => f.name.get(i..).unwrap_or("").to_string(),
                _ => String::new(),
            };
            let date = |t: Option<i64>| t.map_or(Value::Null, Value::DateTime);
            let attributes = Value::record_of(vec![
                ("Kind", Value::text(if extension.is_empty() { "File" } else { extension.trim_start_matches('.') })),
                ("Size", Value::Number(f.size as f64)),
                ("Directory", Value::Logical(false)),
                ("Hidden", Value::Logical(f.hidden)),
                ("ReadOnly", Value::Logical(f.read_only)),
            ]);
            let read = f.content.clone();
            let content = Lazy::new(move || match read() {
                Ok(bytes) => Ok(Value::Binary(bytes.into())),
                Err(m) => Err(Rc::new(MError { reason: Rc::from(DATA_SOURCE_ERROR), message: Rc::from(m), detail: Value::Null })),
            });
            [
                Slot::Lazy(content),
                Slot::Val(Value::text(&f.name)),
                Slot::Val(Value::text(extension)),
                Slot::Val(date(f.accessed)),
                Slot::Val(date(f.modified)),
                Slot::Val(date(f.created)),
                Slot::Val(attributes),
                Slot::Val(Value::text(&f.folder)),
            ]
            .into_iter()
            .collect()
        })
        .collect();
    Ok(Value::table(Table {
        columns: names(&["Content", "Name", "Extension", "Date accessed", "Date modified", "Date created", "Attributes", "Folder Path"]),
        types: vec![Type::Binary, Type::Text, Type::Text, Type::DateTime, Type::DateTime, Type::DateTime, Type::any_record(), Type::Text].into(),
        rows,
    }))
}

// ---------------------------------------------------------------- the web

/// Letters, digits and `-._~` stay; everything else is percent-encoded.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The URL `Web.Contents` asks for: `RelativePath` after the base and `Query` added to its query
/// string.
pub(crate) fn web_url(base: &str, options: Option<&crate::value::Record>) -> EResult<(String, Vec<(String, String)>)> {
    let mut url = base.trim().to_string();
    let mut headers = Vec::new();
    if let Some(o) = options {
        if let Some(Value::Text(p)) = o.get("RelativePath")?
            && !p.trim_start_matches('/').is_empty()
        {
            if !url.ends_with('/') {
                url.push('/');
            }
            url.push_str(p.trim_start_matches('/'));
        }
        match o.get("Query")? {
            None | Some(Value::Null) => {}
            Some(Value::Record(q)) => {
                for (name, slot) in q.iter() {
                    let v = match slot.get()? {
                        Value::Text(t) => t.to_string(),
                        Value::Number(n) => crate::value::number_to_text(n),
                        v => return mismatch(&v, "text for a query parameter"),
                    };
                    url.push(if url.contains('?') { '&' } else { '?' });
                    url.push_str(&url_encode(name));
                    url.push('=');
                    url.push_str(&url_encode(&v));
                }
            }
            Some(v) => return mismatch(&v, "a record of query parameters"),
        }
        match o.get("Headers")? {
            None | Some(Value::Null) => {}
            Some(Value::Record(h)) => {
                for (name, slot) in h.iter() {
                    match slot.get()? {
                        Value::Text(t) => headers.push((name.to_string(), t.to_string())),
                        v => return mismatch(&v, "text for a header"),
                    }
                }
            }
            Some(v) => return mismatch(&v, "a record of headers"),
        }
        if !matches!(o.get("Content")?, None | Some(Value::Null)) {
            return raise("Web.Contents only reads (GET): sending Content isn't supported.");
        }
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return raise(format!("'{url}' isn't a web address: it should start with http:// or https://."));
    }
    Ok((url, headers))
}

fn fetcher(it: &Interp) -> EResult<Fetch> {
    it.host.web().ok_or_else(|| ds("Reading from the web isn't available here.".to_string()))
}

fn web_contents(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let base = text(a, 0)?;
    let options = opt_record(a, 1)?;
    let (url, headers) = web_url(&base, options.as_deref())?;
    let fetch = fetcher(it)?;
    let body = fetch(&url, &headers).map_err(ds)?;
    Ok(Value::Binary(body.into()))
}

// ---------------------------------------------------------------- OData

fn odata_feed(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let url = text(a, 0)?;
    let mut headers: Vec<(String, String)> = vec![("Accept".into(), "application/json".into())];
    if let Value::Record(h) = arg(a, 1) {
        for (name, slot) in h.iter() {
            if let Value::Text(t) = slot.get()? {
                headers.push((name.to_string(), t.to_string()));
            }
        }
    }
    let fetch = fetcher(it)?;
    let (url, _) = web_url(&url, None)?;
    let doc = fetch_json(&fetch, &url, &headers)?;
    // A service document lists entity sets: each one's rows come when it is opened.
    if let Some(sets) = service_sets(&doc) {
        let base = if url.ends_with('/') { url.clone() } else { format!("{url}/") };
        let rows: Vec<Row> = sets
            .into_iter()
            .map(|(name, href)| {
                let target = if href.starts_with("http://") || href.starts_with("https://") { href } else { format!("{base}{href}") };
                let (fetch, headers) = (fetch.clone(), headers.clone());
                let data = Lazy::new(move || feed_table(&fetch, &target, &headers).map(Value::Table).map_err(lazy_error));
                [Slot::Val(Value::text(&name)), Slot::Lazy(data), Slot::Val(Value::text("Table"))].into_iter().collect()
            })
            .collect();
        return Ok(Value::table(Table {
            columns: names(&["Name", "Data", "Signature"]),
            types: vec![Type::Text, Type::any_table(), Type::Text].into(),
            rows,
        }));
    }
    let (rows, next) = feed_page(&doc)?;
    let mut all = rows;
    if let Some(next) = next {
        all.extend(more_pages(&fetch, next, &headers)?);
    }
    Ok(Value::Table(Rc::new(records_table(all)?)))
}

fn fetch_json(fetch: &Fetch, url: &str, headers: &[(String, String)]) -> EResult<Value> {
    let body = fetch(url, headers).map_err(ds)?;
    super::json::parse(&String::from_utf8_lossy(&body))
}

/// The entity sets of a service document: `value` holds records with just `name`, `url` (and
/// `kind`, `title`).
fn service_sets(doc: &Value) -> Option<Vec<(String, String)>> {
    let Value::Record(r) = doc else { return None };
    let Ok(Some(Value::List(items))) = r.get("value") else { return None };
    let mut out = Vec::new();
    for s in &items.items {
        let Ok(Value::Record(e)) = s.get() else { return None };
        if !e.names.iter().all(|n| matches!(&**n, "name" | "url" | "kind" | "title")) {
            return None;
        }
        let (Ok(Some(Value::Text(name))), Ok(Some(Value::Text(url)))) = (e.get("name"), e.get("url")) else { return None };
        out.push((name.to_string(), url.to_string()));
    }
    (!out.is_empty()).then_some(out)
}

/// The records of one page of a feed, and the link to the next page.
fn feed_page(doc: &Value) -> EResult<(Vec<Rc<crate::value::Record>>, Option<String>)> {
    let Value::Record(r) = doc else { return raise("The OData response isn't a JSON object.") };
    // OData v4 keeps rows in `value`; v2 in `d.results` (or `d` itself, a list).
    let (rows, next) = match (r.get("value")?, r.get("d")?) {
        (Some(v), _) => (v, ["@odata.nextLink", "odata.nextLink"].iter().find_map(|k| r.get(k).ok().flatten())),
        (None, Some(Value::Record(d))) => (d.get("results")?.unwrap_or(Value::Null), d.get("__next")?),
        (None, Some(list @ Value::List(_))) => (list, None),
        _ => return raise("The OData response has no rows (no `value`)."),
    };
    let Value::List(items) = rows else { return raise("The OData response's rows aren't a list.") };
    let mut out = Vec::with_capacity(items.len());
    for s in &items.items {
        match s.get()? {
            Value::Record(rec) => out.push(rec),
            v => return mismatch(&v, "a row (a JSON object)"),
        }
    }
    let next = match next {
        Some(Value::Text(t)) => Some(t.to_string()),
        _ => None,
    };
    Ok((out, next))
}

fn more_pages(fetch: &Fetch, mut next: String, headers: &[(String, String)]) -> EResult<Vec<Rc<crate::value::Record>>> {
    let mut out = Vec::new();
    for _ in 0..MAX_PAGES {
        let doc = fetch_json(fetch, &next, headers)?;
        let (rows, more) = feed_page(&doc)?;
        out.extend(rows);
        check_len(out.len())?;
        match more {
            Some(m) => next = m,
            None => return Ok(out),
        }
    }
    raise("The OData feed has more pages than a query follows.")
}

fn feed_table(fetch: &Fetch, url: &str, headers: &[(String, String)]) -> EResult<Rc<Table>> {
    let doc = fetch_json(fetch, url, headers)?;
    let (mut rows, next) = feed_page(&doc)?;
    if let Some(next) = next {
        rows.extend(more_pages(fetch, next, headers)?);
    }
    Ok(Rc::new(records_table(rows)?))
}

/// Records as a table: columns are their fields in the order first met, OData annotations
/// (`@odata.…`, `__metadata`) left out.
fn records_table(rows: Vec<Rc<crate::value::Record>>) -> EResult<Table> {
    let mut columns: Vec<Rc<str>> = Vec::new();
    for r in &rows {
        for n in r.names.iter() {
            if !n.starts_with('@') && !n.starts_with("odata.") && &**n != "__metadata" && !columns.contains(n) {
                columns.push(n.clone());
            }
        }
    }
    if rows.len().saturating_mul(columns.len().max(1)) > MAX_CELLS {
        return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
    }
    let table_rows: Vec<Row> = rows.iter().map(|r| columns.iter().map(|c| r.slot(c).cloned().unwrap_or(Slot::NULL)).collect()).collect();
    Ok(Table::untyped(columns.into(), table_rows))
}

// ---------------------------------------------------------------- XML

/// An XML element read.
struct Node {
    name: String,
    namespace: String,
    attributes: Vec<(String, String, String)>,
    children: Vec<Node>,
    text: String,
}

fn ns_text(r: ResolveResult<'_>) -> String {
    match r {
        ResolveResult::Bound(n) => String::from_utf8_lossy(n.as_ref()).into_owned(),
        _ => String::new(),
    }
}

fn xml_error(e: impl std::fmt::Display) -> EvalError {
    format_error(format!("The XML isn't valid: {e}"))
}

fn element(reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> EResult<Node> {
    let (ns, local) = reader.resolver_mut().resolve_element(e.name());
    let mut node = Node {
        name: String::from_utf8_lossy(local.as_ref()).into_owned(),
        namespace: ns_text(ns),
        attributes: Vec::new(),
        children: Vec::new(),
        text: String::new(),
    };
    for a in e.attributes() {
        let a = a.map_err(xml_error)?;
        if a.key.as_ref().starts_with(b"xmlns") {
            continue;
        }
        let (ns, local) = reader.resolver_mut().resolve_attribute(a.key);
        let value = a.normalized_value(XmlVersion::Explicit1_0).map_err(xml_error)?.into_owned();
        node.attributes.push((String::from_utf8_lossy(local.as_ref()).into_owned(), ns_text(ns), value));
    }
    Ok(node)
}

/// Parses an XML document into its root element. Nesting, element count and text are bounded;
/// a DOCTYPE (and so entity expansion) is refused.
fn parse_xml(src: &str) -> EResult<Node> {
    let mut reader = NsReader::from_reader(src.as_bytes());
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut count = 0usize;
    let mut text = 0usize;
    loop {
        let event = reader.read_event().map_err(xml_error)?;
        match event {
            Event::Start(e) => {
                if root.is_some() {
                    return Err(xml_error("there is more than one root element"));
                }
                count += 1;
                if stack.len() >= MAX_DEPTH || count > MAX_ITEMS {
                    return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                }
                let n = element(&mut reader, &e)?;
                stack.push(n);
            }
            Event::Empty(e) => {
                count += 1;
                if count > MAX_ITEMS {
                    return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                }
                let n = element(&mut reader, &e)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(n),
                    None if root.is_none() => root = Some(n),
                    None => return Err(xml_error("there is more than one root element")),
                }
            }
            Event::End(_) => {
                let Some(n) = stack.pop() else { return Err(xml_error("a closing tag has no opening tag")) };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(n),
                    None => root = Some(n),
                }
            }
            Event::Text(t) => {
                let s = t.xml_content(XmlVersion::Explicit1_0).map_err(xml_error)?;
                text += s.len();
                if text > MAX_TEXT {
                    return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                }
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&s);
                }
            }
            Event::CData(t) => {
                let s = t.decode().map_err(xml_error)?;
                text += s.len();
                if text > MAX_TEXT {
                    return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                }
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&s);
                }
            }
            Event::GeneralRef(r) => {
                let ch = match r.resolve_char_ref().map_err(xml_error)? {
                    Some(c) => c,
                    None => match r.as_ref() {
                        b"amp" => '&',
                        b"lt" => '<',
                        b"gt" => '>',
                        b"apos" => '\'',
                        b"quot" => '"',
                        _ => return Err(xml_error("an entity that isn't defined")),
                    },
                };
                if let Some(n) = stack.last_mut() {
                    n.text.push(ch);
                }
            }
            Event::DocType(_) => return Err(xml_error("DOCTYPE declarations aren't read")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(xml_error("the document ends inside an element"));
    }
    root.ok_or_else(|| xml_error("there is no root element"))
}

fn attributes_table(n: &Node) -> Value {
    let rows: Vec<Row> =
        n.attributes.iter().map(|(name, ns, v)| [Value::text(name), Value::text(ns), Value::text(v)].into_iter().map(Slot::Val).collect()).collect();
    Value::table(Table { columns: names(&["Name", "Namespace", "Value"]), types: vec![Type::Text, Type::Text, Type::Text].into(), rows })
}

/// `Xml.Document`'s rows for elements: name, namespace, value (text, or the children) and
/// attributes.
fn document_table(nodes: &[Node]) -> Value {
    let rows: Vec<Row> = nodes
        .iter()
        .map(|n| {
            let value = if n.children.is_empty() { Value::text(n.text.trim()) } else { document_table(&n.children) };
            [Value::text(&n.name), Value::text(&n.namespace), value, attributes_table(n)].into_iter().map(Slot::Val).collect()
        })
        .collect();
    Value::table(Table {
        columns: names(&["Name", "Namespace", "Value", "Attributes"]),
        types: vec![Type::Text, Type::Text, Type::Any, Type::any_table()].into(),
        rows,
    })
}

fn xml_document(src: &str) -> EResult<Value> {
    let root = parse_xml(src)?;
    Ok(document_table(std::slice::from_ref(&root)))
}

/// Elements grouped by name, in the order first met.
fn groups(nodes: &[Node]) -> Vec<(&str, Vec<&Node>)> {
    let mut out: Vec<(&str, Vec<&Node>)> = Vec::new();
    for n in nodes {
        match out.iter_mut().find(|(name, _)| *name == n.name) {
            Some((_, list)) => list.push(n),
            None => out.push((n.name.as_str(), vec![n])),
        }
    }
    out
}

/// Elements of one name as rows: their attributes (`Attribute:name`) and child elements are the
/// columns. A child that is text alone gives its text; one with structure, or repeated, gives a
/// table of its own.
fn rows_table(elements: &[&Node]) -> Value {
    let mut columns: Vec<String> = Vec::new();
    for e in elements {
        for (name, _, _) in &e.attributes {
            let c = format!("Attribute:{name}");
            if !columns.contains(&c) {
                columns.push(c);
            }
        }
        for c in &e.children {
            if !columns.contains(&c.name) {
                columns.push(c.name.clone());
            }
        }
    }
    let leaves_only = elements.iter().all(|e| e.children.is_empty());
    if leaves_only {
        columns.push("Element:Text".to_string());
    }
    let rows: Vec<Row> = elements
        .iter()
        .map(|e| {
            columns
                .iter()
                .map(|c| {
                    if let Some(attr) = c.strip_prefix("Attribute:") {
                        return Slot::Val(e.attributes.iter().find(|(n, _, _)| n == attr).map_or(Value::Null, |(_, _, v)| Value::text(v)));
                    }
                    if c == "Element:Text" && leaves_only {
                        return Slot::Val(Value::text(e.text.trim()));
                    }
                    let kids: Vec<&Node> = e.children.iter().filter(|k| &k.name == c).collect();
                    Slot::Val(match kids.as_slice() {
                        [] => Value::Null,
                        [one] if one.children.is_empty() && one.attributes.is_empty() => Value::text(one.text.trim()),
                        _ => rows_table(&kids),
                    })
                })
                .collect()
        })
        .collect();
    Value::table(Table::untyped(columns.iter().map(|c| Rc::from(c.as_str())).collect(), rows))
}

/// `Xml.Tables`: the root's child elements by name, each with a table of those elements.
fn xml_tables(src: &str) -> EResult<Value> {
    let root = parse_xml(src)?;
    let rows: Vec<Row> = groups(&root.children)
        .into_iter()
        .map(|(name, elements)| [Slot::Val(Value::text(name)), Slot::Val(rows_table(&elements))].into_iter().collect())
        .collect();
    Ok(Value::table(Table { columns: names(&["Name", "Table"]), types: vec![Type::Text, Type::any_table()].into(), rows }))
}

// ---------------------------------------------------------------- HTML tables

/// A table read from HTML: its caption, class, id and rows of cell text.
#[derive(Default)]
struct HtmlTable {
    caption: String,
    class: String,
    id: String,
    /// Rows of (text, is a header cell, columns spanned, rows spanned).
    rows: Vec<Vec<(String, bool, usize, usize)>>,
}

fn decode_entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "euro" => '€',
        "ndash" => '–',
        "mdash" => '—',
        _ => return None,
    })
}

/// Text with its entities decoded.
fn html_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or(""));
        rest = rest.get(i..).unwrap_or("");
        match rest.find(';').filter(|j| *j <= 10) {
            Some(j) => match decode_entity(rest.get(1..j).unwrap_or("")) {
                Some(c) => {
                    out.push(c);
                    rest = rest.get(j + 1..).unwrap_or("");
                }
                None => {
                    out.push('&');
                    rest = rest.get(1..).unwrap_or("");
                }
            },
            None => {
                out.push('&');
                rest = rest.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of attribute `name` in a tag's text (`td colspan="2"`).
fn tag_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower.get(from..).and_then(|s| s.find(name)).map(|i| i + from) {
        let before_ok = lower.get(..i).and_then(|s| s.chars().last()).is_some_and(|c| c.is_whitespace());
        let after = lower.get(i + name.len()..).unwrap_or("").trim_start();
        if before_ok && let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let start = tag.len() - v.len();
            let raw = tag.get(start..).unwrap_or("");
            return Some(match raw.chars().next() {
                Some(q @ ('"' | '\'')) => raw.get(1..).and_then(|r| r.split(q).next()).unwrap_or("").to_string(),
                _ => raw.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("").to_string(),
            });
        }
        from = i + name.len();
    }
    None
}

/// The tables of an HTML page, nested ones included, in the order they open. A tolerant reader:
/// missing closing tags close where a new row, cell or table starts.
fn html_tables(src: &str) -> EResult<Vec<HtmlTable>> {
    let mut done: Vec<HtmlTable> = Vec::new();
    // Open tables, innermost last, each with its row and cell in progress.
    struct Open {
        table: HtmlTable,
        row: Option<Vec<(String, bool, usize, usize)>>,
        cell: Option<(String, bool, usize, usize)>,
        in_caption: bool,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut order: Vec<usize> = Vec::new();
    let mut slots: Vec<Option<HtmlTable>> = Vec::new();
    let mut rest = src;
    let mut cells = 0usize;
    let close_cell = |o: &mut Open| {
        if let Some(c) = o.cell.take() {
            o.row.get_or_insert_with(Vec::new).push(c);
        }
    };
    let close_row = |o: &mut Open| {
        if let Some(c) = o.cell.take() {
            o.row.get_or_insert_with(Vec::new).push(c);
        }
        if let Some(r) = o.row.take()
            && !r.is_empty()
        {
            o.table.rows.push(r);
        }
    };
    while let Some(lt) = rest.find('<') {
        let text = rest.get(..lt).unwrap_or("");
        if let Some(o) = open.last_mut() {
            if o.in_caption {
                o.table.caption.push_str(text);
            } else if let Some(c) = o.cell.as_mut() {
                c.0.push_str(text);
                if c.0.len() > MAX_TEXT {
                    return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                }
            }
        }
        rest = rest.get(lt..).unwrap_or("");
        // Comments, scripts and styles are skipped whole.
        if rest.starts_with("<!--") {
            rest = rest.find("-->").and_then(|i| rest.get(i + 3..)).unwrap_or("");
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = rest.get(1..gt).unwrap_or("");
        rest = rest.get(gt + 1..).unwrap_or("");
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if !closing && (name == "script" || name == "style") {
            let end = format!("</{name}");
            let lower = rest.to_ascii_lowercase();
            rest = lower.find(&end).and_then(|i| rest.get(i..)).unwrap_or("");
            continue;
        }
        match (name.as_str(), closing) {
            ("table", false) => {
                if open.len() >= MAX_DEPTH || slots.len() >= 10_000 {
                    return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                }
                order.push(slots.len());
                slots.push(None);
                open.push(Open {
                    table: HtmlTable {
                        class: tag_attr(tag, "class").unwrap_or_default(),
                        id: tag_attr(tag, "id").unwrap_or_default(),
                        ..HtmlTable::default()
                    },
                    row: None,
                    cell: None,
                    in_caption: false,
                });
            }
            ("table", true) => {
                if let Some(mut o) = open.pop() {
                    close_row(&mut o);
                    if let Some(i) = order.pop()
                        && let Some(s) = slots.get_mut(i)
                    {
                        *s = Some(o.table);
                    }
                }
            }
            ("caption", c) => {
                if let Some(o) = open.last_mut() {
                    o.in_caption = !c;
                }
            }
            ("tr", false) => {
                if let Some(o) = open.last_mut() {
                    close_row(o);
                    o.row = Some(Vec::new());
                }
            }
            ("tr", true) => {
                if let Some(o) = open.last_mut() {
                    close_row(o);
                }
            }
            ("td" | "th", false) => {
                if let Some(o) = open.last_mut() {
                    close_cell(o);
                    cells += 1;
                    if cells > MAX_CELLS {
                        return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                    }
                    let span = |a: &str| tag_attr(tag, a).and_then(|v| v.trim().parse::<usize>().ok()).map_or(1, |n| n.clamp(1, 1000));
                    o.cell = Some((String::new(), name == "th", span("colspan"), span("rowspan")));
                }
            }
            ("td" | "th", true) => {
                if let Some(o) = open.last_mut() {
                    close_cell(o);
                }
            }
            ("br" | "p" | "div" | "li", _) => {
                if let Some(c) = open.last_mut().and_then(|o| o.cell.as_mut()) {
                    c.0.push(' ');
                }
            }
            _ => {}
        }
    }
    // Tables left open at the end of the page still count.
    while let Some(mut o) = open.pop() {
        close_row(&mut o);
        if let Some(i) = order.pop()
            && let Some(s) = slots.get_mut(i)
        {
            *s = Some(o.table);
        }
    }
    done.extend(slots.into_iter().flatten());
    Ok(done)
}

/// Collapses whitespace and decodes entities, as a browser shows cell text.
fn cell_text(s: &str) -> String {
    html_text(s).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A table's rows laid out on a grid: spanned cells repeat their text.
fn grid(t: &HtmlTable) -> EResult<Vec<Vec<String>>> {
    let mut out: Vec<Vec<Option<String>>> = Vec::new();
    for (r, row) in t.rows.iter().enumerate() {
        let mut c = 0usize;
        for (text, _, colspan, rowspan) in row {
            while out.get(r).and_then(|x| x.get(c)).is_some_and(Option::is_some) {
                c += 1;
            }
            for dr in 0..*rowspan {
                for dc in 0..*colspan {
                    let (rr, cc) = (r + dr, c + dc);
                    if rr >= MAX_ITEMS || cc >= 16_384 {
                        return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
                    }
                    while out.len() <= rr {
                        out.push(Vec::new());
                    }
                    let line = out.get_mut(rr).ok_or(EvalError::Fatal(crate::error::Fatal::TooLarge))?;
                    while line.len() <= cc {
                        line.push(None);
                    }
                    if let Some(slot) = line.get_mut(cc) {
                        *slot = Some(cell_text(text));
                    }
                }
            }
            c += colspan;
        }
    }
    Ok(out.into_iter().map(|r| r.into_iter().map(Option::unwrap_or_default).collect()).collect())
}

/// `Web.Page`: the tables of an HTML page, each with its caption, class, id and data. A first
/// row of header cells (`th`) names the columns.
fn web_page(src: &str) -> EResult<Value> {
    let tables = html_tables(src)?;
    check_len(tables.len())?;
    let mut rows: Vec<Row> = Vec::with_capacity(tables.len());
    for (i, t) in tables.iter().enumerate() {
        let mut g = grid(t)?;
        let width = g.iter().map(Vec::len).max().unwrap_or(0);
        let header = t.rows.first().is_some_and(|r| !r.is_empty() && r.iter().all(|c| c.1));
        let columns: Vec<String> = if header && !g.is_empty() {
            let first = g.remove(0);
            (0..width).map(|c| first.get(c).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| format!("Column{}", c + 1))).collect()
        } else {
            (1..=width).map(|c| format!("Column{c}")).collect()
        };
        let columns = super::table::dedupe(columns);
        if g.len().saturating_mul(width.max(1)) > MAX_CELLS {
            return Err(EvalError::Fatal(crate::error::Fatal::TooLarge));
        }
        let data_rows: Vec<Row> =
            g.into_iter().map(|r| (0..width).map(|c| Slot::Val(Value::text(r.get(c).map_or("", String::as_str)))).collect()).collect();
        let data = Value::table(Table::untyped(columns.into(), data_rows));
        let caption = cell_text(&t.caption);
        rows.push(
            [
                Value::text(if caption.is_empty() { format!("Table {i}") } else { caption }),
                Value::text("Table"),
                Value::text(&t.class),
                Value::text(&t.id),
                data,
            ]
            .into_iter()
            .map(Slot::Val)
            .collect(),
        );
    }
    Ok(Value::table(Table {
        columns: names(&["Caption", "Source", "ClassName", "Id", "Data"]),
        types: vec![Type::Text, Type::Text, Type::Text, Type::Text, Type::any_table()].into(),
        rows,
    }))
}
