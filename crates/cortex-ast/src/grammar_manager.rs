//! # Grammar Manager — Dynamic Wasm Language Plugin System
//!
//! Manages the lifecycle of tree-sitter grammar plugins for non-Core languages.
//! Core 3 (Rust, TypeScript, Python) are statically linked into the binary.
//! All other languages are served as `.wasm` grammars fetched from GitHub tree-sitter releases.
//!
//! ## Cache directory
//! `~/.cortex-works/grammars/<lang>.wasm`
//! `~/.cortex-works/grammars/<lang>_prune.scm`
//!
//! Prune queries are embedded in the binary and copied into the cache dir on startup.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The three statically-compiled language names. They never need downloading.
pub const CORE_LANGUAGES: &[&str] = &["rust", "typescript", "python"];
pub const DOWNLOADABLE_LANGUAGES: &[&str] = &["go", "php", "ruby", "java", "c", "cpp", "c_sharp", "dart"];

/// Valid `action` values for `cortex_manage_ast_languages`.
/// Referenced by both the schema enum and the runtime dispatcher so they cannot drift.
pub const ACTION_STATUS: &str = "status";
pub const ACTION_ADD: &str = "add";

pub fn tool_schema() -> Value {
    let available = DOWNLOADABLE_LANGUAGES.join(", ");
    let description = format!(
        "Download and hot-reload extra Tree-sitter Wasm grammars from GitHub releases. Core languages are built in; use this only when a non-core parser is missing. Call action=status first to see what is already active, then action=add only for the missing parser. Returns machine-readable JSON text. Available to download: {available}."
    );

    json!({
        "name": "cortex_manage_ast_languages",
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "add"],
                    "description": "status: list currently active, core, and downloadable languages. add: download and hot-reload one or more missing parsers; returns an error result when any requested language fails."
                },
                "languages": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Language names to install for action=add (e.g. ['go','php','cpp']). Core languages are already built in and do not need downloading."
                },
                "repoPath": {
                    "type": "string",
                    "description": "Optional absolute repo root whose cached semantic index should be invalidated after new parsers are added. Use this when you plan to re-run semantic or AST-heavy tools on a specific repo right after installing a parser."
                },
                "target_project": {
                    "type": "string",
                    "description": "Optional tracked project path override used instead of repoPath when invalidating cached semantic records for a project already tracked in Cortex DB."
                }
            },
            "required": ["action"]
        }
    })
}

pub fn handle_tool_call(
    args: &Value,
    repo_root: Option<&std::path::Path>,
    workspace_roots: &[PathBuf],
) -> std::result::Result<String, String> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    match action {
        ACTION_STATUS => Ok(serde_json::to_string(&json!({
            "status": "ok",
            "active": crate::inspector::exported_language_config()
                .read()
                .map_err(|_| "language config lock poisoned".to_string())?
                .active_languages(),
            "available_to_download": DOWNLOADABLE_LANGUAGES,
            "core_languages": CORE_LANGUAGES,
        }))
        .unwrap_or_default()),
        ACTION_ADD => add_languages(args, repo_root, workspace_roots),
        _ => Err(format!("Invalid action '{action}'. Must be '{}' or '{}'.", ACTION_STATUS, ACTION_ADD)),
    }
}

