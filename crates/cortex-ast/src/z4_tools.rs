use anyhow::{Context, Result, anyhow};
use ignore::WalkBuilder;
use regex::Regex;
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::TryInto;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::universal::Z4LanguageDriver;

const Z4_SMI_MAGIC: &[u8] = b"Z4SMI001";
const Z4_SMI_LEGACY_MAGIC: &[u8] = b"4SMI001";
const Z4_REG_MAGIC: &[u8] = b"Z4REG001K";
const Z4_REG_LEGACY_MAGIC: &[u8] = b"4REG001K";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub slot: usize,
    pub offset: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RegistryEntry {
    pub offset: usize,
    pub id: u64,
    pub class: u64,
    pub class_label: &'static str,
    pub name_len: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum RegistryArtifact {
    Catalog {
        magic: &'static str,
        entries: Vec<CatalogEntry>,
    },
    Registry {
        magic: &'static str,
        entries: Vec<RegistryEntry>,
    },
}

#[derive(Debug, Default, Clone)]
pub struct RegistryAliases {
    by_rel_path: BTreeMap<String, String>,
    by_file_name: BTreeMap<String, String>,
}

impl RegistryAliases {
    pub fn display_name(&self, rel_path: &str, filename: &str) -> Option<String> {
        let rel = normalize_rel_path(rel_path);
        self.by_rel_path
            .get(&rel)
            .cloned()
            .or_else(|| self.by_file_name.get(filename).cloned())
    }

    pub fn is_empty(&self) -> bool {
        self.by_rel_path.is_empty() && self.by_file_name.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct HexDocRow {
    line_0: u32,
    label: Option<String>,
    raw_hex: String,
    byte_len: usize,
    decoded: String,
}

#[derive(Debug, Clone)]
struct UnitReport {
    alias: String,
    catalog_rel: String,
    defs: usize,
    uses: usize,
    def_files: BTreeSet<String>,
    use_files: BTreeSet<String>,
}

fn doc_hex_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"DOC:\"(0x[0-9A-Fa-f\\]+)\""#).unwrap())
}

fn label_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^@([A-Za-z0-9_]+):").unwrap())
}

fn normalize_rel_path(raw: &str) -> String {
    raw.replace('\\', "/").trim_start_matches("./").to_string()
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn registry_class_label(class: u64, path: &str) -> &'static str {
    match class {
        0x1 => "build_unit",
        0x3 => {
            if path.ends_with(".z4") {
                "source"
            } else {
                "artifact"
            }
        }
        _ => {
            if path.ends_with(".filelist") || path.ends_with(".project.z4") {
                "build_unit"
            } else if path.ends_with(".z4") {
                "source"
            } else if path.ends_with(".so") || path.ends_with(".dylib") {
                "library"
            } else {
                "unknown"
            }
        }
    }
}

fn header_span(
    bytes: &[u8],
    full_magic: &'static [u8],
    legacy_magic: &'static [u8],
) -> Option<(&'static str, usize)> {
    if bytes.starts_with(full_magic) {
        let header_len = if bytes.len() >= 16
            && bytes[full_magic.len()..16].iter().all(|byte| *byte == 0)
        {
            16
        } else {
            full_magic.len()
        };
        let magic = std::str::from_utf8(full_magic).ok()?;
        return Some((magic, header_len));
    }

    if bytes.starts_with(legacy_magic) {
        let header_len = if bytes.len() >= 16
            && bytes[legacy_magic.len()..16].iter().all(|byte| *byte == 0)
        {
            16
        } else {
            legacy_magic.len()
        };
        let magic = std::str::from_utf8(legacy_magic).ok()?;
        return Some((magic, header_len));
    }

    None
}

pub fn is_machine_visible_path(path: &Path) -> bool {
    Z4LanguageDriver::handles_path(path) || Z4LanguageDriver::is_catalog_path(path)
}

pub fn parse_registry_artifact(path: &Path, bytes: &[u8]) -> Result<RegistryArtifact> {
    if let Some((magic, header_len)) = header_span(bytes, Z4_REG_MAGIC, Z4_REG_LEGACY_MAGIC) {
        let mut cursor = header_len;
        let mut entries = Vec::new();

        while cursor < bytes.len() {
            if bytes[cursor..].iter().all(|byte| *byte == 0) {
                break;
            }

            if cursor + 24 > bytes.len() {
                return Err(anyhow!(
                    "Truncated z4.reg record at offset 0x{cursor:x} in {}",
                    path.display()
                ));
            }

            let record_offset = cursor;
            let id = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
            let class = u64::from_le_bytes(bytes[cursor + 8..cursor + 16].try_into().unwrap());
            let name_len =
                u64::from_le_bytes(bytes[cursor + 16..cursor + 24].try_into().unwrap()) as usize;
            cursor += 24;

            if cursor + name_len > bytes.len() {
                return Err(anyhow!(
                    "Invalid name length 0x{name_len:x} at offset 0x{record_offset:x} in {}",
                    path.display()
                ));
            }

            let raw_path = &bytes[cursor..cursor + name_len];
            let entry_path = String::from_utf8_lossy(raw_path).trim().to_string();
            cursor += name_len;

            if entry_path.is_empty() {
                continue;
            }

            entries.push(RegistryEntry {
                offset: record_offset,
                id,
                class,
                class_label: registry_class_label(class, &entry_path),
                name_len,
                path: normalize_rel_path(&entry_path),
            });
        }

        if entries.is_empty() {
            return Err(anyhow!("No z4.reg entries found in {}", path.display()));
        }

        return Ok(RegistryArtifact::Registry { magic, entries });
    }

    if let Some((magic, header_len)) = header_span(bytes, Z4_SMI_MAGIC, Z4_SMI_LEGACY_MAGIC) {
        let mut cursor = header_len;
        let mut slot = 0usize;
        let mut entries = Vec::new();

        while cursor < bytes.len() {
            while cursor < bytes.len() && bytes[cursor] == 0 {
                cursor += 1;
            }
            if cursor >= bytes.len() {
                break;
            }

            let offset = cursor;
            while cursor < bytes.len() && bytes[cursor] != 0 {
                cursor += 1;
            }

            let raw_path = &bytes[offset..cursor];
            let entry_path = String::from_utf8_lossy(raw_path).trim().to_string();
            if !entry_path.is_empty() {
                entries.push(CatalogEntry {
                    slot,
                    offset,
                    path: normalize_rel_path(&entry_path),
                });
                slot += 1;
            }
        }

        if entries.is_empty() {
            return Err(anyhow!("No catalog entries found in {}", path.display()));
        }

        return Ok(RegistryArtifact::Catalog { magic, entries });
    }

    Err(anyhow!(
        "Unsupported z4 registry artifact '{}': expected Z4REG001K/Z4SMI001",
        path.display()
    ))
}

pub fn registry_aliases(repo_root: &Path) -> RegistryAliases {
    let registry_path = repo_root.join("z4.reg");
    let Ok(bytes) = std::fs::read(&registry_path) else {
        return RegistryAliases::default();
    };
    let Ok(RegistryArtifact::Registry { entries, .. }) = parse_registry_artifact(&registry_path, &bytes)
    else {
        return RegistryAliases::default();
    };

    let mut aliases = RegistryAliases::default();
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &entries {
        let file_name = basename(&entry.path);
        *name_counts.entry(file_name).or_insert(0) += 1;
    }

    for entry in entries {
        let file_name = basename(&entry.path);
        let alias = format!("[0x{:x}] {}", entry.id, file_name);
        aliases
            .by_rel_path
            .insert(normalize_rel_path(&entry.path), alias.clone());
        if name_counts.get(&file_name).copied().unwrap_or(0) == 1 {
            aliases.by_file_name.insert(file_name, alias);
        }
    }

    aliases
}

fn render_registry_table(path: &Path, artifact: &RegistryArtifact, max_entries: usize) -> String {
    let mut out = String::new();
    match artifact {
        RegistryArtifact::Registry { magic, entries } => {
            out.push_str("# Z4_REG_READER\n");
            out.push_str(&format!(
                "path={} kind=registry magic={} entries=0x{:x}\n",
                path.display(),
                magic,
                entries.len()
            ));
            for entry in entries.iter().take(max_entries) {
                out.push_str(&format!(
                    "offset=0x{:x} id=0x{:x} class=0x{:x}({}) len=0x{:x} path={}\n",
                    entry.offset,
                    entry.id,
                    entry.class,
                    entry.class_label,
                    entry.name_len,
                    entry.path,
                ));
            }
            if entries.len() > max_entries {
                out.push_str(&format!("... 0x{:x} more entries\n", entries.len() - max_entries));
            }
        }
        RegistryArtifact::Catalog { magic, entries } => {
            out.push_str("# Z4_REG_READER\n");
            out.push_str(&format!(
                "path={} kind=catalog magic={} entries=0x{:x}\n",
                path.display(),
                magic,
                entries.len()
            ));
            for entry in entries.iter().take(max_entries) {
                out.push_str(&format!(
                    "slot=0x{:x} offset=0x{:x} path={}\n",
                    entry.slot,
                    entry.offset,
                    entry.path,
                ));
            }
            if entries.len() > max_entries {
                out.push_str(&format!("... 0x{:x} more entries\n", entries.len() - max_entries));
            }
        }
    }
    out
}

pub fn read_registry(path: &Path, output_format: &str, max_entries: usize) -> Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read z4 registry artifact '{}'", path.display()))?;
    let artifact = parse_registry_artifact(path, &bytes)?;
    let max_entries = max_entries.max(1);

    match output_format {
        "table" => Ok(render_registry_table(path, &artifact, max_entries)),
        "json" => match artifact {
            RegistryArtifact::Registry { magic, entries } => Ok(serde_json::to_string_pretty(&json!({
                "path": path.display().to_string(),
                "kind": "registry",
                "magic": magic,
                "entries": entries.into_iter().take(max_entries).collect::<Vec<_>>()
            }))?),
            RegistryArtifact::Catalog { magic, entries } => Ok(serde_json::to_string_pretty(&json!({
                "path": path.display().to_string(),
                "kind": "catalog",
                "magic": magic,
                "entries": entries.into_iter().take(max_entries).collect::<Vec<_>>()
            }))?),
        },
        other => Err(anyhow!(
            "Unsupported output_format '{}'. Use 'table' or 'json'.",
            other
        )),
    }
}

