//! # Batch Executor — CortexACT
//!
//! Executes an ordered array of tool operations in a single MCP call.
//! Reduces multi-turn round-trips when an agent needs to make several
//! independent edits (e.g. "patch 3 files + run a shell check").
//!
//! ## Design
//!
//! * Operations run **sequentially** (not in parallel) to preserve the
//!   semantic ordering agents rely on (edit A before validate with B).
//! * A **failing** operation writes an `"error"` entry and continues by
//!   default, or aborts the remainder when `fail_fast=true`.
//! * Nested `cortex_act_batch_execute` calls are rejected to prevent
//!   unbounded recursion.
//! * Each operation result includes `index`, `tool_name`, `success`,
//!   and `output` for easy agent parsing.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

const MAX_OUTPUT_CHARS: usize = 4_000;
const OUTPUT_TRUNCATION_SUFFIX: &str = "... [Output truncated to save tokens. Run this tool individually if you need the full output]";

fn truncate_output(output: String) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output;
    }

    let keep = MAX_OUTPUT_CHARS.saturating_sub(OUTPUT_TRUNCATION_SUFFIX.chars().count());
    let mut truncated: String = output.chars().take(keep).collect();
    truncated.push_str(OUTPUT_TRUNCATION_SUFFIX);
    truncated
}

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// A single operation passed inside the `operations` array.
#[derive(Debug, Deserialize)]
pub struct BatchOp {
    /// The tool to invoke (e.g. `"cortex_act_edit_ast"`).
    pub tool_name: String,
    /// Arguments forwarded verbatim to the tool.
    pub parameters: Value,
}

/// Per-operation result included in the batch summary.
#[derive(Debug, Serialize)]
pub struct OpResult {
    /// Zero-based position in the input array.
    pub index: usize,
    /// Tool that was invoked.
    pub tool_name: String,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Tool output (success text) or error message.
    pub output: String,
}

/// Summary returned to the MCP caller.
#[derive(Debug, Serialize)]
pub struct BatchSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    /// Operations not executed because `fail_fast=true` stopped the run early.
    pub skipped: usize,
    pub results: Vec<OpResult>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Execute all operations sequentially. Never panics; always returns a summary.
pub fn execute_batch(
    operations: Vec<BatchOp>,
    workspace_roots: &[PathBuf],
    workspace_names: &[String],
    fail_fast: bool,
) -> BatchSummary {
    let total = operations.len();
    let mut results = Vec::with_capacity(total);

    for (index, op) in operations.into_iter().enumerate() {
        // Reject nested batch calls to prevent recursion.
        if op.tool_name == "cortex_act_batch_execute" {
            results.push(OpResult {
                index,
                tool_name: op.tool_name,
                success: false,
                output: "Nested cortex_act_batch_execute is not allowed".to_string(),
            });
            continue;
        }

        let outcome = crate::act::dispatch::execute_single(
            &op.tool_name,
            &op.parameters,
            workspace_roots,
            workspace_names,
        );
        let success = outcome.is_ok();
        let output = truncate_output(outcome.unwrap_or_else(|e| e));

        results.push(OpResult {
            index,
            tool_name: op.tool_name,
            success,
            output,
        });

        if fail_fast && !success {
            break;
        }
    }

    let passed = results.iter().filter(|r| r.success).count();
    let failed = results.iter().filter(|r| !r.success).count();
    let skipped = total - results.len();

    BatchSummary {
        total,
        passed,
        failed,
        skipped,
        results,
    }
}
