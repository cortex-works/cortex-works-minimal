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
fn ast_execute(ast_state: &mut ServerState, name: &str, args: &Value) -> Result<String, String> {
    let params = json!({ "name": name, "arguments": args });
    let resp = ast_state.tool_call(Value::Null, &params);

    if let Some(msg) = resp
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Err(msg.to_string());
    }

    let text = resp["result"]["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|c| c["text"].as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            serde_json::to_string(&resp["result"]).unwrap_or_default()
        });

    if resp["result"]["isError"].as_bool().unwrap_or(false) {
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
            ToolHandler::Sync(f) => f(name, args, ast_state.workspace_roots()),
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
        let mut results: Vec<Value> = Vec::with_capacity(ops.len());
        for op in &ops {
            let tname = op.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let targs = op.get("parameters").cloned().unwrap_or_else(|| json!({}));
            let r = execute_tool(ast_state, tname, &targs);
            results.push(
                json!({ "tool": tname, "ok": r.is_ok(), "output": r.unwrap_or_else(|e| e) }),
            );
        }
        return ok(serde_json::to_string(&results).unwrap_or_default());
    }

    // ── All other tools ───────────────────────────────────────────────────
    make_rpc(id, execute_tool(ast_state, name, &args))
}
