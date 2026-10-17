//! Query transformations edit M text only. The caller previews or saves the returned formula.
use super::*;
use gridcraft_mquery::{
    self as mq,
    lexer::{Kw, Tok},
};
use serde_json::json;

const MAX_FORMULA: usize = 1 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "data.query.addStep", "Add Query Step", [], None,
            "{formula, name?: query being edited, op: removeColumns|keepColumns|changeType|filter|sort|rename|groupBy|removeDuplicates|replaceValues|splitColumn|addColumn|keepTopRows|promoteHeaders|removeBlankRows|fillDown|unpivotOthers, params: {columns?: [names], column?, ...}} → {formula, step}; appends a transformation to the result, validates its columns; changes no workbook",
            has_doc, add),
        cmd!(query "data.query.removeStep", "Remove Query Step", [], None,
            "{formula, step} → {formula}; reconnects references to the preceding step; refuses to remove a referenced source", always, remove),
        cmd!(query "data.query.renameStep", "Rename Query Step", [], None,
            "{formula, step, to} → {formula}; renames references with lexical scope, leaving fields and text alone", always, rename),
    ]
}

fn err(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}
fn required<'a>(p: &'a Json, key: &str) -> Result<&'a str> {
    str_param(p, key).filter(|s| !s.trim().is_empty()).ok_or_else(|| err(format!("Missing `{key}`.")))
}
fn formula(p: &Json) -> Result<&str> {
    let f = required(p, "formula")?;
    if f.len() > MAX_FORMULA {
        return Err(err("The formula is too long."));
    }
    mq::parse(f).map_err(|e| err(e.message))?;
    Ok(f)
}
fn checked(f: String) -> Result<Json> {
    if f.len() > MAX_FORMULA {
        return Err(err("The formula is too long."));
    }
    mq::parse(&f).map_err(|e| err(e.message))?;
    Ok(json!({"formula": f}))
}

/// The `in` token after the final top-level binding (nested lets are inside its span).
fn result_start(f: &str, steps: &mq::ast::Steps) -> Result<usize> {
    let end = steps.steps.last().map_or(0, |s| s.span.1);
    mq::lexer::lex(f)
        .map_err(|e| err(e.message))?
        .into_iter()
        .find(|t| t.start >= end && t.tok == Tok::Kw(Kw::In))
        .map(|t| t.end)
        .ok_or_else(|| err("The formula has no result."))
}

fn rewrite(f: &str, from: &str, to: &str) -> Result<String> {
    let mut spans = mq::step_edit::references(f, from, to).map_err(err)?;
    let steps = mq::steps(f).map_err(|e| err(e.message))?;
    let step = steps.steps.iter().find(|s| s.name == from).ok_or_else(|| err("The step is missing."))?;
    let tokens = mq::lexer::lex(f).map_err(|e| err(e.message))?;
    // The last identifier before the binding's expression belongs to its declaration.
    let declaration = tokens
        .iter()
        .rev()
        .find(|t| t.end <= step.span.0 && matches!(&t.tok, Tok::Ident(n) if &**n == from))
        .ok_or_else(|| err("The step declaration is missing."))?;
    spans.push((declaration.start, declaration.end));
    spans.sort_unstable();
    spans.dedup();
    let mut out = f.to_string();
    let quoted = mq::quote_identifier(to);
    for (a, b) in spans.into_iter().rev() {
        out.replace_range(a..b, &quoted);
    }
    Ok(out)
}

fn rename(_: &mut Session, p: &Json) -> Result<Json> {
    let f = formula(p)?;
    let (from, to) = (required(p, "step")?, required(p, "to")?);
    if to.chars().count() > 80 || to.chars().any(char::is_control) {
        return Err(err("A step name has at most 80 characters and no control characters."));
    }
    let steps = mq::steps(f).map_err(|e| err(e.message))?;
    if from == to && steps.steps.iter().any(|s| s.name == from) {
        return checked(f.to_string());
    }
    if from != to && steps.steps.iter().any(|s| s.name == to) {
        return Err(err(format!("A step named '{to}' already exists.")));
    }
    checked(rewrite(f, from, to)?)
}