fn decode_hex_literal(raw_hex: &str) -> Result<Vec<u8>> {
    let trimmed = raw_hex.trim().trim_matches('"');
    let Some(hex_body) = trimmed.strip_prefix("0x") else {
        return Err(anyhow!("Expected hex DOC literal starting with 0x"));
    };

    let mut hex_digits = hex_body.trim_end_matches("\\0").to_string();
    hex_digits.retain(|ch| ch.is_ascii_hexdigit());
    if hex_digits.is_empty() {
        return Ok(Vec::new());
    }
    if hex_digits.len() % 2 != 0 {
        hex_digits.insert(0, '0');
    }

    let mut bytes = Vec::with_capacity(hex_digits.len() / 2);
    let mut index = 0usize;
    while index < hex_digits.len() {
        let byte = u8::from_str_radix(&hex_digits[index..index + 2], 16).with_context(|| {
            format!(
                "Invalid hex DOC byte '{}{}'",
                &hex_digits[index..index + 1],
                &hex_digits[index + 1..index + 2]
            )
        })?;
        bytes.push(byte);
        index += 2;
    }
    Ok(bytes)
}

fn render_bytes_for_agent(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        match *byte {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            0 => out.push_str("\\0"),
            0x20..=0x7e => out.push(*byte as char),
            _ => out.push_str(&format!("\\x{:02x}", byte)),
        }
    }
    out
}

