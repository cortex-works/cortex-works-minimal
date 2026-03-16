//! # Markup Editor — CortexACT
//!
//! Byte-level, AST-guided surgical editing for Markdown, HTML, and XML
//! using Tree-sitter.  **Never uses `serde` or text-based regex for mutations.**
//!
//! ## Target syntax
//!
//! | Pattern | Meaning |
//! |---------|---------|
//! | `heading:Name` | Markdown section from heading through end of section |
//! | `table:N` | Nth table (0-indexed) |
//! | `code:N` | Nth fenced code block (0-indexed) |
//! | `tag:div` | First HTML/XML element with that tag name |
//! | `id:main` | Element whose `id` attribute equals `main` |
//! | `query:(node_kind)@cap` | Raw Tree-sitter query; uses first capture |

use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

#[derive(Debug)]
pub struct MarkupEdit {
    pub target: String,
    pub action: String,
    pub code: String,
}

pub fn apply_markup_edits(file_path: &Path, edits: Vec<MarkupEdit>) -> Result<String> {
    let source_bytes = std::fs::read(file_path).context("Failed to read source")?;
    let mut current_source = String::from_utf8_lossy(&source_bytes).into_owned();

    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let lang = resolve_language(ext)?;

    let mut operations: Vec<(usize, usize, MarkupEdit)> = Vec::new();
    for edit in edits {
        let (start, end) = match edit.action.as_str() {
            // Zero-width insertion point at the start of the target node.
            "insert_before" => {
                let (node_start, _) = find_range(&current_source, &lang, ext, &edit.target)?;
                (node_start, node_start)
            }
            // For headings: insert after the heading *line* (not after the whole section body).
            // For all other targets: insert after the node end.
            "insert_after" => {
                if edit.target.starts_with("heading:") {
                    let byte =
                        find_md_heading_end_byte(&current_source, &lang, ext, &edit.target)?;
                    (byte, byte)
                } else {
                    let (_, node_end) =
                        find_range(&current_source, &lang, ext, &edit.target)?;
                    (node_end, node_end)
                }
            }
            _ => find_range(&current_source, &lang, ext, &edit.target)?,
        };
        operations.push((start, end, edit));
    }

    // Bottom-up: descending byte order so earlier offsets stay valid
    operations.sort_by(|a, b| b.0.cmp(&a.0));

    for (start, end, edit) in operations {
        let replacement = match edit.action.as_str() {
            "delete" => String::new(),
            "replace" | "insert_before" | "insert_after" => edit.code.clone(),
            other => anyhow::bail!(
                "Unknown action '{}'. Use replace | delete | insert_before | insert_after",
                other
            ),
        };
        let prefix = &current_source[..start];
        let suffix = &current_source[end..];
        current_source = format!("{}{}{}", prefix, replacement, suffix);
    }

    validate_result(&current_source, &lang, ext)?;
    std::fs::write(file_path, &current_source).context("Failed to write file")?;
    Ok(current_source)
}



fn resolve_language(ext: &str) -> Result<Language> {
    match ext {
        "md" | "markdown" => Ok(Language::from(tree_sitter_md::LANGUAGE)),
        "html" | "htm" => Ok(Language::from(tree_sitter_html::LANGUAGE)),
        "xml" | "svg" => Ok(Language::from(tree_sitter_xml::LANGUAGE_XML)),
        other => anyhow::bail!("Unsupported markup type '{}'. Use md|html|xml|svg", other),
    }
}

fn find_range(source: &str, lang: &Language, ext: &str, target: &str) -> Result<(usize, usize)> {
    if let Some(query_str) = target.strip_prefix("query:") {
        return find_range_via_query(source, lang, query_str);
    }

    let (t_type, t_val) = target.split_once(':').ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid target '{}'. Use 'type:value' or 'query:TSQUERY'",
            target
        )
    })?;

    let mut parser = Parser::new();
    parser
        .set_language(lang)
        .context("Failed to set language")?;
    let tree = parser
        .parse(source, None)
        .context("Failed to parse markup source")?;
    let root = tree.root_node();

    let range = match ext {
        "md" | "markdown" => find_md_range(root, source, t_type, t_val),
        "html" | "htm" | "xml" | "svg" => find_xml_range(root, source, t_type, t_val),
        _ => None,
    };

    range.ok_or_else(|| anyhow::anyhow!("Target '{}' not found in file", target))
}