fn remove(_: &mut Session, p: &Json) -> Result<Json> {
    let f = formula(p)?;
    let from = required(p, "step")?;
    let steps = mq::steps(f).map_err(|e| err(e.message))?;
    let index = steps.steps.iter().position(|s| s.name == from).ok_or_else(|| err(format!("There is no step named '{from}'.")))?;
    let previous = index.checked_sub(1).and_then(|i| steps.steps.get(i));
    let mut refs = mq::step_edit::references(f, from, previous.map_or(from, |p| p.name.as_str())).map_err(err)?;
    refs.sort_unstable();
    if previous.is_some_and(|prev| refs.iter().any(|(a, b)| *a >= prev.span.0 && *b <= prev.span.1)) {
        return Err(err("Removing this step would make its predecessor refer to itself."));
    }
    if previous.is_none() && !refs.is_empty() {
        return Err(err("The source step is referenced and cannot be removed."));
    }
    // Rewire first, then recover the source spans, which may have changed with the name lengths.
    let rewired = match previous {
        Some(prev) => {
            let quoted = mq::quote_identifier(&prev.name);
            let mut out = f.to_string();
            for (a, b) in refs.into_iter().rev() {
                out.replace_range(a..b, &quoted);
            }
            out
        }
        None => f.to_string(),
    };
    let steps = mq::steps(&rewired).map_err(|e| err(e.message))?;
    let body = rewired.get(result_start(&rewired, &steps)?..).unwrap_or("").trim();
    let bindings: Vec<_> = steps
        .steps
        .iter()
        .filter(|s| s.name != from)
        .map(|s| format!("    {} = {}", mq::quote_identifier(&s.name), rewired.get(s.span.0..s.span.1).unwrap_or("")))
        .collect();
    checked(if bindings.is_empty() { body.to_string() } else { format!("let\n{}\nin\n    {body}", bindings.join(",\n")) })
}

fn column_list(p: &Json) -> Result<Vec<String>> {
    let values = match p.get("columns") {
        None => return Ok(Vec::new()),
        Some(Json::Array(v)) => v,
        _ => return Err(err("`columns` must be a list of names.")),
    };
    if values.len() > 16_384 {
        return Err(err("Too many selected columns."));
    }
    let mut names = Vec::new();
    for v in values {
        let Some(n) = v.as_str().filter(|n| !n.is_empty()) else {
            return Err(err("Column names must be nonempty text."));
        };
        if names.iter().any(|x| x == n) {
            return Err(err("A column is selected more than once."));
        }
        names.push(n.to_string());
    }
    Ok(names)
}
fn list(names: &[String]) -> String {
    format!("{{{}}}", names.iter().map(|n| mq::quote_text(n)).collect::<Vec<_>>().join(", "))
}
fn literal(value: &Json) -> Result<String> {
    match value {
        Json::Null => Ok("null".into()),
        Json::String(s) => Ok(mq::quote_text(s)),
        Json::Bool(_) | Json::Number(_) => Ok(value.to_string()),
        _ => Err(err("Use a text, number, logical or null value.")),
    }
}

