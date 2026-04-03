use anyhow::{Result, anyhow};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

use crate::inspector::Symbol;

#[derive(Debug, Clone, Copy, Default)]
pub struct Z4FileSummary {
    pub labels: usize,
    pub hex_labels: usize,
    pub hex_literals: usize,
    pub doc_hex_rows: usize,
    pub bytes: usize,
    pub lines: usize,
    pub label_density_per_kib: usize,
    pub hex_density_per_kib: usize,
}

#[derive(Debug, Clone)]
struct Z4LabelLine {
    name: String,
    line: u32,
    start_byte: usize,
    signature: String,
    hex_literal_count: usize,
    is_hex_label: bool,
}

pub struct Z4LanguageDriver;

fn contains_todo_fixme(s: &str) -> bool {
    let up = s.to_ascii_uppercase();
    up.contains("TODO") || up.contains("FIXME")
}

fn def_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            Regex::new(r"^\s*(function|class|def|func|struct|interface|enum)\s+([a-zA-Z0-9_]+)").unwrap(),
            Regex::new(r"^\s*(?:public|private|protected)?\s*(?:static\s*)?(?:fn|var|val)\s+([a-zA-Z0-9_]+)").unwrap(),
            Regex::new(r"^\s*(?:public|private|protected)?\s*(?:static\s*)?func\s+([a-zA-Z0-9_]+)").unwrap(),
        ]
    })
}

fn is_definition_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }

    if contains_todo_fixme(t) {
        return true;
    }

    if !(t.starts_with("function")
        || t.starts_with("class")
        || t.starts_with("def")
        || t.starts_with("func")
        || t.starts_with("struct")
        || t.starts_with("interface")
        || t.starts_with("enum")
        || t.starts_with("public")
        || t.starts_with("private")
        || t.starts_with("protected")
        || t.starts_with("static")
        || t.starts_with("fn")
        || t.starts_with("var")
        || t.starts_with("val"))
    {
        return false;
    }

    def_regexes().iter().any(|re| re.is_match(line))
}

fn z4_label_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^@([A-Za-z0-9_]+):").unwrap())
}

fn z4_hex_label_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^@([fF][0-9A-Fa-f]{3,}|[0-9A-Fa-f]{4,}):").unwrap())
}

fn z4_hex_literal_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"0x[0-9A-Fa-f]+").unwrap())
}

