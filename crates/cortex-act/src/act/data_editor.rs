use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

#[derive(Debug)]
pub struct DataEdit {
    pub target: String, // e.g., "$.paths['/users'].get"
    pub action: String, // "set", "delete"
    pub value: Option<String>,
}

pub fn apply_data_edits(file_path: &Path, edits: Vec<DataEdit>) -> Result<String> {
    let source_bytes = std::fs::read(file_path).context("Failed to read original source")?;
    let mut current_source = String::from_utf8_lossy(&source_bytes).into_owned();

    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Use Language::from(LanguageFn) — the correct API for tree-sitter >= 0.24
    let lang: Language = match ext {
        "json" => Language::from(tree_sitter_json::LANGUAGE),
        "yaml" | "yml" => Language::from(tree_sitter_yaml::LANGUAGE),
        // TOML support removed: tree-sitter-toml v0.20 depends on tree-sitter 0.20 which
        // conflicts with the workspace tree-sitter 0.26, causing duplicate C symbols when
        // linking with lld. Use cortex_fs_manage(action=write) to overwrite TOML files.
        "toml" => anyhow::bail!(
            "TOML editing via cortex_act_edit_data_graph is not supported (tree-sitter version conflict). \
             Use cortex_fs_manage(action=write) to rewrite the TOML file instead."
        ),
        _ => anyhow::bail!("Unsupported file type for data graph editor: {}", ext),
    };

    // Apply each edit sequentially (re-parses tree after each change).
    // This is O(n * parse_cost) but safe for any combination of normal edits
    // and upserts whose insertions would invalidate pre-computed byte offsets.
    for edit in edits {
        current_source = apply_single_edit(&current_source, &lang, ext, &edit)?;
    }

    std::fs::write(file_path, &current_source).context("Failed to write to file")?;
    Ok(current_source)
}

/// Apply one `DataEdit` to `source` and return the updated string.
///
/// For `set` actions, if the exact target path is not found the function
/// automatically tries a **graceful upsert**: it navigates to the *parent*
/// object and inserts the missing key-value pair there — no manual
/// `replace`-on-parent workaround required.
fn apply_single_edit(
    source: &str,
    lang: &Language,
    ext: &str,
    edit: &DataEdit,
) -> Result<String> {
    match find_node_range(source, lang, ext, &edit.target) {
        Ok((start, end)) => {
            let prefix = &source[..start];
            let suffix = &source[end..];
            let replacement = match edit.action.as_str() {
                "delete" => String::new(),
                "replace" | "set" => {
                    let raw = edit.value.clone().unwrap_or_default();
                    if ext == "json" {
                        coerce_json_value(&raw)
                    } else {
                        raw
                    }
                }
                _ => anyhow::bail!("Unknown action: {}", edit.action),
            };
            Ok(format!("{}{}{}", prefix, replacement, suffix))
        }
        Err(find_err) if edit.action == "set" => {
            // Graceful upsert: target leaf did not exist, try to insert it
            // into the parent object rather than returning an error.
            upsert_into_parent(source, lang, ext, &edit.target, edit.value.as_deref())
                .with_context(|| {
                    format!(
                        "Path '{}' not found and upsert fallback also failed. \
                         Original find error: {}",
                        edit.target, find_err
                    )
                })
        }
        Err(e) => Err(e),
    }
}