fn operation(op: &str, p: &Json, prev: &str, columns: &[String]) -> Result<(&'static str, String)> {
    let selected = list(columns);
    let col = || required(p, "column").map(mq::quote_text);
    let field = || required(p, "column").map(|n| format!("[{}]", mq::quote_identifier(n)));
    let name = || required(p, "name").map(mq::quote_text);
    let needs_columns = matches!(op, "removeColumns" | "keepColumns" | "changeType" | "removeDuplicates" | "fillDown" | "unpivotOthers" | "groupBy");
    if needs_columns && columns.is_empty() {
        return Err(err("Select at least one column."));
    }
    Ok(match op {
        "removeColumns" => ("Removed Columns", format!("Table.RemoveColumns({prev}, {selected})")),
        "keepColumns" => ("Removed Other Columns", format!("Table.SelectColumns({prev}, {selected})")),
        "changeType" => {
            let t = match required(p, "type")? {
                "text" => "type text",
                "number" => "type number",
                "integer" => "Int64.Type",
                "date" => "type date",
                "datetime" => "type datetime",
                "time" => "type time",
                "logical" => "type logical",
                "percentage" => "Percentage.Type",
                _ => return Err(err("Unknown column type.")),
            };
            let pairs = columns.iter().map(|n| format!("{{{}, {t}}}", mq::quote_text(n))).collect::<Vec<_>>().join(", ");
            (
                "Changed Type",
                format!("Table.TransformColumnTypes({prev}, {{{pairs}}}, {})", mq::quote_text(str_param(p, "culture").unwrap_or("en-US"))),
            )
        }
        "filter" => {
            let f = field()?;
            let value = literal(p.get("value").unwrap_or(&Json::Null))?;
            let predicate = match required(p, "operator")? {
                operator @ ("=" | "<>" | ">" | ">=" | "<" | "<=") => format!("{f} {operator} {value}"),
                "contains" => format!("Text.Contains({f}, {value})"),
                "startsWith" => format!("Text.StartsWith({f}, {value})"),
                "endsWith" => format!("Text.EndsWith({f}, {value})"),
                "isNull" => format!("{f} = null"),
                _ => return Err(err("Unknown filter operator.")),
            };
            ("Filtered Rows", format!("Table.SelectRows({prev}, each {predicate})"))
        }
        "sort" => {
            let order = match str_param(p, "order").unwrap_or("ascending") {
                "ascending" => "Order.Ascending",
                "descending" => "Order.Descending",
                _ => return Err(err("Unknown sort order.")),
            };
            ("Sorted Rows", format!("Table.Sort({prev}, {{{{{}, {order}}}}})", col()?))
        }
        "rename" => ("Renamed Columns", format!("Table.RenameColumns({prev}, {{{{{}, {}}}}})", col()?, name()?)),
        "groupBy" => {
            let aggregation = required(p, "aggregation")?;
            let (expression, t) = match aggregation {
                "count" => ("Table.RowCount(_)".into(), "Int64.Type"),
                "sum" | "average" | "min" | "max" => {
                    let f = match aggregation {
                        "sum" => "Sum",
                        "average" => "Average",
                        "min" => "Min",
                        _ => "Max",
                    };
                    (format!("List.{f}({})", field()?), "type number")
                }
                _ => return Err(err("Unknown aggregation.")),
            };
            ("Grouped Rows", format!("Table.Group({prev}, {selected}, {{{{{}, each {expression}, {t}}}}})", name()?))
        }
        "removeDuplicates" => ("Removed Duplicates", format!("Table.Distinct({prev}, {selected})")),
        "replaceValues" => {
            if columns.is_empty() {
                return Err(err("Select at least one column."));
            }
            let replacer = if p["textOnly"] == true { "Replacer.ReplaceText" } else { "Replacer.ReplaceValue" };
            (
                "Replaced Value",
                format!(
                    "Table.ReplaceValue({prev}, {}, {}, {replacer}, {selected})",
                    literal(p.get("from").unwrap_or(&Json::Null))?,
                    literal(p.get("to").unwrap_or(&Json::Null))?
                ),
            )
        }
        "splitColumn" => {
            let delimiter = required(p, "delimiter")?;
            let names = p.get("names").and_then(Json::as_array).ok_or_else(|| err("Give output column names."))?;
            if names.len() < 2 || names.len() > 100 {
                return Err(err("Use 2 to 100 output columns."));
            }
            let names = column_list(&json!({"columns":names}))?;
            (
                "Split Column by Delimiter",
                format!(
                    "Table.SplitColumn({prev}, {}, Splitter.SplitTextByDelimiter({}, QuoteStyle.Csv), {})",
                    col()?,
                    mq::quote_text(delimiter),
                    list(&names)
                ),
            )
        }
        "addColumn" => {
            let expression = required(p, "expression")?;
            mq::parse(&format!("each ({expression})")).map_err(|e| err(e.message))?;
            ("Added Custom", format!("Table.AddColumn({prev}, {}, each ({expression}))", name()?))
        }
        "keepTopRows" => {
            let n = p["count"].as_u64().filter(|n| *n <= 1_048_576).ok_or_else(|| err("Row count must be an integer from 0 to 1048576."))?;
            ("Kept First Rows", format!("Table.FirstN({prev}, {n})"))
        }
        "promoteHeaders" => ("Promoted Headers", format!("Table.PromoteHeaders({prev}, [PromoteAllScalars = true])")),
        "removeBlankRows" => (
            "Removed Blank Rows",
            format!("Table.SelectRows({prev}, each not List.IsEmpty(List.RemoveMatchingItems(Record.FieldValues(_), {{\"\", null}})))"),
        ),
        "fillDown" => ("Filled Down", format!("Table.FillDown({prev}, {selected})")),
        "unpivotOthers" => ("Unpivoted Other Columns", format!("Table.UnpivotOtherColumns({prev}, {selected}, \"Attribute\", \"Value\")")),
        "navigate" => {
            let row = p["row"].as_u64().filter(|r| *r < 1_048_576).ok_or_else(|| err("Invalid navigation row."))?;
            ("Navigation", format!("{prev}{{{row}}}[{}]", mq::quote_identifier(required(p, "column")?)))
        }
        _ => return Err(err(format!("Unknown transformation '{op}'."))),
    })
}

