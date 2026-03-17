use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static WORKSPACE_ALIASES: OnceLock<RwLock<Vec<(PathBuf, String)>>> = OnceLock::new();

fn alias_store() -> &'static RwLock<Vec<(PathBuf, String)>> {
    WORKSPACE_ALIASES.get_or_init(|| RwLock::new(Vec::new()))
}

fn default_alias(root: &Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("root")
        .to_string()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn set_workspace_aliases(workspace_roots: &[PathBuf], workspace_names: &[String]) {
    let mut entries = Vec::with_capacity(workspace_roots.len());
    for (index, root) in workspace_roots.iter().enumerate() {
        let alias = workspace_names
            .get(index)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_alias(root));
        entries.push((root.clone(), alias));
    }

    if let Ok(mut guard) = alias_store().write() {
        *guard = entries;
    }
}

fn root_for_alias(alias: &str) -> Option<PathBuf> {
    let alias = alias.trim();
    if alias.is_empty() {
        return None;
    }

    alias_store().read().ok().and_then(|guard| {
        guard
            .iter()
            .find(|(_, candidate_alias)| candidate_alias == alias)
            .map(|(path, _)| path.clone())
    })
}

fn alias_for_root(root: &Path) -> String {
    if let Ok(guard) = alias_store().read() {
        for (candidate, alias) in guard.iter() {
            if paths_match(candidate, root) {
                return alias.clone();
            }
        }
    }
    default_alias(root)
}

/// Returns the primary workspace root used as the default base for relative paths.
pub fn primary_root(workspace_roots: &[PathBuf]) -> PathBuf {
    workspace_roots
        .first()
        .cloned()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Resolve a path argument using the same `[FolderName]/...` convention as cortex-ast.
pub fn resolve_path(workspace_roots: &[PathBuf], raw: &str) -> PathBuf {
    let repo_root = primary_root(workspace_roots);
    resolve_path_from_root(&repo_root, workspace_roots, raw)
}

pub fn resolve_prefixed_path(raw: &str, workspace_roots: &[PathBuf]) -> PathBuf {
    resolve_path(workspace_roots, raw)
}

/// Resolve a path argument using an explicit primary repo root plus workspace roots.
pub fn resolve_path_from_root(repo_root: &Path, workspace_roots: &[PathBuf], raw: &str) -> PathBuf {
    let pb = PathBuf::from(raw);
    if pb.is_absolute() {
        return pb;
    }

    if let Some(inner) = raw.strip_prefix('[') {
        if let Some((folder_name, tail)) = inner.split_once(']') {
            // Strip any leading path separator (forward-slash or backslash) so
            // `[Folder]/path` and `[Folder]\path` both resolve correctly on all
            // platforms, including Windows where callers may use backslashes.
            let subpath = tail.trim_start_matches(['/', '\\']);

            for root in workspace_roots {
                let configured_alias = alias_for_root(root);
                let default_name = default_alias(root);
                if configured_alias == folder_name || default_name == folder_name {
                    return if subpath.is_empty() {
                        root.clone()
                    } else {
                        root.join(subpath)
                    };
                }
            }

            if let Some(root) = root_for_alias(folder_name)
                .filter(|root| workspace_roots.is_empty() || workspace_roots.iter().any(|candidate| candidate == root))
            {
                return if subpath.is_empty() {
                    root
                } else {
                    root.join(subpath)
                };
            }
        }
    }

    repo_root.join(raw)
}

pub fn resolve_path_string(workspace_roots: &[PathBuf], raw: &str) -> String {
    resolve_path(workspace_roots, raw)
        .to_string_lossy()
        .into_owned()
}

#[allow(dead_code)]
pub fn alias_for_workspace_root(root: &Path) -> String {
    alias_for_root(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_prefixed_paths_against_workspace_root_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("fixture-root");
        let nested = root.join("src");
        std::fs::create_dir_all(&nested).expect("create src");

        set_workspace_aliases(&[root.clone()], &["ProjectA".to_string()]);
        let aliased = resolve_path(&[root.clone()], "[ProjectA]/src/lib.rs");
        assert_eq!(aliased, nested.join("lib.rs"));

        let resolved = resolve_path(&[root.clone()], "[fixture-root]/src/lib.rs");
        assert_eq!(resolved, nested.join("lib.rs"));

        let windows_style = resolve_path(&[root.clone()], r#"[ProjectA]\src\lib.rs"#);
        assert_eq!(windows_style, nested.join("lib.rs"));
    }
}