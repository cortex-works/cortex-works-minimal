//! cortex-act tool schemas for the unified tool registry.
//!
//! Schemas and handlers are co-located in [`register_tools`] — no separate
//! `TOOL_NAMES` const array.  `cortex-mcp` remains the single source of
//! truth for the public MCP tool surface.

use std::path::PathBuf;

use serde_json::{Value, json};
use super::registry::CortexTool;

/// Synchronous handler shared by all cortex-act tools.
fn act_handler(
    name: &str,
    args: &Value,
    workspace_roots: &[PathBuf],
    workspace_names: &[String],
) -> Result<String, String> {
    cortex_act::act::dispatch::execute_single(name, args, workspace_roots, workspace_names)
}

/// Return all cortex-act tool registry entries (schema + handler).
///
/// Replaces the old fragmented `pub const TOOL_NAMES` + `pub fn tools()` split.
pub fn register_tools() -> Vec<CortexTool> {
    act_schemas()
        .into_iter()
        .map(|schema| {
            let name = schema["name"].as_str().unwrap_or("").to_string();
            CortexTool {
                handler: act_handler,
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
            "description": "Edit Rust, TypeScript, or Python by symbol name instead of line number. This is a secondary path on the z4-first branch: do not use it for .z4 sources. In z4=true repos prefer cortex_fs_manage for source mutation and cortex_z4_atomic_sync for commit-locked finalization.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Repo-relative, workspace-prefixed (multi-root), or absolute path to the source file." },
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
                    }
                },
                "required": ["file", "edits"]
            }
        }),
        // ── Data Graph Editor ─────────────────────────────────────────────
        json!({
            "name": "cortex_act_edit_data_graph",
            "description": "Structural JSON and YAML edits via JSONPath-like targets. Use this for side-config changes when line-based patches would be brittle. On z4-first branches this is for non-.z4 auxiliary files only.\n\nAction semantics:\n• set    — update an existing key or INSERT a new key (upsert). JSON: works for any depth including top-level ($.newKey). YAML: only updates existing keys; to add a new key to YAML use action=replace on the parent path.\n• replace — same as set for existing keys (preferred alias when the key is known to exist).\n• delete  — remove the target key entirely.\n\nFor TOML rewrites use cortex_fs_manage(action=write) on the whole file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Repo-relative, workspace-prefixed (multi-root), or absolute path to the JSON or YAML file." },
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
            "description": "Structural Markdown, HTML, and XML edits by section or node target. Use this for non-z4 docs or markup when you know the heading, tag, table, or id you want to change and want to avoid brittle text replacement. This is not a normal z4 source-edit path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Repo-relative, workspace-prefixed (multi-root), or absolute path to the markup file." },
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
            "description": "Edit SQL DDL statements such as CREATE TABLE or CREATE INDEX by statement type and object name. Use this for non-z4 schema files where line-based editing is risky or the same token appears multiple times.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Repo-relative, workspace-prefixed (multi-root), or absolute path to the SQL file." },
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
            "description": "Run a bounded shell command synchronously and return its output. On z4=true repos this is the preferred verification entry point: run_diagnostics=true drives the repo's z4 validation flow and surfaces host-binary mismatch hints when z4c is not runnable on the current machine. Use it for quick diagnostics, one-shot builds, or short repo-local commands. Do not use it for watch mode, dev servers, or anything that should keep running.\n\n• PATH is automatically augmented on Unix to include ~/.cargo/bin, ~/.local/bin, and /usr/local/bin — so cargo, node, python3, etc. are available even when launched from an IDE with a reduced PATH.\n• Use run_diagnostics=true to auto-detect the build system (cargo/tsc/go/maven/gradle) and run compiler checks without specifying the command.\n• Use problem_matcher to turn raw error output into structured JSON (supported values: 'cargo', 'tsc', 'eslint', 'go', 'python').\n• On timeout, the process is killed and the partial output is returned with a 'Timed out' prefix.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command":         { "type": "string",  "description": "Shell command to run (required unless run_diagnostics=true). Executed via 'sh -c' on Unix and 'cmd /C' on Windows." },
                    "cwd":             { "type": "string",  "description": "Repo-relative, workspace-prefixed (multi-root), or absolute working directory. Defaults to the primary workspace root." },
                    "timeout_secs":    { "type": "integer", "description": "Hard kill timeout in seconds. Default 30; automatically 60 when run_diagnostics=true.", "default": 30 },
                    "run_diagnostics": { "type": "boolean", "description": "When true, auto-detect manifest in cwd and run the correct compiler check (cargo check, tsc --noEmit, go build, etc.). In z4=true repos this runs the z4 validation flow rooted at ./z4c and build/compiler.filelist. Ignores the command field.", "default": false },
                    "problem_matcher": { "type": "string",  "description": "Named error extractor. Supported: 'cargo', 'tsc', 'eslint', 'go', 'python'. Returns structured JSON errors instead of raw output on failure." }
                }
            }
        }),
        // ── Batch Execute (Meta-Tool) ─────────────────────────────────────
        json!({
            "name": "cortex_act_batch_execute",
            "description": "Execute multiple Cortex tool calls in one round-trip and return a JSON BatchSummary object. Operations run sequentially, not in parallel. On z4-first branches use this for compact inspect-or-verify bundles such as reg_reader -> unit_scan -> symbol_analyzer -> run_diagnostics.\n\n• Supports all 17 active Cortex tools as operation tool_name values. If you include cortex_mcp_hot_reload, make it the LAST operation because it restarts the worker.\n• Nesting cortex_act_batch_execute inside itself is not allowed.\n• Each operation result includes index, tool_name, success, output, output_chars, and truncated.\n• Omit parameters when a tool does not need any; it defaults to an empty object.\n• Use fail_fast=true when later operations depend on earlier ones succeeding.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "operations": {
                        "type": "array",
                        "description": "Ordered list of tool operations to execute sequentially.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool_name":  { "type": "string", "description": "Any active Cortex tool name. Nesting 'cortex_act_batch_execute' is not allowed. If you use 'cortex_mcp_hot_reload', place it last." },
                                "parameters": { "type": "object", "description": "Parameters object for the tool, identical to calling the tool directly. Optional; defaults to {}." }
                            },
                            "required": ["tool_name"]
                        }
                    },
                    "fail_fast": {
                        "type": "boolean",
                        "description": "Stop after the first failing operation and skip the rest. Default false (all operations run regardless).",
                        "default": false
                    },
                    "max_chars_per_op": {
                        "type": "integer",
                        "description": "Maximum output characters per operation before truncation. Default 4000. Increase for operations that return large outputs like z4 map_overview, deep_slice, or build-unit scans.",
                        "default": 4000
                    }
                },
                "required": ["operations"]
            }
        }),
        // ── Exact / Ripgrep-style Search ──────────────────────────────
        json!({
            "name": "cortex_search_exact",
            "description": "Regex search over source files (ripgrep-style, ignore-aware). Returns file paths and 1-based line numbers. This is especially good for z4 phase ids, build-unit ids, hex labels, and opcode mnemonics when you already know the text shape.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "regex_pattern":   { "type": "string",  "description": "Regex pattern to search for (Rust `regex` crate syntax), e.g. 'f16ab1d44[0-9a-f]+' or '0x16ab1d44|compiler.filelist'." },
                    "project_path":    { "type": "string",  "description": "Repo-relative, workspace-prefixed (multi-root), or absolute path to the workspace root to search. Omit to use the primary workspace root." },
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
            "description": "Restart the MCP worker to pick up a newly built binary. The supervisor respawns on the same stdio channel without disconnecting the IDE. On this z4-first branch use it immediately after rebuilding any z4 tool-surface, schema, validation, or diagnostics change, and if you batch it, make it the last operation because it restarts the worker.\n\nTypical workflow after rebuilding:\n1. cargo build --release -p cortex-mcp (in terminal)\n2. cortex_mcp_hot_reload (this tool)\n3. Re-initialize the MCP session if the client requires it\n4. Optionally refresh tools/list to see updated schemas",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Optional reason string for tracing logs." }
                }
            }
        }),
        json!({
            "name": "cortex_z4_atomic_sync",
            "description": "Preferred finalization tool for z4-first work. Stage explicit paths, enforce z4 machine-surface hygiene, run z4 validation, then create a focused git commit using a required hex phase id. Use this after z4 source or catalog edits when you want commit-locked behavior without sweeping unrelated repo changes into the commit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Repo-relative, workspace-prefixed (multi-root), or absolute path inside the target git repository. Defaults to the primary workspace root." },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Explicit files or directories to stage and commit, for example 'parser.z4' or 'build/compiler.filelist'. Only these paths are included in the atomic sync." },
                    "phase_id": { "type": "string", "description": "Required hex phase id for the commit message, e.g. '0x16ab1d44'." },
                    "summary": { "type": "string", "description": "Required short ASCII commit summary appended after the hex phase id." },
                    "purge_untracked": { "type": "boolean", "description": "When true, run git clean -fd limited to the same pathspecs after a successful commit.", "default": false }
                },
                "required": ["paths", "phase_id", "summary"]
            }
        }),
        // ── File System God (write / patch / mkdir / delete / rename / move / copy) ──
        json!({
            "name": "cortex_fs_manage",
            "description": "Write, patch, create, delete, rename, move, or copy files and directories. Use this for physical file operations, not structured source edits. In z4=true repos this is the default mutation path for .z4, .filelist, and .project.z4 changes so z4 project validation can run in the mutation flow.\n\naction=write: create or overwrite a file with raw content. Use for TOML, plain text, or any file type not covered by a structural editor.\naction=patch: update a single key in .env, .ini, or key=value files (not JSON/YAML). Use patch_action=set to write a key, patch_action=delete to remove it.\naction=mkdir: create one or more directories (including parents).\naction=delete: remove files or directories (non-empty dirs included).\naction=rename / move / copy: paths[0]=source, paths[1]=destination.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action":   { "type": "string", "enum": ["write", "patch", "mkdir", "delete", "rename", "move", "copy"], "description": "Operation to perform." },
                    "paths":    { "type": "array", "items": { "type": "string" }, "description": "Repo-relative, workspace-prefixed (multi-root), or absolute paths. For write/patch use paths[0]. For rename/move/copy use paths[0]=source, paths[1]=destination. For delete/mkdir you may pass multiple paths." },
                    "path":     { "type": "string", "description": "Legacy single repo-relative, workspace-prefixed, or absolute path fallback for backward compatibility." },
                    "new_path": { "type": "string", "description": "Legacy destination fallback using a repo-relative, workspace-prefixed, or absolute path." },
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


