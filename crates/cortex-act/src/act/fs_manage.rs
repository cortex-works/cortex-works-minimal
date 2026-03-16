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
use std::path::PathBuf;

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

    Ok(format!("Written {} bytes to `{}`", content.len(), raw_path))
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
            crate::act::env_patcher::patch_env(&file, action, target, value_as_string.as_deref())
                .map_err(|e| anyhow::anyhow!("patch({patch_type}) failed: {}", e))
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

    for raw_path in &paths {
        let path = crate::act::pathing::resolve_path(workspace_roots, raw_path);
        match std::fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create directory '{}'", raw_path))
        {
            Ok(_) => {
                tracing::info!("[fs_manage] Created directory: {}", raw_path);
                created += 1;
            }
            Err(e) => {
                failed.push(format!("'{}' ({})", raw_path, e));
            }
        }
    }

    if failed.is_empty() {
        Ok(format!("Successfully created {} paths.", created))
    } else {
        Ok(format!(
            "Successfully created {} paths. Failed to create {} path(s): [{}]",
            created,
            failed.len(),
            failed.join(", ")
        ))
    }
}

// ── delete ────────────────────────────────────────────────────────────────────

fn handle_delete(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let paths = extract_paths(args);
    if paths.is_empty() {
        return Err(anyhow::anyhow!("'paths' required for delete"));
    }

    let mut deleted = 0usize;
    let mut failed: Vec<String> = Vec::new();

    for raw_path in &paths {
        let path = crate::act::pathing::resolve_path(workspace_roots, raw_path);
        if !path.exists() {
            failed.push(format!("'{}' (Not Found)", raw_path));
            continue;
        }

        let was_dir = path.is_dir();

        let remove_result = if was_dir {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("Failed to remove directory '{}'", raw_path))
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to delete '{}'", raw_path))
        };

        match remove_result {
            Ok(_) => {
                if was_dir {
                    tracing::info!("[fs_manage] Removed directory: {}", raw_path);
                } else {
                    tracing::info!("[fs_manage] Deleted file: {}", raw_path);
                }
                let project = find_project_root(&path);
                crate::fire_tombstone(project, path.to_string_lossy().into_owned());
                deleted += 1;
            }
            Err(e) => failed.push(format!("'{}' ({})", raw_path, e)),
        }
    }

    if failed.is_empty() {
        Ok(format!("Successfully deleted {} paths.", deleted))
    } else {
        Ok(format!(
            "Successfully deleted {} paths. Failed to delete {} path(s): [{}]",
            deleted,
            failed.len(),
            failed.join(", ")
        ))
    }
}

// ── rename / move ─────────────────────────────────────────────────────────────

fn handle_rename(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let (src_str, dst_str) = require_src_dst_paths(args, "rename/move")?;

    let src = crate::act::pathing::resolve_path(workspace_roots, &src_str);
    let dst = crate::act::pathing::resolve_path(workspace_roots, &dst_str);

    if !src.exists() {
        return Err(anyhow::anyhow!("Source does not exist: {}", src_str));
    }

    // Auto-create destination parent directories if they don't exist.
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create parent dir '{}'", parent.display()))?;
        }
    }

    std::fs::rename(&src, &dst)
        .with_context(|| format!("Failed to rename '{}' → '{}'", src_str, dst_str))?;
    tracing::info!("[fs_manage] Renamed: '{}' → '{}'", src_str, dst_str);

    // Tombstone the old path, then queue index of the new path.
    let project = find_project_root(&src);
    crate::fire_tombstone(project.clone(), src.to_string_lossy().into_owned());
    crate::fire_index_modified(project, dst.to_string_lossy().into_owned());

    Ok(format!("Renamed: `{}` → `{}`", src_str, dst_str))
}

// ── copy ──────────────────────────────────────────────────────────────────────

fn handle_copy(args: &Value, workspace_roots: &[PathBuf]) -> Result<String> {
    let (src_str, dst_str) = require_src_dst_paths(args, "copy")?;

    let src = crate::act::pathing::resolve_path(workspace_roots, &src_str);
    let dst = crate::act::pathing::resolve_path(workspace_roots, &dst_str);

    if !src.exists() {
        return Err(anyhow::anyhow!("Source does not exist: {}", src_str));
    }

    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create parent dir '{}'", parent.display()))?;
        }
    }

    std::fs::copy(&src, &dst)
        .with_context(|| format!("Failed to copy '{}' → '{}'", src_str, dst_str))?;
    tracing::info!("[fs_manage] Copied: '{}' → '{}'", src_str, dst_str);

    // Queue index of the new copy so it appears in the Vector DB.
    let project = find_project_root(&src);
    crate::fire_index_modified(project, dst.to_string_lossy().into_owned());

    Ok(format!("Copied: `{}` → `{}`", src_str, dst_str))
}

// ── Helper: project root detection ───────────────────────────────────────────

/// Walk upward from `path` to find the first directory containing `.git` or
/// `.cortexast.json`.  Falls back to the file's parent directory.
fn find_project_root(path: &std::path::Path) -> String {
    let mut dir = if path.is_file() || !path.exists() {
        match path.parent() {
            Some(p) => p.to_path_buf(),
            None => return String::new(),
        }
    } else {
        path.to_path_buf()
    };

    loop {
        if dir.join(".git").exists() || dir.join(".cortexast.json").exists() {
            return dir.to_string_lossy().to_string();
        }
        if !dir.pop() {
            // No marker found — fall back to direct parent.
            return path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
        }
    }
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
}