impl Z4LanguageDriver {
    pub fn handles_path(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("z4"))
            .unwrap_or(false)
    }

    fn scan_labels(source_text: &str) -> Vec<Z4LabelLine> {
        let mut labels = Vec::new();
        let mut byte_idx = 0usize;

        for (line_idx, chunk) in source_text.split_inclusive('\n').enumerate() {
            let line = chunk.trim_end_matches(['\r', '\n']);
            if let Some(caps) = z4_label_regex().captures(line) {
                let name = caps
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                labels.push(Z4LabelLine {
                    name,
                    line: line_idx as u32,
                    start_byte: byte_idx,
                    signature: line.trim().to_string(),
                    hex_literal_count: z4_hex_literal_regex().find_iter(line).count(),
                    is_hex_label: z4_hex_label_regex().is_match(line),
                });
            }
            byte_idx += chunk.len();
        }

        labels
    }

    pub fn extract_symbols(source_text: &str) -> Vec<Symbol> {
        let labels = Self::scan_labels(source_text);
        if labels.is_empty() {
            return vec![];
        }

        let last_line = source_text.lines().count().saturating_sub(1) as u32;
        labels
            .iter()
            .enumerate()
            .map(|(idx, label)| {
                let end_byte = labels
                    .get(idx + 1)
                    .map(|next| next.start_byte)
                    .unwrap_or(source_text.len());
                let line_end = labels
                    .get(idx + 1)
                    .map(|next| next.line.saturating_sub(1))
                    .unwrap_or(last_line);

                Symbol {
                    name: label.name.clone(),
                    kind: if label.is_hex_label {
                        "hex_label".to_string()
                    } else {
                        "label".to_string()
                    },
                    line: label.line,
                    line_end,
                    start_byte: label.start_byte,
                    end_byte,
                    signature: Some(label.signature.clone()),
                }
            })
            .collect()
    }

    pub fn summarize_file(source_text: &str) -> Z4FileSummary {
        let labels = Self::scan_labels(source_text);
        let hex_labels = labels.iter().filter(|label| label.is_hex_label).count();
        let bytes = source_text.len();
        let lines = source_text.lines().count();
        let hex_literals = z4_hex_literal_regex().find_iter(source_text).count();
        let doc_hex_rows = source_text
            .lines()
            .filter(|line| line.contains("DATA DOC:\"0x"))
            .count();

        let label_density_per_kib = if bytes == 0 {
            0
        } else {
            labels.len().saturating_mul(1024) / bytes
        };
        let hex_density_per_kib = if bytes == 0 {
            0
        } else {
            hex_literals.saturating_mul(1024) / bytes
        };

        Z4FileSummary {
            labels: labels.len(),
            hex_labels,
            hex_literals,
            doc_hex_rows,
            bytes,
            lines,
            label_density_per_kib,
            hex_density_per_kib,
        }
    }

    pub fn render_density_map(source_text: &str) -> String {
        let summary = Self::summarize_file(source_text);
        let labels = Self::scan_labels(source_text);
        let bucket_count = if source_text.is_empty() {
            1usize
        } else {
            source_text.len().div_ceil(1024).clamp(1, 8)
        };
        let bucket_size = source_text.len().max(1).div_ceil(bucket_count);

        let mut label_buckets = vec![0usize; bucket_count];
        let mut hex_label_buckets = vec![0usize; bucket_count];
        let mut hex_literal_buckets = vec![0usize; bucket_count];

        for label in labels {
            let bucket_idx = (label.start_byte / bucket_size).min(bucket_count - 1);
            label_buckets[bucket_idx] += 1;
            hex_literal_buckets[bucket_idx] += label.hex_literal_count;
            if label.is_hex_label {
                hex_label_buckets[bucket_idx] += 1;
            }
        }

        let mut out = String::from("# Z4_HEX_LABEL_DENSITY\n");
        out.push_str(&format!(
            "labels=0x{:x} hex_labels=0x{:x} hex_literals=0x{:x} doc_hex_rows=0x{:x} bytes=0x{:x} lines=0x{:x} label_density_per_kib=0x{:x} hex_density_per_kib=0x{:x}\n",
            summary.labels,
            summary.hex_labels,
            summary.hex_literals,
            summary.doc_hex_rows,
            summary.bytes,
            summary.lines,
            summary.label_density_per_kib,
            summary.hex_density_per_kib,
        ));

        for bucket_idx in 0..bucket_count {
            let start = bucket_idx.saturating_mul(bucket_size);
            let end = ((bucket_idx + 1).saturating_mul(bucket_size)).min(source_text.len());
            out.push_str(&format!(
                "0x{start:04x}..0x{end:04x} labels=0x{:x} hex_labels=0x{:x} hex_literals=0x{:x}\n",
                label_buckets[bucket_idx],
                hex_label_buckets[bucket_idx],
                hex_literal_buckets[bucket_idx],
            ));
        }

        out
    }

    pub fn read_symbol(
        path: &Path,
        source_text: &str,
        symbol_name: &str,
        skeleton_only: bool,
        instance_index: Option<usize>,
    ) -> Result<String> {
        const MAX_SYMBOL_LINES: usize = 500;

        let symbols = Self::extract_symbols(source_text);
        let mut all_matches: Vec<&Symbol> = symbols
            .iter()
            .filter(|symbol| symbol.name == symbol_name)
            .collect();
        if all_matches.is_empty() {
            all_matches = symbols
                .iter()
                .filter(|symbol| symbol.name.eq_ignore_ascii_case(symbol_name))
                .collect();
        }

        if all_matches.is_empty() {
            let mut available: Vec<String> = symbols
                .iter()
                .map(|symbol| format!("  {} {}", symbol.kind, symbol.name))
                .collect();
            available.sort();
            available.truncate(30);
            return Err(anyhow!(
                "Symbol `{}` not found in {}.\nAvailable symbols (showing {}):\n{}",
                symbol_name,
                path.display(),
                available.len(),
                available.join("\n")
            ));
        }

        let idx = instance_index
            .unwrap_or(0)
            .min(all_matches.len().saturating_sub(1));
        let symbol = all_matches[idx];
        let disambiguation = if all_matches.len() > 1 {
            format!(
                "// ⚠️ Disambiguation: Found {} instances of `{}` in this file. Showing instance {} of {} (1-based). Use `instance_index` param (0-based, 0..{}) to select a specific one.\n",
                all_matches.len(),
                symbol.name,
                idx + 1,
                all_matches.len(),
                all_matches.len() - 1,
            )
        } else {
            String::new()
        };

        let start_line = symbol.line as usize + 1;
        let end_line = symbol.line_end as usize + 1;
        let header = format!(
            "{disambiguation}// {} `{}` — {}:L{}-L{}\n",
            symbol.kind,
            symbol.name,
            path.display(),
            start_line,
            end_line,
        );

        let raw_body = &source_text[symbol.start_byte..symbol.end_byte];
        let rendered_body = if skeleton_only {
            symbol
                .signature
                .as_deref()
                .map(|signature| format!("{}\n", signature))
                .unwrap_or_else(|| format!("@{}:\n", symbol.name))
        } else {
            raw_body.to_string()
        };

        let symbol_lines = end_line.saturating_sub(start_line) + 1;
        if symbol_lines > MAX_SYMBOL_LINES && !skeleton_only {
            let truncated = rendered_body
                .lines()
                .take(MAX_SYMBOL_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(format!(
                "{header}{truncated}\n\n> ⚠️ **Symbol truncated** — `{}` is {} lines (limit: {}, stopped at L{}).",
                symbol.name,
                symbol_lines,
                MAX_SYMBOL_LINES,
                start_line + MAX_SYMBOL_LINES - 1,
            ));
        }

        Ok(format!("{header}{rendered_body}"))
    }
}

