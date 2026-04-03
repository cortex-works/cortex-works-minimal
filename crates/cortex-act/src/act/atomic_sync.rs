use anyhow::{Context, Result, anyhow};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

fn extract_paths(args: &Value) -> Vec<String> {
    let mut out = Vec::new();

    if let Some(arr) = args.get("paths").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(path) = item.as_str() {
                if !path.trim().is_empty() {
                    out.push(path.to_string());
                }
            }
        }
    }

    if out.is_empty() {
        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            if !path.trim().is_empty() {
                out.push(path.to_string());
            }
        }
    }

    out
}

fn phase_id_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^0x[0-9A-Fa-f]+$").unwrap())
}

fn quoted_string_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\"([^\"\\]*(?:\\.[^\"\\]*)*)\""#).unwrap())
}

fn summarize_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{}\n{}", stdout, stderr),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => format!("exit status: {}", output.status),
    }
}

fn git_output(repo_root: &Path, args: &[String]) -> Result<Output> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute git {}", args.join(" ")))?;
    Ok(output)
}

fn git_repo_root(cwd: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to execute git rev-parse --show-toplevel")?;
    if !output.status.success() {
        anyhow::bail!(
            "Not a git repository: {}",
            summarize_output(&output)
        );
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn normalize_summary(summary: &str) -> Result<String> {
    let normalized = summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if normalized.is_empty() {
        anyhow::bail!("'summary' must not be empty");
    }
    if !normalized.is_ascii() {
        anyhow::bail!("'summary' must be ASCII-only for z4 atomic commits");
    }
    Ok(normalized)
}

fn build_commit_message(phase_id: &str, summary: &str) -> String {
    format!("[Z4_{}] {}", phase_id, summary)
}

fn allow_machine_string(value: &str) -> bool {
    if value.starts_with("0x") {
        let trimmed = value.trim_end_matches("\\0");
        return trimmed
            .chars()
            .skip(2)
            .all(|ch| ch.is_ascii_hexdigit());
    }

    if value.chars().any(|ch| ch.is_ascii_whitespace()) {
        return false;
    }

    value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '-' | ':' | '%' | '@')
    })
}

fn lint_z4_source(path: &Path, source_text: &str) -> Result<()> {
    if source_text.contains('#') {
        anyhow::bail!(
            "Refusing atomic sync for '{}': '#' comment surface detected",
            path.display()
        );
    }

    for caps in quoted_string_regex().captures_iter(source_text) {
        let Some(inner) = caps.get(1).map(|m| m.as_str()) else {
            continue;
        };
        if allow_machine_string(inner) {
            continue;
        }
        anyhow::bail!(
            "Refusing atomic sync for '{}': human-readable quoted string detected ({})",
            path.display(),
            inner
        );
    }

    Ok(())
}

fn collect_z4_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("z4"))
                .unwrap_or(false)
            {
                files.push(path.clone());
            }
            continue;
        }

        if !path.is_dir() {
            continue;
        }

        let walker = WalkBuilder::new(path)
            .standard_filters(true)
            .hidden(true)
            .build();
        for entry_result in walker {
            let Ok(entry) = entry_result else { continue };
            let entry_path = entry.path();
            if entry_path.is_file()
                && entry_path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("z4"))
                    .unwrap_or(false)
            {
                files.push(entry_path.to_path_buf());
            }
        }
    }

    files.sort();
    files.dedup();
    files
}

fn lint_paths(paths: &[PathBuf]) -> Result<()> {
    for file in collect_z4_files(paths) {
        let source_text = std::fs::read_to_string(&file)
            .with_context(|| format!("Failed to read '{}'", file.display()))?;
        lint_z4_source(&file, &source_text)?;
    }
    Ok(())
}

fn git_pathspecs(repo_root: &Path, paths: &[PathBuf]) -> Result<Vec<String>> {
    let mut pathspecs = Vec::new();
    for path in paths {
        let rel = path.strip_prefix(repo_root).map_err(|_| {
            anyhow!(
                "Path '{}' is outside git repo '{}'",
                path.display(),
                repo_root.display()
            )
        })?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            anyhow::bail!("Refusing to atomic-sync the entire repo root; pass explicit paths instead.");
        }
        pathspecs.push(rel);
    }
    Ok(pathspecs)
}

