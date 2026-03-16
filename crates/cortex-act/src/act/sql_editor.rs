use anyhow::{Context, Result};
use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::path::Path;

#[derive(Debug)]
pub struct SqlEdit {
    pub target: String, // e.g., "create_table:users"
    pub action: String, // "replace", "delete"
    pub code: String,
}

pub fn apply_sql_surgery(file_path: &Path, edits: Vec<SqlEdit>) -> Result<String> {
    let source_bytes = std::fs::read(file_path).context("Failed to read original source")?;
    let current_source = String::from_utf8_lossy(&source_bytes).into_owned();

    let dialect = GenericDialect {};
    let ast = Parser::parse_sql(&dialect, &current_source).context("Failed to parse SQL file")?;

    let mut operations = Vec::new();

    for edit in edits {
        let parts: Vec<&str> = edit.target.splitn(2, ':').collect();
        if parts.len() < 2 {
            anyhow::bail!("Invalid SQL target. Use e.g. 'create_table:users'");
        }
        let t_type = parts[0];
        let t_name = parts[1];

        let mut found_range = None;
        for stmt in &ast {
            match (t_type, stmt) {
                ("create_table", Statement::CreateTable { name, .. }) => {
                    if name.to_string() == t_name {
                        found_range = find_statement_range(&current_source, stmt);
                        break;
                    }
                }
                ("create_index", Statement::CreateIndex { name, .. }) => {
                    if let Some(n) = name {
                        if n.to_string() == t_name {
                            found_range = find_statement_range(&current_source, stmt);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some((start, end)) = found_range {
            operations.push((start, end, edit));
        } else {
            anyhow::bail!("SQL target '{}' not found", edit.target);
        }
    }

    let mut final_source = current_source.clone();
    operations.sort_by(|a, b| b.0.cmp(&a.0));

    for (start, end, edit) in operations {
        let prefix = &final_source[..start];
        let suffix = &final_source[end..];
        let replacement = if edit.action == "delete" {
            ""
        } else {
            &edit.code
        };
        final_source = format!("{}{}{}", prefix, replacement, suffix);
    }

    std::fs::write(file_path, &final_source).context("Failed to write SQL file")?;
    Ok(final_source)
}

fn find_statement_range(source: &str, stmt: &Statement) -> Option<(usize, usize)> {
    // sqlparser's stmt.to_string() normalizes whitespace / casing which means it
    // NEVER matches verbatim source text.  Instead we extract the logical name from
    // the AST and scan the original bytes with a case-insensitive pattern.
    let (kw, name) = match stmt {
        Statement::CreateTable { name, .. } => ("CREATE TABLE", name.to_string()),
        Statement::CreateIndex { name, .. } => (
            "CREATE INDEX",
            name.as_ref().map(|n| n.to_string()).unwrap_or_default(),
        ),
        _ => {
            // Generic fall-back: try the normalised string (better than nothing)
            let s = stmt.to_string();
            if let Some(pos) = source.find(&s) {
                return Some((pos, pos + s.len()));
            }
            return None;
        }
    };

    // Scan the source for `<KEYWORD>\s+...<name>` (case-insensitive).
    // We look for the keyword start, then verify the name appears nearby before
    // the next `;` or statement boundary.
    let src_upper = source.to_uppercase();
    let kw_upper = kw.to_uppercase();
    let name_upper = name
        .to_uppercase()
        // sqlparser may quote the name — strip quotes for matching
        .trim_matches('"')
        .trim_matches('`')
        .to_string();

    let mut search_from = 0;
    while let Some(kw_pos) = src_upper[search_from..].find(&kw_upper) {
        let abs_kw = search_from + kw_pos;
        let window = &src_upper[abs_kw..std::cmp::min(abs_kw + 512, src_upper.len())];

        // Check the name appears within the next 512 bytes of the keyword
        if window.contains(name_upper.as_str()) {
            // Find the end: advance past the keyword, then find the closing `;`
            let after_kw = abs_kw;
            let stmt_end = find_sql_statement_end(source, after_kw);
            return Some((abs_kw, stmt_end));
        }
        search_from = abs_kw + kw_upper.len();
    }
    None
}

/// Return the byte index of the end of the SQL statement starting at `from`.
/// We track parenthesis depth so we don't stop on a `;` inside `DEFAULT 'a;b'`.
fn find_sql_statement_end(source: &str, from: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = from;
    let mut depth: i32 = 0;
    let mut in_str: Option<u8> = None;

    while i < bytes.len() {
        let ch = bytes[i];
        match (in_str, ch) {
            (Some(delim), c) if c == delim => {
                in_str = None;
            }
            (Some(_), _) => {}
            (None, b'\'') | (None, b'"') => {
                in_str = Some(ch);
            }
            (None, b'(') => {
                depth += 1;
            }
            (None, b')') => {
                depth -= 1;
            }
            (None, b';') if depth == 0 => {
                return i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    source.len()
}