fn find_range_via_query(source: &str, lang: &Language, query_str: &str) -> Result<(usize, usize)> {
    let mut parser = Parser::new();
    parser
        .set_language(lang)
        .context("Failed to set language")?;
    let tree = parser.parse(source, None).context("Failed to parse")?;
    let root = tree.root_node();
    let query = Query::new(lang, query_str)
        .map_err(|e| anyhow::anyhow!("Invalid Tree-sitter query: {:?}", e))?;

    // tree-sitter 0.26: TextProvider requires FnMut(Node) -> IntoIterator<Item: AsRef<[u8]>>
    let src_bytes = source.as_bytes();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, root, |node: Node| {
        std::iter::once(&src_bytes[node.start_byte()..node.end_byte()])
    });

    // StreamingIterator::next returns Option<&QueryMatch> (reference, not owned)
    let (start, end) = if let Some(m) = matches.next() {
        m.captures
            .first()
            .map(|cap| (cap.node.start_byte(), cap.node.end_byte()))
            .ok_or_else(|| anyhow::anyhow!("Query '{}' has no captures", query_str))?
    } else {
        return Err(anyhow::anyhow!("Query '{}' matched nothing", query_str));
    };

    Ok((start, end))
}

// ── Markdown ──────────────────────────────────────────────────────────────

fn find_md_range(
    root: Node<'_>,
    source: &str,
    t_type: &str,
    t_val: &str,
) -> Option<(usize, usize)> {
    match t_type {
        "heading" => find_md_section(root, source, t_val),
        "table" => find_md_by_kind_index(root, "table", t_val.parse().ok()),
        "code" => find_md_by_kind_index(root, "fenced_code_block", t_val.parse().ok()),
        _ => None,
    }
}

/// Returns the byte offset of the end of the heading *line* for `target` (which must have
/// the form `"heading:Name"`).  Used by `insert_after` so content is injected immediately
/// after the heading text rather than after the whole section body.
fn find_md_heading_end_byte(
    source: &str,
    lang: &Language,
    _ext: &str,
    target: &str,
) -> Result<usize> {
    let t_val = target
        .strip_prefix("heading:")
        .ok_or_else(|| anyhow::anyhow!("insert_after: expected 'heading:Name', got '{}'", target))?;
    let mut parser = Parser::new();
    parser
        .set_language(lang)
        .context("Failed to set language")?;
    let tree = parser
        .parse(source, None)
        .context("Failed to parse markup source")?;
    let root = tree.root_node();
    let heading_node = find_md_heading_node(root, source, t_val)
        .ok_or_else(|| anyhow::anyhow!("Heading '{}' not found", t_val))?;
    Ok(heading_node.end_byte())
}

fn find_md_section(root: Node<'_>, source: &str, heading_text: &str) -> Option<(usize, usize)> {
    let heading_node = find_md_heading_node(root, source, heading_text)?;
    let heading_level =
        md_heading_level(&source[heading_node.start_byte()..heading_node.end_byte()]);
    let section_start = heading_node.start_byte();
    let section_end = find_md_section_end(root, source, heading_node, heading_level);
    Some((section_start, section_end))
}



fn find_md_heading_node<'a>(root: Node<'a>, source: &str, heading_text: &str) -> Option<Node<'a>> {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "atx_heading" || node.kind() == "setext_heading" {
            let text = &source[node.start_byte()..node.end_byte()];
            if text.contains(heading_text) {
                return Some(node);
            }
        }
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    None
}

fn md_heading_level(text: &str) -> usize {
    text.trim_start()
        .chars()
        .take_while(|c| *c == '#')
        .count()
        .max(1)
}