fn scan_hex_docs(source_text: &str) -> Vec<HexDocRow> {
    source_text
        .lines()
        .enumerate()
        .filter_map(|(line_idx, line)| {
            let caps = doc_hex_regex().captures(line)?;
            let raw_hex = caps.get(1)?.as_str().to_string();
            let decoded_bytes = decode_hex_literal(&raw_hex).ok()?;
            let label = label_regex()
                .captures(line)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string());
            Some(HexDocRow {
                line_0: line_idx as u32,
                label,
                byte_len: decoded_bytes.len(),
                decoded: render_bytes_for_agent(&decoded_bytes),
                raw_hex,
            })
        })
        .collect()
}

fn filter_rows_to_symbol(
    rows: Vec<HexDocRow>,
    source_text: &str,
    symbol_name: &str,
) -> Result<Vec<HexDocRow>> {
    let symbols = Z4LanguageDriver::extract_symbols(source_text);
    let Some(symbol) = symbols
        .iter()
        .find(|symbol| symbol.name == symbol_name || symbol.name.eq_ignore_ascii_case(symbol_name))
    else {
        return Err(anyhow!("Symbol '{}' not found in z4 source", symbol_name));
    };

    Ok(rows
        .into_iter()
        .filter(|row| symbol.line <= row.line_0 && row.line_0 <= symbol.line_end)
        .collect())
}

