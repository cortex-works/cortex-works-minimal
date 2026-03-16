//! # cortex-act — library surface
//!
//! Exposes the full `act` module tree so `cortex-mcp` (and tests) can call
//! `cortex_act::act::dispatch::execute_single(name, args, workspace_roots, workspace_names)` directly without
//! going through the stdio MCP loop.
//!
pub mod act;

use serde::{Deserialize, Serialize};

/// Public parameter model for shell execution requests.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellExecParams {
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
    pub run_diagnostics: Option<bool>,
    pub problem_matcher: Option<String>,
}

// ─── Semantic index hooks ────────────────────────────────────────────────────
//
// The minimal branch keeps filesystem mutations independent from any
// background indexer process. These hooks stay as no-ops so fs_manage does not
// need target-specific branching.

/// No-op in the minimal branch.
#[inline]
pub fn fire_tombstone(_project_path: String, _file_path: String) {}

/// No-op in the minimal branch.
#[inline]
pub fn fire_index_modified(_project_path: String, _file_path: String) {}
