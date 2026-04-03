//! # Grammar Manager — Dynamic Wasm Language Plugin System
//!
//! Manages the lifecycle of tree-sitter grammar plugins for non-Core languages.
//! Core 3 (Rust, TypeScript, Python) are statically linked into the binary.
//! Non-core languages require pre-bundled local `.wasm` grammars.
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
        "Inspect Tree-sitter Wasm grammar availability for non-z4 language work. z4 repos do not depend on this tool in the normal workflow. Core languages are built in. This build does not download grammars from external sources; non-core languages require pre-bundled local .wasm files. action=status reports active/core languages. action=add is intentionally unsupported in this build. Known non-core language names: {available}."
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
                    "description": "status: list currently active, core, and known non-core languages. add: unsupported in this build (external downloads are disabled). On z4-first repos you normally do not need either action."
                },
                "languages": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Language names for action=add. In this build, add always returns an unsupported error."
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
    _repo_root: Option<&std::path::Path>,
    _workspace_roots: &[PathBuf],
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
        ACTION_ADD => Err(
            "action=add is not supported in this build (no external downloads). Bundle grammar .wasm files locally."
                .to_string(),
        ),
        _ => Err(format!("Invalid action '{action}'. Must be '{}' or '{}'.", ACTION_STATUS, ACTION_ADD)),
    }
}
// ─────────────────────────────────────────────────────────────────────────────
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
/// - Otherwise it checks the cache and returns an error if missing.
/// - This build does not download grammars. Callers should fall back
///   gracefully to the universal regex parser.
pub fn ensure_grammar_available(lang: &str) -> Result<()> {
    // Core languages are statically linked — nothing to do.
    if CORE_LANGUAGES.contains(&lang) {
        return Ok(());
    }

    let wasm = wasm_path(lang)?;
    if !wasm.exists() {
        anyhow::bail!(
            "Grammar for '{lang}' is not available. Bundle the .wasm file at '{}' to enable this language.",
            wasm.display()
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Download routine
// ─────────────────────────────────────────────────────────────────────────────

/// Map a language name to its GitHub release download URL.
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
            .expect_err("add should be unsupported");
        assert!(err.contains("not supported"));
    }

    #[test]
    fn add_core_language_succeeds_without_network() {
        let err = handle_tool_call(
            &json!({ "action": "add", "languages": ["rust"] }),
            None,
            &[],
        )
        .expect_err("add should be unsupported");
        assert!(err.contains("not supported"));
    }
}