/// Insert a new key-value pair into the JSON parent object when the target
/// leaf does not yet exist (upsert / auto-create semantics).
///
/// Example: target `$.scripts.test` with value `"jest"` inserts
/// `"test": "jest"` into the `scripts` object even if the key is absent.
///
/// Limitations:
/// - JSON only. For YAML / TOML use `action="replace"` on the parent path.
/// - The parent path must resolve to a JSON object (`{...}`).
fn upsert_into_parent(
    source: &str,
    lang: &Language,
    ext: &str,
    target: &str,
    value: Option<&str>,
) -> Result<String> {
    if ext != "json" {
        anyhow::bail!(
            "Upsert (auto-insert missing key) is supported for JSON only. \
             For YAML/TOML use action=\"replace\" on the parent object path."
        );
    }

    let segments = parse_path(target);
    if segments.len() < 2 {
        anyhow::bail!(
            "Cannot upsert at '{}': path needs at least two segments \
             (e.g., $.parent.new_key)",
            target
        );
    }

    let new_key = segments.last().unwrap().clone();
    let parent_path = format!("$.{}", segments[..segments.len() - 1].join("."));

    let (parent_start, parent_end) =
        find_node_range(source, lang, ext, &parent_path).with_context(|| {
            format!(
                "Parent path '{}' not found; cannot upsert key '{}'",
                parent_path, new_key
            )
        })?;

    let parent_src = &source[parent_start..parent_end];

    // Verify the parent is a JSON object
    let open_pos = parent_src.find('{').ok_or_else(|| {
        anyhow::anyhow!(
            "Parent at '{}' is not a JSON object (no '{{' found). \
             Use action=\"replace\" to overwrite the entire parent value.",
            parent_path
        )
    })?;
    let close_pos = parent_src.rfind('}').ok_or_else(|| {
        anyhow::anyhow!(
            "Malformed JSON: no closing '}}' in parent object at '{}'",
            parent_path
        )
    })?;

    let new_value = coerce_json_value(value.unwrap_or("null"));

    // Determine entry indentation from the line containing the closing brace
    let before_close = &parent_src[..close_pos];
    let entry_indent = if let Some(nl_pos) = before_close.rfind('\n') {
        let brace_line = &before_close[nl_pos + 1..];
        let brace_spaces = brace_line.len() - brace_line.trim_start().len();
        " ".repeat(brace_spaces + 2)
    } else {
        "  ".to_string() // single-line object fallback
    };

    // Comma separator: required when the object already has entries
    let inner = parent_src[open_pos + 1..close_pos].trim();
    let separator = if inner.is_empty() { "" } else { "," };

    let new_pair = format!(
        "{}\n{}\"{}\":{}",
        separator, entry_indent, new_key, new_value
    );

    let abs_close = parent_start + close_pos;
    Ok(format!("{}{}{}", &source[..abs_close], new_pair, &source[abs_close..]))
}



