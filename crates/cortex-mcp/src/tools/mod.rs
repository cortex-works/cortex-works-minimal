//! Tool registry and dispatcher for `cortex-mcp`.
//!
//! ## Key entry points
//! * [`all_schemas`]  — merged schema list (AST tools + registered act tools).
//! * [`execute_tool`] — routes a call to the registered handler or AST fallback.
//! * [`dispatch`]     — wraps `execute_tool` in a JSON-RPC 2.0 response.

pub mod act;
pub mod registry;

use std::collections::HashMap;
use std::sync::OnceLock;

use cortexast::server::ServerState;
use registry::{CortexTool, ToolHandler};
use serde_json::{Value, json};

#[allow(dead_code)]
pub fn mark_rules_read() {}

use cortex_act::act::batch_executor::DEFAULT_MAX_OUTPUT_CHARS;

// ─── Global registry ─────────────────────────────────────────────────────────

static TOOL_REGISTRY: OnceLock<(HashMap<String, CortexTool>, Vec<String>)> = OnceLock::new();

fn get_registry() -> &'static (HashMap<String, CortexTool>, Vec<String>) {
    TOOL_REGISTRY.get_or_init(build_registry)
}

fn build_registry() -> (HashMap<String, CortexTool>, Vec<String>) {
    let mut map: HashMap<String, CortexTool> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for tool in act::register_tools() {
        order.push(tool.name.clone());
        map.insert(tool.name.clone(), tool);
    }

    (map, order)
}

// ─── Inner helpers ────────────────────────────────────────────────────────────

/// Call an AST tool via `ServerState::tool_call` and unwrap the content text.
fn rpc_text(resp: &Value) -> String {
    resp.get("result")
        .and_then(|result| result.get("content"))
        .and_then(|content| content.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .or_else(|| {
            resp.get("error")
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| serde_json::to_string(resp).unwrap_or_default())
}

/// Returns `(possibly-truncated output, was_truncated, raw_char_count)`.
fn truncate_batch_output(output: String, max_chars: usize) -> (String, bool, usize) {
    let raw_chars = output.chars().count();
    let suffix = "... [truncated — call this tool individually for full output]";
    if raw_chars <= max_chars {
        return (output, false, raw_chars);
    }
    let keep = max_chars.saturating_sub(suffix.chars().count());
    let mut truncated: String = output.chars().take(keep).collect();
    truncated.push_str(suffix);
    (truncated, true, raw_chars)
}

fn ast_execute(ast_state: &mut ServerState, name: &str, args: &Value) -> Result<String, String> {
    let params = json!({ "name": name, "arguments": args });
    let resp = ast_state.tool_call(Value::Null, &params);

    tracing::debug!(tool = name, response = ?resp, "ast tool response");

    if let Some(msg) = resp
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        tracing::error!(tool = name, message = %msg, response = ?resp, "ast tool rpc error");
        return Err(msg.to_string());
    }

    let text = rpc_text(&resp);

    if resp["result"]["isError"].as_bool().unwrap_or(false) {
        tracing::error!(tool = name, output = %text, response = ?resp, "ast tool returned error result");
        Err(text)
    } else {
        Ok(text)
    }
}

/// Wrap a `Result<String, String>` into a JSON-RPC 2.0 response object.
#[inline]
fn make_rpc(id: Value, result: Result<String, String>) -> Value {
    match result {
        Ok(t) => json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "content": [{ "type":"text","text": t }], "isError": false }
        }),
        Err(e) => json!({
            "jsonrpc": "2.0", "id": id,
            "result": { "content": [{ "type":"text","text": e }], "isError": true }
        }),
    }
}

// ─── Combined tools/list ──────────────────────────────────────────────────────

