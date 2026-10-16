//! Power Query queries in a workbook file: the Data Mashup custom XML part.
//!
//! The format is the open specification [MS-QDEFF] (Query Definition File Format). A custom XML
//! part whose root is `<DataMashup>` holds base64 of a binary stream: a version, then four
//! length-prefixed sections: an OPC zip with the queries as an M section document
//! (`Formulas/Section1.m`), a permissions XML, a metadata block that says where each query loads
//! to, and permission bindings.
//!
//! The part arrives through the pass-through of unmodelled parts ([`crate::passthrough`]), and
//! stays there: reading fills `Workbook::queries` from it, and writing leaves the kept bytes alone
//! unless the queries changed, in which case the part is written anew. A file's own permission
//! bindings can't be reproduced (they are tied to the machine that wrote them), so a rewritten
//! part has none, which the specification allows: the reader falls back to default permissions.
//! Excel was observed to list the queries of a part written this way (docs/power-query.md).
//!
//! This crate doesn't parse M. It only splits the section document into its members, which takes
//! knowing where text, quoted names and comments end.

use std::sync::Arc;

use gridcraft_model::{Preserved, PreservedPart, PreservedRel, Query, Workbook};

use crate::IoError;
use crate::package::{Package, ZipOut, normalize};
use crate::xml::{self, esc, esc_attr};

const NS: &str = "http://schemas.microsoft.com/DataMashup";
const REL_CUSTOM_XML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const REL_PROPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const CT_PROPS: &str = "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";
/// The part names used when a workbook gets its first queries here. The writer renames them if
/// they are taken.
const ITEM: &str = "customXml/itemQueries.xml";
const ITEM_PROPS: &str = "customXml/itemPropsQueries.xml";
const ITEM_ID: &str = "{5C7A1E0B-3D42-4F86-9A1B-6E2F0C8D4A37}";
/// Largest section document and metadata read.
const MAX_SECTION: u64 = 16 << 20;
/// Most queries read from one workbook.
const MAX_QUERIES: usize = 10_000;

// ---------------------------------------------------------------- the binary stream

/// The four sections of the stream.
struct Streams<'a> {
    package: &'a [u8],
    metadata: &'a [u8],
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    let (head, tail) = b.split_at_checked(n)?;
    *b = tail;
    Some(head)
}

fn take_u32(b: &mut &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = take(b, 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn take_block<'a>(b: &mut &'a [u8]) -> Option<&'a [u8]> {
    let n = take_u32(b)? as usize;
    take(b, n)
}

fn streams(mut b: &[u8]) -> Option<Streams<'_>> {
    if take_u32(&mut b)? != 0 {
        return None;
    }
    let package = take_block(&mut b)?;
    let _permissions = take_block(&mut b)?;
    let metadata = take_block(&mut b)?;
    Some(Streams { package, metadata })
}

