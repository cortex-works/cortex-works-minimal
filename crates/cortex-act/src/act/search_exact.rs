//! # `cortex_search_exact` — Deterministic Regex Search Over Source Files
//!
//! Complements `cortex_semantic_code_search` for cases where vector
//! similarity is unreliable: exact variable renames, literal string hunts,
//! import paths, error codes, etc.
//!
//! ## Safety constraints
//! To prevent regex blackholes on large repositories:
//! * **Max 10 files** with at least one match are returned.
//! * **Max 50 result lines** total across all files.
//! Once either cap is reached the walk terminates immediately.

use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::path::PathBuf;

/// Hard cap: stop after this many distinct files contain a match.
const MAX_FILES: usize = 10;
/// Hard cap: stop after this many total matched lines.
const MAX_RESULTS: usize = 50;

/// Entry-point called by [`crate::act::dispatch`].
pub fn run(args: &Value, workspace_roots: &[PathBuf]) -> Result<String, String> {
    // ── Parameters ────────────────────────────────────────────────────────
    let pattern = args
        .get("regex_pattern")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "'regex_pattern' is required and must be a non-empty string".to_string())?;

    let project_root = args
        .get("project_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| crate::act::pathing::resolve_path(workspace_roots, s))
        .unwrap_or_else(|| crate::act::pathing::primary_root(workspace_roots));
    let project_display = project_root.to_string_lossy().to_string();

    // Optional extension filter: "rs", ".rs", "tsx" — normalised to lowercase no-dot.
    let ext_filter: Option<String> = args
        .get("file_extension")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_start_matches('.').to_lowercase());

    // Optional glob pattern to restrict which file paths are searched.
    // Matched against the full path string (e.g. "crates/cortex-act/**" or "src/*.rs").
    let include_pattern: Option<glob::Pattern> = args
        .get("include_pattern")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .and_then(|s| glob::Pattern::new(s).ok());

    // Configurable result cap (overrides the hard MAX_RESULTS const).
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(MAX_RESULTS)
        .min(500); // hard ceiling to prevent runaway output

    // ── Compile regex ─────────────────────────────────────────────────────
    let re = Regex::new(pattern)
        .map_err(|e| format!("Invalid regex `{pattern}`: {e}"))?;

    // Derive effective caps.
    let max_files = (max_results / 5).max(5).min(MAX_FILES * 5);

    // ── Walk ──────────────────────────────────────────────────────────────
    let walker = WalkBuilder::new(&project_root)
        .standard_filters(true) // honour .gitignore + skip .git/
        .hidden(true)           // skip dot-prefixed files/dirs
        .build();

    let mut match_lines: Vec<String> = Vec::new();
    let mut files_scanned: usize = 0;
    let mut files_with_matches: usize = 0;
    let mut capped = false;

    'walk: for entry_res in walker {
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[search_exact] walk error: {e}");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Safety: stop walk once caps are reached.
        if files_with_matches >= max_files || match_lines.len() >= max_results {
            capped = true;
            break 'walk;
        }

        // ── Include-pattern filter (glob match on full path string) ───────
        if let Some(ref pat) = include_pattern {
            let path_str = path.to_string_lossy();
            let rel_path = path
                .strip_prefix(&project_root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"));
            let matched = rel_path
                .as_deref()
                .map(|rel| pat.matches(rel))
                .unwrap_or(false)
                || pat.matches(&path_str);
            if !matched {
                continue;
            }
        }

        // ── Extension filter ──────────────────────────────────────────────
        if let Some(ref ext) = ext_filter {
            match path.extension().and_then(|e| e.to_str()) {
                Some(e) if e.to_lowercase() == *ext => {}
                _ => continue,
            }
        }

        // ── Skip non-UTF-8 / binary files gracefully ──────────────────────
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        files_scanned += 1;
        let path_str = path.to_string_lossy();
        let mut file_had_match = false;

        for (line_idx, line_text) in content.lines().enumerate() {
            if match_lines.len() >= max_results {
                capped = true;
                break;
            }
            if re.is_match(line_text) {
                if !file_had_match {
                    files_with_matches += 1;
                    file_had_match = true;
                }
                match_lines.push(format!(
                    "{}:{}: {}",
                    path_str,
                    line_idx + 1,
                    line_text.trim(),
                ));
            }
        }
    }

    // ── Format ────────────────────────────────────────────────────────────
    if match_lines.is_empty() {
        return Ok(format!(
            "No matches for `{pattern}` in `{project_display}` \
             ({files_scanned} file(s) scanned{ext_note}).",
            ext_note = ext_filter
                .as_deref()
                .map(|e| format!(", filtering *.{e}"))
                .unwrap_or_default(),
        ));
    }

    let cap_note = if capped {
        format!(
            " ⚠️ Results capped at {max_results} lines / {max_files} files. Increase max_results or narrow your pattern."
        )
    } else {
        String::new()
    };

    let mut out = format!(
        "## Exact Search — `{pattern}`\n\
         _{n} match(es) across {files_with_matches} file(s) ({files_scanned} scanned){ext_note}{cap_note}_\n\n\
         ```\n",
        n        = match_lines.len(),
        ext_note = ext_filter
            .as_deref()
            .map(|e| format!(" (*.{e})"))
            .unwrap_or_default(),
    );

    for m in &match_lines {
        out.push_str(m);
        out.push('\n');
    }
    out.push_str("```\n");

    Ok(out)
}