pub fn run(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let phase_id = args
        .get("phase_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("'phase_id' required (example: 0x16ab1d44)"))?;
    if !phase_id_regex().is_match(phase_id) {
        anyhow::bail!("'phase_id' must match 0x[0-9A-Fa-f]+");
    }

    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("'summary' required"))?;
    let summary = normalize_summary(summary)?;
    let purge_untracked = args
        .get("purge_untracked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let cwd = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|cwd| crate::act::pathing::resolve_path(workspace_roots, cwd))
        .unwrap_or_else(|| crate::act::pathing::primary_root(workspace_roots));
    let repo_root = git_repo_root(&cwd)?;

    let raw_paths = extract_paths(args);
    if raw_paths.is_empty() {
        anyhow::bail!("'paths' required for cortex_z4_atomic_sync");
    }

    let resolved_paths: Vec<PathBuf> = raw_paths
        .iter()
        .map(|raw| crate::act::pathing::resolve_path_from_root(&repo_root, workspace_roots, raw))
        .collect();
    lint_paths(&resolved_paths)?;

    let validation_summary = crate::act::fs_manage::maybe_validate_z4_projects(&resolved_paths)?;
    let pathspecs = git_pathspecs(&repo_root, &resolved_paths)?;

    let mut add_args = vec!["add".to_string(), "--".to_string()];
    add_args.extend(pathspecs.iter().cloned());
    let add = git_output(&repo_root, &add_args)?;
    if !add.status.success() {
        anyhow::bail!("git add failed: {}", summarize_output(&add));
    }

    let mut diff_args = vec![
        "diff".to_string(),
        "--cached".to_string(),
        "--quiet".to_string(),
        "--".to_string(),
    ];
    diff_args.extend(pathspecs.iter().cloned());
    let diff = git_output(&repo_root, &diff_args)?;
    if diff.status.success() {
        anyhow::bail!(
            "No staged changes found for the requested paths: {}",
            pathspecs.join(",")
        );
    }

    let commit_message = build_commit_message(phase_id, &summary);
    let mut commit_args = vec![
        "commit".to_string(),
        "-m".to_string(),
        commit_message.clone(),
        "--only".to_string(),
        "--".to_string(),
    ];
    commit_args.extend(pathspecs.iter().cloned());
    let commit = git_output(&repo_root, &commit_args)?;
    if !commit.status.success() {
        anyhow::bail!("git commit failed: {}", summarize_output(&commit));
    }

    let clean_summary = if purge_untracked {
        let mut clean_args = vec!["clean".to_string(), "-fd".to_string(), "--".to_string()];
        clean_args.extend(pathspecs.iter().cloned());
        let clean = git_output(&repo_root, &clean_args)?;
        if !clean.status.success() {
            anyhow::bail!("git clean failed: {}", summarize_output(&clean));
        }
        Some(summarize_output(&clean))
    } else {
        None
    };

    let mut out = String::new();
    out.push_str(&format!(
        "Committed {} in {}\npaths={}\n",
        commit_message,
        repo_root.display(),
        pathspecs.join(",")
    ));
    if let Some(validation_summary) = validation_summary {
        if !validation_summary.trim().is_empty() {
            out.push_str(&validation_summary);
            out.push('\n');
        }
    }
    if let Some(clean_summary) = clean_summary {
        if !clean_summary.trim().is_empty() {
            out.push_str(&format!("git clean: {}\n", clean_summary));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_surface_lint_rejects_spaces_inside_quotes() {
        let err = lint_z4_source(Path::new("sample.z4"), "@a: DATA DOC:\"hello world\"\n")
            .expect_err("human string must be rejected");
        assert!(err.to_string().contains("human-readable quoted string"));
    }

    #[test]
    fn machine_strings_pass_lint() {
        lint_z4_source(
            Path::new("sample.z4"),
            "@a: DATA DOC:\"0x41444400\"\n@b: MOVE IA:%rax IB:0x10\n",
        )
        .expect("machine surface should pass");
    }

    #[test]
    fn commit_message_is_hex_scoped() {
        assert_eq!(
            build_commit_message("0x16ab1d44", "sync parser map"),
            "[Z4_0x16ab1d44] sync parser map"
        );
    }
}