fn find_md_section_end(
    root: Node<'_>,
    source: &str,
    heading_node: Node<'_>,
    level: usize,
) -> usize {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut seen = false;

    while let Some(node) = stack.pop() {
        if node.id() == heading_node.id() {
            seen = true;
        } else if seen && (node.kind() == "atx_heading" || node.kind() == "setext_heading") {
            let this_lvl = md_heading_level(&source[node.start_byte()..node.end_byte()]);
            if this_lvl <= level {
                return node.start_byte();
            }
        }
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    source.len()
}

fn find_md_by_kind_index(root: Node<'_>, kind: &str, idx: Option<usize>) -> Option<(usize, usize)> {
    let target_idx = idx?;
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut count = 0usize;

    while let Some(node) = stack.pop() {
        if node.kind() == kind {
            if count == target_idx {
                return Some((node.start_byte(), node.end_byte()));
            }
            count += 1;
        }
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    None
}

// ── HTML / XML ────────────────────────────────────────────────────────────

fn find_xml_range(
    root: Node<'_>,
    source: &str,
    t_type: &str,
    t_val: &str,
) -> Option<(usize, usize)> {
    let mut cursor = root.walk();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        match t_type {
            "tag" => {
                if node.kind() == "element" {
                    if element_tag_name(node, source).as_deref() == Some(t_val) {
                        return Some((node.start_byte(), node.end_byte()));
                    }
                }
            }
            "id" => {
                // HTML uses lowercase "attribute" with field-named "name"/"value" children.
                // XML  uses "Attribute" with unnamed "Name"/"AttValue" children.
                let kind = node.kind();
                if kind == "attribute" || kind == "Attribute" {
                    // Resolve attribute name: try field access (HTML) then named-child (XML).
                    let attr_name = node
                        .child_by_field_name("name")
                        .and_then(|n| source.get(n.start_byte()..n.end_byte()))
                        .map(str::to_string)
                        .or_else(|| {
                            let mut c = node.walk();
                            node.named_children(&mut c)
                                .find(|n| n.kind() == "Name")
                                .and_then(|n| source.get(n.start_byte()..n.end_byte()))
                                .map(str::to_string)
                        });

                    if attr_name.as_deref() == Some("id") {
                        // Resolve attribute value: try field access (HTML) then named-child (XML).
                        let attr_val = node
                            .child_by_field_name("value")
                            .and_then(|n| source.get(n.start_byte()..n.end_byte()))
                            .map(str::to_string)
                            .or_else(|| {
                                let mut c = node.walk();
                                node.named_children(&mut c)
                                    .find(|n| n.kind() == "AttValue")
                                    .and_then(|n| source.get(n.start_byte()..n.end_byte()))
                                    .map(str::to_string)
                            });

                        if let Some(raw) = attr_val {
                            if raw.trim_matches('"').trim_matches('\'') == t_val {
                                return node.parent().map(|p| (p.start_byte(), p.end_byte()));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    None
}

fn element_tag_name(element: Node<'_>, source: &str) -> Option<String> {
    // ── HTML grammar (tree-sitter-html) ──────────────────────────────────
    // element has a named "start_tag" field, which has a "tag_name" field.
    if let Some(start_tag) = element.child_by_field_name("start_tag") {
        let name_node = start_tag
            .child_by_field_name("tag_name")
            .or_else(|| start_tag.child_by_field_name("name"))
            .or_else(|| {
                // Defensive fallback: first named child whose kind looks like a name
                let mut c = start_tag.walk();
                start_tag.named_children(&mut c).find(|n| {
                    matches!(n.kind(), "tag_name" | "identifier" | "Name")
                })
            })?;
        return Some(source[name_node.start_byte()..name_node.end_byte()].to_string());
    }

    // ── XML grammar (tree-sitter-xml 0.7+) ───────────────────────────────
    // element children are STag (start+end) or EmptyElemTag (self-closing).
    // Neither has named field entries; the tag name is the first `Name` child.
    let mut cursor = element.walk();
    for child in element.named_children(&mut cursor) {
        match child.kind() {
            "EmptyElemTag" | "STag" => {
                let mut c2 = child.walk();
                if let Some(name_node) = child.named_children(&mut c2).find(|n| n.kind() == "Name") {
                    return Some(
                        source[name_node.start_byte()..name_node.end_byte()].to_string(),
                    );
                }
            }
            _ => {}
        }
    }
    None
}

// ── Post-edit validation ───────────────────────────────────────────────────

fn validate_result(source: &str, lang: &Language, ext: &str) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(lang)
        .context("Validation: set_language failed")?;
    if let Some(tree) = parser.parse(source, None) {
        if tree.root_node().has_error() {
            match ext {
                "md" | "markdown" => {
                    eprintln!(
                        "[cortex-act] markup_editor: post-edit tree has minor errors \
                         (markdown is lenient — file written)"
                    );
                }
                _ => anyhow::bail!(
                    "Post-edit AST validation failed: result contains syntax errors. \
                     Disk write aborted."
                ),
            }
        }
    }
    Ok(())
}
