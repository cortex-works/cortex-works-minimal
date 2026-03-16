//! # Central Tool Dispatch — CortexACT
//!
//! Single entry-point `execute_single(tool_name, args, workspace_roots)` that dispatches to
//! every registered tool.  Returns `Ok(success_text)` / `Err(error_message)`.
//!
//! Used by:
//!   * `main.rs` – wraps result in a JSON-RPC envelope
//!   * `batch_executor.rs` – calls in a loop, collects per-op results

use serde_json::{Value, json};
use std::path::PathBuf;

/// Dispatch a single tool call.
///
/// # Returns
/// * `Ok(text)` – tool succeeded; `text` is the human/agent-readable output.
/// * `Err(msg)` – tool failed; `msg` is a human-readable error string.
pub fn execute_single(name: &str, args: &Value, workspace_roots: &[PathBuf]) -> Result<String, String> {
    // ── Convenience helpers ───────────────────────────────────────────────
    macro_rules! req_str {
        ($field:expr) => {
            args.get($field)
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("'{}' required", $field))?
        };
    }
    macro_rules! req_arr {
        ($field:expr) => {
            args.get($field)
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("'{}' array required", $field))?
                .clone()
        };
    }

    match name {
        // ── AST Semantic Patcher ──────────────────────────────────────────
        "cortex_act_edit_ast" => {
            let file_str = req_str!("file");
            let edits_val = req_arr!("edits");
            let llm_url = args
                .get("llm_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let file_path = crate::act::pathing::resolve_path(workspace_roots, file_str);

            let mut edits = Vec::new();
            for item in &edits_val {
                let target = item
                    .get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let action = item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("replace")
                    .to_string();
                let code = item
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if target.is_empty() {
                    return Err("Each edit must have a 'target'".to_string());
                }
                edits.push(crate::act::editor::AstEdit {
                    target,
                    action,
                    code,
                });
            }

            crate::act::editor::apply_ast_edits(&file_path, edits, llm_url.as_deref())
                .map(|result| {
                    let preview: String = result.chars().take(500).collect();
                    serde_json::to_string(&json!({
                        "status":  "ok",
                        "message": format!("Applied {} edit(s) to {}", edits_val.len(), file_str),
                        "preview": preview,
                    }))
                    .unwrap_or_default()
                })
                .map_err(|e| format!("cortex_act_edit_ast failed: {}", e))
        }

        // ── Data Graph Editor (JSON / YAML / TOML) ─────────────────────────
        "cortex_act_edit_data_graph" => {
            let file_str = req_str!("file");
            let edits_val = req_arr!("edits");
            let file_path = crate::act::pathing::resolve_path(workspace_roots, file_str);

            let mut edits = Vec::new();
            for item in &edits_val {
                let target = item
                    .get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let action = item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("set")
                    .to_string();
                let value = item
                    .get("value")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if target.is_empty() {
                    return Err("Each edit must have a 'target'".to_string());
                }
                edits.push(crate::act::data_editor::DataEdit {
                    target,
                    action,
                    value,
                });
            }

            crate::act::data_editor::apply_data_edits(&file_path, edits)
                .map(|_| format!("Successfully patched {}", file_str))
                .map_err(|e| format!("cortex_act_edit_data_graph failed: {}", e))
        }

        // ── Markup Editor (Markdown / HTML / XML) ─────────────────────────
        "cortex_act_edit_markup" => {
            let file_str = req_str!("file");
            let edits_val = req_arr!("edits");
            let file_path = crate::act::pathing::resolve_path(workspace_roots, file_str);

            let mut edits = Vec::new();
            for item in &edits_val {
                let target = item
                    .get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let action = item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("replace")
                    .to_string();
                let code = item
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if target.is_empty() {
                    return Err("Each edit must have a 'target'".to_string());
                }
                edits.push(crate::act::markup_editor::MarkupEdit {
                    target,
                    action,
                    code,
                });
            }

            crate::act::markup_editor::apply_markup_edits(&file_path, edits)
                .map(|_| format!("Successfully patched {}", file_str))
                .map_err(|e| format!("cortex_act_edit_markup failed: {}", e))
        }

        // ── SQL DDL Surgery ────────────────────────────────────────────────
        "cortex_act_sql_surgery" => {
            let file_str = req_str!("file");
            let edits_val = req_arr!("edits");
            let file_path = crate::act::pathing::resolve_path(workspace_roots, file_str);

            let mut edits = Vec::new();
            for item in &edits_val {
                let target = item
                    .get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let action = item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("replace")
                    .to_string();
                let code = item
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if target.is_empty() {
                    return Err("Each edit must have a 'target'".to_string());
                }
                edits.push(crate::act::sql_editor::SqlEdit {
                    target,
                    action,
                    code,
                });
            }

            crate::act::sql_editor::apply_sql_surgery(&file_path, edits)
                .map(|_| format!("Successfully patched {}", file_str))
                .map_err(|e| format!("cortex_act_sql_surgery failed: {}", e))
        }

        // ── Synchronous Shell Exec (+ optional manifest-aware diagnostics) ───
        "cortex_act_shell_exec" => {
            let run_diag = args.get("run_diagnostics").and_then(|v| v.as_bool()).unwrap_or(false);
            let cwd_opt  = args.get("cwd").and_then(|v| v.as_str());
            let problem_matcher = args.get("problem_matcher").and_then(|v| v.as_str());
            let resolved_cwd = cwd_opt
                .map(|cwd| crate::act::pathing::resolve_path_string(workspace_roots, cwd));
            let default_cwd = crate::act::pathing::primary_root(workspace_roots)
                .to_string_lossy()
                .into_owned();

            let (command, effective_cwd, timeout_secs): (String, Option<String>, u64) = if run_diag {
                let root = resolved_cwd.as_deref().unwrap_or(default_cwd.as_str());
                let base = std::path::Path::new(root);
                let cmd = if base.join("Cargo.toml").exists() {
                    "cargo check 2>&1".to_string()
                } else if base.join("package.json").exists() {
                    "npx tsc --noEmit 2>&1".to_string()
                } else if base.join("go.mod").exists() {
                    "go build ./... 2>&1".to_string()
                } else if base.join("pom.xml").exists() {
                    if base.join("mvnw").exists() {
                        "./mvnw compile -q 2>&1".to_string()
                    } else {
                        "mvn compile -q 2>&1".to_string()
                    }
                } else if base.join("build.gradle").exists() || base.join("build.gradle.kts").exists() {
                    if base.join("gradlew").exists() {
                        "./gradlew assemble -q 2>&1".to_string()
                    } else {
                        "gradle assemble -q 2>&1".to_string()
                    }
                } else {
                    return Err(format!(
                        "No supported manifest found in '{}'. \
                         run_diagnostics supports: Cargo.toml, package.json, go.mod, pom.xml, build.gradle.",
                        root
                    ));
                };
                let timeout = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(60);
                (cmd, Some(root.to_string()), timeout)
            } else {
                let cmd = args.get("command").and_then(|v| v.as_str())
                    .ok_or_else(|| "'command' required (or set run_diagnostics: true)".to_string())?
                    .to_string();
                let timeout = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(30);
                (cmd, resolved_cwd, timeout)
            };

            crate::act::shell_exec::run_sync_with_problem_matcher(
                &command,
                effective_cwd.as_deref(),
                timeout_secs,
                problem_matcher,
            )
                .map_err(|e| format!("cortex_act_shell_exec failed: {}", e))
        }

        // ── Search / query helpers ────────────────────────────────────────
        "cortex_semantic_code_search" => crate::act::semantic_search::run(args, workspace_roots)
            .map_err(|e| format!("cortex_semantic_code_search failed: {e}")),

        "cortex_search_exact" => crate::act::search_exact::run(args, workspace_roots)
            .map_err(|e| format!("cortex_search_exact failed: {e}")),

        "cortex_mcp_hot_reload" => crate::act::hot_reload::run(args)
            .map_err(|e| format!("cortex_mcp_hot_reload failed: {e}")),

        // ── Safe FS operations ─────────────────────────────────
        "cortex_fs_manage" => crate::act::fs_manage::run(args, workspace_roots)
            .map_err(|e| format!("cortex_fs_manage failed: {e}")),

        other => Err(format!(
            "Unknown tool: '{}'. Available: cortex_act_edit_ast, cortex_fs_manage, \
             cortex_act_edit_data_graph, cortex_act_edit_markup, cortex_act_sql_surgery, \
             cortex_act_shell_exec, cortex_act_batch_execute, \
             cortex_semantic_code_search, cortex_search_exact, cortex_mcp_hot_reload",
            other
        )),
    }
}