/// Parse a simple JSONPath-like string into segments
/// Coerce a user-supplied value string into a valid JSON token.
///
/// Rules (applied in order):
///  1. JSON literal (`true`, `false`, `null`) → pass through unchanged.
///  2. Already a properly quoted JSON string (starts and ends with `"`) → pass through.
///  3. Parses as a finite f64 (JSON number) → pass through.
///  4. Starts with `{` or `[` (JSON object / array) → pass through.
///  5. Anything else → treat as a plain text string: escape `\` and `"`, then wrap in `"\u2026"`.
fn coerce_json_value(val: &str) -> String {
    let t = val.trim();
    // JSON literals
    if matches!(t, "true" | "false" | "null") {
        return t.to_string();
    }
    // Already a quoted JSON string
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        return t.to_string();
    }
    // JSON number
    if !t.is_empty() && t.parse::<f64>().is_ok() {
        return t.to_string();
    }
    // JSON object or array passthrough
    if t.starts_with('{') || t.starts_with('[') {
        return t.to_string();
    }
    // Plain string: escape backslashes and double-quotes, then wrap
    let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn parse_path(target: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;

    let target = target.strip_prefix('$').unwrap_or(target);

    for c in target.chars() {
        match c {
            '\'' | '"' => in_quote = !in_quote,
            '.' if !in_quote => {
                if !current.is_empty() {
                    segments.push(current.clone());
                    current.clear();
                }
            }
            '[' if !in_quote => {
                if !current.is_empty() {
                    segments.push(current.clone());
                    current.clear();
                }
            }
            ']' if !in_quote => {
                if !current.is_empty() {
                    segments.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                if !in_quote || (c != '\'' && c != '"') {
                    current.push(c);
                }
            }
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}



fn find_node_range(
    source: &str,
    lang: &Language,
    ext: &str,
    target: &str,
) -> Result<(usize, usize)> {
    let mut parser = Parser::new();
    parser
        .set_language(lang)
        .context("Failed to set language")?;
    let tree = parser
        .parse(source, None)
        .context("Failed to parse data file")?;
    let root = tree.root_node();

    let segments = parse_path(target);
    if segments.is_empty() {
        return Ok((root.start_byte(), root.end_byte()));
    }

    let mut current_node = root;

    for seg in &segments {
        // Detect numeric index (array access)
        let maybe_index: Option<usize> = seg.parse().ok();

        let mut found = false;
        let mut cursor = current_node.walk();
        let children: Vec<Node> = current_node.children(&mut cursor).collect();

        for child in &children {
            // ── JSON ──────────────────────────────────────────────────────
            if ext == "json" {
                if child.kind() == "pair" {
                    if let Some(kn) = child.child_by_field_name("key") {
                        let kt = source[kn.start_byte()..kn.end_byte()].trim_matches('"');
                        if kt == seg.as_str() {
                            current_node = child.child_by_field_name("value").unwrap_or(*child);
                            found = true;
                            break;
                        }
                    }
                }
                // Array index: collect `value` children of an `array` node
                if child.kind() == "array" {
                    if let Some(idx) = maybe_index {
                        let mut arr_cursor = child.walk();
                        let items: Vec<Node> = child
                            .children(&mut arr_cursor)
                            .filter(|n| n.kind() != "," && n.kind() != "[" && n.kind() != "]")
                            .collect();
                        if let Some(&item) = items.get(idx) {
                            current_node = item;
                            found = true;
                            break;
                        }
                    }
                }
            // ── YAML ──────────────────────────────────────────────────────
            } else if ext == "yaml" || ext == "yml" {
                if child.kind() == "block_mapping_pair" || child.kind() == "flow_pair" {
                    if let Some(kn) = child.child_by_field_name("key") {
                        let kt =
                            source[kn.start_byte()..kn.end_byte()].trim_matches(&['"', '\''][..]);
                        if kt == seg.as_str() {
                            current_node = child.child_by_field_name("value").unwrap_or(*child);
                            found = true;
                            break;
                        }
                    }
                }
                // Sequence index
                if child.kind() == "block_sequence" || child.kind() == "flow_sequence" {
                    if let Some(idx) = maybe_index {
                        let mut seq_cursor = child.walk();
                        let items: Vec<Node> = child
                            .children(&mut seq_cursor)
                            .filter(|n| {
                                n.kind() == "block_sequence_item"
                                    || n.kind() == "flow_node"
                                    || n.kind() == "block_mapping"
                            })
                            .collect();
                        if let Some(&item) = items.get(idx) {
                            current_node = item;
                            found = true;
                            break;
                        }
                    }
                }
            // ── TOML ──────────────────────────────────────────────────────
            } else if ext == "toml" {
                if child.kind() == "pair" {
                    if let Some(kn) = child.child(0) {
                        let kt =
                            source[kn.start_byte()..kn.end_byte()].trim_matches(&['"', '\''][..]);
                        if kt == seg.as_str() {
                            current_node = child.child(2).unwrap_or(*child);
                            found = true;
                            break;
                        }
                    }
                }
            }
        }

        if !found {
            // Try deep search as fallback
            if let Some(n) = search_segment_deep(current_node, source, ext, seg.as_str()) {
                current_node = n;
                found = true;
            }
        }

        if !found {
            anyhow::bail!("Path segment '{}' not found in '{}'", seg, target);
        }
    }

    Ok((current_node.start_byte(), current_node.end_byte()))
}

fn search_segment_deep<'a>(node: Node<'a>, source: &str, ext: &str, seg: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if ext == "json" {
            if child.kind() == "pair" {
                if let Some(kn) = child.child_by_field_name("key") {
                    let key_text = &source[kn.start_byte()..kn.end_byte()];
                    let kt = key_text.trim_matches('"');
                    if kt == seg {
                        return child.child_by_field_name("value").or(Some(child));
                    }
                }
            }
        } else if ext == "yaml" || ext == "yml" {
            if child.kind() == "block_mapping_pair" || child.kind() == "flow_pair" {
                if let Some(kn) = child.child_by_field_name("key") {
                    let key_text = &source[kn.start_byte()..kn.end_byte()];
                    let kt = key_text.trim_matches(&['"', '\''][..]);
                    if kt == seg {
                        return child.child_by_field_name("value").or(Some(child));
                    }
                }
            }
        } else if ext == "toml" {
            if child.kind() == "pair" {
                if let Some(kn) = child.child(0) {
                    let key_text = &source[kn.start_byte()..kn.end_byte()];
                    let kt = key_text.trim_matches(&['"', '\''][..]);
                    if kt == seg {
                        return child.child(2).or(Some(child));
                    }
                }
            }
        }
        if let Some(deeper) = search_segment_deep(child, source, ext, seg) {
            return Some(deeper);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::coerce_json_value;

    #[test]
    fn plain_string_gets_quoted() {
        assert_eq!(coerce_json_value("Hacked by Cortex"), r#""Hacked by Cortex""#);
    }

    #[test]
    fn string_with_quotes_is_escaped() {
        assert_eq!(coerce_json_value(r#"say "hello""#), r#""say \"hello\"""#);
    }

    #[test]
    fn json_literals_pass_through() {
        assert_eq!(coerce_json_value("true"), "true");
        assert_eq!(coerce_json_value("false"), "false");
        assert_eq!(coerce_json_value("null"), "null");
    }

    #[test]
    fn numbers_pass_through() {
        assert_eq!(coerce_json_value("42"), "42");
        assert_eq!(coerce_json_value("3.14"), "3.14");
        assert_eq!(coerce_json_value("-7"), "-7");
    }

    #[test]
    fn already_quoted_string_passes_through() {
        assert_eq!(coerce_json_value(r#""already quoted""#), r#""already quoted""#);
    }

    #[test]
    fn json_object_passes_through() {
        assert_eq!(coerce_json_value(r#"{"key": 1}"#), r#"{"key": 1}"#);
    }

    #[test]
    fn json_array_passes_through() {
        assert_eq!(coerce_json_value("[1, 2, 3]"), "[1, 2, 3]");
    }

    // ── upsert integration tests ──────────────────────────────────────────────

    fn write_temp_json(content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cortex_dg_test_{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn upsert_adds_key_to_existing_object() {
        let path = write_temp_json(
            "{\n  \"scripts\": {\n    \"start\": \"node index.js\"\n  }\n}",
        );
        let edits = vec![super::DataEdit {
            target: "$.scripts.test".to_string(),
            action: "set".to_string(),
            value: Some("jest".to_string()),
        }];
        let result = super::apply_data_edits(&path, edits).unwrap();
        assert!(
            result.contains("\"test\":\"jest\""),
            "should contain new key: {}",
            result
        );
        assert!(
            result.contains("\"start\": \"node index.js\""),
            "existing key must remain: {}",
            result
        );
    }

    #[test]
    fn upsert_adds_key_to_empty_object() {
        let path = write_temp_json(r#"{"scripts":{}}"#);
        let edits = vec![super::DataEdit {
            target: "$.scripts.test".to_string(),
            action: "set".to_string(),
            value: Some("jest".to_string()),
        }];
        let result = super::apply_data_edits(&path, edits).unwrap();
        assert!(
            result.contains("\"test\":\"jest\""),
            "should insert into empty object: {}",
            result
        );
    }
}


