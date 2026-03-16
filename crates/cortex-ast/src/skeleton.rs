use anyhow::Result;
use ignore::WalkBuilder;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_FILES_DEFAULT: usize = 200;
const MAX_FILES_CAP: usize = 500;

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "cs", "cpp", "c", "h", "hpp",
    "rb", "php", "dart", "swift", "kt",
];

#[derive(Default)]
struct FileSkeleton {
    fns: Vec<String>,
    structs: Vec<String>,
}

pub fn render_project_skeleton(
    project_root: &Path,
    workspace_roots: &[PathBuf],
    target_dirs: &[PathBuf],
    args: &Value,
) -> Result<String> {
    let max_files = args
        .get("max_files")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).min(MAX_FILES_CAP))
        .unwrap_or(MAX_FILES_DEFAULT);

    let ext_filter: Vec<String> = args
        .get("extensions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str())
                .map(|s| s.trim_start_matches('.').to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let mut file_symbols: BTreeMap<String, FileSkeleton> = BTreeMap::new();
    let mut files_processed = 0usize;
    let targets: Vec<PathBuf> = if target_dirs.is_empty() {
        if workspace_roots.len() > 1 {
            workspace_roots.to_vec()
        } else {
            vec![project_root.to_path_buf()]
        }
    } else {
        target_dirs.to_vec()
    };

    'targets: for target_dir in &targets {
        let target_prefix = logical_prefix(project_root, workspace_roots, target_dir);

        for entry in WalkBuilder::new(target_dir)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .build()
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.file_type().map(|t| !t.is_file()).unwrap_or(true) {
                continue;
            }

            let path = entry.path().to_path_buf();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            let kept_ext = if ext_filter.is_empty() {
                SUPPORTED_EXTENSIONS.contains(&ext.as_str())
            } else {
                ext_filter.contains(&ext)
            };
            if !kept_ext {
                continue;
            }

            let rel = path
                .strip_prefix(target_dir)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            let rel = if target_prefix == "." || target_prefix.is_empty() {
                rel
            } else if rel.is_empty() {
                target_prefix.clone()
            } else {
                format!("{}/{}", target_prefix, rel)
            };

            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let symbols = crate::inspector::extract_symbols_from_source(&path, &source);
            if symbols.is_empty() {
                continue;
            }

            let rel_lower = rel.to_lowercase();
            let mut skel = FileSkeleton::default();
            for sym in &symbols {
                let sym_text: &str = if sym.end_byte > sym.start_byte && sym.end_byte <= source.len() {
                    &source[sym.start_byte..sym.end_byte]
                } else {
                    sym.signature.as_deref().unwrap_or(sym.name.as_str())
                };

                let mut tag_parts: Vec<&str> = Vec::new();
                if sym_text.contains("todo!()")
                    || sym_text.contains("todo! ()")
                    || sym_text.contains("unimplemented!()")
                    || sym_text.contains("// TODO")
                    || sym_text.contains("pass")
                {
                    tag_parts.push("INCOMPLETE");
                }
                if rel_lower.contains("mock")
                    || rel_lower.contains("stub")
                    || rel_lower.contains("test_helper")
                    || sym.name.to_lowercase().contains("mock")
                    || sym.name.to_lowercase().contains("stub")
                {
                    tag_parts.push("MOCK");
                }
                let tag_suffix = if tag_parts.is_empty() {
                    String::new()
                } else {
                    format!(" <{}>", tag_parts.join(","))
                };

                let display = sym
                    .signature
                    .as_deref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| sym.name.clone());

                match sym.kind.as_str() {
                    "function" | "method" => skel.fns.push(format!("{}{}", display, tag_suffix)),
                    "struct" | "class" | "interface" | "enum" | "trait" | "type" => {
                        skel.structs.push(format!("{}{}", sym.name, tag_suffix));
                    }
                    _ => {}
                }
            }

            if !skel.fns.is_empty() || !skel.structs.is_empty() {
                file_symbols.insert(rel, skel);
                files_processed += 1;
            }

            if files_processed >= max_files {
                break 'targets;
            }
        }
    }

    if file_symbols.is_empty() {
        return Ok(format!(
            "*No supported source files found in `{}`.*",
            project_root.display()
        ));
    }

    let mut out = format!(
        "# Project Skeleton — `{}`\n# Files: {}  (cap: {})\n\n",
        project_root.display(),
        files_processed,
        max_files,
    );

    for (file, skel) in &file_symbols {
        out.push_str(&format!("{}:\n", file));
        if !skel.fns.is_empty() {
            out.push_str(&format!("  fn: [{}]\n", skel.fns.join(", ")));
        }
        if !skel.structs.is_empty() {
            out.push_str(&format!("  struct: [{}]\n", skel.structs.join(", ")));
        }
    }

    let total_lines = out.lines().count();
    if total_lines > 2000 {
        let truncated: String = out.lines().take(2000).collect::<Vec<_>>().join("\n");
        Ok(format!(
            "{}\n# ... (truncated at 2000 lines / {} total)",
            truncated, total_lines
        ))
    } else {
        Ok(out)
    }
}

fn logical_prefix(project_root: &Path, workspace_roots: &[PathBuf], target_dir: &Path) -> String {
    for root in workspace_roots {
        if let Ok(rel) = target_dir.strip_prefix(root) {
            let root_name = root
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("root");
            let rel = rel.to_string_lossy().replace('\\', "/");
            return if rel.is_empty() {
                format!("[{root_name}]")
            } else {
                format!("[{root_name}]/{rel}")
            };
        }
    }

    target_dir
        .strip_prefix(project_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| target_dir.to_string_lossy().replace('\\', "/"))
}