fn add_languages(
    args: &Value,
    repo_root: Option<&std::path::Path>,
    workspace_roots: &[PathBuf],
) -> std::result::Result<String, String> {
    let requested = args
        .get("languages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "No languages provided for 'add' action".to_string())?;

    let mut loaded_langs = Vec::new();
    let mut failed_langs = Vec::new();
    let mut exts_to_invalidate = Vec::new();

    {
        let mut cfg = crate::inspector::exported_language_config()
            .write()
            .map_err(|_| "language config lock poisoned".to_string())?;
        for item in requested {
            let Some(lang) = item.as_str() else { continue };

            if cfg.active_languages().contains(&lang.to_string()) {
                loaded_langs.push(lang.to_string());
                continue;
            }

            match cfg.add_wasm_driver(lang) {
                Ok(_) => {
                    loaded_langs.push(lang.to_string());
                    exts_to_invalidate.extend(cfg.extensions_for_language(lang));
                }
                Err(e) => {
                    eprintln!("Failed to add wasm driver for {}: {}", lang, e);
                    failed_langs.push(json!({
                        "language": lang,
                        "error": e.to_string(),
                    }));
                }
            }
        }
    }

    let repo_root = repo_root
        .map(std::path::Path::to_path_buf)
        .or_else(|| workspace_roots.first().cloned())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut invalidated = 0;
    if !exts_to_invalidate.is_empty() {
        let db_dir = crate::config::central_cache_dir(workspace_roots)
            .unwrap_or_else(|| repo_root.join(".cortexast"))
            .join("db");
        if db_dir.exists() {
            if let Ok(mut index) = crate::vector_store::CodebaseIndex::open(
                &repo_root,
                &db_dir,
                "nomic-embed-text",
                60,
            ) {
                let refs: Vec<&str> = exts_to_invalidate.iter().map(|s| s.as_str()).collect();
                invalidated = index.invalidate_extensions(&refs);
            }
        }
    }

    let payload = json!({
        "status": if failed_langs.is_empty() { "ok" } else { "partial_failure" },
        "loaded_languages": loaded_langs,
        "failed_languages": failed_langs,
        "invalidated_records": invalidated,
        "invalidated_extensions": exts_to_invalidate,
        "repo_root": repo_root,
    });

    let text = serde_json::to_string(&payload).unwrap_or_default();
    if payload["failed_languages"].as_array().map(|arr| arr.is_empty()).unwrap_or(true) {
        Ok(text)
    } else {
        Err(text)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cache directory helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `~/.cortex-works/grammars/`, creating it if necessary.
pub fn grammar_cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot resolve $HOME")?;
    let dir = home.join(".cortex-works").join("grammars");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating grammar cache dir: {}", dir.display()))?;
    Ok(dir)
}

/// Absolute path to the cached `.wasm` file for a language.
pub fn wasm_path(lang: &str) -> Result<PathBuf> {
    Ok(grammar_cache_dir()?.join(format!("{lang}.wasm")))
}

/// Absolute path to the cached `.scm` prune-query file for a language.
pub fn scm_path(lang: &str) -> Result<PathBuf> {
    Ok(grammar_cache_dir()?.join(format!("{lang}_prune.scm")))
}

// ─────────────────────────────────────────────────────────────────────────────
// Ensure grammar is available — the core function
// ─────────────────────────────────────────────────────────────────────────────

/// Ensure `{lang}.wasm` exists in the local cache.
///
/// - If `lang` is one of [`CORE_LANGUAGES`] this is a no-op.
/// - Otherwise it checks the cache. Missing `.wasm` files are downloaded.
/// - Network or I/O errors are returned as `Err(...)`.  Callers should fall back
///   gracefully to the universal regex parser.
pub fn ensure_grammar_available(lang: &str) -> Result<()> {
    // Core languages are statically linked — nothing to do.
    if CORE_LANGUAGES.contains(&lang) {
        return Ok(());
    }

    let wasm = wasm_path(lang)?;
    if !wasm.exists() {
        download_artifact(lang, "wasm", &wasm)?;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Download routine
// ─────────────────────────────────────────────────────────────────────────────

/// Map a language name to its GitHub release download URL.
/// Falls back to a predictable naming convention.
fn github_wasm_url(lang: &str) -> String {
    // Some langs have non-standard repo names or non-standard release asset names.
    match lang {
        "c_sharp"  => return "https://github.com/tree-sitter/tree-sitter-c-sharp/releases/latest/download/tree-sitter-c_sharp.wasm".to_string(),
        "cpp"      => return "https://github.com/tree-sitter/tree-sitter-cpp/releases/latest/download/tree-sitter-cpp.wasm".to_string(),
        "c"        => return "https://github.com/tree-sitter/tree-sitter-c/releases/latest/download/tree-sitter-c.wasm".to_string(),
        // yaml grammar is maintained by ikatyang, not the main tree-sitter org.
        "yaml"     => return "https://github.com/ikatyang/tree-sitter-yaml/releases/latest/download/tree-sitter-yaml.wasm".to_string(),
        // toml: nickel-lang maintains a wasm-releasing fork.
        "toml"     => return "https://github.com/nickel-lang/tree-sitter-toml/releases/latest/download/tree-sitter-toml.wasm".to_string(),
        // markdown: official tree-sitter org, asset uses hyphen not underscore.
        "markdown" => return "https://github.com/tree-sitter/tree-sitter-markdown/releases/latest/download/tree-sitter-markdown.wasm".to_string(),
        _ => {}
    }
    let repo_name = Box::leak(format!("tree-sitter-{lang}").into_boxed_str()) as &str;
    format!("https://github.com/tree-sitter/{repo_name}/releases/latest/download/{repo_name}.wasm")
}

/// Download a grammar `.wasm` and write it to `dest`.
fn download_artifact(lang: &str, kind: &str, dest: &PathBuf) -> Result<()> {
    if kind != "wasm" {
        anyhow::bail!("unsupported grammar artifact kind: {kind}");
    }

    let url = github_wasm_url(lang);

    eprintln!("[grammar_manager] Downloading {url} → {}", dest.display());

    let response = ureq::get(&url)
        .call()
        .with_context(|| format!("HTTP GET {url}"))?;

    let status = response.status();
    if status != 200 {
        anyhow::bail!("HTTP {status} fetching {url}");
    }

    let mut body: Vec<u8> = Vec::new();
    use std::io::Read;
    response
        .into_reader()
        .read_to_end(&mut body)
        .with_context(|| format!("reading response body from {url}"))?;

    std::fs::write(dest, &body).with_context(|| format!("writing {}", dest.display()))?;

    eprintln!(
        "[grammar_manager] Saved {lang}.wasm ({} bytes) → {}",
        body.len(),
        dest.display()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Query .scm content loader
// ─────────────────────────────────────────────────────────────────────────────

/// Read the body-prune query for a language from the local cache.
/// Returns `None` if no `.scm` file exists (grammar has no pruning queries).
pub fn load_prune_scm(lang: &str) -> Option<String> {
    let path = scm_path(lang).ok()?;
    std::fs::read_to_string(path).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Bootstrap — seed embedded prune queries into the local cache
// ─────────────────────────────────────────────────────────────────────────────

/// Copy the prune `.scm` files that are embedded in the binary (via
/// `include_str!`) into the grammar cache directory so that Wasm drivers can
/// find them at runtime.
///
/// Each entry is `(embedded_content, cache_lang_name)`.  A file is only
/// written when it doesn't already exist, so user-supplied overrides are
/// preserved.
///
/// Call this once before `load_cached_wasm_drivers()` to ensure prune queries
/// are available on the very first run (before any `.wasm` download).
pub fn bootstrap_embedded_queries() {
    const QUERIES: &[(&str, &str)] = &[
        (include_str!("../queries/go_prune.scm"), "go"),
        (include_str!("../queries/php_prune.scm"), "php"),
        (include_str!("../queries/java_prune.scm"), "java"),
        (include_str!("../queries/dart_prune.scm"), "dart"),
        // Note: the repo file is named `csharp_prune.scm`; the grammar lang is `c_sharp`.
        (include_str!("../queries/csharp_prune.scm"), "c_sharp"),
        (include_str!("../queries/cpp_prune.scm"), "cpp"),
        (include_str!("../queries/ruby_prune.scm"), "ruby"),
        (include_str!("../queries/c_prune.scm"), "c"),
    ];

    let Ok(dir) = grammar_cache_dir() else { return };

    for (content, lang) in QUERIES {
        let dest = dir.join(format!("{lang}_prune.scm"));
        if !dest.exists() {
            if let Err(e) = std::fs::write(&dest, content) {
                eprintln!(
                    "[grammar_manager] bootstrap: failed to write {}: {e}",
                    dest.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_schema_uses_shared_language_catalog() {
        let schema = tool_schema();
        let description = schema["description"].as_str().expect("description");

        for lang in DOWNLOADABLE_LANGUAGES {
            assert!(description.contains(lang), "description should list {lang}: {description}");
        }

        let action_enum = schema["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert_eq!(action_enum.len(), 2);
        assert!(action_enum.iter().any(|item| item == "status"));
        assert!(action_enum.iter().any(|item| item == "add"));
    }

    #[test]
    fn status_returns_structured_json() {
        let reply = handle_tool_call(&json!({ "action": "status" }), None, &[])
            .expect("status should succeed");
        let payload: Value = serde_json::from_str(&reply).expect("status payload json");

        assert_eq!(payload["status"], "ok");
        assert!(payload["active"].as_array().is_some());
        assert_eq!(payload["core_languages"].as_array().map(|v| v.len()), Some(CORE_LANGUAGES.len()));
        assert_eq!(
            payload["available_to_download"].as_array().map(|v| v.len()),
            Some(DOWNLOADABLE_LANGUAGES.len())
        );
    }

    #[test]
    fn add_requires_languages_array() {
        let err = handle_tool_call(&json!({ "action": "add" }), None, &[])
            .expect_err("missing languages should fail");
        assert!(err.contains("No languages provided"));
    }

    #[test]
    fn add_core_language_succeeds_without_network() {
        let temp = tempfile::tempdir().expect("tempdir");
        let reply = handle_tool_call(
            &json!({ "action": "add", "languages": ["rust"] }),
            Some(temp.path()),
            &[],
        )
        .expect("core language add should succeed");
        let payload: Value = serde_json::from_str(&reply).expect("add payload json");

        assert_eq!(payload["status"], "ok");
        assert!(payload["loaded_languages"].to_string().contains("rust"));
        assert_eq!(payload["failed_languages"].as_array().map(|v| v.len()), Some(0));
    }
}
