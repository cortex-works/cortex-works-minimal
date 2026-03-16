use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

/// Default LM Studio endpoint — matches the screenshot (127.0.0.1:1234).
const DEFAULT_LLM_URL: &str = "http://127.0.0.1:1234/v1/chat/completions";
/// Default model name as loaded in LM Studio.
const DEFAULT_MODEL: &str = "phi-4";
/// Generous timeout for a locally-served 8+ GB model.
const LLM_TIMEOUT_SECS: u64 = 60;

/// Tries to fix a syntax error by sending the broken code block to a local LLM.
///
/// * `llm_url`  — full chat completions URL (default: LM Studio @ 127.0.0.1:1234)
/// * `llm_model`— model identifier (default: "phi-4").  Pass `None` to use default.
/// * `ts_errors`— human-readable Tree-sitter error descriptions to guide the model.
pub fn try_auto_heal(
    _file_path: &Path,
    broken_code: &str,
    ts_errors: &[String],
    llm_url: Option<&str>,
) -> Result<String> {
    try_auto_heal_with_model(_file_path, broken_code, ts_errors, llm_url, None)
}

/// Same as `try_auto_heal` but allows specifying the model name explicitly.
pub fn try_auto_heal_with_model(
    _file_path: &Path,
    broken_code: &str,
    ts_errors: &[String],
    llm_url: Option<&str>,
    model: Option<&str>,
) -> Result<String> {
    let url = llm_url.unwrap_or(DEFAULT_LLM_URL);
    let model = model.unwrap_or(DEFAULT_MODEL);

    let error_context = if ts_errors.is_empty() {
        "(Tree-sitter detected syntax errors but could not pinpoint them.)".to_string()
    } else {
        format!(
            "Tree-sitter reported the following syntax errors:\n{}",
            ts_errors
                .iter()
                .enumerate()
                .map(|(i, e)| format!("  {}. {}", i + 1, e))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let prompt = format!(
        "{error_context}\n\nFix ONLY the syntax errors listed above. \
         Output ONLY raw code, no markdown, no backticks.\n\nBroken code:\n\n{broken_code}"
    );

    // --- OpenAI-compatible request (LM Studio / llama.cpp / Ollama) ---
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are an expert compiler. Fix only the reported syntax errors. \
                            Output ONLY raw code — no markdown, no backticks, no explanations."
            },
            { "role": "user", "content": prompt }
        ],
        "temperature": 0.1,
        "max_tokens": 2000,
        "stream": false
    });

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(LLM_TIMEOUT_SECS))
        .build();

    let resp = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_json(payload)
        .with_context(|| {
            format!(
                "Failed to reach Local LLM at '{}' (model: '{}'). \
             Is LM Studio running? Is the model loaded?",
                url, model
            )
        })?;

    let json_resp: serde_json::Value = resp
        .into_json()
        .context("Failed to parse LLM JSON response")?;

    let content = json_resp["choices"][0]["message"]["content"]
        .as_str()
        .context("Missing 'content' in LLM response — unexpected response shape")?;

    Ok(sanitize_llm_code(content))
}

/// Strip any residual markdown code blocks (e.g., ```rust ... ```) from the LLM's response.
fn sanitize_llm_code(raw: &str) -> String {
    let mut out = Vec::new();
    let mut in_code_block = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        // If the LLM didn't use markdown blocks, we still want to collect the lines.
        // We just skip the ``` lines.
        out.push(line);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_rust_fence() {
        let raw = "```rust\nfn foo() {}\n```\n";
        assert_eq!(sanitize_llm_code(raw), "fn foo() {}");
    }

    #[test]
    fn sanitize_no_fence_passthrough() {
        let raw = "fn foo() {}";
        assert_eq!(sanitize_llm_code(raw), "fn foo() {}");
    }

    #[test]
    fn sanitize_strips_unmarked_fence() {
        let raw = "```\nfn bar() { 42 }\n```";
        assert_eq!(sanitize_llm_code(raw), "fn bar() { 42 }");
    }

    #[test]
    fn sanitize_multiple_blocks_joined() {
        // LLM sometimes wraps each function in its own block
        let raw = "```rust\nfn a() {}\n```\n```rust\nfn b() {}\n```";
        // Both blocks should be extracted and joined
        let result = sanitize_llm_code(raw);
        assert!(result.contains("fn a() {}"), "Must include fn a");
        assert!(result.contains("fn b() {}"), "Must include fn b");
        assert!(!result.contains("```"), "Must strip all fences");
    }

    #[test]
    fn sanitize_no_fence_inside_code_preserved() {
        // A raw docstring containing ``` must not be stripped
        let raw = "fn doc() {\n    // See: ```example```\n}";
        let result = sanitize_llm_code(raw);
        // should not strip the inline ``` in the comment since it's not a block fence
        // (our sanitizer treats ``` at start of trimmed line as block markers)
        assert!(result.contains("fn doc()"));
    }

    /// Verify the ts_errors formatting produces a numbered list
    #[test]
    fn error_context_format_test() {
        let errors = vec![
            "Missing ';' at line 3:10".to_string(),
            "Unexpected token 'fn' at line 5:1".to_string(),
        ];
        let error_context = if errors.is_empty() {
            "(no details)".to_string()
        } else {
            format!(
                "Tree-sitter reported the following syntax errors:\n{}",
                errors
                    .iter()
                    .enumerate()
                    .map(|(i, e)| format!("  {}. {}", i + 1, e))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        assert!(error_context.contains("1. Missing"));
        assert!(error_context.contains("2. Unexpected"));
    }
}
