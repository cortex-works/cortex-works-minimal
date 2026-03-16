use std::path::{Path, PathBuf};

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

/// Resolve a path argument using an explicit primary repo root plus workspace roots.
pub fn resolve_path_from_root(repo_root: &Path, workspace_roots: &[PathBuf], raw: &str) -> PathBuf {
    let pb = PathBuf::from(raw);
    if pb.is_absolute() {
        return pb;
    }

    if let Some(inner) = raw.strip_prefix('[') {
        if let Some(bracket_end) = inner.find(']') {
            let folder_name = &inner[..bracket_end];
            let subpath = inner
                .get(bracket_end + 2..)
                .unwrap_or("")
                .trim_start_matches('/');

            for root in workspace_roots {
                let root_name = root
                    .file_name()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                if root_name == folder_name {
                    return if subpath.is_empty() {
                        root.clone()
                    } else {
                        root.join(subpath)
                    };
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_prefixed_paths_against_workspace_root_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("fixture-root");
        let nested = root.join("src");
        std::fs::create_dir_all(&nested).expect("create src");

        let resolved = resolve_path(&[root.clone()], "[fixture-root]/src/lib.rs");
        assert_eq!(resolved, nested.join("lib.rs"));
    }
}