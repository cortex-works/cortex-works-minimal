//! cortex-act tool schemas for the unified tool registry.
//!
//! Schemas and handlers are co-located in [`register_tools`] — no separate
//! `TOOL_NAMES` const array.  `cortex-mcp` remains the single source of
//! truth for the public MCP tool surface.

use std::path::PathBuf;

use serde_json::{Value, json};
use super::registry::{CortexTool, ToolHandler};

/// Synchronous handler shared by all cortex-act tools.
fn act_handler(
    name: &str,
    args: &Value,
    workspace_roots: &[PathBuf],
    workspace_names: &[String],
) -> Result<String, String> {
    cortex_act::act::dispatch::execute_single(name, args, workspace_roots, workspace_names)
}

/// Return all cortex-act tool registry entries (schema + handler + gate flags).
///
/// Replaces the old fragmented `pub const TOOL_NAMES` + `pub fn tools()` split.
pub fn register_tools() -> Vec<CortexTool> {
    /// Tools that require the mutation gate to be unlocked before executing.
    const MUTATION_TOOLS: &[&str] = &[
        "cortex_act_edit_ast",
        "cortex_act_edit_data_graph",
        "cortex_act_edit_markup",
        "cortex_act_sql_surgery",
        "cortex_fs_manage",
        "cortex_act_shell_exec",
    ];

    act_schemas()
        .into_iter()
        .map(|schema| {
            let name = schema["name"].as_str().unwrap_or("").to_string();
            CortexTool {
                is_rules_reader: false,
                is_mutation: MUTATION_TOOLS.contains(&name.as_str()),
                handler: ToolHandler::Sync(act_handler),
                schema,
                name,
            }
        })
        .collect()
}

