//! Eligibility for typing-triggered previews, not a database authorization boundary.
//! Deliberately accept only one simple SELECT. Other SQL still uses explicit Preview.
//! Database permissions remain essential: even views can invoke server-side code.

/// No scripts, comments, functions, parameters, locking or SELECT INTO in automatic previews.
/// This conservative scan does not validate SQL syntax; the database does that afterwards.
pub(super) fn automatic_select(sql: &str) -> bool {
    let mut chars = sql.trim().chars().peekable();
    let mut first = true;
    let mut previous_word = String::new();
    let mut quoted_identifier = false;
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut word = String::from(c);
            while chars.peek().is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
            word.make_ascii_uppercase();
            if first && word != "SELECT" {
                return false;
            }
            first = false;
            if matches!(
                word.as_str(),
                "INSERT"
                    | "UPDATE"
                    | "DELETE"
                    | "MERGE"
                    | "REPLACE"
                    | "INTO"
                    | "CREATE"
                    | "ALTER"
                    | "DROP"
                    | "TRUNCATE"
                    | "EXEC"
                    | "EXECUTE"
                    | "CALL"
                    | "DO"
                    | "SET"
                    | "GRANT"
                    | "REVOKE"
                    | "DENY"
                    | "COPY"
                    | "LOAD"
                    | "PRAGMA"
                    | "ATTACH"
                    | "DETACH"
                    | "VACUUM"
                    | "REINDEX"
                    | "ANALYZE"
                    | "LOCK"
                    | "UNLOCK"
                    | "FOR"
                    | "OUTFILE"
                    | "DUMPFILE"
                    | "BEGIN"
                    | "COMMIT"
                    | "ROLLBACK"
                    | "GO"
            ) {
                return false;
            }
            previous_word = word;
            quoted_identifier = false;
            continue;
        }
        if first {
            return false;
        }
        match c {
            '\'' | '"' | '`' | '[' => {
                let end = if c == '[' { ']' } else { c };
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        return false;
                    } // Escaping differs across database dialects.
                    if c == end {
                        if chars.peek() == Some(&end) {
                            chars.next();
                        } else {
                            closed = true;
                            break;
                        }
                    }
                }
                if !closed {
                    return false;
                }
                quoted_identifier = end != '\'';
                previous_word.clear();
            }
            '(' => {
                // No stored functions, including schema-qualified/quoted functions. SQL syntax
                // such as TOP (10), IN (...) and derived tables remains usable.
                if quoted_identifier
                    || (!previous_word.is_empty()
                        && !matches!(
                            previous_word.as_str(),
                            "SELECT"
                                | "TOP"
                                | "IN"
                                | "EXISTS"
                                | "FROM"
                                | "JOIN"
                                | "WHERE"
                                | "ON"
                                | "AND"
                                | "OR"
                                | "NOT"
                                | "HAVING"
                                | "WHEN"
                                | "THEN"
                                | "ELSE"
                                | "AS"
                        ))
                {
                    return false;
                }
                depth = depth.saturating_add(1);
                if depth > 64 {
                    return false;
                }
                previous_word.clear();
                quoted_identifier = false;
            }
            ')' => {
                let Some(next) = depth.checked_sub(1) else { return false };
                depth = next;
                previous_word.clear();
                quoted_identifier = false;
            }
            ';' => return !first && depth == 0 && chars.all(char::is_whitespace),
            '-' if chars.peek() == Some(&'-') => return false,
            '/' if chars.peek() == Some(&'*') => return false,
            '#' | '$' | '@' | '\\' | '!' => return false,
            _ if c.is_ascii_digit() || "+-*/%=<>|&^~,.?:".contains(c) => {
                previous_word.clear();
                quoted_identifier = false;
            }
            _ => return false,
        }
    }
    !first && depth == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_previews_accept_simple_selects_only() {
        for sql in [
            "SELECT * FROM pedidos WHERE valor < 300",
            "select p.id, p.nome FROM dbo.pedidos p JOIN clientes c ON p.id = c.id;",
            "SELECT TOP (50) * FROM [dbo].[pedidos] WHERE id IN (1, 2)",
            "SELECT 'DROP TABLE x; -- literal', \"delete\" FROM `table`",
            "SELECT * FROM (SELECT id FROM pedidos) AS q WHERE id > 1",
        ] {
            assert!(automatic_select(sql), "{sql}");
        }
        for sql in [
            "",
            "DELETE FROM pedidos",
            "UPDATE pedidos SET id=1",
            "ALTER TABLE x ADD id INT",
            "DROP TABLE x",
            "SELECT * INTO backup FROM pedidos",
            "SELECT 1; DROP TABLE x",
            "SELECT 1; SELECT 2",
            "WITH x AS (DELETE FROM pedidos RETURNING *) SELECT * FROM x",
            "SELECT * FROM pedidos FOR UPDATE",
            "SELECT dangerous_function()",
            "SELECT dbo.fn()",
            "SELECT [fn]()",
            "SELECT `fn`()",
            "SELECT \"fn\"()",
            "SELECT COUNT(*) FROM pedidos",
            "SELECT 1 /*!; DROP TABLE x */",
            "SELECT 1 -- trailing comment",
            "SELECT $q$payload$q$",
            "SELECT 'unterminated",
            "SELECT (1",
            "SELECT 1)",
            "SELECT 'back\\slash'",
            "SELECT @x := 1",
            "SELECT 函数()",
            "SELECT função()",
        ] {
            assert!(!automatic_select(sql), "{sql}");
        }
    }
}
