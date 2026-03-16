//! cortex_mcp_hot_reload — Seamless Rebirth step 1: exit(42) for supervisor restart.
//!
//! ## Seamless Rebirth Protocol
//!
//! 1. Agent builds new release binary (`cargo build --release -p cortex-mcp`).
//! 2. Agent calls `cortex_mcp_hot_reload`.
//! 3. `main.rs` writes the success JSON-RPC response and flushes stdout.
//! 4. This function exits the worker with code **42** after a short delay.
//! 5. The supervisor process detects exit(42) and restarts the worker.
//! 6. The client should call `initialize` again and then refresh `tools/list`
//!    if it needs to re-read the schema from the restarted worker.
//! 7. The IDE stays on the same stdio channel because the supervisor process survives.

use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

/// Schedule process exit with code 42 so the supervisor can restart the worker
/// with the newly compiled binary.  The 500 ms delay lets the success response
/// that `main.rs` already wrote to stdout finish flushing before termination.
pub fn run(args: &Value) -> Result<String> {
    let reason = args
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("no reason provided")
        .to_string();

    tracing::info!(
        reason,
        "cortex_mcp_hot_reload: Seamless Rebirth — worker exits with code 42 in 500ms"
    );

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        std::process::exit(42);
    });

    Ok("Seamless Rebirth: worker will exit(42) in ~500ms. Supervisor will restart with the new binary on the same stdio channel. After restart, re-initialize the MCP session and refresh tools/list if you need fresh schema data.".to_string())
}