/// Full MCP tool schema for all cortex-act tools (private; consumed by [`register_tools`]).
fn act_schemas() -> Vec<Value> {
    vec![
        // ── AST Semantic Patcher ──────────────────────────────────────────
        json!({
            "name": "cortex_act_edit_ast",
            "description": "Edit Rust, TypeScript, or Python by symbol name instead of line number. Replaces or deletes whole symbols such as functions, structs, classes, or methods. Read the current symbol first with cortex_symbol_analyzer(read_source) when precision matters.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Workspace-prefixed path (e.g. [FolderName]/path/to/file) or absolute path to the source file." },
                    "edits": {
                        "type": "array",
                        "description": "Edits to apply (bottom-up order enforced automatically).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": { "type": "string", "description": "Symbol name or 'kind:name' (e.g. 'login' or 'function:login')." },
                                "action": { "type": "string", "enum": ["replace", "delete"], "description": "replace: swap entire symbol body. delete: remove symbol." },
                                "code":   { "type": "string", "description": "Full replacement source (required for replace)." }
                            },
                            "required": ["target", "action"]
                        }
                    },
                    "llm_url": { "type": "string", "description": "Auto-Healer LLM endpoint override. Default: http://127.0.0.1:1234/v1/chat/completions." }
                },
                "required": ["file", "edits"]
            }
        }),
        // ── Data Graph Editor ─────────────────────────────────────────────
        json!({
            "name": "cortex_act_edit_data_graph",
            "description": "Structural JSON and YAML edits via JSONPath-like targets. Preserves surrounding formatting/comments when possible. Use this for config mutations; for TOML rewrites use cortex_fs_manage(write).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Workspace-prefixed path (e.g. [FolderName]/path/to/file) or absolute path to the data file." },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": { "type": "string", "description": "JSONPath-like target (e.g. '$.key[0].nested')." },
                                "action": { "type": "string", "enum": ["set", "delete", "replace"] },
                                "value":  { "type": "string", "description": "New value (required for set/replace)." }
                            },
                            "required": ["target", "action"]
                        }
                    }
                },
                "required": ["file", "edits"]
            }
        }),
        // ── Markup Editor ─────────────────────────────────────────────────
        json!({
            "name": "cortex_act_edit_markup",
            "description": "Structural Markdown, HTML, and XML edits by section or node target. Best when you know the heading/tag/id you need and want to avoid brittle text replacement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Workspace-prefixed path (e.g. [FolderName]/path/to/file) or absolute path to the markup file." },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": { "type": "string", "description": "Type:Value target (e.g. 'heading:Setup', 'table:0', 'tag:div', 'id:app')." },
                                "action": {
                                    "type": "string",
                                    "enum": ["replace", "delete", "insert_before", "insert_after"],
                                    "description": "replace: swap entire node/section. delete: remove node/section. insert_before: inject before target without touching it. insert_after: inject after heading line, preserving section body."
                                },
                                "code": { "type": "string", "description": "Content to insert or replacement content (required for replace/insert_before/insert_after)." }
                            },
                            "required": ["target", "action"]
                        }
                    }
                },
                "required": ["file", "edits"]
            }
        }),
        // ── SQL Surgery ───────────────────────────────────────────────────
        json!({
            "name": "cortex_act_sql_surgery",
            "description": "Edit SQL DDL statements such as CREATE TABLE or CREATE INDEX by statement type and object name. Useful for large schema files where line-based editing is risky.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Workspace-prefixed path (e.g. [FolderName]/path/to/file) or absolute path to the SQL file." },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": { "type": "string", "description": "Type:Name (e.g. 'create_table:users')." },
                                "action": { "type": "string", "enum": ["replace", "delete"] },
                                "code":   { "type": "string", "description": "Full replacement statement (required for replace)." }
                            },
                            "required": ["target", "action"]
                        }
                    }
                },
                "required": ["file", "edits"]
            }
        }),

        // ── Synchronous Shell Exec + optional diagnostics ──────────────
        json!({
            "name": "cortex_act_shell_exec",
            "description": "Run a bounded shell command synchronously and return its output. Use run_diagnostics=true for cargo/tsc/go compiler checks. Do not use this for long-running watch/server processes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command":         { "type": "string",  "description": "Shell command to run (required unless run_diagnostics=true)." },
                    "cwd":             { "type": "string",  "description": "Workspace-prefixed path (e.g. [FolderName]/path/to/dir) or absolute path for the working directory (optional)." },
                    "timeout_secs":    { "type": "integer", "description": "Hard timeout in seconds. Default 30 (60 when run_diagnostics=true).", "default": 30 },
                    "run_diagnostics": { "type": "boolean", "description": "When true, auto-detect manifest and run the correct compiler check. Ignores command field.", "default": false },
                    "problem_matcher": { "type": "string",  "description": "Named error extractor for failed output (e.g. 'cargo', 'tsc', 'eslint'). When set, failures return structured JSON errors instead of raw tail." }
                }
            }
        }),
        // ── Batch Execute (Meta-Tool) ─────────────────────────────────────
        json!({
            "name": "cortex_act_batch_execute",
            "description": "Execute multiple Cortex tool calls in one round-trip. Best for independent reads or a small edit+verify sequence. Runs sequentially, supports optional fail-fast behavior, and truncates oversized per-operation output. Example operations: [{\"tool_name\": \"cortex_act_shell_exec\", \"parameters\": {\"command\": \"cargo check\", \"cwd\": \"[ProjectA]\"}}]",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fail_fast": {
                        "type": "boolean",
                        "description": "When true, stop after the first failing operation. Default false.",
                        "default": false
                    },
                    "operations": {
                        "type": "array",
                        "description": "Ordered list of operations to execute.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool_name":  { "type": "string", "description": "Any active Cortex tool name, e.g. 'cortex_act_edit_ast', 'cortex_code_explorer', or 'cortex_symbol_analyzer'." },
                                "parameters": { "type": "object", "description": "Parameters for the tool." }
                            },
                            "required": ["tool_name", "parameters"]
                        }
                    }
                },
                "required": ["operations"]
            }
        }),
        json!({
            "name": "cortex_semantic_code_search",
            "description": "Concept-based code search over the local semantic index. Use this when you know the intent but not exact filenames or symbol names. If the index is missing or stale, prefer cortex_search_exact or cortex_code_explorer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query":        { "type": "string",  "description": "Natural-language description of the code you are looking for (e.g. 'authentication middleware', 'database connection pool')." },
                    "project_path": { "type": "string",  "description": "Workspace-prefixed path (e.g. [FolderName]) or absolute path to the workspace root. Omit to search across all indexed projects." },
                    "limit":        { "type": "integer", "description": "Max number of symbols to return. Default 5.", "default": 5 },
                    "extract_code":   { "type": "boolean", "description": "When true, reads each matched source file and injects the exact code body using tree-sitter — one-shot RAG that eliminates a follow-up cortex_code_explorer call. Default false.", "default": false },
                    "min_similarity": { "type": "number",  "description": "Minimum cosine similarity threshold [0.0–1.0]. Results below this score are filtered out. Default 0.0.", "default": 0.0 }
                },
                "required": ["query"]
            }
        }),
        // ── Exact / Ripgrep-style Search ──────────────────────────────
        json!({
            "name": "cortex_search_exact",
            "description": "Regex search over source files (ripgrep-style, ignore-aware). Returns file paths and 1-based line numbers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "regex_pattern":   { "type": "string",  "description": "Regex pattern to search for (Rust `regex` crate syntax, e.g. 'fn handle_auth' or 'TODO|FIXME')." },
                    "project_path":    { "type": "string",  "description": "Workspace-prefixed path (e.g. [FolderName]) or absolute path to the workspace root to search. Omit to use the primary workspace root." },
                    "file_extension":  { "type": "string",  "description": "Optional extension filter, e.g. 'rs', 'ts', '.py'. Omit to search all files." },
                    "include_pattern": { "type": "string",  "description": "Glob pattern to restrict which file paths are searched (e.g. 'crates/cortex-act/**' or '*/src/*.rs'). Matched against the full path string." },
                    "max_results":     { "type": "integer", "description": "Max matched lines to return (default 50, hard cap 500). Increase when searching large codebases.", "default": 50 }
                },
                "required": ["regex_pattern"]
            }
        }),
        // cortex_learn_rule — unregistered (ultra-lean mode)
        // ── Write File (removed — using cortex_fs_manage action=write) ───
        // ── Seamless Rebirth: MCP Worker Hot Reload ───────────────────────
        json!({
            "name": "cortex_mcp_hot_reload",
            "description": "Restart the MCP worker to load a newly built binary. The supervisor respawns the worker on the same stdio channel; after restart, the client should re-initialize and refresh tools/list if it needs updated schema.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Optional reason string for tracing logs." }
                }
            }
        }),
        // ── File System God (write / patch / mkdir / delete / rename / move / copy) ──
        json!({
            "name": "cortex_fs_manage",
            "description": "Write, patch, create, delete, rename, move, or copy files/directories. Use this for physical filesystem changes, not structural code edits. In the minimal branch it does not promise automatic semantic re-indexing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action":   { "type": "string", "enum": ["write", "patch", "mkdir", "delete", "rename", "move", "copy"], "description": "Operation to perform." },
                    "paths":    { "type": "array", "items": { "type": "string" }, "description": "Workspace-prefixed paths (e.g. [FolderName]/path/to/file) or absolute paths. For write/patch use paths[0]. For rename/move/copy use paths[0]=source, paths[1]=destination. For delete/mkdir you may pass multiple paths." },
                    "path":     { "type": "string", "description": "Legacy single workspace-prefixed or absolute path fallback for backward compatibility." },
                    "new_path": { "type": "string", "description": "Legacy destination fallback using a workspace-prefixed or absolute path." },
                    "content":  { "type": "string", "description": "File content to write (required for write)." },
                    "patch_action": { "type": "string", "enum": ["set", "delete"], "description": "Sub-action for action=patch. Defaults to set. Use this instead of overloading the top-level action field." },
                    "type":     { "type": "string", "enum": ["env", "ini", "kv"], "description": "Key-value file format (required for patch)." },
                    "target":   { "type": "string", "description": "Key name to modify (required for patch)." },
                    "value":    { "description": "New value (required for patch action=set)." }
                },
                "required": ["action"]
            }
        }),
    ]
}