/// Text of an XML part: UTF-16 when a byte-order mark says so, otherwise UTF-8.
fn xml_text(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], big: bool| {
        let units = rest.as_chunks::<2>().0.iter().map(|p| if big { u16::from_be_bytes(*p) } else { u16::from_le_bytes(*p) });
        char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect::<String>()
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(rest, false),
        [0xfe, 0xff, rest @ ..] => utf16(rest, true),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let at = |i: usize| chunk.get(i).copied().unwrap_or(0) as u32;
        let n = (at(0) << 16) | (at(1) << 8) | at(2);
        for (k, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if k <= chunk.len() {
                out.push(ALPHABET.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A') as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The binary stream of a custom XML part, when the part is a Data Mashup.
fn payload(part: &[u8]) -> Option<Vec<u8>> {
    let text = xml_text(part);
    // The root element, past the declaration and any comments.
    let mut rest = text.as_str();
    let root = loop {
        let at = rest.find('<')?;
        let tag = rest.get(at..)?;
        if tag.starts_with("<?") || tag.starts_with("<!") {
            rest = tag.get(tag.find('>')? + 1..)?;
        } else {
            break tag;
        }
    };
    let open_end = root.find('>')?;
    let open = root.get(..open_end)?;
    let name = open.get(1..)?.split(|c: char| c.is_whitespace() || c == '/').next()?;
    if name.rsplit(':').next()? != "DataMashup" || !open.contains(NS) {
        return None;
    }
    let body = root.get(open_end + 1..)?;
    base64_decode(body.get(..body.find("</")?)?)
}

// ---------------------------------------------------------------- the section document

/// One query of the section document.
#[derive(Debug, PartialEq)]
struct Member {
    name: String,
    description: String,
    formula: String,
}

struct Scan<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Scan<'a> {
    fn rest(&self) -> &'a str {
        self.s.get(self.i..).unwrap_or("")
    }

    /// Skips whitespace and comments.
    fn skip_trivia(&mut self) {
        loop {
            let r = self.rest();
            let t = r.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
            self.i += r.len() - t.len();
            if t.starts_with("//") {
                self.i += t.find('\n').map_or(t.len(), |n| n + 1);
            } else if t.starts_with("/*") {
                self.i += t.get(2..).and_then(|x| x.find("*/")).map_or(t.len(), |n| n + 4);
            } else {
                return;
            }
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.rest().starts_with(word) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    /// From an opening quote: the text up to its closing quote, a doubled quote being one quote.
    fn quoted(&mut self) -> Option<String> {
        if !self.eat("\"") {
            return None;
        }
        let mut out = String::new();
        loop {
            let r = self.rest();
            let n = r.find('"')?;
            out.push_str(r.get(..n)?);
            self.i += n + 1;
            if self.eat("\"") {
                out.push('"');
            } else {
                return Some(out);
            }
        }
    }

    /// A member name: `#"quoted"`, or everything up to whitespace or `=`.
    fn name(&mut self) -> Option<String> {
        if self.rest().starts_with("#\"") {
            self.i += 1;
            return self.quoted();
        }
        let r = self.rest();
        let n = r.find(|c: char| c.is_whitespace() || c == '=' || c == ';').unwrap_or(r.len());
        self.i += n;
        (n > 0).then(|| r.get(..n).unwrap_or("").to_string())
    }

    /// Skips to the first of `stops` outside text, quoted names and comments; returns what was
    /// passed over and stops at (not past) the stop character.
    fn until(&mut self, stops: &[char]) -> Option<&'a str> {
        let start = self.i;
        loop {
            let r = self.rest();
            let c = r.chars().next()?;
            if stops.contains(&c) {
                return self.s.get(start..self.i);
            }
            if c == '"' {
                self.quoted()?;
            } else if r.starts_with("//") {
                self.i += r.find('\n').map_or(r.len(), |n| n + 1);
            } else if r.starts_with("/*") {
                self.i += r.get(2..).and_then(|x| x.find("*/")).map_or(r.len(), |n| n + 4);
            } else {
                self.i += c.len_utf8();
            }
        }
    }

    /// A literal attribute record before a member (`[ Description = "…" ]`): its description.
    fn attributes(&mut self) -> Option<String> {
        if !self.eat("[") {
            return Some(String::new());
        }
        let inside = self.until(&[']'])?;
        self.i += 1;
        let mut scan = Scan { s: inside, i: 0 };
        loop {
            scan.skip_trivia();
            let field = scan.name();
            scan.skip_trivia();
            if !scan.eat("=") {
                return Some(String::new());
            }
            scan.skip_trivia();
            if field.as_deref() == Some("Description") && scan.rest().starts_with('"') {
                return Some(unescape(&scan.quoted()?));
            }
            // Another field: on to the next one, if there is one.
            if scan.until(&[',']).is_none() {
                return Some(String::new());
            }
            scan.i += 1;
        }
    }
}

/// Undoes M's `#(…)` escapes in a text literal.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("#(") {
        out.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i + 2..).unwrap_or("");
        let Some(close) = after.find(')') else {
            out.push_str(rest.get(i..).unwrap_or(""));
            return out;
        };
        for part in after.get(..close).unwrap_or("").split(',') {
            match part.trim() {
                "cr" => out.push('\r'),
                "lf" => out.push('\n'),
                "tab" => out.push('\t'),
                "#" => out.push('#'),
                hex => out.extend(u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)),
            }
        }
        rest = after.get(close + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

fn text_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\"\""),
            '\r' => out.push_str("#(cr)"),
            '\n' => out.push_str("#(lf)"),
            '\t' => out.push_str("#(tab)"),
            '#' if chars.peek() == Some(&'(') => out.push_str("#(#)"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Splits a section document (`section Section1; shared Query1 = …;`) into its members. A
/// document that ends badly gives the members before the damage.
fn split_section(src: &str) -> Vec<Member> {
    let mut scan = Scan { s: src, i: 0 };
    let mut out = Vec::new();
    scan.skip_trivia();
    if scan.attributes().is_none() {
        return out;
    }
    scan.skip_trivia();
    if scan.eat("section") {
        if scan.until(&[';']).is_none() {
            return out;
        }
        scan.i += 1;
    }
    while out.len() < MAX_QUERIES {
        scan.skip_trivia();
        let Some(description) = scan.attributes() else { break };
        scan.skip_trivia();
        if scan.rest().starts_with("shared") && scan.rest().get(6..).is_some_and(|r| r.starts_with(char::is_whitespace)) {
            scan.i += 6;
            scan.skip_trivia();
        }
        let Some(name) = scan.name() else { break };
        scan.skip_trivia();
        if !scan.eat("=") {
            break;
        }
        let Some(formula) = scan.until(&[';']) else { break };
        scan.i += 1;
        out.push(Member { name, description, formula: formula.trim().to_string() });
    }
    out
}

/// A query name as M source: bare when it is a plain identifier, otherwise `#"quoted"`.
fn member_name(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "and",
        "as",
        "each",
        "else",
        "error",
        "false",
        "if",
        "in",
        "is",
        "let",
        "meta",
        "not",
        "null",
        "or",
        "otherwise",
        "section",
        "shared",
        "then",
        "true",
        "try",
        "type",
    ];
    let plain = |part: &str| {
        let mut cs = part.chars();
        cs.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic()) && cs.all(|c| c == '_' || c.is_ascii_alphanumeric())
    };
    if !name.is_empty() && name.split('.').all(plain) && !KEYWORDS.contains(&name) {
        name.to_string()
    } else {
        format!("#\"{}\"", name.replace('"', "\"\""))
    }
}