/// Regex-based skeleton extraction for unsupported languages.
///
/// Output is line-based: definition-ish lines are kept, gaps are collapsed to a single `...` line.
pub fn render_universal_skeleton(source_text: &str) -> String {
    let minified = source_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(5)
        .any(|l| l.len() > 2_000);
    if minified {
        return "/* MINIFIED_OR_GENERATED — skipped */\n".to_string();
    }
    let max_kept_lines: usize = 600;

    let mut out = String::new();
    let mut last_kept_line: Option<usize> = None;
    let mut kept: usize = 0;

    for (idx, line) in source_text.lines().enumerate() {
        if kept >= max_kept_lines {
            out.push_str("...\n");
            break;
        }

        if !is_definition_line(line) {
            continue;
        }

        if let Some(prev) = last_kept_line {
            if idx > prev + 1 {
                out.push_str("...\n");
            }
        }

        out.push_str(line.trim());
        out.push('\n');
        last_kept_line = Some(idx);
        kept += 1;
    }

    if out.trim().is_empty() {
        let head: String = source_text
            .lines()
            .take(50)
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        return format!("/* TRUNCATED */\n{}\n", head);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z4_extracts_hex_labels_and_ranges() {
        let source = "@f11c: PUSH IA:%rbp\n@alpha: MOVE IA:%rax IB:0x10\n@beta: DATA DOC:\"0xA01\\0\"\n";
        let symbols = Z4LanguageDriver::extract_symbols(source);

        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "f11c");
        assert_eq!(symbols[0].kind, "hex_label");
        assert!(symbols[1].end_byte > symbols[1].start_byte);
    }

    #[test]
    fn z4_density_map_tracks_hex_content() {
        let source = "@f11c: PUSH IA:%rbp\n@alpha: MOVE IA:%rax IB:0x10\n@beta: DATA DOC:\"0xA01\\0\"\n";
        let rendered = Z4LanguageDriver::render_density_map(source);

        assert!(rendered.contains("# Z4_HEX_LABEL_DENSITY"));
        assert!(rendered.contains("hex_labels=0x1"));
        assert!(rendered.contains("doc_hex_rows=0x1"));
    }

    #[test]
    fn z4_read_symbol_respects_label_boundaries() {
        let source = "@alpha: MOVE IA:%rax IB:0x10\n@beta: RET\n";
        let rendered = Z4LanguageDriver::read_symbol(
            Path::new("sample.z4"),
            source,
            "alpha",
            false,
            None,
        )
        .expect("z4 symbol read");

        assert!(rendered.contains("@alpha: MOVE IA:%rax IB:0x10"));
        assert!(!rendered.contains("@beta: RET"));
    }
}