pub fn decode_hex_bridge(
    path: Option<&Path>,
    symbol_name: Option<&str>,
    doc_hex: Option<&str>,
    max_entries: usize,
) -> Result<String> {
    if let Some(raw_hex) = doc_hex {
        let bytes = decode_hex_literal(raw_hex)?;
        return Ok(format!(
            "# Z4_HEX_BRIDGE\nbytes=0x{:x} text=\"{}\"\n",
            bytes.len(),
            render_bytes_for_agent(&bytes)
        ));
    }

    let path = path.ok_or_else(|| anyhow!("'path' required when 'doc_hex' is omitted"))?;
    let source_text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read z4 source '{}'", path.display()))?;
    let mut rows = scan_hex_docs(&source_text);
    if let Some(symbol_name) = symbol_name {
        rows = filter_rows_to_symbol(rows, &source_text, symbol_name)?;
    }
    if rows.is_empty() {
        return Err(anyhow!("No DATA/DEFINE DOC hex literals found in {}", path.display()));
    }

    let max_entries = max_entries.max(1);
    let mut out = String::new();
    out.push_str("# Z4_HEX_BRIDGE\n");
    out.push_str(&format!("path={} docs=0x{:x}\n", path.display(), rows.len()));
    for row in rows.iter().take(max_entries) {
        let label = row.label.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "line=0x{:x} label=@{} bytes=0x{:x} text=\"{}\"\n",
            row.line_0 + 1,
            label,
            row.byte_len,
            row.decoded,
        ));
    }
    if rows.len() > max_entries {
        out.push_str(&format!("... 0x{:x} more docs\n", rows.len() - max_entries));
    }
    Ok(out)
}

fn discover_catalogs(search_root: &Path) -> Vec<PathBuf> {
    let mut catalogs = Vec::new();
    let walker = WalkBuilder::new(search_root)
        .standard_filters(true)
        .hidden(true)
        .build();

    for entry_result in walker {
        let Ok(entry) = entry_result else { continue };
        let path = entry.path();
        if path.is_file() && Z4LanguageDriver::is_catalog_path(path) {
            catalogs.push(path.to_path_buf());
        }
    }

    catalogs.sort();
    catalogs
}

fn selected_catalogs(repo_root: &Path, selected_path: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(selected_path) = selected_path {
        if selected_path.is_dir() {
            let catalogs = discover_catalogs(selected_path);
            if catalogs.is_empty() {
                return Err(anyhow!(
                    "No z4 catalog files found under '{}'",
                    selected_path.display()
                ));
            }
            return Ok(catalogs);
        }

        if !Z4LanguageDriver::is_catalog_path(selected_path) {
            return Err(anyhow!(
                "'path' must point to a .filelist or .project.z4 file, got '{}'",
                selected_path.display()
            ));
        }

        return Ok(vec![selected_path.to_path_buf()]);
    }

    let catalogs = discover_catalogs(repo_root);
    if catalogs.is_empty() {
        return Err(anyhow!("No z4 build unit catalogs found under '{}'", repo_root.display()));
    }
    Ok(catalogs)
}

fn rel_display(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

fn catalog_entries(path: &Path) -> Result<Vec<CatalogEntry>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read z4 catalog '{}'", path.display()))?;
    match parse_registry_artifact(path, &bytes)? {
        RegistryArtifact::Catalog { entries, .. } => Ok(entries),
        RegistryArtifact::Registry { .. } => Err(anyhow!(
            "Expected catalog (.filelist/.project.z4), got registry '{}'",
            path.display()
        )),
    }
}

fn resolve_unit_source_path(repo_root: &Path, catalog_path: &Path, entry_path: &str) -> PathBuf {
    let repo_candidate = repo_root.join(entry_path);
    if repo_candidate.exists() {
        return repo_candidate;
    }
    catalog_path.parent().unwrap_or(repo_root).join(entry_path)
}