/// The section document for these queries, with the line ends Excel writes between members.
/// Formulas are written as they are, so that reading gives them back unchanged.
fn section_document(queries: &[Query]) -> String {
    let mut out = String::from("section Section1;");
    for q in queries {
        out.push_str("\r\n\r\n");
        if !q.description.is_empty() {
            out.push_str(&format!("[ Description = {} ]\r\n", text_literal(&q.description)));
        }
        let formula = q.formula.trim();
        out.push_str(&format!("shared {} = {formula}", member_name(&q.name)));
        // A formula that ends in a line comment would swallow the terminator.
        if formula.lines().last().is_some_and(|l| l.contains("//")) {
            out.push_str("\r\n");
        }
        out.push(';');
    }
    out
}

// ---------------------------------------------------------------- metadata

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex =
            (c == b'%').then(|| b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        // Unreserved characters stay, as Excel leaves them (`Em%20Estoque%20(2024)`).
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'!' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The table each query fills, from the metadata block: (query name, table name).
fn fill_targets(metadata: &[u8]) -> Vec<(String, String)> {
    let mut b = metadata;
    let mut out = Vec::new();
    let Some(xml_bytes) = take_u32(&mut b).and_then(|_| take_block(&mut b)) else { return out };
    if xml_bytes.len() as u64 > MAX_SECTION {
        return out;
    }
    let Ok(root) = xml::parse(xml_text(xml_bytes).as_bytes()) else { return out };
    let Some(items) = root.child("Items") else { return out };
    for item in items.kids("Item") {
        let Some(location) = item.child("ItemLocation") else { continue };
        if location.child("ItemType").map(|t| t.text.trim()) != Some("Formula") {
            continue;
        }
        // `Section1/Query` is a query; `Section1/Query/Step` is one of its steps.
        let path = location.child("ItemPath").map(|p| p.text.trim()).unwrap_or("");
        let mut segments = path.split('/');
        let (Some(_), Some(query), None) = (segments.next(), segments.next(), segments.next()) else { continue };
        let entry =
            |ty: &str| item.child("StableEntries").and_then(|e| e.kids("Entry").find(|x| x.attr("Type") == Some(ty))).and_then(|x| x.attr("Value"));
        let is_table = entry("FillObjectType").is_none_or(|t| t == "sTable");
        if entry("FillEnabled") == Some("l1")
            && is_table
            && let Some(target) = entry("FillTarget").and_then(|t| t.strip_prefix('s'))
        {
            out.push((percent_decode(query), target.to_string()));
        }
    }
    out
}

fn metadata_xml(queries: &[Query]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><LocalPackageMetadataFile xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><Items><Item><ItemLocation><ItemType>AllFormulas</ItemType><ItemPath /></ItemLocation><StableEntries /></Item>",
    );
    for q in queries {
        out.push_str(&format!(
            "<Item><ItemLocation><ItemType>Formula</ItemType><ItemPath>Section1/{}</ItemPath></ItemLocation><StableEntries>",
            esc(&percent_encode(&q.name))
        ));
        let mut entry = |ty: &str, value: &str| out.push_str(&format!("<Entry Type=\"{ty}\" Value=\"{}\" />", esc_attr(value)));
        entry("IsPrivate", "l0");
        entry("ResultType", "sTable");
        entry("FillToDataModelEnabled", "l0");
        match &q.load_to {
            Some(table) => {
                entry("FillEnabled", "l1");
                entry("FillObjectType", "sTable");
                entry("FillTarget", &format!("s{table}"));
            }
            None => {
                entry("FillEnabled", "l0");
                entry("FillObjectType", "sConnectionOnly");
            }
        }
        out.push_str("</StableEntries></Item>");
    }
    out.push_str("</Items></LocalPackageMetadataFile>");
    out
}

