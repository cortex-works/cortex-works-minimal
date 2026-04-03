//! cortex_fs_manage — unified physical file-system tool.
//!
//! Single entry-point for physical file-system mutations in the minimal branch.
//! It does not perform structural code edits and does not guarantee semantic
//! re-indexing after changes.
//!
//! ## Supported actions
//! | action   | required params              | effect                                    |
//! |----------|------------------------------|-------------------------------------------|
//! | `write`  | `paths[0]`, `content`        | Create/overwrite file; auto-creates dirs. |
//! | `patch`  | `paths[0]`, `type`, `target`, `action` | Key-value patch (.env / .ini / kv). |
//! | `mkdir`  | `paths[]`                    | Create directory tree(s) (like mkdir -p). |
//! | `delete` | `paths[]`                    | Remove file/dir(s).                       |
//! | `rename` | `paths[0]`, `paths[1]`       | Rename in-place.                          |
//! | `move`   | `paths[0]`, `paths[1]`       | Same as rename (can cross directories).   |
//! | `copy`   | `paths[0]`, `paths[1]`       | Duplicate a file.                         |
//!
//! Backward compatibility: legacy `path` + `new_path` is still accepted.

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn require_primary_path(args: &Value, action: &str) -> Result<String> {
    extract_paths(args)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("'paths' required for {action} (for single-file ops, use paths[0])"))
}

fn require_src_dst_paths(args: &Value, action: &str) -> Result<(String, String)> {
    let paths = extract_paths(args);
    if paths.len() >= 2 {
        return Ok((paths[0].clone(), paths[1].clone()));
    }

    let src = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("'{action}' requires paths[0] and paths[1] (or legacy path/new_path)"))?;

    let dst = args
        .get("new_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("'{action}' requires paths[0] and paths[1] (or legacy path/new_path)"))?;

    Ok((src.to_string(), dst.to_string()))
}

/// Entry point called by `dispatch.rs`.
pub fn run(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("'action' required (write | patch | mkdir | delete | rename | move | copy)"))?;

    match action {
        "write"          => handle_write(args, workspace_roots),
        "patch"          => handle_patch(args, workspace_roots),
        "mkdir"          => handle_mkdir(args, workspace_roots),
        "delete"         => handle_delete(args, workspace_roots),
        "rename" | "move" => handle_rename(args, workspace_roots),
        "copy"           => handle_copy(args, workspace_roots),
        other => Err(anyhow::anyhow!(
            "Unknown action '{}'. Supported: write, patch, mkdir, delete, rename, move, copy",
            other
        )),
    }
}

fn append_z4_validation(base: String, paths: &[PathBuf]) -> Result<String> {
    match maybe_validate_z4_projects(paths)? {
        Some(summary) if !summary.is_empty() => Ok(format!("{base}\n{summary}")),
        _ => Ok(base),
    }
}

fn summarize_process_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{}\n{}", stdout, stderr),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => format!("exit status: {}", output.status),
    }
}

pub(crate) fn maybe_validate_z4_projects(paths: &[PathBuf]) -> Result<Option<String>> {
    let mut roots = BTreeSet::new();

    for path in paths {
        if let Some(project_root) = find_project_root_path(path) {
            if cortexast::config::load_config(&project_root).z4 {
                roots.insert(project_root);
            }
        }
    }

    if roots.is_empty() {
        return Ok(None);
    }

    let mut summaries = Vec::new();
    for project_root in roots {
        summaries.push(run_z4_validation(&project_root)?);
    }

    Ok(Some(summaries.join("\n")))
}

fn run_z4_validation(project_root: &Path) -> Result<String> {
    let z4c = project_root.join("z4c");
    let filelist = project_root.join("build/compiler.filelist");
    if !z4c.exists() || !filelist.exists() {
        anyhow::bail!(
            "z4=true validation requires '{}' and '{}'",
            z4c.display(),
            filelist.display()
        );
    }

    let temp_binary = std::env::temp_dir().join(format!(
        "cortex-works-z4-validate-{}",
        std::process::id()
    ));
    let compile = Command::new(&z4c)
        .current_dir(project_root)
        .arg("-f")
        .arg(&filelist)
        .arg("-o")
        .arg(&temp_binary)
        .output()
        .with_context(|| {
            format!(
                "Failed to execute {}. z4=true validation requires a host-runnable z4c binary.",
                z4c.display()
            )
        })?;
    if !compile.status.success() {
        let summary = summarize_process_output(&compile);
        let _ = std::fs::remove_file(&temp_binary);
        anyhow::bail!(
            "z4c compile validation failed for '{}': {}",
            project_root.display(),
            summary
        );
    }

    let run = Command::new(&temp_binary)
        .current_dir(project_root)
        .output()
        .with_context(|| {
            format!(
                "Failed to execute compiled validation artifact '{}'",
                temp_binary.display()
            )
        })?;
    let _ = std::fs::remove_file(&temp_binary);
    if !run.status.success() {
        anyhow::bail!(
            "z4c runtime validation failed for '{}': {}",
            project_root.display(),
            summarize_process_output(&run)
        );
    }

    Ok(format!("z4c validation ok: {}", project_root.display()))
}