fn render_build_units(
    repo_root: &Path,
    catalogs: &[PathBuf],
    max_units: usize,
    max_entries: usize,
) -> Result<String> {
    let aliases = registry_aliases(repo_root);
    let max_units = max_units.max(1);
    let max_entries = max_entries.max(1);
    let mut out = String::new();
    out.push_str("# Z4_UNIT_SCAN\n");
    out.push_str(&format!("mode=build_units units=0x{:x}\n", catalogs.len()));

    for catalog in catalogs.iter().take(max_units) {
        let rel = rel_display(repo_root, catalog);
        let alias = aliases
            .display_name(&rel, &basename(&rel))
            .unwrap_or_else(|| rel.clone());
        let entries = catalog_entries(catalog)?;
        out.push_str(&format!("unit={} files=0x{:x}\n", alias, entries.len()));
        for entry in entries.iter().take(max_entries) {
            out.push_str(&format!("  0x{:x} {}\n", entry.slot, entry.path));
        }
        if entries.len() > max_entries {
            out.push_str(&format!("  ... 0x{:x} more files\n", entries.len() - max_entries));
        }
    }

    if catalogs.len() > max_units {
        out.push_str(&format!("... 0x{:x} more units\n", catalogs.len() - max_units));
    }
    Ok(out)
}

fn build_unit_report(
    repo_root: &Path,
    catalog_path: &Path,
    symbol_name: &str,
    aliases: &RegistryAliases,
) -> Result<Option<UnitReport>> {
    let entries = catalog_entries(catalog_path)?;
    let rel = rel_display(repo_root, catalog_path);
    let alias = aliases
        .display_name(&rel, &basename(&rel))
        .unwrap_or_else(|| rel.clone());
    let mut report = UnitReport {
        alias,
        catalog_rel: rel,
        defs: 0,
        uses: 0,
        def_files: BTreeSet::new(),
        use_files: BTreeSet::new(),
    };

    for entry in entries {
        if !entry.path.ends_with(".z4") {
            continue;
        }

        let abs_source = resolve_unit_source_path(repo_root, catalog_path, &entry.path);
        let Ok(source_text) = std::fs::read_to_string(&abs_source) else {
            continue;
        };

        let defs_in_file = Z4LanguageDriver::extract_symbols(&source_text)
            .into_iter()
            .filter(|symbol| symbol.name == symbol_name || symbol.name.eq_ignore_ascii_case(symbol_name))
            .count();
        let usage_hits = Z4LanguageDriver::find_usages(&source_text, symbol_name);
        let non_definition_uses = usage_hits
            .iter()
            .filter(|hit| hit.category != "Definitions")
            .count();

        if defs_in_file > 0 {
            report.defs += defs_in_file;
            report.def_files.insert(entry.path.clone());
        }
        if non_definition_uses > 0 {
            report.uses += non_definition_uses;
            report.use_files.insert(entry.path.clone());
        }
    }

    if report.defs == 0 && report.uses == 0 {
        return Ok(None);
    }

    Ok(Some(report))
}

fn rename_guard_status(reports: &[UnitReport]) -> (&'static str, usize, usize) {
    let units_with_defs = reports.iter().filter(|report| report.defs > 0).count();
    let units_with_uses = reports.iter().filter(|report| report.uses > 0).count();

    let status = if reports.is_empty() {
        "not_found"
    } else if units_with_defs == 0 {
        "usage_without_definition"
    } else if units_with_defs == 1 && reports.len() == 1 {
        "single_unit_safe"
    } else if units_with_defs == 1 && units_with_uses > 1 {
        "cross_unit_references"
    } else if units_with_defs == 1 {
        "single_definition_multi_file"
    } else {
        "multi_unit_collision"
    };

    (status, units_with_defs, units_with_uses)
}

fn render_rename_guard(
    repo_root: &Path,
    catalogs: &[PathBuf],
    symbol_name: &str,
    max_units: usize,
) -> Result<String> {
    let aliases = registry_aliases(repo_root);
    let mut reports = Vec::new();
    for catalog in catalogs {
        if let Some(report) = build_unit_report(repo_root, catalog, symbol_name, &aliases)? {
            reports.push(report);
        }
    }

    if reports.is_empty() {
        return Ok(format!(
            "# Z4_UNIT_SCAN\nmode=rename_guard symbol={} status=not_found units=0x0\n",
            symbol_name
        ));
    }

    let total_defs: usize = reports.iter().map(|report| report.defs).sum();
    let total_uses: usize = reports.iter().map(|report| report.uses).sum();
    let (status, units_with_defs, units_with_uses) = rename_guard_status(&reports);
    let max_units = max_units.max(1);

    let mut out = String::new();
    out.push_str("# Z4_UNIT_SCAN\n");
    out.push_str(&format!(
        "mode=rename_guard symbol={} status={} units=0x{:x} def_units=0x{:x} use_units=0x{:x} defs=0x{:x} uses=0x{:x}\n",
        symbol_name,
        status,
        reports.len(),
        units_with_defs,
        units_with_uses,
        total_defs,
        total_uses,
    ));
    for report in reports.iter().take(max_units) {
        out.push_str(&format!(
            "unit={} defs=0x{:x} uses=0x{:x}\n",
            report.alias,
            report.defs,
            report.uses,
        ));
        if !report.def_files.is_empty() {
            out.push_str(&format!(
                "  def_files={}\n",
                report.def_files.iter().cloned().collect::<Vec<_>>().join(",")
            ));
        }
        if !report.use_files.is_empty() {
            out.push_str(&format!(
                "  use_files={}\n",
                report.use_files.iter().cloned().collect::<Vec<_>>().join(",")
            ));
        }
        out.push_str(&format!("  catalog={}\n", report.catalog_rel));
    }
    if reports.len() > max_units {
        out.push_str(&format!("... 0x{:x} more units\n", reports.len() - max_units));
    }
    Ok(out)
}

