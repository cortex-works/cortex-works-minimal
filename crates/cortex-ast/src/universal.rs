use anyhow::{Result, anyhow};
use regex::Regex;
use std::collections::BTreeSet;
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

#[derive(Debug, Clone)]
struct Z4BootLine {
    name: String,
    env: String,
    file: String,
    line: u32,
    start_byte: usize,
    end_byte: usize,
    signature: String,
}

#[derive(Debug, Clone)]
struct Z4BindingBlock {
    name: String,
    id: Option<String>,
    abi: Option<String>,
    mode: Option<String>,
    line: u32,
    line_end: u32,
    start_byte: usize,
    end_byte: usize,
    signature: String,
}

#[derive(Debug, Clone)]
pub struct Z4UsageHit {
    pub category: &'static str,
    pub line_0: u32,
}

#[derive(Debug, Clone)]
pub struct Z4CallEdge {
    pub category: &'static str,
    pub target: String,
    pub line_0: u32,
}

#[derive(Debug, Clone)]
pub struct Z4CatalogSummary {
    pub kind: &'static str,
    pub entries: Vec<String>,
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

fn z4_boot_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*boot\s+([A-Za-z0-9_]+)\s+([A-Za-z0-9_]+)\s+([^\s]+)\s*$").unwrap())
}

fn z4_key_value_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+?)\s*$").unwrap())
}

fn z4_call_os_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bCALL\b[^\n]*\bOS:@([A-Za-z0-9_]+)").unwrap())
}

fn z4_branch_os_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bBRANCH\b[^\n]*\bOS:@([A-Za-z0-9_]+)").unwrap())
}

fn z4_branchz_os_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bBRANCHZ\b[^\n]*\bOS:@([A-Za-z0-9_]+)").unwrap())
}

fn z4_branchz_oe_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bBRANCHZ\b[^\n]*\bOE:@([A-Za-z0-9_]+)").unwrap())
}