// ---------------------------------------------------------------- reading

/// The queries of a Data Mashup part; `tables` are the workbook's table names.
fn queries_of(part: &[u8], has_table: &dyn Fn(&str) -> Option<String>) -> Option<Vec<Query>> {
    let binary = payload(part)?;
    let s = streams(&binary)?;
    let mut package = Package::open(s.package).ok()?;
    let section = package.read_limited("Formulas/Section1.m", MAX_SECTION).ok()??;
    let fills = fill_targets(s.metadata);
    let members = split_section(&xml_text(&section));
    Some(
        members
            .into_iter()
            .map(|m| {
                let load_to = fills.iter().find(|(q, _)| *q == m.name).and_then(|(_, table)| has_table(table));
                Query { name: m.name, formula: m.formula, description: m.description, load_to }
            })
            .collect(),
    )
}

/// The index of the kept part that is the Data Mashup, and its queries.
fn kept_queries(wb: &Workbook) -> Option<(usize, Vec<Query>)> {
    let p = wb.preserved.as_deref()?;
    let has_table = |name: &str| wb.table(name).and_then(|(si, ti)| wb.sheets.get(si)?.tables.get(ti).map(|t| t.name.clone()));
    p.parts.iter().enumerate().find_map(|(i, part)| {
        // Only parts the workbook points at as custom XML.
        let is_item = p.rels.iter().any(|r| r.rel_type == REL_CUSTOM_XML && normalize(&r.target) == normalize(&part.name));
        if !is_item {
            return None;
        }
        queries_of(&part.data, &has_table).map(|q| (i, q))
    })
}

/// Fills `wb.queries` from the workbook's Data Mashup part, if it has one.
pub(crate) fn read(wb: &mut Workbook) {
    if let Some((_, queries)) = kept_queries(wb) {
        wb.queries = queries;
    }
}

// ---------------------------------------------------------------- writing

