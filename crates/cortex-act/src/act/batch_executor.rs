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

pub const DEFAULT_MAX_OUTPUT_CHARS: usize = 4_000;
const OUTPUT_TRUNCATION_SUFFIX: &str = "... [truncated — call this tool individually for full output]";

/// Returns `(possibly-truncated output, was_truncated, raw_char_count)`.
pub fn summarize_output(output: String, max_chars: usize) -> (String, bool, usize) {
    let raw_chars = output.chars().count();
    if raw_chars <= max_chars {
        return (output, false, raw_chars);
    }
    let keep = max_chars.saturating_sub(OUTPUT_TRUNCATION_SUFFIX.chars().count());
    let mut truncated: String = output.chars().take(keep).collect();
    truncated.push_str(OUTPUT_TRUNCATION_SUFFIX);
    (truncated, true, raw_chars)
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
    /// Character count of the raw output before truncation.
    pub output_chars: usize,
    /// `true` when the output was cut to `max_chars_per_op`.
    pub truncated: bool,
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

/// Shared batch loop used by both the standalone act dispatcher and the MCP
/// gateway so behavior cannot drift between the two entry points.
pub fn execute_batch_with<F>(
    operations: Vec<BatchOp>,
    fail_fast: bool,
    max_chars_per_op: usize,
    mut execute: F,
) -> BatchSummary
where
    F: FnMut(&str, &Value) -> Result<String, String>,
{
    let total = operations.len();
    let mut results = Vec::with_capacity(total);

    for (index, op) in operations.into_iter().enumerate() {
        // Reject nested batch calls to prevent recursion.
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

        let outcome = execute(&op.tool_name, &op.parameters);
        let success = outcome.is_ok();
        let raw = outcome.unwrap_or_else(|e| e);
        let (output, truncated, raw_chars) = summarize_output(raw, max_chars_per_op);

        results.push(OpResult {
            index,
            tool_name: op.tool_name,
            success,
            output,
            output_chars: raw_chars,
            truncated,
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

/// Execute all operations sequentially. Never panics; always returns a summary.
pub fn execute_batch(
    operations: Vec<BatchOp>,
    workspace_roots: &[PathBuf],
    workspace_names: &[String],
    fail_fast: bool,
    max_chars_per_op: usize,
) -> BatchSummary {
    execute_batch_with(
        operations,
        fail_fast,
        max_chars_per_op,
        |tool_name, parameters| {
            crate::act::dispatch::execute_single(
                tool_name,
                parameters,
                workspace_roots,
                workspace_names,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_batch_respects_fail_fast() {
        let summary = execute_batch_with(
            vec![
                BatchOp {
                    tool_name: "cortex_act_batch_execute".to_string(),
                    parameters: json!({}),
                },
                BatchOp {
                    tool_name: "cortex_search_exact".to_string(),
                    parameters: json!({}),
                },
            ],
            true,
            DEFAULT_MAX_OUTPUT_CHARS,
            |_, _| Ok("should not run".to_string()),
        );

        assert_eq!(summary.total, 2);
        assert_eq!(summary.passed, 0);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.results.len(), 1);
        assert_eq!(summary.results[0].output, "Nested cortex_act_batch_execute is not allowed");
    }

    #[test]
    fn truncation_preserves_raw_char_count() {
        let summary = execute_batch_with(
            vec![BatchOp {
                tool_name: "cortex_search_exact".to_string(),
                parameters: json!({}),
            }],
            false,
            16,
            |_, _| Ok("abcdefghijklmnopqrstuvwxyz".to_string()),
        );

        assert_eq!(summary.results.len(), 1);
        assert!(summary.results[0].truncated);
        assert_eq!(summary.results[0].output_chars, 26);
        assert!(summary.results[0].output.ends_with("call this tool individually for full output]"));
    }
}
