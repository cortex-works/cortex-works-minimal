//! # `cortex_semantic_code_search` — Vector ANN Search Over Indexed Symbols
//!
//! Performs a nearest-neighbour search over the `code_nodes` LanceDB table
//! that a local semantic indexer populates.
//!
//! ## One-Shot RAG mode (`extract_code: true`)
//! When `extract_code` is `true`, the tool reads each matched source file and
//! uses the cortex-ast tree-sitter engine to slice out the exact code body for
//! every symbol — eliminating the extra `cortex_code_explorer` round-trip.
//!
//! ## Graceful degradation
//! * If the embedding model is unavailable a zero-vector search still returns
//!   the most-recently-indexed rows — less precise but never broken.
//! * If the `code_nodes` table is empty a helpful hint is returned reminding
//!   the user to build or refresh the local semantic index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cortex_db::{
    LanceDb,
    code_store::{CodeNode, delete_file_nodes, search_code_semantic, upsert_code_nodes},
    embed::{embed_code_node, embed_query},
};
use cortexast::{
    inspector::extract_symbols_from_source,
    scanner::{ScanOptions, scan_workspace},
};
use serde_json::Value;
use xxhash_rust::xxh3::xxh3_64;

const MAX_INDEX_FILE_BYTES: u64 = 512 * 1024;

struct IndexBuildStats {
    files_scanned: usize,
    files_indexed: usize,
    symbols_indexed: usize,
}