/// Return the merged tool schema array for all tools.
///
/// Order: `cortex-ast` (dynamic, from `ServerState`) first, then every
/// registered tool in declaration order (act → mesh → scout).
/// AST tools that are also explicitly registered are served from the registry
/// only (no duplication).
pub fn all_schemas(ast_state: &ServerState) -> Vec<Value> {
    let (registry, order) = get_registry();

    // AST tools first — from ServerState; de-duplicate against registry.
    let ast_resp = ast_state.tool_list(Value::Null);
    let mut all: Vec<Value> = ast_resp["result"]["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|s| {
                    !registry.contains_key(s["name"].as_str().unwrap_or(""))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    // Registered tools in declaration order; skip unlisted (Null schema) shims.
    for name in order {
        if let Some(tool) = registry.get(name) {
            if !tool.schema.is_null() {
                all.push(tool.schema.clone());
            }
        }
    }

    all
}

// ─── Inner result dispatch ────────────────────────────────────────────────────

/// Execute any tool and return `Ok(text)` / `Err(message)`.
///
/// Resolution order:
/// 1. Registry hit → handler dispatch.
/// 2. AST fallthrough → `ast_execute`.
pub fn execute_tool(
    ast_state: &mut ServerState,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let (registry, _) = get_registry();

    if let Some(tool) = registry.get(name) {
        return match &tool.handler {
            ToolHandler::Sync(f) => f(
                name,
                args,
                ast_state.workspace_roots(),
                ast_state.workspace_root_names(),
            ),
            ToolHandler::Ast    => ast_execute(ast_state, name, args),
        };
    }

    // AST tool fallthrough — any unregistered name goes to cortex-ast.
    ast_execute(ast_state, name, args)
}



// ─── Top-level MCP dispatcher ─────────────────────────────────────────────────

/// Route a `tools/call` MCP request to the handler and return a JSON-RPC 2.0 response.
///
/// `cortex_act_batch_execute` is handled inline (needs `&mut ast_state`).
/// All other requests go through `execute_tool` → `make_rpc`.
pub fn dispatch(ast_state: &mut ServerState, id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // ── cortex_act_batch_execute ──────────────────────────────────────────
    if name == "cortex_act_batch_execute" {
        use cortex_act::act::batch_executor::{BatchOp, BatchSummary, OpResult};

        let id2 = id.clone();
        let ok = move |t: String| {
            json!({ "jsonrpc":"2.0","id":id,  "result":{"content":[{"type":"text","text":t}],"isError":false} })
        };
        let err = move |e: String| {
            json!({ "jsonrpc":"2.0","id":id2, "result":{"content":[{"type":"text","text":e}],"isError":true} })
        };
        let ops = match args.get("operations").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => return err("'operations' array required".into()),
        };
        let fail_fast = args
            .get("fail_fast")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_chars = args
            .get("max_chars_per_op")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS);
        let mut operations: Vec<BatchOp> = Vec::with_capacity(ops.len());
        for op in &ops {
            let tname = op.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            if tname.is_empty() {
                return err("Each operation must have a 'tool_name'".into());
            }
            operations.push(BatchOp {
                tool_name: tname.to_string(),
                parameters: op.get("parameters").cloned().unwrap_or_else(|| json!({})),
            });
        }
        let total = operations.len();
        let mut results = Vec::with_capacity(total);

        for (index, op) in operations.into_iter().enumerate() {
            if op.tool_name == "cortex_act_batch_execute" {
                let msg = "Nested cortex_act_batch_execute is not allowed".to_string();
                let chars = msg.chars().count();
                results.push(OpResult {
                    index,
                    tool_name: op.tool_name,
                    success: false,
                    output: msg,
                    output_chars: chars,
                    truncated: false,
                });
                if fail_fast {
                    break;
                }
                continue;
            }

            let outcome = execute_tool(ast_state, &op.tool_name, &op.parameters);
            let success = outcome.is_ok();
            let (output, truncated, output_chars) =
                truncate_batch_output(outcome.unwrap_or_else(|e| e), max_chars);

            results.push(OpResult {
                index,
                tool_name: op.tool_name,
                success,
                output,
                output_chars,
                truncated,
            });

            if fail_fast && !success {
                break;
            }
        }

        let passed = results.iter().filter(|result| result.success).count();
        let failed = results.iter().filter(|result| !result.success).count();
        let skipped = total - results.len();
        let summary = BatchSummary {
            total,
            passed,
            failed,
            skipped,
            results,
        };
        return ok(serde_json::to_string(&summary).unwrap_or_default());
    }

    // ── All other tools ───────────────────────────────────────────────────
    make_rpc(id, execute_tool(ast_state, name, &args))
}
