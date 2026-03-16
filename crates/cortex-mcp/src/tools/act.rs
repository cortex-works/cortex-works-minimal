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
            "description": "Structural JSON and YAML edits via JSONPath-like targets. Preserves surrounding formatting/comments when possible.\n\nAction semantics:\n• set    — update an existing key or INSERT a new key (upsert). JSON: works for any depth including top-level ($.newKey). YAML: only updates existing keys; to add a new key to YAML use action=replace on the parent path.\n• replace — same as set for existing keys (preferred alias when the key is known to exist).\n• delete  — remove the target key entirely.\n\nFor TOML rewrites use cortex_fs_manage(action=write) on the whole file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Workspace-prefixed path (e.g. [FolderName]/path/to/file) or absolute path to the JSON or YAML file." },
                    "edits": {
                        "type": "array",
                        "description": "Ordered list of edits to apply.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": { "type": "string", "description": "JSONPath-like dot-notation target (e.g. '$.key', '$.section.sub', '$.array[0]'). Use '$' alone to address the root object." },
                                "action": { "type": "string", "enum": ["set", "replace", "delete"], "description": "set/replace: write a value (set can upsert missing keys in JSON). delete: remove the key." },
                                "value":  { "type": "string", "description": "New value as a JSON-compatible string (required for set/replace). Booleans, numbers, and objects are accepted as strings and will be coerced." }
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
            "description": "Run a bounded shell command synchronously and return its output. Do not use this for long-running watch/server processes.\n\n• PATH is automatically augmented on Unix to include ~/.cargo/bin, ~/.local/bin, and /usr/local/bin — so cargo, node, python3, etc. are available even when launched from an IDE with a reduced PATH.\n• Use run_diagnostics=true to auto-detect the build system (cargo/tsc/go/maven/gradle) and run compiler checks without specifying the command.\n• Use problem_matcher to turn raw error output into structured JSON (supported values: 'cargo', 'tsc', 'eslint', 'go', 'python').\n• On timeout, the process is killed and the partial output is returned with a 'Timed out' prefix.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command":         { "type": "string",  "description": "Shell command to run (required unless run_diagnostics=true). Executed via 'sh -c' on Unix and 'cmd /C' on Windows." },
                    "cwd":             { "type": "string",  "description": "Workspace-prefixed path (e.g. [FolderName]) or absolute path for the working directory. Defaults to the primary workspace root." },
                    "timeout_secs":    { "type": "integer", "description": "Hard kill timeout in seconds. Default 30; automatically 60 when run_diagnostics=true.", "default": 30 },
                    "run_diagnostics": { "type": "boolean", "description": "When true, auto-detect manifest in cwd and run the correct compiler check (cargo check, tsc --noEmit, go build, etc.). Ignores the command field.", "default": false },
                    "problem_matcher": { "type": "string",  "description": "Named error extractor. Supported: 'cargo', 'tsc', 'eslint', 'go', 'python'. Returns structured JSON errors instead of raw output on failure." }
                }
            }
        }),
        // ── Batch Execute (Meta-Tool) ─────────────────────────────────────
        json!({
            "name": "cortex_act_batch_execute",
            "description": "Execute multiple Cortex tool calls in one round-trip. Runs sequentially (not in parallel). Best for independent reads, edit+verify pairs, or any sequence of ≤10 operations.\n\n• Supports all 14 active Cortex tools as operation tool_name values.\n• Nesting cortex_act_batch_execute inside itself is NOT allowed.\n• Each operation's output is independently truncated to max_chars_per_op.\n• Results include per-operation success/failure, output, output_chars, and truncated flag.\n• Use fail_fast=true when later operations depend on earlier ones succeeding.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "operations": {
                        "type": "array",
                        "description": "Ordered list of tool operations to execute sequentially.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool_name":  { "type": "string", "description": "Any active Cortex tool name (e.g. 'cortex_act_edit_ast', 'cortex_code_explorer', 'cortex_act_shell_exec'). Nesting 'cortex_act_batch_execute' is not allowed." },
                                "parameters": { "type": "object", "description": "Parameters object for the tool, identical to calling the tool directly." }
                            },
                            "required": ["tool_name", "parameters"]
                        }
                    },
                    "fail_fast": {
                        "type": "boolean",
                        "description": "Stop after the first failing operation and skip the rest. Default false (all operations run regardless).",
                        "default": false
                    },
                    "max_chars_per_op": {
                        "type": "integer",
                        "description": "Maximum output characters per operation before truncation. Default 4000. Increase for operations that return large outputs like map_overview or deep_slice.",
                        "default": 4000
                    }
                },
                "required": ["operations"]
            }
        }),
        json!({
            "name": "cortex_semantic_code_search",
            "description": "Concept-based code search over the local semantic index. Use this when you know the intent but not the exact symbol name or filename (e.g. 'database connection pool', 'auth middleware'). If the index is missing, stale, or returns no results, fall back immediately to cortex_search_exact or cortex_code_explorer — do not assume the code does not exist.",
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
            "description": "Restart the MCP worker to pick up a newly built binary. The supervisor respawns on the same stdio channel without disconnecting the IDE.\n\nTypical workflow after rebuilding:\n1. cargo build --release -p cortex-mcp (in terminal)\n2. cortex_mcp_hot_reload (this tool)\n3. Re-initialize the MCP session if the client requires it\n4. Optionally refresh tools/list to see updated schemas",
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
            "description": "Write, patch, create, delete, rename, move, or copy files and directories. Use this for physical file operations — not for structured code edits (use cortex_act_edit_ast / cortex_act_edit_data_graph for those).\n\naction=write: create or overwrite a file with raw content. Use for TOML, plain text, or any file type not covered by a structural editor.\naction=patch: update a single key in .env, .ini, or key=value files (not JSON/YAML). Use patch_action=set to write a key, patch_action=delete to remove it.\naction=mkdir: create one or more directories (including parents).\naction=delete: remove files or directories (non-empty dirs included).\naction=rename / move / copy: paths[0]=source, paths[1]=destination.",
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