fn trim_wrapped_quotes(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

fn token_regex(token: &str) -> Regex {
    Regex::new(&format!(
        r"(?:^|[^A-Za-z0-9_]){}(?:$|[^A-Za-z0-9_])",
        regex::escape(token)
    ))
    .unwrap()
}

static Z4_BUILTIN_REGISTERS: &[&str] = &[
    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9", "r10", "r11",
    "r12", "r13", "r14", "r15", "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6",
    "xmm7", "xmm8", "xmm9", "xmm10", "xmm11", "xmm12", "xmm13", "xmm14", "xmm15", "ymm0",
    "ymm1", "ymm2", "ymm3", "ymm4", "ymm5", "ymm6", "ymm7", "ymm8", "ymm9", "ymm10",
    "ymm11", "ymm12", "ymm13", "ymm14", "ymm15",
];

impl Z4LanguageDriver {
    pub fn handles_path(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("z4"))
            .unwrap_or(false)
    }

    pub fn is_catalog_path(path: &Path) -> bool {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        extension == "filelist" || file_name == "z4.reg" || file_name.ends_with(".project.z4")
    }

    pub fn is_analysis_candidate(path: &Path) -> bool {
        Self::handles_path(path) || Self::is_catalog_path(path)
    }

    pub fn is_builtin_register(symbol_name: &str) -> bool {
        let normalized = symbol_name.trim().trim_start_matches('%').to_ascii_lowercase();
        Z4_BUILTIN_REGISTERS.contains(&normalized.as_str())
    }

    pub fn looks_like_assembly(source_text: &str) -> bool {
        !Self::scan_labels(source_text).is_empty()
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

    fn scan_boot_lines(source_text: &str) -> Vec<Z4BootLine> {
        let mut boots = Vec::new();
        let mut byte_idx = 0usize;

        for (line_idx, chunk) in source_text.split_inclusive('\n').enumerate() {
            let line = chunk.trim_end_matches(['\r', '\n']);
            if let Some(caps) = z4_boot_regex().captures(line) {
                boots.push(Z4BootLine {
                    name: caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default(),
                    env: caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
                    file: caps.get(3).map(|m| m.as_str().to_string()).unwrap_or_default(),
                    line: line_idx as u32,
                    start_byte: byte_idx,
                    end_byte: byte_idx + chunk.len(),
                    signature: line.trim().to_string(),
                });
            }
            byte_idx += chunk.len();
        }

        boots
    }

    fn scan_binding_blocks(source_text: &str) -> Vec<Z4BindingBlock> {
        let mut blocks = Vec::new();
        let mut current: Option<Z4BindingBlock> = None;
        let mut byte_idx = 0usize;
        let total_lines = source_text.lines().count().saturating_sub(1) as u32;

        for (line_idx, chunk) in source_text.split_inclusive('\n').enumerate() {
            let line = chunk.trim_end_matches(['\r', '\n']);
            let trimmed = line.trim();

            if trimmed == "[[binding]]" {
                if let Some(mut block) = current.take() {
                    block.line_end = line_idx.saturating_sub(1) as u32;
                    block.end_byte = byte_idx;
                    block.signature = format!(
                        "[[binding]] id={} symbol={} abi={} mode={}",
                        block.id.as_deref().unwrap_or("?"),
                        block.name,
                        block.abi.as_deref().unwrap_or("?"),
                        block.mode.as_deref().unwrap_or("?"),
                    );
                    blocks.push(block);
                }

                current = Some(Z4BindingBlock {
                    name: String::new(),
                    id: None,
                    abi: None,
                    mode: None,
                    line: line_idx as u32,
                    line_end: line_idx as u32,
                    start_byte: byte_idx,
                    end_byte: source_text.len(),
                    signature: String::from("[[binding]]"),
                });
            } else if let Some(block) = current.as_mut() {
                if let Some(caps) = z4_key_value_regex().captures(trimmed) {
                    let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    match key {
                        "id" => block.id = Some(trim_wrapped_quotes(value)),
                        "symbol" => block.name = trim_wrapped_quotes(value),
                        "abi" => block.abi = Some(trim_wrapped_quotes(value)),
                        "mode" => block.mode = Some(trim_wrapped_quotes(value)),
                        _ => {}
                    }
                }
            }

            byte_idx += chunk.len();
        }

        if let Some(mut block) = current.take() {
            block.line_end = total_lines;
            block.end_byte = source_text.len();
            block.signature = format!(
                "[[binding]] id={} symbol={} abi={} mode={}",
                block.id.as_deref().unwrap_or("?"),
                block.name,
                block.abi.as_deref().unwrap_or("?"),
                block.mode.as_deref().unwrap_or("?"),
            );
            blocks.push(block);
        }

        blocks.into_iter().filter(|block| !block.name.is_empty()).collect()
    }

    fn labels_to_symbols(labels: &[Z4LabelLine], source_text: &str) -> Vec<Symbol> {
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

    pub fn extract_symbols(source_text: &str) -> Vec<Symbol> {
        let labels = Self::scan_labels(source_text);
        if !labels.is_empty() {
            return Self::labels_to_symbols(&labels, source_text);
        }

        let boots = Self::scan_boot_lines(source_text);
        if !boots.is_empty() {
            return boots
                .into_iter()
                .map(|boot| Symbol {
                    name: boot.name,
                    kind: "boot".to_string(),
                    line: boot.line,
                    line_end: boot.line,
                    start_byte: boot.start_byte,
                    end_byte: boot.end_byte,
                    signature: Some(boot.signature),
                })
                .collect();
        }

        let bindings = Self::scan_binding_blocks(source_text);
        if !bindings.is_empty() {
            return bindings
                .into_iter()
                .map(|binding| Symbol {
                    name: binding.name,
                    kind: "binding".to_string(),
                    line: binding.line,
                    line_end: binding.line_end,
                    start_byte: binding.start_byte,
                    end_byte: binding.end_byte,
                    signature: Some(binding.signature),
                })
                .collect();
        }

        vec![]
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
        let boots = Self::scan_boot_lines(source_text);
        if !boots.is_empty() {
            let mut out = String::from("# Z4_BOOT_MAP\n");
            for boot in boots {
                out.push_str(&format!(
                    "boot name={} env={} file={}\n",
                    boot.name, boot.env, boot.file
                ));
            }
            return out;
        }

        let bindings = Self::scan_binding_blocks(source_text);
        if !bindings.is_empty() {
            let mut out = String::from("# Z4_BINDING_MAP\n");
            for binding in bindings {
                out.push_str(&format!(
                    "id={} symbol={} abi={} mode={}\n",
                    binding.id.as_deref().unwrap_or("?"),
                    binding.name,
                    binding.abi.as_deref().unwrap_or("?"),
                    binding.mode.as_deref().unwrap_or("?"),
                ));
            }
            return out;
        }

        let summary = Self::summarize_file(source_text);
        let labels = Self::scan_labels(source_text);
        if labels.is_empty() {
            let head = source_text
                .lines()
                .take(50)
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("\n");
            return format!("/* TRUNCATED */\n{}\n", head);
        }
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

    pub fn find_usages(source_text: &str, symbol_name: &str) -> Vec<Z4UsageHit> {
        let query = symbol_name.trim();
        if query.is_empty() {
            return vec![];
        }

        if Self::is_builtin_register(query) {
            return vec![];
        }

        if query.starts_with('%') {
            return Self::find_variable_usages(source_text, query);
        }

        let label_query = query.trim_start_matches('@');
        let hits = Self::find_label_usages(source_text, label_query);
        if !hits.is_empty() {
            return hits;
        }

        Self::find_variable_usages(source_text, &format!("%{query}"))
    }

    fn find_label_usages(source_text: &str, symbol_name: &str) -> Vec<Z4UsageHit> {
        let definition = format!("@{symbol_name}:");
        let reference = format!("@{symbol_name}");
        let reference_re = token_regex(&reference);
        let call_ref = format!("OS:{reference}");
        let branch_success_ref = format!("OS:{reference}");
        let branch_failure_ref = format!("OE:{reference}");

        source_text
            .lines()
            .enumerate()
            .filter_map(|(line_idx, line)| {
                let trimmed = line.trim_start();
                if trimmed.starts_with(&definition) {
                    return Some(Z4UsageHit {
                        category: "Definitions",
                        line_0: line_idx as u32,
                    });
                }
                if !reference_re.is_match(line) {
                    return None;
                }

                let category = if trimmed.contains("CALL") && trimmed.contains(&call_ref) {
                    "Calls"
                } else if trimmed.contains("BRANCHZ") && trimmed.contains(&branch_success_ref) {
                    "Branch Success"
                } else if trimmed.contains("BRANCHZ") && trimmed.contains(&branch_failure_ref) {
                    "Branch Failure"
                } else if trimmed.contains("BRANCH") && trimmed.contains(&branch_success_ref) {
                    "Branches"
                } else {
                    "Other"
                };

                Some(Z4UsageHit {
                    category,
                    line_0: line_idx as u32,
                })
            })
            .collect()
    }

    fn find_variable_usages(source_text: &str, variable_name: &str) -> Vec<Z4UsageHit> {
        let variable_re = token_regex(variable_name);

        source_text
            .lines()
            .enumerate()
            .filter_map(|(line_idx, line)| {
                if !variable_re.is_match(line) {
                    return None;
                }

                let trimmed = line.trim_start();
                let category = if trimmed.starts_with("boot ") {
                    "Boot Refs"
                } else if trimmed.contains("META") {
                    "Metadata"
                } else {
                    "Operand Refs"
                };

                Some(Z4UsageHit {
                    category,
                    line_0: line_idx as u32,
                })
            })
            .collect()
    }

    pub fn collect_outgoing_edges(source_text: &str, symbol: &Symbol) -> Vec<Z4CallEdge> {
        let mut edges = Vec::new();
        for (line_idx, line) in source_text.lines().enumerate() {
            let line_0 = line_idx as u32;
            if line_0 < symbol.line || line_0 > symbol.line_end {
                continue;
            }

            if let Some(caps) = z4_call_os_regex().captures(line) {
                if let Some(target) = caps.get(1) {
                    edges.push(Z4CallEdge {
                        category: "Call",
                        target: target.as_str().to_string(),
                        line_0,
                    });
                }
            }
            if let Some(caps) = z4_branchz_os_regex().captures(line) {
                if let Some(target) = caps.get(1) {
                    edges.push(Z4CallEdge {
                        category: "Branch Success",
                        target: target.as_str().to_string(),
                        line_0,
                    });
                }
            }
            if let Some(caps) = z4_branchz_oe_regex().captures(line) {
                if let Some(target) = caps.get(1) {
                    edges.push(Z4CallEdge {
                        category: "Branch Failure",
                        target: target.as_str().to_string(),
                        line_0,
                    });
                }
            }
            if let Some(caps) = z4_branch_os_regex().captures(line) {
                if let Some(target) = caps.get(1) {
                    edges.push(Z4CallEdge {
                        category: "Branch",
                        target: target.as_str().to_string(),
                        line_0,
                    });
                }
            }
        }
        edges
    }

    pub fn summarize_catalog(path: &Path, bytes: &[u8]) -> Option<Z4CatalogSummary> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = if bytes.starts_with(b"Z4SMI001") || bytes.starts_with(b"4SMI001") {
            "Z4_SMI_CATALOG"
        } else if bytes.starts_with(b"Z4REG001K")
            || bytes.starts_with(b"4REG001K")
            || file_name == "z4.reg"
        {
            "Z4_REGISTRY"
        } else {
            return None;
        };

        let mut entries = BTreeSet::new();
        for chunk in bytes.split(|byte| *byte == 0) {
            let Ok(text) = std::str::from_utf8(chunk) else {
                continue;
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let looks_like_entry = trimmed.ends_with(".z4")
                || trimmed.contains(".filelist")
                || trimmed.ends_with(".so")
                || trimmed.ends_with(".dylib")
                || trimmed.starts_with("build/")
                || trimmed.starts_with("bin/")
                || trimmed.starts_with("lib/");
            if looks_like_entry {
                entries.insert(trimmed.to_string());
            }
        }

        if entries.is_empty() {
            return None;
        }

        Some(Z4CatalogSummary {
            kind,
            entries: entries.into_iter().collect(),
        })
    }

    pub fn render_binary_catalog(path: &Path, bytes: &[u8]) -> Option<String> {
        let summary = Self::summarize_catalog(path, bytes)?;
        let mut out = format!(
            "# {}\nentries=0x{:x}\n",
            summary.kind,
            summary.entries.len()
        );
        for entry in summary.entries.iter().take(64) {
            out.push_str(entry);
            out.push('\n');
        }
        if summary.entries.len() > 64 {
            out.push_str(&format!(
                "... 0x{:x} more entries\n",
                summary.entries.len() - 64
            ));
        }
        Some(out)
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

    #[test]
    fn z4_extracts_boot_and_binding_symbols() {
        let boot_source = "boot memory Z4_MEMORY_POLICY_PATH memory_policy.z4\n";
        let boot_symbols = Z4LanguageDriver::extract_symbols(boot_source);

        assert_eq!(boot_symbols.len(), 1);
        assert_eq!(boot_symbols[0].name, "memory");
        assert_eq!(boot_symbols[0].kind, "boot");

        let binding_source = "[[binding]]\nid = \"printf\"\nsymbol = \"z4_printf\"\nabi = \"sysv\"\nmode = \"sync\"\n";
        let binding_symbols = Z4LanguageDriver::extract_symbols(binding_source);

        assert_eq!(binding_symbols.len(), 1);
        assert_eq!(binding_symbols[0].name, "z4_printf");
        assert_eq!(binding_symbols[0].kind, "binding");

        let rendered = Z4LanguageDriver::render_density_map(binding_source);
        assert!(rendered.contains("# Z4_BINDING_MAP"));
        assert!(rendered.contains("id=printf symbol=z4_printf abi=sysv mode=sync"));
    }

    #[test]
    fn z4_tracks_usages_and_control_flow_edges() {
        let source = "@entry: CALL OS:@target\nBRANCH OS:@loop\nBRANCHZ OS:@ok OE:@fail\nRET\n@target: RET\n@loop: RET\n@ok: RET\n@fail: RET\n";

        let target_hits = Z4LanguageDriver::find_usages(source, "target");
        assert!(target_hits.iter().any(|hit| hit.category == "Definitions"));
        assert!(target_hits.iter().any(|hit| hit.category == "Calls"));

        let symbols = Z4LanguageDriver::extract_symbols(source);
        let entry = symbols
            .iter()
            .find(|symbol| symbol.name == "entry")
            .expect("entry symbol");
        let edges = Z4LanguageDriver::collect_outgoing_edges(source, entry);

        assert!(edges.iter().any(|edge| edge.category == "Call" && edge.target == "target"));
        assert!(edges.iter().any(|edge| edge.category == "Branch" && edge.target == "loop"));
        assert!(edges.iter().any(|edge| edge.category == "Branch Success" && edge.target == "ok"));
        assert!(edges.iter().any(|edge| edge.category == "Branch Failure" && edge.target == "fail"));
    }

    #[test]
    fn z4_renders_binary_catalog_summaries() {
        let bytes = b"Z4SMI001\0\0\0\0\0\0\0\0crt0.z4\0fmt.z4\0build/compiler.filelist\0";
        let summary = Z4LanguageDriver::summarize_catalog(Path::new("build/compiler.filelist"), bytes)
            .expect("catalog summary");

        assert_eq!(summary.kind, "Z4_SMI_CATALOG");
        assert_eq!(summary.entries.len(), 3);

        let rendered = Z4LanguageDriver::render_binary_catalog(Path::new("build/compiler.filelist"), bytes)
            .expect("catalog render");
        assert!(rendered.contains("# Z4_SMI_CATALOG"));
        assert!(rendered.contains("entries=0x3"));
        assert!(rendered.contains("crt0.z4"));
    }
}
