//! Unified tool registry entry type for `cortex-mcp`.
//!
//! Every explicitly registered tool in the MCP surface is represented as a
//! single [`CortexTool`] struct that bundles its name, MCP input schema, and
//! execution handler together.
//!
//! # Design
//! Tools are registered once at startup in [`super::build_registry`] into a
//! `HashMap<String, CortexTool>`.  This replaces the fragmented
//! `TOOL_NAMES: &[&str]` const arrays that previously lived in `act.rs`,
//! plus a variety of hidden compatibility shims.
//!
//! AST tools are not stored in this registry. Their schema is served
//! dynamically by `ServerState::tool_list`, and `tools/call` falls back to the
//! AST dispatcher when a name is not found here.

use std::path::PathBuf;

use serde_json::Value;

/// Signature for a stateless, synchronous tool handler.
///
/// * `name`: exact tool name from `tools/call` — lets one function serve a
///   whole family of tools (e.g. all cortex-act tools share `act_handler`).
/// * `args`: the `"arguments"` object from the MCP request.
/// * `workspace_roots`: workspace folders captured by the AST server from MCP
///   `initialize`, used to resolve `[FolderName]/...` paths for ACT tools.
///
/// Returns `Ok(output_text)` or `Err(error_message)`.
pub type SyncFn = fn(
    name: &str,
    args: &Value,
    workspace_roots: &[PathBuf],
    workspace_names: &[String],
) -> Result<String, String>;

// ─── Registry entry ──────────────────────────────────────────────────────────

/// A single unified tool registry entry.
///
/// Registered once at startup into the global `TOOL_REGISTRY`
/// (`HashMap<String, CortexTool>`).  Both schema export (`tools/list`) and
/// request dispatch (`tools/call`) operate purely against this map, with no
/// supplementary `TOOL_NAMES` arrays.
pub struct CortexTool {
    /// Exact string tool name as it appears in MCP `tools/call` requests.
    pub name: String,

    /// Full MCP tool descriptor JSON (the object with `"name"`, `"description"`,
    /// `"inputSchema"` fields) returned by `tools/list`.
    ///
    /// Set to `Value::Null` only for intentionally hidden compatibility paths.
    pub schema: Value,

    /// Describes how to execute this tool.
    pub handler: SyncFn,
}