pub fn unit_scan(
    repo_root: &Path,
    action: &str,
    selected_path: Option<&Path>,
    symbol_name: Option<&str>,
    max_units: usize,
    max_entries: usize,
) -> Result<String> {
    let catalogs = selected_catalogs(repo_root, selected_path)?;
    match action {
        "build_units" => render_build_units(repo_root, &catalogs, max_units, max_entries),
        "rename_guard" => {
            let symbol_name = symbol_name
                .map(str::trim)
                .filter(|symbol| !symbol.is_empty())
                .ok_or_else(|| anyhow!("'symbol_name' required for action='rename_guard'"))?;
            render_rename_guard(repo_root, &catalogs, symbol_name, max_units)
        }
        other => Err(anyhow!(
            "Unsupported cortex_z4_unit_scan action '{}'. Use 'build_units' or 'rename_guard'.",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_z4_registry_records() {
        let bytes = b"Z4REG001K\0\0\0\0\0\0\0\x86\x29\xb2\x04\0\0\0\0\x03\0\0\0\0\0\0\0\x08\0\0\0\0\0\0\0alloc.z4";
        let artifact = parse_registry_artifact(Path::new("z4.reg"), bytes).expect("registry parse");

        match artifact {
            RegistryArtifact::Registry { entries, .. } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].offset, 0x10);
                assert_eq!(entries[0].id, 0x04b22986);
                assert_eq!(entries[0].class_label, "source");
                assert_eq!(entries[0].path, "alloc.z4");
            }
            _ => panic!("expected registry artifact"),
        }
    }

    #[test]
    fn parses_real_smi_catalog_records() {
        let bytes = b"Z4SMI001\0\0\0\0\0\0\0\0crt0.z4\0alloc.z4\0";
        let artifact = parse_registry_artifact(Path::new("build/compiler.filelist"), bytes)
            .expect("catalog parse");

        match artifact {
            RegistryArtifact::Catalog { entries, .. } => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].slot, 0);
                assert_eq!(entries[0].offset, 0x10);
                assert_eq!(entries[1].path, "alloc.z4");
            }
            _ => panic!("expected catalog artifact"),
        }
    }

    #[test]
    fn hex_bridge_decodes_ascii_docs() {
        let rendered = decode_hex_bridge(None, None, Some("0x41444400"), 8).expect("hex bridge");
        assert!(rendered.contains("bytes=0x4"));
        assert!(rendered.contains("ADD\\0"));
    }

    #[test]
    fn unit_scan_reports_single_unit_safe_symbols() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("build")).expect("build dir");
        std::fs::write(
            dir.path().join("build/compiler.filelist"),
            b"Z4SMI001\0\0\0\0\0\0\0\0alpha.z4\0beta.z4\0",
        )
        .expect("catalog");
        std::fs::write(
            dir.path().join("alpha.z4"),
            "@alpha: CALL OS:@beta\nRET\n@beta: RET\n",
        )
        .expect("alpha source");
        std::fs::write(dir.path().join("beta.z4"), "@gamma: RET\n").expect("beta source");

        let rendered = unit_scan(
            dir.path(),
            "rename_guard",
            Some(&dir.path().join("build/compiler.filelist")),
            Some("beta"),
            8,
            8,
        )
        .expect("unit scan");

        assert!(rendered.contains("status=single_unit_safe"));
        assert!(rendered.contains("defs=0x1"));
        assert!(rendered.contains("uses=0x1"));
    }
}