fn add(s: &mut Session, p: &Json) -> Result<Json> {
    let f = formula(p)?;
    let op = required(p, "op")?;
    let params = p.get("params").unwrap_or(&Json::Null);
    let columns = column_list(params)?;
    let steps = mq::steps(f).map_err(|e| err(e.message))?;
    let wrap = steps.result.is_none();
    let previous = steps.result.as_deref().unwrap_or("Source");
    let (base, expr) = operation(op, params, &mq::quote_identifier(previous), &columns)?;
    let preview = super::query::preview(s, &json!({"formula":f,"name":p.get("name"),"rows":1}))?;
    let available: Vec<_> = preview["columns"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str()).collect();
    if preview["kind"] != "table" {
        return Err(err("Transformations require a table result."));
    }
    let single_column = if matches!(op, "filter" | "sort" | "rename" | "splitColumn" | "navigate")
        || (op == "groupBy" && str_param(params, "aggregation") != Some("count"))
    {
        str_param(params, "column")
    } else {
        None
    };
    for column in columns.iter().map(String::as_str).chain(single_column) {
        if !available.contains(&column) {
            return Err(err(format!("There is no column named '{column}'.")));
        }
    }
    if matches!(op, "rename" | "addColumn" | "groupBy")
        && let Some(name) = str_param(params, "name")
    {
        let collision = if op == "groupBy" {
            columns.iter().any(|c| c == name)
        } else {
            available.contains(&name) && !(op == "rename" && str_param(params, "column") == Some(name))
        };
        if collision {
            return Err(err(format!("A column named '{name}' already exists.")));
        }
    }
    let tokens = mq::lexer::lex(f).map_err(|e| err(e.message))?;
    let taken = |name: &str| tokens.iter().any(|t| matches!(&t.tok,Tok::Ident(n) if &**n == name)) || name == previous;
    let mut step = base.to_string();
    let mut k = 1;
    while taken(&step) {
        step = format!("{base} {k}");
        k += 1;
    }
    let quoted = mq::quote_identifier(&step);
    let out = if wrap {
        format!("let\n    Source = (\n{f}\n    ),\n    {quoted} = {expr}\nin\n    {quoted}")
    } else {
        let end = steps.steps.last().map_or(0, |s| s.span.1);
        let body = result_start(f, &steps)?;
        // Keep trivia and comments before `in`; the result is the new step.
        let in_token = mq::lexer::lex(f).map_err(|e| err(e.message))?.into_iter().find(|t| t.end == body).ok_or_else(|| err("Missing in clause."))?;
        format!("{},\n    {quoted} = {expr}\n{}in\n    {quoted}", f.get(..end).unwrap_or(""), f.get(end..in_token.start).unwrap_or(""))
    };
    let mut result = checked(out)?;
    result["step"] = json!(step);
    Ok(result)
}