/// Entry-point called by [`crate::act::dispatch`].
pub fn run(args: &Value, workspace_roots: &[PathBuf]) -> Result<String, String> {
    // ── Parameters ────────────────────────────────────────────────────────
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "'query' is required and must be a non-empty string".to_string())?
        .to_string();

    let project_filter = args
        .get("project_path")
        .and_then(|v| v.as_str())
        .map(|s| crate::act::pathing::resolve_path_string(workspace_roots, s));

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5) as usize;

    // When true: read each matched file and inject the exact code body.
    let extract_code = args
        .get("extract_code")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Discard results whose cosine similarity falls below this threshold.
    let min_similarity = args
        .get("min_similarity")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;

    // ── Embed query ───────────────────────────────────────────────────────
    // Returns None when the model is unavailable; zero-vector degrades
    // gracefully to recency-ranked results.
    let raw_vec = embed_query(&query);
    let model_available = raw_vec.is_some();
    let query_vec = raw_vec.unwrap_or_else(|| vec![0.0_f32; 512]);

    // ── Open DB ───────────────────────────────────────────────────────────
    let db = LanceDb::open_default_sync()
        .map_err(|e| format!("DB open error: {e}"))?;

    // ── Search ────────────────────────────────────────────────────────────
    let results = search_code_semantic(
        &db,
        query_vec.clone(),
        project_filter.as_deref(),
        limit,
    )
    .map_err(|e| format!("Semantic search failed: {e}"))?;

    // ── Filter & format ───────────────────────────────────────────────────
    let mut results: Vec<_> = results
        .into_iter()
        .filter(|r| !r.symbol_name.trim().is_empty())
        .filter(|r| !r.kind.eq_ignore_ascii_case("file"))
        .filter(|r| r.similarity >= min_similarity)
        .collect();

    let mut index_build_stats = None;
    if results.is_empty() {
        if let Some(project_root) = project_filter.as_deref() {
            let stats = ensure_project_symbol_index(Path::new(project_root), &db)?;
            if stats.symbols_indexed > 0 {
                results = search_code_semantic(
                    &db,
                    query_vec,
                    Some(project_root),
                    limit,
                )
                .map_err(|e| format!("Semantic search failed after index refresh: {e}"))?
                .into_iter()
                .filter(|r| r.similarity >= min_similarity)
                .collect();
            }
            index_build_stats = Some(stats);
        }
    }

    if results.is_empty() {
        let auto_index_note = match index_build_stats {
            Some(stats) if stats.files_scanned == 0 => {
                "\n- Automatic indexing found no readable source files under the supplied `project_path`."
                    .to_string()
            }
            Some(stats) if stats.symbols_indexed == 0 => format!(
                "\n- Automatic indexing scanned {} file(s) but found no supported symbols to embed.",
                stats.files_scanned
            ),
            Some(stats) => format!(
                "\n- Automatic indexing refreshed {} file(s) and {} symbol(s), but this query still returned no matches.",
                stats.files_indexed, stats.symbols_indexed
            ),
            None if project_filter.is_none() => {
                "\n- Pass `project_path` to let this tool build or refresh the local symbol index automatically."
                    .to_string()
            }
            None => String::new(),
        };

        return Ok(format!(
            "No indexed symbols found for query: `{query}`\
             {min_sim_note}\n\n\
             **Tips:**\n\
                         - Filter by `project_path` to scope the search to a single repo.\n\
                         - Use `cortex_search_exact` for literal names and `cortex_code_explorer` when you already know the target area.\
                         {auto_index_note}",
            min_sim_note = if min_similarity > 0.0 {
                format!(" (similarity >= {min_similarity:.2})")
            } else {
                String::new()
            },
        ));
    }

    let rebuild_note = index_build_stats
        .as_ref()
        .filter(|stats| stats.symbols_indexed > 0)
        .map(|stats| {
            format!(
                "\n_Rebuilt local symbol index on demand: {} file(s), {} symbol(s)._\n",
                stats.files_indexed, stats.symbols_indexed
            )
        })
        .unwrap_or_default();

    let model_warn = if !model_available {
        "\n> ⚠️  **Embedding model unavailable** — scores reflect keyword/recency ranking, NOT semantic similarity. Results may be off-topic.\n"
    } else {
        ""
    };

    let mut out = format!(
        "## Semantic Code Search — `{query}`\n\
         _{n} result(s){scope}_{rebuild_note}\n{model_warn}",
        n = results.len(),
        scope = project_filter
            .as_deref()
            .map(|p| format!(" · scoped to `{p}`"))
            .unwrap_or_default(),
        rebuild_note = rebuild_note,
        model_warn = model_warn,
    );

    // Cache opened files to avoid re-reading the same file for each symbol.
    let mut file_cache: HashMap<String, String> = HashMap::new();

    for (i, r) in results.iter().enumerate() {
        let tags_str = if r.tags.is_empty() {
            String::new()
        } else {
            format!("  🏷 `{}`", r.tags)
        };
        out.push_str(&format!(
            "{}. **`{}`** `({})` — `{}`  · Score: **{:.3}**{}\n   Project: `{}`\n",
            i + 1,
            r.symbol_name,
            r.kind,
            r.file_path,
            r.similarity,
            tags_str,
            r.project_path,
        ));

        if extract_code {
            let abs_path = Path::new(&r.project_path).join(&r.file_path);
            let abs_str  = abs_path.to_string_lossy().to_string();
            let content  = file_cache
                .entry(abs_str)
                .or_insert_with(|| {
                    std::fs::read_to_string(&abs_path).unwrap_or_default()
                });

            if !content.is_empty() {
                match extract_symbol_code(&abs_path, content, &r.symbol_name, &r.kind) {
                    Some(code) => {
                        let lang = lang_id(&r.file_path);
                        // Hard limit: 100 lines max to prevent token explosion.
                        let truncated: String = code
                            .lines()
                            .take(100)
                            .collect::<Vec<_>>()
                            .join("\n");
                        let note = if code.lines().count() > 100 {
                            format!(" _(truncated at 100/{} lines)_", code.lines().count())
                        } else {
                            String::new()
                        };
                        out.push_str(&format!("\n{note}\n```{lang}\n{truncated}\n```\n"));
                    }
                    None => {
                        out.push_str(
                            "   _(code extraction failed — symbol may be macro-generated or minified)_\n",
                        );
                    }
                }
            } else {
                out.push_str(
                    "   _(source file not readable — it may have been deleted or moved)_\n",
                );
            }
        }

        out.push('\n');
    }

    Ok(out)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────────────────