fn block(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), IoError> {
    let n = u32::try_from(bytes.len()).map_err(|_| IoError::TooLarge("the queries are too large to save".into()))?;
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

/// An XML document as these parts hold it: UTF-8 with a byte-order mark.
fn xml_bytes(xml: &str) -> Vec<u8> {
    [0xef, 0xbb, 0xbf].into_iter().chain(xml.bytes()).collect()
}

/// The custom XML part for these queries.
fn item_part(queries: &[Query]) -> Result<Vec<u8>, IoError> {
    const XSD: &str = "xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
    let mut zip = ZipOut::new();
    // The package description is in no namespace. Its version is the lowest that reads the
    // package: a reader refuses packages newer than itself.
    zip.add(
        "Config/Package.xml",
        &xml_bytes(&format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><Package {XSD}><Version>2.21.0.0</Version><MinVersion>2.21.0.0</MinVersion><Culture>en-US</Culture></Package>"
        )),
    )?;
    zip.add(
        "[Content_Types].xml",
        &xml_bytes(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"xml\" ContentType=\"text/xml\" /><Default Extension=\"m\" ContentType=\"application/x-ms-m\" /></Types>",
        ),
    )?;
    zip.add("Formulas/Section1.m", section_document(queries).as_bytes())?;
    let package = zip.finish()?;
    let permissions = xml_bytes(&format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><PermissionList {XSD}><CanEvaluateFuturePackages>false</CanEvaluateFuturePackages><FirewallEnabled>true</FirewallEnabled></PermissionList>"
    ));
    let mut metadata = Vec::new();
    metadata.extend_from_slice(&0u32.to_le_bytes());
    block(&mut metadata, &xml_bytes(&metadata_xml(queries)))?;
    // The metadata's content is a package too: an empty one.
    block(&mut metadata, &ZipOut::new().finish()?)?;
    let mut stream = Vec::new();
    stream.extend_from_slice(&0u32.to_le_bytes());
    block(&mut stream, &package)?;
    block(&mut stream, &permissions)?;
    block(&mut stream, &metadata)?;
    // No permission bindings: they can only be made on the machine that trusts the queries.
    block(&mut stream, &[])?;
    Ok(format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><DataMashup xmlns=\"{NS}\">{}</DataMashup>", base64_encode(&stream)).into_bytes())
}