// ── write ─────────────────────────────────────────────────────────────────────

fn handle_write(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let raw_path = require_primary_path(args, "write")?;
    let path = crate::act::pathing::resolve_path(workspace_roots, &raw_path);

    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("'content' required for write"))?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create parent dir '{}'", parent.display()))?;
        }
    }
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write '{}'", raw_path))?;

    tracing::info!("[fs_manage] Wrote {} bytes to: {}", content.len(), raw_path);

    // Queue index of the new/updated file.
    let project = find_project_root(&path);
    crate::fire_index_modified(project, path.to_string_lossy().into_owned());

    append_z4_validation(
        format!("Written {} bytes to `{}`", content.len(), raw_path),
        std::slice::from_ref(&path),
    )
}

// ── patch (.env / .ini / kv) ─────────────────────────────────────────────────

fn handle_patch(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let raw_file = require_primary_path(args, "patch")?;
    let file = crate::act::pathing::resolve_path_string(workspace_roots, &raw_file);

    let patch_type = args
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("env");

    let action = args
        .get("patch_action")
        .and_then(|v| v.as_str())
        .or_else(|| {
            args.get("action")
                .and_then(|v| v.as_str())
                .filter(|value| *value != "patch")
        })
        .unwrap_or("set");

    let target = args
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("'target' (key name) required for patch"))?;

    let value_as_string: Option<String> = args.get("value").map(|v| {
        if let Some(s) = v.as_str() {
            s.to_string()
        } else if let Some(n) = v.as_i64() {
            n.to_string()
        } else if let Some(n) = v.as_f64() {
            n.to_string()
        } else if let Some(b) = v.as_bool() {
            b.to_string()
        } else {
            v.to_string()
        }
    });

    match patch_type {
        "env" | "ini" | "kv" => {
            let result = crate::act::env_patcher::patch_env(
                &file,
                action,
                target,
                value_as_string.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("patch({patch_type}) failed: {}", e))?;
            let file_path = PathBuf::from(&file);
            let project = find_project_root(&file_path);
            crate::fire_index_modified(project, file.clone());
            append_z4_validation(result, std::slice::from_ref(&file_path))
        }
        other => Err(anyhow::anyhow!(
            "Unknown patch type: '{}'. Use: env | ini | kv",
            other
        )),
    }
}

// ── mkdir ─────────────────────────────────────────────────────────────────────

fn handle_mkdir(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let paths = extract_paths(args);
    if paths.is_empty() {
        return Err(anyhow::anyhow!("'paths' required for mkdir"));
    }

    let mut created = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let mut created_paths: Vec<PathBuf> = Vec::new();

    for raw_path in &paths {
        let path = crate::act::pathing::resolve_path(workspace_roots, raw_path);
        let display_path = path.to_string_lossy().into_owned();
        match std::fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create directory '{}'", display_path))
        {
            Ok(_) => {
                tracing::info!("[fs_manage] Created directory: {}", display_path);
                created += 1;
                created_paths.push(path);
            }
            Err(e) => {
                failed.push(format!("'{}' ({})", display_path, e));
            }
        }
    }

    let base = if failed.is_empty() {
        format!("Successfully created {} paths.", created)
    } else {
        format!(
            "Successfully created {} paths. Failed to create {} path(s): [{}]",
            created,
            failed.len(),
            failed.join(", ")
        )
    };

    append_z4_validation(base, &created_paths)
}

// ── delete ────────────────────────────────────────────────────────────────────