/// Guess a Markdown fenced-code language identifier from a file extension.
fn lang_id(file_path: &str) -> &'static str {
    match Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "rs"                  => "rust",
        "ts" | "tsx"          => "typescript",
        "js" | "jsx" | "mjs" => "javascript",
        "py"                  => "python",
        "go"                  => "go",
        "java"                => "java",
        "kt" | "kts"          => "kotlin",
        "cs"                  => "csharp",
        "cpp" | "cc" | "cxx" => "cpp",
        "c" | "h"             => "c",
        "rb"                  => "ruby",
        "php"                 => "php",
        "dart"                => "dart",
        "swift"               => "swift",
        "scala"               => "scala",
        _                     => "",
    }
}

/// Try to extract the exact source snippet for `symbol_name` / `kind` from
/// `file_content` using the cortex-ast tree-sitter engine.
///
/// Returns `None` when the symbol cannot be located (unsupported language,
/// ambiguous name, parse error, bad byte range, etc.).
fn extract_symbol_code(
    abs_path: &Path,
    file_content: &str,
    symbol_name: &str,
    kind: &str,
) -> Option<String> {
    let symbols =
        cortexast::inspector::extract_symbols_from_source(abs_path, file_content);

    // Prefer exact kind+name; fall back to name-only when kinds differ
    // across languages (e.g. "method" vs "function").
    let sym = symbols
        .iter()
        .find(|s| s.name == symbol_name && s.kind.eq_ignore_ascii_case(kind))
        .or_else(|| symbols.iter().find(|s| s.name == symbol_name))?;

    if sym.end_byte > sym.start_byte && sym.end_byte <= file_content.len() {
        Some(file_content[sym.start_byte..sym.end_byte].to_string())
    } else {
        None
    }
}

fn ensure_project_symbol_index(project_root: &Path, db: &LanceDb) -> Result<IndexBuildStats, String> {
    let scan_opts = ScanOptions {
        repo_root: project_root.to_path_buf(),
        target: PathBuf::from("."),
        max_file_bytes: MAX_INDEX_FILE_BYTES,
        exclude_dir_names: vec![
            ".git".to_string(),
            "node_modules".to_string(),
            "target".to_string(),
            "dist".to_string(),
            "build".to_string(),
            "out".to_string(),
            ".cortexast".to_string(),
        ],
        workspace_roots: Vec::new(),
        extra_glob_excludes: Vec::new(),
    };

    let entries = scan_workspace(&scan_opts)
        .map_err(|e| format!("Automatic semantic index refresh failed to scan `{}`: {e}", project_root.display()))?;

    let project_path = normalize_path(project_root);
    let mut stats = IndexBuildStats {
        files_scanned: entries.len(),
        files_indexed: 0,
        symbols_indexed: 0,
    };

    for entry in entries {
        let rel_path = normalize_rel_path(&entry.rel_path);
        let content = match std::fs::read_to_string(&entry.abs_path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        delete_file_nodes(db, &project_path, &rel_path)
            .map_err(|e| format!("Automatic semantic index refresh failed to clear stale rows for `{rel_path}`: {e}"))?;

        let symbols = extract_symbols_from_source(&entry.abs_path, &content);
        if symbols.is_empty() {
            continue;
        }

        let mut nodes = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            let snippet = if symbol.end_byte > symbol.start_byte && symbol.end_byte <= content.len() {
                &content[symbol.start_byte..symbol.end_byte]
            } else {
                ""
            };
            let signature = symbol.signature.as_deref().unwrap_or("");
            let vector = embed_code_node(
                &project_path,
                &rel_path,
                &symbol.kind,
                &symbol.name,
                "",
                signature,
            )
            .unwrap_or_else(|| vec![0.0_f32; 512]);

            nodes.push(CodeNode {
                id: format!("{}::{}::{}", project_path, rel_path, symbol.name),
                project_path: project_path.clone(),
                file_path: rel_path.clone(),
                symbol_name: symbol.name,
                kind: symbol.kind,
                content_hash: format!("{:016x}", xxh3_64(snippet.as_bytes())),
                tags: String::new(),
                vector,
            });
        }

        upsert_code_nodes(db, &nodes)
            .map_err(|e| format!("Automatic semantic index refresh failed to store `{rel_path}`: {e}"))?;

        stats.files_indexed += 1;
        stats.symbols_indexed += nodes.len();
    }

    Ok(stats)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_rel_path(path: &Path) -> String {
    normalize_path(path).trim_start_matches("./").to_string()
}