/// The kept content to write so that the file holds `wb.queries`: `None` when what is kept
/// already does (including: no queries and no part).
pub(crate) fn preserved_with_queries(wb: &Workbook) -> Result<Option<Arc<Preserved>>, IoError> {
    let kept = kept_queries(wb);
    match &kept {
        Some((_, q)) if *q == wb.queries => return Ok(None),
        None if wb.queries.is_empty() => return Ok(None),
        _ => {}
    }
    let mut p = wb.preserved.as_deref().cloned().unwrap_or_default();
    match kept {
        // The queries changed: the same part with new content.
        Some((i, _)) if !wb.queries.is_empty() => {
            let data = item_part(&wb.queries)?;
            if let Some(part) = p.parts.get_mut(i) {
                part.data = Arc::from(data);
            }
        }
        // Every query was deleted: the part goes, with its properties part.
        Some((i, _)) => {
            let part = p.parts.remove(i);
            let owned: Vec<String> = part.rels.iter().filter(|r| !r.external).map(|r| normalize(&r.target)).collect();
            p.parts.retain(|x| !owned.contains(&normalize(&x.name)));
            p.rels.retain(|r| normalize(&r.target) != normalize(&part.name));
        }
        // The workbook's first queries.
        None => {
            let props = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?><ds:datastoreItem ds:itemID=\"{ITEM_ID}\" xmlns:ds=\"http://schemas.openxmlformats.org/officeDocument/2006/customXml\"><ds:schemaRefs><ds:schemaRef ds:uri=\"{NS}\"/></ds:schemaRefs></ds:datastoreItem>"
            );
            p.parts.push(PreservedPart {
                name: ITEM.into(),
                content_type: "application/xml".into(),
                data: Arc::from(item_part(&wb.queries)?),
                rels: vec![PreservedRel { id: "rId1".into(), rel_type: REL_PROPS.into(), target: ITEM_PROPS.into(), external: false }],
            });
            p.parts.push(PreservedPart { name: ITEM_PROPS.into(), content_type: CT_PROPS.into(), data: Arc::from(props.into_bytes()), rels: vec![] });
            p.rels.push(PreservedRel { id: "rIdQueries".into(), rel_type: REL_CUSTOM_XML.into(), target: ITEM.into(), external: false });
        }
    }
    Ok(Some(Arc::new(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, formula: &str, description: &str, load_to: Option<&str>) -> Query {
        Query { name: name.into(), formula: formula.into(), description: description.into(), load_to: load_to.map(str::to_string) }
    }

    #[test]
    fn a_section_document_splits_into_its_queries() {
        let src = "section Section1;\r\n\r\nshared Sales = let\r\n    Source = Csv.Document(\"a;b\"),  // split; here\r\n    Note = \"semi;colon \"\"quoted\"\"\" /* ; */\r\nin\r\n    Source;\r\n\r\n[ Description = \"Totals; by #(lf)region\" ]\r\nshared #\"By Region\" = Table.Group(Sales, {\"Region\"}, {});\r\nHelper = (x) => x + 1;\r\n";
        let m = split_section(src);
        assert_eq!(m.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Sales", "By Region", "Helper"]);
        assert!(m[0].formula.starts_with("let\r\n    Source") && m[0].formula.ends_with("in\r\n    Source"));
        assert_eq!(m[1].description, "Totals; by \nregion");
        assert_eq!(m[1].formula, "Table.Group(Sales, {\"Region\"}, {})");
        assert_eq!(m[2].formula, "(x) => x + 1");
        // Damage loses only what follows it.
        assert_eq!(split_section("section S; shared A = 1; shared B = \"open").len(), 1);
        assert!(split_section("").is_empty() && split_section("section").is_empty() && split_section("[[[[").is_empty());
    }

    #[test]
    fn written_queries_read_back() {
        let queries = vec![
            query("Sales", "let\n    Source = 1 // the source\nin\n    Source", "", Some("Sales")),
            query("By Region (2024)", "Table.Group(Sales, {\"R\"}, {})", "Totals \"by\" region;\n#(not an escape)", None),
            query("let", "1", "", Some("Missing")),
            query("Ação.Total", "\"ç; ☃\"", "", None),
        ];
        let part = item_part(&queries).unwrap();
        let has_table = |name: &str| (name == "Sales").then(|| "Sales".to_string());
        let back = queries_of(&part, &has_table).unwrap();
        // A fill target that isn't a table of the workbook isn't kept.
        let mut want = queries.clone();
        want[2].load_to = None;
        assert_eq!(back, want);
        // The same part as Excel writes it: UTF-16 with a byte-order mark.
        let text = String::from_utf8(part).unwrap();
        let utf16: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(queries_of(&utf16, &has_table).unwrap(), want);
    }

    /// A workbook with the table `Sales` at A1:B2 and two queries, one loaded to that table.
    fn workbook() -> Workbook {
        use gridcraft_core::{CellRef, RangeRef, Value};
        use gridcraft_model::{Table, TableColumn, TotalsFn};
        let mut wb = Workbook::new();
        let sh = wb.sheet_mut(0).unwrap();
        for (a, v) in [("A1", Value::text("Region")), ("B1", Value::text("Total")), ("A2", Value::text("North")), ("B2", Value::Number(3.0))] {
            sh.set_value(CellRef::parse(a).unwrap(), v);
        }
        sh.tables.push(Table {
            id: 1,
            name: "Sales".into(),
            range: RangeRef::parse("A1:B2").unwrap(),
            header_row: true,
            totals_row: false,
            columns: ["Region", "Total"]
                .iter()
                .map(|n| TableColumn { name: (*n).into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None })
                .collect(),
            style: "TableStyleMedium2".into(),
            banded_rows: true,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: true,
            filter: vec![],
            table_type: None,
            connection_id: None,
        });
        wb.queries = vec![
            query("Sales", "#table({\"Region\", \"Total\"}, {{\"North\", 3}})", "by region", Some("Sales")),
            query("Helper Fn", "(x) => x + 1", "", None),
        ];
        wb
    }

    /// The Data Mashup part a workbook was read with.
    fn kept_part(wb: &Workbook) -> Option<Arc<[u8]>> {
        let (i, _) = kept_queries(wb)?;
        wb.preserved.as_deref()?.parts.get(i).map(|p| p.data.clone())
    }

    #[test]
    fn queries_survive_saving_and_opening() {
        let wb = workbook();
        let bytes = crate::write_xlsx(&wb).unwrap();
        let (back, report) = crate::read_xlsx(&bytes).unwrap();
        assert_eq!(back.queries, wb.queries, "{:?}", report.warnings);
        // Saved again unchanged, the part is the one that was read, byte for byte.
        let first = kept_part(&back).unwrap();
        let (again, _) = crate::read_xlsx(&crate::write_xlsx(&back).unwrap()).unwrap();
        assert_eq!(again.queries, wb.queries);
        assert!(kept_part(&again).unwrap()[..] == first[..]);
        // A changed query is written anew, in the same part.
        let mut edited = again.clone();
        edited.queries[1].formula = "(x) => x + 2".into();
        edited.queries.push(query("Third", "1", "", None));
        let (reread, _) = crate::read_xlsx(&crate::write_xlsx(&edited).unwrap()).unwrap();
        assert_eq!(reread.queries, edited.queries);
        assert_eq!(reread.preserved.as_deref().unwrap().parts.len(), again.preserved.as_deref().unwrap().parts.len());
        // A query whose table is gone comes back as a connection only.
        let mut untabled = reread.clone();
        untabled.sheet_mut(0).unwrap().tables.clear();
        let (reread, _) = crate::read_xlsx(&crate::write_xlsx(&untabled).unwrap()).unwrap();
        assert_eq!(reread.queries[0].load_to, None);
        // With every query deleted the part goes too.
        let mut emptied = reread.clone();
        emptied.queries.clear();
        let (reread, _) = crate::read_xlsx(&crate::write_xlsx(&emptied).unwrap()).unwrap();
        assert!(reread.queries.is_empty() && kept_part(&reread).is_none());
        assert!(reread.preserved.as_deref().is_none_or(|p| p.parts.is_empty() && p.rels.is_empty()));
        // A workbook without queries has no such part to begin with.
        assert!(preserved_with_queries(&Workbook::new()).unwrap().is_none());
    }

    #[test]
    fn the_written_part_has_what_excel_needs() {
        // Observed in Excel: it ignores the queries of a part whose package description is in
        // the Data Mashup namespace, or whose metadata doesn't end with a package.
        let part = item_part(&[query("My Query (1)", "1", "", None)]).unwrap();
        let binary = payload(&part).unwrap();
        let s = streams(&binary).unwrap();
        let mut package = Package::open(s.package).unwrap();
        let config = xml_text(&package.read("Config/Package.xml").unwrap().unwrap());
        assert!(config.contains("<Package xmlns:xsd=") && !config.contains(NS), "{config}");
        let empty_zip = ZipOut::new().finish().unwrap();
        assert_eq!(empty_zip.len(), 22);
        let tail = &s.metadata[s.metadata.len() - 26..];
        assert_eq!(tail[..4], 22u32.to_le_bytes());
        assert_eq!(tail[4..], empty_zip[..]);
        assert!(xml_text(s.metadata.get(8..).unwrap()).contains("<ItemPath>Section1/My%20Query%20(1)</ItemPath>"));
    }

    #[test]
    fn a_part_shaped_like_excels_is_read() {
        // Built here in the shape Excel gives the part: UTF-16 outside, marked UTF-8 inside, a
        // description attribute, a quoted name with its path percent-encoded, query ids and step
        // items in the metadata, and permission bindings.
        const XSD: &str = "xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
        let section = "section Section1;\r\n\r\n[ Description = \"reads Stock\" ]\r\nshared Base = let\r\n    Source = 1\r\nin\r\n    Source;\r\n\r\nshared #\"In Stock (2024)\" = let\r\n    Kept = Base, // a comment; with a semicolon\r\n    Note = \"a;b\"\r\nin\r\n    Kept;";
        let item = |path: &str, entries: &str| {
            format!("<Item><ItemLocation><ItemType>Formula</ItemType><ItemPath>{path}</ItemPath></ItemLocation>{entries}</Item>")
        };
        let entry = |ty: &str, value: &str| format!("<Entry Type=\"{ty}\" Value=\"{value}\" />");
        let metadata_xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><LocalPackageMetadataFile {XSD}><Items><Item><ItemLocation><ItemType>AllFormulas</ItemType><ItemPath /></ItemLocation><StableEntries /></Item>{}{}{}</Items></LocalPackageMetadataFile>",
            item(
                "Section1/Base",
                &format!(
                    "<StableEntries>{}{}{}</StableEntries>",
                    entry("IsPrivate", "l0"),
                    entry("FillEnabled", "l0"),
                    entry("FillObjectType", "sConnectionOnly")
                )
            ),
            item("Section1/Base/Source", "<StableEntries />"),
            item(
                "Section1/In%20Stock%20(2024)",
                &format!(
                    "<StableEntries>{}{}{}{}{}</StableEntries>",
                    entry("FillEnabled", "l1"),
                    entry("FillObjectType", "sTable"),
                    entry("QueryID", "s00000000-0000-0000-0000-000000000001"),
                    entry("FillTarget", "sLoaded"),
                    entry("BufferNextRefresh", "l1")
                )
            ),
        );
        let mut zip = ZipOut::new();
        zip.add(
            "Config/Package.xml",
            &xml_bytes(&format!("<Package {XSD}><Version>2.100.0.0</Version><MinVersion>2.21.0.0</MinVersion><Culture>pt-BR</Culture></Package>")),
        )
        .unwrap();
        zip.add("[Content_Types].xml", &xml_bytes("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>")).unwrap();
        zip.add("Formulas/Section1.m", section.as_bytes()).unwrap();
        let mut metadata = 0u32.to_le_bytes().to_vec();
        block(&mut metadata, &xml_bytes(&metadata_xml)).unwrap();
        block(&mut metadata, &ZipOut::new().finish().unwrap()).unwrap();
        let mut stream = 0u32.to_le_bytes().to_vec();
        for part in [&zip.finish().unwrap()[..], &xml_bytes("<PermissionList/>"), &metadata, &[1, 0, 0, 0, 9, 9, 9, 9]] {
            block(&mut stream, part).unwrap();
        }
        let xml = format!("<?xml version=\"1.0\" encoding=\"utf-16\"?><DataMashup xmlns=\"{NS}\">{}</DataMashup>", base64_encode(&stream));
        let utf16: Vec<u8> = [0xff, 0xfe].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        let has_table = |name: &str| name.eq_ignore_ascii_case("loaded").then(|| "Loaded".to_string());
        assert_eq!(
            queries_of(&utf16, &has_table).unwrap(),
            [
                query("Base", "let\r\n    Source = 1\r\nin\r\n    Source", "reads Stock", None),
                query(
                    "In Stock (2024)",
                    "let\r\n    Kept = Base, // a comment; with a semicolon\r\n    Note = \"a;b\"\r\nin\r\n    Kept",
                    "",
                    Some("Loaded")
                ),
            ]
        );
    }

    #[test]
    fn other_custom_xml_and_damage_are_not_queries() {
        let none = |_: &str| -> Option<String> { None };
        assert!(queries_of(b"<root xmlns=\"urn:other\">AAAA</root>", &none).is_none());
        assert!(queries_of(format!("<DataMashup xmlns=\"{NS}\">not base64!</DataMashup>").as_bytes(), &none).is_none());
        assert!(queries_of(format!("<DataMashup xmlns=\"{NS}\">{}</DataMashup>", base64_encode(&[1, 0, 0, 0])).as_bytes(), &none).is_none());
        // Lengths that run past the end, and a package that isn't a zip.
        assert!(
            queries_of(
                format!("<DataMashup xmlns=\"{NS}\">{}</DataMashup>", base64_encode(&[0, 0, 0, 0, 255, 255, 255, 127, 1, 2])).as_bytes(),
                &none
            )
            .is_none()
        );
        let mut bad = vec![0u8; 4];
        for part in [&b"PK-not-a-zip"[..], b"", b"", b""] {
            block(&mut bad, part).unwrap();
        }
        assert!(queries_of(format!("<DataMashup xmlns=\"{NS}\">{}</DataMashup>", base64_encode(&bad)).as_bytes(), &none).is_none());
        assert!(queries_of(b"", &none).is_none() && queries_of(b"<", &none).is_none() && queries_of(b"<?xml", &none).is_none());
    }

    #[test]
    fn names_and_text_are_written_as_m() {
        assert_eq!(member_name("Sales"), "Sales");
        assert_eq!(member_name("Sales.2024"), "#\"Sales.2024\"");
        assert_eq!(member_name("My Query"), "#\"My Query\"");
        assert_eq!(member_name("a\"b"), "#\"a\"\"b\"");
        assert_eq!(member_name("type"), "#\"type\"");
        let literal = text_literal("a#(b)\r\n\t\"");
        assert_eq!(literal, "\"a#(#)(b)#(cr)#(lf)#(tab)\"\"\"");
        assert_eq!(unescape(&Scan { s: &literal, i: 0 }.quoted().unwrap()), "a#(b)\r\n\t\"");
        assert_eq!(percent_decode(&percent_encode("By Region/á%")), "By Region/á%");
        assert_eq!(base64_decode(&base64_encode(b"any carnal pleas")).unwrap(), b"any carnal pleas");
        assert_eq!(base64_encode(b"a"), "YQ==");
    }
}