/// Returns `true` when `path` falls under at least one workspace root.
///
/// Used as a safety guard before destructive operations: if the agent tries to
/// delete an absolute path that is outside every known workspace root we reject
/// it to prevent accidental destruction of unrelated filesystem trees.
fn is_within_workspace(path: &std::path::Path, workspace_roots: &[PathBuf]) -> bool {
    if workspace_roots.is_empty() {
        return true;
    }

    // Fast path: relative paths are always considered "within" the workspace
    // (they were resolved from a workspace root by resolve_path).
    if path.is_relative() {
        return true;
    }
    // Try exact prefix match on the canonical forms of both paths.
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for root in workspace_roots {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        if canonical_path.starts_with(&canonical_root) {
            return true;
        }
    }
    false
}

fn handle_delete(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let paths = extract_paths(args);
    if paths.is_empty() {
        return Err(anyhow::anyhow!("'paths' required for delete"));
    }

    let mut deleted = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let mut deleted_paths: Vec<PathBuf> = Vec::new();

    for raw_path in &paths {
        let path = crate::act::pathing::resolve_path(workspace_roots, raw_path);
        let display_path = path.to_string_lossy().into_owned();
        if !path.exists() {
            failed.push(format!("'{}' (Not Found)", display_path));
            continue;
        }

        let was_dir = path.is_dir();

        // Safety guard: reject absolute paths that escape all workspace roots.
        // This prevents accidental `remove_dir_all` on unrelated filesystem trees
        // (e.g. an agent passing "/" or "~" as the delete target).
        if path.is_absolute() && !is_within_workspace(&path, workspace_roots) {
            failed.push(format!(
                "'{}' (blocked: path is outside all workspace roots — use an explicit workspace-relative path)",
                display_path
            ));
            continue;
        }

        let remove_result = if was_dir {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("Failed to remove directory '{}'", display_path))
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to delete '{}'", display_path))
        };

        match remove_result {
            Ok(_) => {
                if was_dir {
                    tracing::info!("[fs_manage] Removed directory: {}", display_path);
                } else {
                    tracing::info!("[fs_manage] Deleted file: {}", display_path);
                }
                let project = find_project_root(&path);
                crate::fire_tombstone(project, path.to_string_lossy().into_owned());
                deleted += 1;
                deleted_paths.push(path);
            }
            Err(e) => failed.push(format!("'{}' ({})", display_path, e)),
        }
    }

    let base = if failed.is_empty() {
        format!("Successfully deleted {} paths.", deleted)
    } else {
        format!(
            "Successfully deleted {} paths. Failed to delete {} path(s): [{}]",
            deleted,
            failed.len(),
            failed.join(", ")
        )
    };

    append_z4_validation(base, &deleted_paths)
}

// ── rename / move ─────────────────────────────────────────────────────────────

fn handle_rename(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let (src_str, dst_str) = require_src_dst_paths(args, "rename/move")?;

    let src = crate::act::pathing::resolve_path(workspace_roots, &src_str);
    let dst = crate::act::pathing::resolve_path(workspace_roots, &dst_str);
    let src_display = src.to_string_lossy().into_owned();
    let dst_display = dst.to_string_lossy().into_owned();

    if !src.exists() {
        return Err(anyhow::anyhow!("Source does not exist: {}", src_display));
    }

    // Auto-create destination parent directories if they don't exist.
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create parent dir '{}'", parent.display()))?;
        }
    }

    std::fs::rename(&src, &dst)
        .with_context(|| format!("Failed to rename '{}' → '{}'", src_display, dst_display))?;
    tracing::info!("[fs_manage] Renamed: '{}' → '{}'", src_display, dst_display);

    // Tombstone the old path, then queue index of the new path.
    let project = find_project_root(&src);
    crate::fire_tombstone(project.clone(), src.to_string_lossy().into_owned());
    crate::fire_index_modified(project, dst.to_string_lossy().into_owned());

    append_z4_validation(
        format!("Renamed: `{}` → `{}`", src_display, dst_display),
        &[src.clone(), dst.clone()],
    )
}

// ── copy ──────────────────────────────────────────────────────────────────────

fn handle_copy(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let (src_str, dst_str) = require_src_dst_paths(args, "copy")?;

    let src = crate::act::pathing::resolve_path(workspace_roots, &src_str);
    let dst = crate::act::pathing::resolve_path(workspace_roots, &dst_str);
    let src_display = src.to_string_lossy().into_owned();
    let dst_display = dst.to_string_lossy().into_owned();

    if !src.exists() {
        return Err(anyhow::anyhow!("Source does not exist: {}", src_display));
    }

    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create parent dir '{}'", parent.display()))?;
        }
    }

    std::fs::copy(&src, &dst)
        .with_context(|| format!("Failed to copy '{}' → '{}'", src_display, dst_display))?;
    tracing::info!("[fs_manage] Copied: '{}' → '{}'", src_display, dst_display);

    // Queue index of the new copy so it appears in the Vector DB.
    let project = find_project_root(&src);
    crate::fire_index_modified(project, dst.to_string_lossy().into_owned());

    append_z4_validation(
        format!("Copied: `{}` → `{}`", src_display, dst_display),
        &[src.clone(), dst.clone()],
    )
}

