use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ─── Symbol type (local to cortex-act, independent of CortexAST inspector) ───

pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AstEdit {
    /// e.g. "class:Auth" or "function:login" or just the bare identifier "login"
    pub target: String,
    pub action: String, // "replace", "delete"
    pub code: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedAstEdit {
    pub file_path: String,
    pub updated_source: String,
    pub edit_count: usize,
    pub message: String,
}

// ─── Tree-sitter based symbol extraction for core-3 languages ─────────────────

/// Extract named symbols (functions, classes, structs, impls) from source using
/// Tree-sitter for Rust and simple regex for other languages.
pub fn extract_symbols(file_path: &Path, source: &str) -> Vec<Symbol> {
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => extract_via_tree_sitter_rust(source),
        _ => extract_via_regex(source),
    }
}

fn extract_via_tree_sitter_rust(source: &str) -> Vec<Symbol> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_rust::language().into())
        .is_err()
    {
        return Vec::new();
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut symbols = Vec::new();
    let root = tree.root_node();
    collect_rust_symbols(root, source, &mut symbols);
    symbols
}

fn get_name_child<'a>(node: tree_sitter::Node<'a>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "type_identifier" {
            return source.get(child.byte_range()).map(|s| s.to_string());
        }
    }
    None
}

fn collect_rust_symbols(node: tree_sitter::Node, source: &str, out: &mut Vec<Symbol>) {
    let kind = match node.kind() {
        "function_item" => Some("function"),
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "impl_item" => Some("impl"),
        "trait_item" => Some("trait"),
        "mod_item" => Some("mod"),
        _ => None,
    };
    if let Some(k) = kind {
        if let Some(name) = get_name_child(node, source) {
            out.push(Symbol {
                name,
                kind: k.to_string(),
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_symbols(child, source, out);
    }
}

fn extract_via_regex(source: &str) -> Vec<Symbol> {
    // Language-aware heuristic symbol extractor for non-Rust files.
    // Uses full-match start (not capture-group start) and proper block-end detection.
    let patterns: &[(&str, &str)] = &[
        // Rust / general
        (r"(?m)^(?:pub\s+)?(?:async\s+)?fn\s+(\w+)", "function"),
        (r"(?m)^(?:pub\s+)?struct\s+(\w+)", "struct"),
        (r"(?m)^(?:pub\s+)?enum\s+(\w+)", "enum"),
        // TS / JS  (export / async variants)
        (
            r"(?m)^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+(\w+)",
            "function",
        ),
        (r"(?m)^(?:export\s+)?(?:default\s+)?class\s+(\w+)", "class"),
        (r"(?m)^(?:export\s+)?interface\s+(\w+)", "interface"),
        // Python
        (r"(?m)^def\s+(\w+)", "function"),
        (r"(?m)^class\s+(\w+)", "class"),
        // Go
        (r"(?m)^func\s+(?:\([^)]+\)\s+)?(\w+)", "function"),
        (r"(?m)^type\s+(\w+)\s+struct", "struct"),
        // PHP  (method / class with visibility modifiers)
        (
            r"(?m)^\s+(?:public\s+|private\s+|protected\s+)?(?:static\s+)?function\s+(\w+)",
            "function",
        ),
        (r"(?m)^(?:abstract\s+|final\s+)?class\s+(\w+)", "class"),
        // C# / Java / C++ — return-type + name + `(`
        (
            r"(?m)^\s+(?:public\s+|private\s+|protected\s+|internal\s+)?(?:static\s+|async\s+|virtual\s+|override\s+)?(?:[\w<>\[\],?]+\s+)(\w+)\s*\(",
            "function",
        ),
    ];

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut symbols = Vec::new();

    for (pat, kind) in patterns {
        let re = match regex::Regex::new(pat) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for cap in re.captures_iter(source) {
            let name_cap = match cap.get(1) {
                Some(m) => m,
                None => continue,
            };
            let full_match = match cap.get(0) {
                Some(m) => m,
                None => continue,
            };

            let name = name_cap.as_str().to_string();
            // Skip reserved / common noise words that regex may catch
            if matches!(
                name.as_str(),
                "if" | "for" | "while" | "return" | "new" | "this" | "var" | "let" | "const"
            ) {
                continue;
            }
            // Deduplicate: first pattern wins
            if seen.contains(&name) {
                continue;
            }
            seen.insert(name.clone());

            // `decl_start` = beginning of the whole declaration, including any
            // immediately-preceding decorator lines (e.g. Python `@app.get("/")`).
            // backtrack_decorators() walks backward through `@`-prefixed lines so
            // that we replace the full decorated symbol, not just the `def` part.
            // Without this, a replacement that includes its own decorator would
            // leave the old decorator in place producing a duplicate.
            let decl_start = backtrack_decorators(source, full_match.start());

            // Find the real end of the block:
            // · Brace-delimited langs: count { } until depth returns to 0
            // · Python: stop when indent returns to same / shallower level
            let after_decl = &source[decl_start..];
            let block_end = find_block_end(after_decl);
            let end_byte = decl_start + block_end;

            symbols.push(Symbol {
                name,
                kind: kind.to_string(),
                start_byte: decl_start,
                end_byte,
            });
        }
    }
    symbols
}

/// Walk backward from `def_start` in `source` to include any immediately
/// preceding decorator lines (lines whose trimmed form starts with `@`,
/// as in Python `@app.get("/")` or TypeScript `@Injectable()`).
///
/// Returns the adjusted start byte; equals `def_start` for symbols that have
/// no preceding decorators.  This prevents the classic duplicate-decorator bug
/// where a regex match starts at `def` but the existing `@line` is above it:
/// replacing `def_start..end` leaves the old decorator behind and the new code
/// prepends another one on top of it.
fn backtrack_decorators(source: &str, def_start: usize) -> usize {
    let prefix = &source[..def_start];
    if prefix.is_empty() {
        return def_start;
    }
    // Pattern anchors are `^def`, `^class`, etc. so `def_start` is always at
    // column 0 — meaning `prefix` ends with '\n' (or is empty above).
    // After split('\n'), the last element is the empty column-0 prefix of the
    // declaration line itself.  We walk from the second-to-last element upward.
    let lines: Vec<&str> = prefix.split('\n').collect();
    let n = lines.len();
    let mut extra_bytes: usize = 0;
    // `i` starts at the line immediately above the declaration
    let mut i = n.saturating_sub(2);
    loop {
        let line = lines[i];
        if line.trim_start().starts_with('@') {
            extra_bytes += line.len() + 1; // +1 accounts for the '\n' that follows
        } else {
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    def_start.saturating_sub(extra_bytes)
}

/// Locate the end of the first top-level block starting at `src`.
/// Works for both brace-delimited ({ }) and indentation-based (Python) sources.
fn find_block_end(src: &str) -> usize {
    // ── brace-delimited ───────────────────────────────────────────────────────
    if let Some(first_brace) = src.find('{') {
        // Only treat as brace-delimited if `{` appears within the first 3 lines.
        let prefix_lines = src[..first_brace].chars().filter(|&c| c == '\n').count();
        if prefix_lines <= 3 {
            let mut depth: i32 = 0;
            let mut in_str: Option<char> = None;
            let mut prev = '\0';
            for (i, ch) in src.char_indices() {
                // Rudimentary string & char literal skip (no escape handling)
                if let Some(delim) = in_str {
                    if ch == delim && prev != '\\' {
                        in_str = None;
                    }
                } else {
                    match ch {
                        '"' | '\'' | '`' => {
                            in_str = Some(ch);
                        }
                        '{' => {
                            depth += 1;
                        }
                        '}' if depth > 0 => {
                            depth -= 1;
                            if depth == 0 {
                                return i + 1; // include closing `}`
                            }
                        }
                        _ => {}
                    }
                }
                prev = ch;
            }
            return src.len(); // unbalanced – return entire rest
        }
    }

    // ── indentation-based (Python / YAML / …) ────────────────────────────────
    let mut lines = src.lines();
    let first_line = match lines.next() {
        Some(l) => l,
        None => return src.len(),
    };
    let base_indent = first_line.len() - first_line.trim_start().len();
    let mut byte_pos = first_line.len() + 1; // +1 for '\n'
    let mut last_content_pos = byte_pos;
    for line in lines {
        let indent = line.len() - line.trim_start().len();
        // A non-empty line with indent <= base_indent signals the end of this block
        if !line.trim().is_empty() && indent <= base_indent {
            break;
        }
        byte_pos += line.len() + 1;
        if !line.trim().is_empty() {
            last_content_pos = byte_pos;
        }
    }
    // Use the position after the last content line (strip spurious trailing blank line)
    last_content_pos.min(src.len())
}

// ─── Core AST Editor ──────────────────────────────────────────────────────────

pub fn apply_ast_edits(
    file_path: &Path,
    edits: Vec<AstEdit>,
) -> Result<String> {
    // 0. Permission Guard
    check_write_permission(file_path)?;

    let plan = plan_ast_edits(file_path, None, edits)?;

    // 4. Commit to disk
    std::fs::write(file_path, &plan.updated_source).context("Failed to write to file")?;
    Ok(plan.updated_source)
}

pub fn plan_ast_edits(
    file_path: &Path,
    source_override: Option<&str>,
    edits: Vec<AstEdit>,
) -> Result<PlannedAstEdit> {
    let current_source = match source_override {
        Some(source) => source.to_string(),
        None => {
            let source_bytes = std::fs::read(file_path)
                .context("Failed to read original source")?;
            String::from_utf8_lossy(&source_bytes).into_owned()
        }
    };
    let operations = collect_ast_operations(file_path, &current_source, &edits)?;
    let updated_source = render_ast_edits(&current_source, operations);
    validate_ast_output(file_path, &updated_source)?;

    Ok(PlannedAstEdit {
        file_path: file_path.display().to_string(),
        updated_source,
        edit_count: edits.len(),
        message: format!(
            "Planned {} AST edit(s) for {}",
            edits.len(),
            file_path.display()
        ),
    })
}

fn collect_ast_operations(
    file_path: &Path,
    current_source: &str,
    edits: &[AstEdit],
) -> Result<Vec<(usize, usize, AstEdit)>> {
    // 1. Gather targeted byte ranges with symbol extractor
    let mut operations = Vec::new();
    let symbols = extract_symbols(file_path, current_source);

    for edit in edits {
        let sym = symbols.iter().find(|s| {
            let full_name = format!("{}:{}", s.kind, s.name);
            edit.target == full_name || edit.target == s.name
        });

        if let Some(s) = sym {
            operations.push((s.start_byte, s.end_byte, edit.clone()));
        } else {
            anyhow::bail!(
                "AST target not found in source: '{}'. Use `map_overview` first to discover symbol names.",
                edit.target
            );
        }
    }

    // Sort descending (Bottom-Up) to preserve byte offsets
    operations.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(operations)
}

fn render_ast_edits(current_source: &str, operations: Vec<(usize, usize, AstEdit)>) -> String {
    let mut rendered = current_source.to_string();

    // 2. Apply edits in-memory
    for (start, end, edit) in operations {
        let prefix = &rendered[..start];
        let suffix = &rendered[end..];
        let replacement = match edit.action.as_str() {
            "delete" => "",
            _ => edit.code.as_str(),
        };
        // Ensure a newline separates the replacement from what follows
        // (critical for Python indentation-based blocks: use double newline)
        let sep = if !replacement.is_empty() && !replacement.ends_with('\n') {
            "\n\n"
        } else if !replacement.is_empty()
            && !replacement.ends_with("\n\n")
            && !suffix.starts_with('\n')
        {
            "\n"
        } else {
            ""
        };
        rendered = format!("{}{}{}{}", prefix, replacement, sep, suffix);
    }

    rendered
}

fn validate_ast_output(file_path: &Path, current_source: &str) -> Result<()> {
    // 3. Tree-sitter validation for Rust files (fast, no Wasm needed)
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext == "rs" {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::language().into())
            .ok();
        if let Some(tree) = parser.parse(current_source, None) {
            if tree.root_node().has_error() {
                let ts_errors = collect_ts_errors(tree.root_node(), current_source);
                anyhow::bail!(
                    "Edit produced {} syntax error(s): {}. Edit aborted safely.",
                    ts_errors.len(),
                    ts_errors.join("; ")
                );
            }
        }
    }

    Ok(())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn check_write_permission(path: &Path) -> Result<()> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("Cannot stat {:?} — file may not exist", path))?;
    if meta.permissions().readonly() {
        anyhow::bail!("Permission denied: {:?} is read-only.", path);
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("Write permission denied on {:?}.", path))?;
    Ok(())
}

fn collect_ts_errors(node: tree_sitter::Node, source: &str) -> Vec<String> {
    let mut errors = Vec::new();
    collect_ts_errors_inner(node, source, &mut errors);
    errors
}

fn collect_ts_errors_inner(node: tree_sitter::Node, source: &str, out: &mut Vec<String>) {
    if node.is_error() || node.is_missing() {
        let row = node.start_position().row + 1;
        let col = node.start_position().column + 1;
        let snippet: String = source
            .get(node.start_byte()..node.end_byte())
            .unwrap_or("<unknown>")
            .chars()
            .take(40)
            .collect();
        if node.is_missing() {
            out.push(format!("Missing '{}' at line {}:{}", node.kind(), row, col));
        } else {
            out.push(format!(
                "Unexpected '{}' at line {}:{}",
                snippet.trim(),
                row,
                col
            ));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ts_errors_inner(child, source, out);
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_rs(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn bottom_up_sort_preserves_byte_offsets() {
        let source = "AAAA BBBB CCCC";
        let mut ops: Vec<(usize, usize, &str)> = vec![(0, 4, "X"), (5, 9, "Y"), (10, 14, "Z")];
        ops.sort_by(|a, b| b.0.cmp(&a.0));
        let mut buf = source.to_string();
        for (start, end, rep) in ops {
            buf = format!("{}{}{}", &buf[..start], rep, &buf[end..]);
        }
        assert_eq!(buf, "X Y Z");
    }

    #[test]
    fn ts_error_collection_on_broken_rust() {
        use tree_sitter::Parser;
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::language().into())
            .unwrap();
        let broken = "fn broken() { let x = 5;";
        let tree = parser.parse(broken, None).unwrap();
        assert!(tree.root_node().has_error());
        let errors = collect_ts_errors(tree.root_node(), broken);
        assert!(!errors.is_empty());
    }

    #[test]
    fn backtrack_decorators_single() {
        let source = "@app.get(\"/\")\ndef foo():\n    pass\n";
        let def_start = source.find("def foo").unwrap();
        let adjusted = backtrack_decorators(source, def_start);
        assert_eq!(&source[adjusted..def_start + 3], "@app.get(\"/\")\ndef");
    }

    #[test]
    fn backtrack_decorators_multiple() {
        let source = "@decorator1\n@decorator2\ndef bar():\n    pass\n";
        let def_start = source.find("def bar").unwrap();
        let adjusted = backtrack_decorators(source, def_start);
        assert_eq!(&source[adjusted..def_start + 3], "@decorator1\n@decorator2\ndef");
    }

    #[test]
    fn backtrack_decorators_none() {
        let source = "def plain():\n    pass\n";
        let def_start = source.find("def plain").unwrap();
        let adjusted = backtrack_decorators(source, def_start);
        assert_eq!(adjusted, def_start);
    }

    #[test]
    fn backtrack_decorators_stops_at_blank_line() {
        let source = "@other_func_decorator\ndef other():\n    pass\n\n@app.get(\"/\")\ndef foo():\n    pass\n";
        let def_start = source.find("def foo").unwrap();
        let adjusted = backtrack_decorators(source, def_start);
        // Should include @app.get("/") but NOT @other_func_decorator
        assert_eq!(&source[adjusted..def_start + 3], "@app.get(\"/\")\ndef");
    }

    // `PermissionsExt::from_mode` is Unix-only; skip this test on Windows.
    #[cfg(unix)]
    #[test]
    fn permission_guard_catches_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let f = temp_rs("fn main() {}");
        let path = f.path();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(check_write_permission(path).is_err());
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).ok();
    }

    #[test]
    fn permission_guard_passes_for_writable() {
        let f = temp_rs("fn main() {}");
        assert!(check_write_permission(f.path()).is_ok());
    }

    #[test]
    fn plan_ast_edits_uses_override_source_without_writing_file() {
        let f = temp_rs("fn greet() { println!(\"hi\"); }\n");
        let original = std::fs::read_to_string(f.path()).unwrap();

        let plan = plan_ast_edits(
            f.path(),
            Some(&original),
            vec![AstEdit {
                target: "greet".to_string(),
                action: "replace".to_string(),
                code: "fn greet() { println!(\"bye\"); }".to_string(),
            }],
        )
        .unwrap();

        assert!(plan.updated_source.contains("bye"));
        assert_eq!(std::fs::read_to_string(f.path()).unwrap(), original);
    }
}
