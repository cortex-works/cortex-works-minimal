//! # cortex-mcp — Minimal MCP Gateway
//!
//! Single `stdio` JSON-RPC 2.0 server exposing the 13 core Cortex-Works tools.
//! Fully synchronous main loop — no daemon threads, no background services.
//!
//! ## Hot-reload (Seamless Rebirth)
//! The supervisor wrapper re-spawns the worker on exit code 42.
//! `cortex_mcp_hot_reload` triggers this to pick up a newly built binary.

mod tools;

use std::io::{BufRead, Write};

use tracing_subscriber::{EnvFilter, fmt};

fn main() -> anyhow::Result<()> {
    if std::env::var("CORTEX_WORKER_MODE").ok().as_deref() != Some("1") {
        return run_supervisor_loop();
    }
    run_worker_stdio_server()
}

fn run_supervisor_loop() -> anyhow::Result<()> {
    loop {
        let exe = std::env::current_exe()?;
        let mut child = std::process::Command::new(exe)
            .env("CORTEX_WORKER_MODE", "1")
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;

        let status = child.wait()?;
        let code = status.code().unwrap_or(1);

        if code == 42 {
            eprintln!("[cortex-mcp supervisor] Hot-reload requested — restarting worker…");
            continue;
        }
        std::process::exit(code);
    }
}

fn run_worker_stdio_server() -> anyhow::Result<()> {
    // ── Tracing ───────────────────────────────────────────────────────────
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,lance=warn,datafusion=warn")),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_thread_names(true)
        .init();

    // ── cortex-ast state (stateful: tracks workspace root) ────────────────
    let mut ast_state = cortexast::server::ServerState::default();

    tracing::info!(
        "cortex-mcp v{} ready — 13 tools active",
        env!("CARGO_PKG_VERSION")
    );

    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout());

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let msg: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let params = msg
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // ── cortex_mcp_hot_reload — Seamless Rebirth ──────────────────────
        // Write response *before* triggering exit(42) so the agent gets the ack.
        if method == "tools/call"
            && params
                .get("name")
                .and_then(|n| n.as_str())
                .map(|n| n == "cortex_mcp_hot_reload")
                .unwrap_or(false)
        {
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let msg_text = cortex_act::act::hot_reload::run(&args)
                .unwrap_or_else(|e| format!("hot_reload error: {e}"));
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{"type": "text", "text": msg_text}], "isError": false }
            });
            let mut s = serde_json::to_string(&resp)?;
            s.push('\n');
            out.write_all(s.as_bytes())?;
            out.flush()?;
            continue;
        }

        let response = match method {
            // ── initialize ────────────────────────────────────────────────
            "initialize" => {
                ast_state.capture_init_root(&params);
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": { "listChanged": true },
                            "resources": { "subscribe": false, "listChanged": false }
                        },
                        "serverInfo": {
                            "name":    "cortex-mcp",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                })
            }

            // ── tools/list ────────────────────────────────────────────────
            "tools/list" => {
                let all = tools::all_schemas(&ast_state);
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": all }
                })
            }

            // ── tools/call ────────────────────────────────────────────────
            "tools/call" => {
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                tracing::debug!(tool = name, "tools/call");
                tools::dispatch(&mut ast_state, id, &params)
            }

            // ── housekeeping ──────────────────────────────────────────────
            "notifications/initialized" | "notifications/cancelled" => continue,

            "ping" => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} }),

            "resources/list" => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "resources": [] }
            }),

            "prompts/list" => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "prompts": [] }
            }),

            _ => {
                tracing::warn!(method = method, "unknown MCP method");
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Method not found: {method}") }
                })
            }
        };

        let mut s = serde_json::to_string(&response)?;
        s.push('\n');
        out.write_all(s.as_bytes())?;
        out.flush()?;
    }

    tracing::info!("stdin closed — exiting");
    Ok(())
}