// ── Helper: project root detection ───────────────────────────────────────────

/// Walk upward from `path` to find the first directory containing `.git` or
/// `.cortexast.json`.  Falls back to the file's parent directory.
fn find_project_root_path(path: &Path) -> Option<PathBuf> {
    let mut dir = if path.is_file() || !path.exists() {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };

    loop {
        if dir.join(".git").exists() || dir.join(".cortexast.json").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return path.parent().map(|p| p.to_path_buf());
        }
    }
}

pub(crate) fn is_z4_path(path: &Path) -> bool {
    find_project_root_path(path)
        .map(|project_root| cortexast::config::load_config(&project_root).z4)
        .unwrap_or(false)
}

fn find_project_root(path: &Path) -> String {
    find_project_root_path(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn write_supports_legacy_path_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("legacy.txt");

        let args = json!({
            "action": "write",
            "path": file_path.to_string_lossy().to_string(),
            "content": "hello"
        });

        let result = run(&args, &[]).expect("write should succeed with legacy path");
        assert!(result.contains("Written"));
        assert_eq!(std::fs::read_to_string(&file_path).expect("read file"), "hello");
    }

    #[test]
    fn rename_uses_paths_source_and_destination() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, "payload").expect("write src");

        let args = json!({
            "action": "rename",
            "paths": [
                src.to_string_lossy().to_string(),
                dst.to_string_lossy().to_string()
            ]
        });

        let result = run(&args, &[]).expect("rename should succeed");
        assert!(result.contains("Renamed"));
        assert!(!src.exists());
        assert!(dst.exists());
        assert_eq!(std::fs::read_to_string(&dst).expect("read dst"), "payload");
    }

    #[test]
    fn delete_batch_is_resilient_and_reports_partial_failures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keep_missing = dir.path().join("missing.txt");
        let file_a = dir.path().join("a.txt");
        let file_b = dir.path().join("b.txt");
        std::fs::write(&file_a, "a").expect("write a");
        std::fs::write(&file_b, "b").expect("write b");

        let args = json!({
            "action": "delete",
            "paths": [
                file_a.to_string_lossy().to_string(),
                keep_missing.to_string_lossy().to_string(),
                file_b.to_string_lossy().to_string()
            ]
        });

        let result = run(&args, &[]).expect("delete should not fail on partial errors");
        assert!(result.contains("Successfully deleted 2 paths."));
        assert!(result.contains("Failed to delete 1 path"));
        assert!(result.contains("Not Found"));
        assert!(!file_a.exists());
        assert!(!file_b.exists());
    }

    #[test]
    fn write_supports_workspace_prefixed_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("fixture-root");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let args = json!({
            "action": "write",
            "paths": ["[fixture-root]/prefixed.txt"],
            "content": "hello"
        });

        let result = run(&args, std::slice::from_ref(&workspace)).expect("prefixed write should succeed");
        assert!(result.contains("Written"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("prefixed.txt")).expect("read file"),
            "hello"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_runs_z4_validation_when_enabled() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("build")).expect("create build dir");
        std::fs::write(dir.path().join(".cortexast.json"), r#"{"z4":true}"#)
            .expect("write config");
        std::fs::write(dir.path().join("build/compiler.filelist"), "main.z4\n")
            .expect("write compiler filelist");

        let validator = dir.path().join("z4c");
        std::fs::write(
            &validator,
            "#!/bin/sh\nout=\"\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then\n    shift\n    out=\"$1\"\n  fi\n  shift\ndone\ncat <<'EOF' > \"$out\"\n#!/bin/sh\nexit 0\nEOF\nchmod +x \"$out\"\n",
        )
        .expect("write validator shim");
        let mut perms = std::fs::metadata(&validator)
            .expect("validator metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&validator, perms).expect("chmod validator");

        let target = dir.path().join("main.z4");
        let args = json!({
            "action": "write",
            "paths": [target.to_string_lossy().to_string()],
            "content": "@z4_main: RET\n"
        });

        let result = run(&args, &[]).expect("z4 write should succeed");
        assert!(result.contains("Written"));
        assert!(result.contains("z4c validation ok:"));
    }
}
