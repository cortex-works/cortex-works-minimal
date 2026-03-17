use anyhow::Result;
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::chronos::{checkpoint_symbol, compare_symbol, list_checkpoints};
use crate::config::load_config;
use crate::inspector::{
    call_hierarchy, extract_symbols_from_source, find_implementations, find_usages,
    propagation_checklist, read_symbol_with_options, render_skeleton, repo_map_with_filter,
    run_diagnostics,
};
use crate::scanner::{ScanOptions, scan_workspace};
use crate::slicer::{slice_paths_to_xml, slice_to_xml};
use crate::vector_store::{CodebaseIndex, IndexJob};
use rayon::prelude::*;

#[derive(Default)]
pub struct ServerState {
    /// All workspace roots, populated from (highest priority first):
    ///   1. MCP `initialize` params — all `workspaceFolders` entries, or `rootUri` / `rootPath`.
    ///   2. CLI `--root` / `CORTEXAST_ROOT` env var — startup bootstrap.
    ///   3. IDE-specific env vars (VSCODE_WORKSPACE_FOLDER, IDEA_INITIAL_DIRECTORY, …).
    ///   4. Find-up heuristic on tool args (`path` / `target_dir` / `target`).
    ///   5. `cwd` — last resort; refused if it equals $HOME or OS root.
    ///
    /// `workspace_roots[0]` is the "primary" root used as fallback for single-root tools.
    /// Multi-root workspaces (VS Code `.code-workspace`, Zed multi-project, JetBrains
    /// polyrepo) populate multiple entries; single-root editors produce exactly one entry.
    workspace_roots: Vec<PathBuf>,
    workspace_root_names: Vec<String>,
}

fn default_workspace_alias(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("root")
        .to_string()
}

/// Returns `true` for "useless" roots that indicate the server started with the
/// wrong cwd (usually $HOME or filesystem root on any OS).
fn is_dead_root(p: &std::path::Path) -> bool {
    // `parent() == None` is the universal OS-root test across all platforms:
    //   `/`.parent()    → None  (Unix)
    //   `C:\`.parent()  → None  (Windows drive root — the old `count <= 1`
    //                            check missed this because C:\ has 2 components:
    //                            Prefix("C:") + RootDir)
    if p.parent().is_none() {
        return true;
    }
    // Bare single-component paths (e.g. ".") are also useless.
    if p.components().count() <= 1 {
        return true;
    }
    // Also catch $HOME specifically — no real project lives directly there.
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        if p == std::path::Path::new(home.trim()) {
            return true;
        }
    }
    false
}

/// Parse a file URI (or plain path string) into an OS-native `PathBuf`.
///
/// Handles the cross-platform quirk that a simple `trim_start_matches("file://")`
/// gets wrong on Windows:
///
/// | URI input                        | Unix result             | Windows result       |
/// |----------------------------------|-------------------------|----------------------|
/// | `file:///Users/hero/project`     | `/Users/hero/project`   | (same — harmless)    |
/// | `file:///C:/Users/hero/project`  | `/C:/Users/hero/proj`   | `C:/Users/hero/proj` |
/// | plain `/Users/hero/project`      | `/Users/hero/project`   | (same)               |
///
/// On Windows, RFC 8089 file URIs encode the drive as `file:///C:/...`; after
/// stripping `file://` the leftover `/C:/...` must have its leading slash
/// removed to produce a valid Windows path.  We detect this with a byte-level
/// check (`bytes[1]` is ASCII alpha + `bytes[2]` == `:`), which cannot fire
/// for a legitimate Unix absolute path segment (e.g. `/Users/...`).
fn extract_path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);

    // Windows drive-root artifact: strip the spurious leading `/` in `/C:/...`
    // so the result is a valid Windows path `C:/...`.
    let rest = if rest.starts_with('/')
        && rest.len() >= 3
        && rest.as_bytes()[1].is_ascii_alphabetic()
        && rest.as_bytes()[2] == b':'
    {
        &rest[1..]
    } else {
        rest
    };

    let s = rest.trim_end_matches('/');
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

impl ServerState {
    /// Returns the primary workspace root (first in `workspace_roots`).
    #[allow(dead_code)]
    pub(crate) fn primary_root(&self) -> Option<&PathBuf> {
        self.workspace_roots.first()
    }

    /// Returns all currently known workspace roots in priority order.
    pub fn workspace_roots(&self) -> &[PathBuf] {
        &self.workspace_roots
    }

    pub fn workspace_root_names(&self) -> &[String] {
        &self.workspace_root_names
    }

    /// Parses the MCP `initialize` request parameters and stores all workspace roots.
    ///
    /// Priority: `workspaceFolders` (all entries) → `rootUri` → `rootPath`.
    /// When `workspaceFolders` contains multiple entries (VS Code multi-root workspace,
    /// Zed multi-project, JetBrains polyrepo) every folder is captured so that scanning,
    /// path relativisation, and cache-dir hashing all see the complete set.
    pub fn capture_init_root(&mut self, params: &serde_json::Value) {
        // Collect every workspaceFolders entry (multi-root path).
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut names: Vec<String> = Vec::new();

        if let Some(arr) = params.get("workspaceFolders").and_then(|f| f.as_array()) {
            for folder in arr {
                let raw_path = folder
                    .get("uri")
                    .or_else(|| folder.get("path"))
                    .and_then(|v| v.as_str());
                if let Some(path) = raw_path.and_then(extract_path_from_uri) {
                    let name = folder
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| default_workspace_alias(&path));
                    roots.push(path);
                    names.push(name);
                }
            }
        }

        // Fallback to rootUri / rootPath when workspaceFolders is absent or empty.
        if roots.is_empty() {
            let raw_uri = params
                .get("rootUri")
                .or_else(|| params.get("rootPath"))
                .and_then(|v| v.as_str());
            if let Some(r) = raw_uri.and_then(extract_path_from_uri) {
                names.push(default_workspace_alias(&r));
                roots.push(r);
            }
        }

        // The protocol root is authoritative — overwrite any earlier bootstrap
        // value (env vars / --root) so the editor's own answer always wins.
        if !roots.is_empty() {
            self.workspace_roots = roots;
            self.workspace_root_names = names;
        }
    }

    fn repo_root_from_params(&mut self, params: &serde_json::Value) -> Result<PathBuf, String> {
        // ── Step 1: Explicit parameter (highest priority) ─────────────────────
        if let Some(path) = params.get("repoPath").and_then(|v| v.as_str()) {
            let pb = PathBuf::from(path);
            self.workspace_roots = vec![pb.clone()];
            return Ok(pb);
        }

        // ── Step 2: Cached root (from MCP `initialize` or prior successful call)
        // This covers: --root CLI flag, CORTEXAST_ROOT, any IDE env var captured
        // at startup, and the MCP initialize protocol root (authoritative).
        if let Some(root) = self.workspace_roots.first() {
            return Ok(root.clone());
        }

        // ── Step 3: Cross-IDE environment variable cascade ────────────────────
        // Reached only when workspace_roots wasn't populated at startup (e.g. the IDE
        // didn't export env vars into the MCP subprocess, AND no initialize was
        // received yet). Belt-and-suspenders: check the vars directly here too.
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        // PWD and INIT_CWD must be filtered — they equal $HOME when the IDE
        // spawns the process in the wrong dir, which is a dead root.
        let env_root = std::env::var("CORTEXAST_ROOT")
            .ok()
            .or_else(|| std::env::var("VSCODE_WORKSPACE_FOLDER").ok())
            .or_else(|| std::env::var("IDEA_INITIAL_DIRECTORY").ok())
            .or_else(|| {
                std::env::var("INIT_CWD")
                    .ok()
                    .filter(|v| v.trim() != home.trim())
            })
            .or_else(|| {
                std::env::var("PWD")
                    .ok()
                    .filter(|v| v.trim() != home.trim())
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        if let Some(pb) = env_root {
            self.workspace_roots = vec![pb.clone()];
            return Ok(pb);
        }

        // ── Step 4: Find-up heuristic on the tool's path hint ─────────────────
        // Walk the hint's ancestor chain looking for a project root marker
        // (.git, Cargo.toml, package.json). This recovers cleanly even when the
        // hint is relative, as long as we can anchor it to an absolute base.
        let target_hint = params
            .get("target_dir")
            .or_else(|| params.get("path"))
            .or_else(|| params.get("target"))
            .and_then(|v| v.as_str());

        if let Some(hint) = target_hint {
            let hint_path = PathBuf::from(hint);
            let abs = if hint_path.is_absolute() {
                hint_path
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(hint_path)
            };
            let mut current = abs;
            while let Some(parent) = current.parent() {
                if parent.join(".git").exists()
                    || parent.join("Cargo.toml").exists()
                    || parent.join("package.json").exists()
                {
                    let found = parent.to_path_buf();
                    self.workspace_roots = vec![found.clone()];
                    return Ok(found);
                }
                current = parent.to_path_buf();
            }
        }

        // ── Step 5: CRITICAL safeguard — last resort is cwd ──────────────────
        let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if is_dead_root(&fallback) {
            return Err(format!(
                "CRITICAL: Workspace root resolved to '{}' (OS root or Home directory). \
                This would allow tools to destructively scan the entire filesystem. \
                Please provide the 'repoPath' parameter pointing to your project directory, \
                e.g. repoPath='/Users/you/projects/my-app'.",
                fallback.display()
            ));
        }

        self.workspace_roots = vec![fallback.clone()];
        Ok(fallback)
    }

    /// Resolves the Omni-AST target project logic using the Cortex-DB project map.
    /// `target_project` must be a path that exists in the DB-backed known_projects map.
    fn resolve_target_project(&mut self, params: &serde_json::Value) -> Result<PathBuf, String> {
        // 1. Retrieve standard `repo_root` as fallback
        let base_root = self.repo_root_from_params(params)?;

        // 2. Check for Omni-AST `target_project` override
        if let Some(target_proj_str) = params
            .get("target_project")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            // 3. Load Cortex-DB project map
            let db = cortex_db::LanceDb::open_default_sync()
                .map_err(|e| format!("Failed to open Cortex DB for project map lookup: {e}"))?;
            let project_map = cortex_db::project_store::list_all(&db)
                .map_err(|e| format!("Failed to read project map from Cortex DB: {e}"))?;

            let codebases = project_map
                .get("projects")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "Invalid project map format: missing `projects` array.".to_string())?;

            // 4. Resolve by exact path match (or canonical-equivalent match)
            let mut resolved_path = None;
            let target_canon = std::fs::canonicalize(target_proj_str).ok();
            for codebase in codebases {
                let path = codebase
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if path.is_empty() {
                    continue;
                }

                if target_proj_str == path {
                    resolved_path = Some(PathBuf::from(path));
                    break;
                }

                if let (Some(tc), Ok(pc)) = (target_canon.as_ref(), std::fs::canonicalize(path)) {
                    if *tc == pc {
                        resolved_path = Some(PathBuf::from(path));
                        break;
                    }
                }
            }

            // 5. Enforce project-map membership
            let override_path = match resolved_path {
                Some(p) => p,
                None => {
                    return Err(format!(
                        "CRITICAL: Omni-AST target_project '{}' is not present in the Cortex DB project map. Track it first via cortex_mesh_manage_map(action='track').",
                        target_proj_str
                    ));
                }
            };

            if !override_path.exists() {
                return Err(format!(
                    "CRITICAL: Omni-AST target_project path does not exist on disk: '{}'",
                    override_path.display()
                ));
            }

            return Ok(override_path);
        }

        // Default to the standard base_root
        Ok(base_root)
    }

    pub fn tool_list(&self, id: serde_json::Value) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": crate::tool_schemas::all_tool_schemas()
            }
        })
    }

    pub fn tool_call(
        &mut self,
        id: serde_json::Value,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let max_chars = negotiated_max_chars(&args);

        let ok = |text: String| {
            let text = force_inline_truncate(text, max_chars);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{"type":"text","text": text }], "isError": false }
            })
        };

        let err = |msg: String| {
            let msg = force_inline_truncate(msg, max_chars);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{"type":"text","text": msg }], "isError": true }
            })
        };

        match name {
            // ── Megatools ────────────────────────────────────────────────
            "cortex_manage_ast_languages" => {
                let repo_root = if args
                    .get("action")
                    .and_then(|v| v.as_str())
                    .map(|v| v.trim() == "add")
                    .unwrap_or(false)
                {
                    Some(match self.resolve_target_project(&args) {
                        Ok(root) => root,
                        Err(e) => return err(e),
                    })
                } else {
                    None
                };

                match crate::grammar_manager::handle_tool_call(
                    &args,
                    repo_root.as_deref(),
                    &self.workspace_roots,
                ) {
                    Ok(text) => ok(text),
                    Err(message) => err(message),
                }
            }

            "cortex_code_explorer" => {
                let action = args
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                match action {
                    "workspace_topology" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let cfg = load_config(&repo_root);
                        let discovery_opts = crate::workspace::WorkspaceDiscoveryOptions {
                            max_depth: cfg.huge_codebase.member_scan_depth,
                            include_patterns: cfg.huge_codebase.include_members.clone(),
                            exclude_patterns: cfg.huge_codebase.exclude_members.clone(),
                        };
                        let topology_roots = effective_workspace_roots(&repo_root, &self.workspace_roots, &args);
                        match crate::workspace::render_workspace_topology(&topology_roots, &discovery_opts) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("workspace_topology failed: {e}")),
                        }
                    }
                    "map_overview" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let target_strs = parse_string_array_arg(&args, "target_dirs", "target_dir");
                        if target_strs.is_empty() {
                            return err(
                                "Error: action 'map_overview' requires 'target_dirs' (array of directories). \
                                In multi-root workspaces, prefer explicit prefixes such as target_dirs=['[Host]','[Daemon]'] instead of '.'. \
                                You can operate on multiple workspace roots simultaneously. Provide arrays of target directories to analyze cross-repo features.".to_string()
                            );
                        }
                        let search_filter = args
                            .get("search_filter")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let max_chars = Some(max_chars);
                        let ignore_gitignore = args
                            .get("ignore_gitignore")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let exclude_dirs: Vec<String> = args
                            .get("exclude")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let target_dirs: Vec<PathBuf> = target_strs
                            .iter()
                            .map(|target_str| resolve_path(&repo_root, &self.workspace_roots, target_str))
                            .collect();

                        // Proactive guardrail: agents often hallucinate paths.
                        for (target_str, target_dir) in target_strs.iter().zip(target_dirs.iter()) {
                            if !target_dir.exists() {
                                let mut entries: Vec<String> = Vec::new();
                                if let Ok(rd) = std::fs::read_dir(&repo_root) {
                                    for e in rd.flatten() {
                                        if let Some(name) = e.file_name().to_str() {
                                            entries.push(name.to_string());
                                        }
                                    }
                                }
                                entries.sort();
                                let shown: Vec<String> = entries.into_iter().take(30).collect();
                                return err(format!(
                                    "Error: Path '{}' does not exist in repo root '{}'.\n\
Available top-level entries in this repo: [{}].\n\
Please correct your target_dirs array (or pass repoPath explicitly).\n\
You can operate on multiple workspace roots simultaneously. Provide arrays of target directories (e.g. target_dirs=['[ProjectA]','[ProjectB]']) to analyze cross-repo features.",
                                    target_str,
                                    repo_root.display(),
                                    shown
                                        .into_iter()
                                        .map(|s| format!("'{}'", s))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ));
                            }
                        }

                        match repo_map_with_filter(
                            &target_dirs,
                            search_filter,
                            max_chars,
                            ignore_gitignore,
                            &exclude_dirs,
                        ) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("repo_map failed: {e}")),
                        }
                    }
                    "skeleton" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let target_strs = parse_string_array_arg(&args, "target_dirs", "target_dir");
                        let target_dirs: Vec<PathBuf> = if target_strs.is_empty() {
                            effective_workspace_roots(&repo_root, &self.workspace_roots, &args)
                        } else {
                            target_strs
                                .iter()
                                .map(|target_str| resolve_path(&repo_root, &self.workspace_roots, target_str))
                                .collect()
                        };
                        match crate::skeleton::render_project_skeleton(
                            &repo_root,
                            &self.workspace_roots,
                            &target_dirs,
                            &args,
                        ) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("skeleton failed: {e}")),
                        }
                    }
                    "deep_slice" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let Some(target_str) = args.get("target").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'deep_slice' requires the 'target' parameter \
                                (relative path to a file or directory within the repo, e.g. 'src'). \
                                Please call cortex_code_explorer again with action='deep_slice' and target='<path>'.".to_string()
                            );
                        };
                        let target = resolve_path(&repo_root, &self.workspace_roots, target_str);

                        // Proactive path guard: give a "did you mean?" hint when the target
                        // doesn't exist (e.g. agent passes "orchestrator" instead of "orchestrator.rs").
                        {
                            let target_abs = target.clone();
                            if !target_abs.exists() {
                                let stem = target_abs
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(target_str)
                                    .to_ascii_lowercase();
                                let parent = target_abs.parent().unwrap_or(&repo_root);
                                let search_root = if parent.exists() { parent } else { &repo_root };
                                let mut suggestions: Vec<String> = Vec::new();
                                if let Ok(rd) = std::fs::read_dir(search_root) {
                                    for e in rd.flatten() {
                                        let fname = e.file_name();
                                        let fname_str = fname.to_string_lossy();
                                        if fname_str.to_ascii_lowercase().contains(&stem) {
                                            if let Ok(rel) = e.path().strip_prefix(&repo_root) {
                                                suggestions
                                                    .push(rel.to_string_lossy().replace('\\', "/"));
                                            }
                                        }
                                    }
                                }
                                suggestions.sort();
                                suggestions.truncate(5);
                                let hint = if suggestions.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        "\nDid you mean one of: {}",
                                        suggestions
                                            .iter()
                                            .map(|s| format!("'{s}'"))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )
                                };
                                return err(format!(
                                    "Error: Target '{}' does not exist in repo root '{}'.{hint}\n\
                                    Tip: Use cortex_code_explorer(action='workspace_topology') first, then cortex_code_explorer(action='map_overview', target_dirs=['[ProjectA]']) \
                                    to browse the repo structure first.",
                                    target_str,
                                    repo_root.display(),
                                ));
                            }
                        }

                        let budget_tokens = args
                            .get("budget_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(32_000) as usize;
                        let skeleton_only = args
                            .get("skeleton_only")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let mut cfg = load_config(&repo_root);

                        // Merge per-call exclude dirs into config so build_scan_options picks them up.
                        if let Some(arr) = args.get("exclude").and_then(|v| v.as_array()) {
                            let extra: Vec<String> = arr
                                .iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect();
                            cfg.scan.exclude_dir_names.extend(extra);
                        }

                        // `single_file=true` bypasses all vector search — returns exactly the
                        // target file/dir without any semantic cross-file expansion.
                        let single_file = args
                            .get("single_file")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        // `only_dirs` scopes vector-search candidates to one or more subdirectories
                        // (poly-repo support). When combined with `query=`, prevents cross-module spill.
                        let only_dirs = parse_string_array_arg(&args, "only_dirs", "only_dir");
                        let only_dir_prefixes: Vec<String> = only_dirs
                            .iter()
                            .filter_map(|s| normalize_scope_prefix(&repo_root, &self.workspace_roots, s))
                            .collect();

                        // Optional vector search query (skipped when single_file=true).
                        if !single_file {
                            if let Some(q) = args
                                .get("query")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                let query_limit = args
                                    .get("query_limit")
                                    .and_then(|v| v.as_u64())
                                    .map(|n| n as usize);
                                match self.run_query_slice(
                                    &repo_root,
                                    &target,
                                    &only_dir_prefixes,
                                    q,
                                    query_limit,
                                    budget_tokens,
                                    skeleton_only,
                                    &cfg,
                                ) {
                                    Ok(xml) => return ok(xml),
                                    Err(e) => return err(format!("query slice failed: {e}")),
                                }
                            }
                        }

                        match slice_to_xml(&repo_root, &self.workspace_roots, &target, budget_tokens, &cfg, skeleton_only)
                        {
                            Ok((xml, _meta)) => ok(xml),
                            Err(e) => err(format!("slice failed: {e}")),
                        }
                    }
                    _ => err(format!(
                        "Error: Invalid or missing 'action' for cortex_code_explorer: received '{action}'. \
                        Choose one of: 'workspace_topology' (project-only workspace summary), 'map_overview' (repo structure map), 'deep_slice' (token-budgeted content slice), or 'skeleton' (project-wide YAML signatures). \
                        You can operate on multiple workspace roots simultaneously. Provide arrays of target directories, e.g. target_dirs=['[ProjectA]','[ProjectB]']."
                    )),
                }
            }
            "cortex_symbol_analyzer" => {
                let action = args
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                match action {
                    "read_source" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'read_source' requires both 'path' (source file containing the symbol) \
                                and 'symbol_name'. You omitted 'path'. \
                                Please call cortex_symbol_analyzer again with action='read_source', path='<file>', and symbol_name='<name>'. \
                                Tip: use cortex_code_explorer(action=map_overview) first if you are unsure of the file path.".to_string()
                            );
                        };
                        let abs = resolve_path(&repo_root, &self.workspace_roots, p);
                        let skeleton_only = args
                            .get("skeleton_only")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        // Multi-symbol batching: symbol_names: ["A", "B", ...]
                        if let Some(arr) = args.get("symbol_names").and_then(|v| v.as_array()) {
                            let mut out_parts: Vec<String> = Vec::new();
                            for v in arr {
                                let Some(sym) = v.as_str().filter(|s| !s.trim().is_empty()) else {
                                    continue;
                                };
                                match read_symbol_with_options(&abs, sym, skeleton_only, None) {
                                    Ok(s) => out_parts.push(s),
                                    Err(e) => {
                                        out_parts.push(format!("// ERROR reading `{sym}`: {e}"))
                                    }
                                }
                            }
                            if out_parts.is_empty() {
                                return err(
                                    "Error: action 'read_source' with 'symbol_names' requires a non-empty array of symbol name strings. \
                                    You provided an empty array or all entries were blank. \
                                    Example: symbol_names=['process_request', 'handle_error']".to_string()
                                );
                            }
                            return ok(out_parts.join("\n\n"));
                        }

                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'read_source' requires both 'path' and 'symbol_name'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='read_source', path='<file>', and symbol_name='<name>'. \
                                For batch extraction of multiple symbols from the same file, use symbol_names=['A','B'] instead.".to_string()
                            );
                        };
                        let instance_index = args
                            .get("instance_index")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as usize);
                        match read_symbol_with_options(&abs, sym, skeleton_only, instance_index) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("read_symbol failed: {e}")),
                        }
                    }
                    "find_usages" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str())
                        else {
                            return err(
                                "Error: action 'find_usages' requires both 'symbol_name' and 'target_dir'. You omitted 'target_dir'. \
                                Use '.' to search the entire repo. \
                                Please call cortex_symbol_analyzer again with action='find_usages', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'find_usages' requires both 'symbol_name' and 'target_dir'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='find_usages', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let target_dir = resolve_path(&repo_root, &self.workspace_roots, target_str);
                        match find_usages(&target_dir, sym) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("find_usages failed: {e}")),
                        }
                    }
                    "find_implementations" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str())
                        else {
                            return err(
                                "Error: action 'find_implementations' requires both 'symbol_name' and 'target_dir'. You omitted 'target_dir'. \
                                Use '.' to search the entire repo. \
                                Please call cortex_symbol_analyzer again with action='find_implementations', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'find_implementations' requires both 'symbol_name' and 'target_dir'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='find_implementations', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let target_dir = resolve_path(&repo_root, &self.workspace_roots, target_str);
                        match find_implementations(&target_dir, sym) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("find_implementations failed: {e}")),
                        }
                    }
                    "blast_radius" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let Some(target_str) = args.get("target_dir").and_then(|v| v.as_str())
                        else {
                            return err(
                                "Error: action 'blast_radius' requires both 'symbol_name' and 'target_dir'. You omitted 'target_dir'. \
                                Use '.' to search the entire repo. \
                                Please call cortex_symbol_analyzer again with action='blast_radius', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'blast_radius' requires both 'symbol_name' and 'target_dir'. You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='blast_radius', symbol_name='<name>', and target_dir='.'.".to_string()
                            );
                        };
                        let target_dir = resolve_path(&repo_root, &self.workspace_roots, target_str);
                        match call_hierarchy(&target_dir, sym) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("call_hierarchy failed: {e}")),
                        }
                    }
                    "propagation_checklist" => {
                        let repo_root = match self.resolve_target_project(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        // Legacy mode: changed_path checklist (if provided).
                        if let Some(changed_path) = args
                            .get("changed_path")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                        {
                            let abs = resolve_path(&repo_root, &self.workspace_roots, changed_path);
                            let max_symbols =
                                args.get("max_symbols")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(20) as usize;

                            let mut out = String::new();
                            out.push_str("Propagation checklist\n");
                            out.push_str(&format!("Changed: {}\n\n", abs.display()));

                            let ext = abs
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            if ext == "proto" {
                                let raw = std::fs::read_to_string(&abs);
                                if let Ok(text) = raw {
                                    let syms = extract_symbols_from_source(&abs, &text);
                                    if !syms.is_empty() {
                                        out.push_str("Detected contract symbols (sample):\n");
                                        for s in syms.into_iter().take(max_symbols) {
                                            out.push_str(&format!("- [{}] {}\n", s.kind, s.name));
                                        }
                                        out.push('\n');
                                    }
                                }

                                out.push_str("Checklist (Proto → generated clients):\n");
                                out.push_str("- Regenerate Rust stubs (prost/tonic build, buf, or your codegen pipeline)\n");
                                out.push_str("- Regenerate TypeScript/JS clients (grpc-web/connect/buf generate, etc.)\n");
                                out.push_str("- Update server handlers for any renamed RPCs/messages/enums\n");
                                out.push_str("- Run `cortex_run_diagnostics` and service-level tests\n\n");
                                out.push_str("Suggested CortexAST probes (fast, AST-accurate):\n");
                                out.push_str("- `cortex_code_explorer` action=map_overview with `search_filter` set to the service/message name\n");
                                out.push_str("- `cortex_symbol_analyzer` action=find_usages for each renamed message/service to find all consumers\n");
                            } else {
                                out.push_str("Checklist (API change propagation):\n");
                                out.push_str("- `cortex_symbol_analyzer` action=find_usages on the changed symbol(s) to locate all call sites\n");
                                out.push_str("- `cortex_symbol_analyzer` action=blast_radius to understand blast radius before refactoring\n");
                                out.push_str("- Update dependent modules/services and re-run `cortex_run_diagnostics`\n");
                            }

                            return ok(out);
                        }

                        // New mode: symbol-based cross-boundary checklist.
                        let Some(sym) = args
                            .get("symbol_name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                        else {
                            return err(
                                "Error: action 'propagation_checklist' requires 'symbol_name' (the shared type/struct/interface to trace). \
                                You omitted 'symbol_name'. \
                                Please call cortex_symbol_analyzer again with action='propagation_checklist' and symbol_name='<name>'. \
                                Alternatively, pass 'changed_path' (path to a .proto or contract file) for legacy file-based mode.".to_string()
                            );
                        };
                        let target_str = args
                            .get("target_dir")
                            .and_then(|v| v.as_str())
                            .unwrap_or(".");
                        let target_dir = resolve_path(&repo_root, &self.workspace_roots, target_str);
                        let ignore_gitignore = args
                            .get("ignore_gitignore")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        // `only_dir` overrides `target_dir` — scopes scan to a single microservice
                        // directory in poly-repo setups without changing the default API surface.
                        let scan_dir = if let Some(od) = args
                            .get("only_dir")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            resolve_path(&repo_root, &self.workspace_roots, od)
                        } else {
                            target_dir
                        };

                        let aliases: Vec<String> = args
                            .get("aliases")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty())
                                    .map(|s| s.to_string())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();

                        match propagation_checklist(&scan_dir, sym, &aliases, ignore_gitignore) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("propagation_checklist failed: {e}")),
                        }
                    }
                    _ => err(format!(
                        "Error: Invalid or missing 'action' for cortex_symbol_analyzer: received '{action}'. \
                        Choose one of: 'read_source' (extract symbol AST), 'find_usages' (trace all call sites), 'find_implementations' (find implementors of a trait/interface), \
                        'blast_radius' (call hierarchy before rename/delete), or 'propagation_checklist' (cross-module update checklist). \
                        Example: cortex_symbol_analyzer with action='find_usages', symbol_name='my_fn', and target_dir='.'"
                    )),
                }
            }
            "cortex_chronos" => {
                let action = args
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                match action {
                    "save_checkpoint" => {
                        let repo_root = match self.repo_root_from_params(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let cfg = load_config(&repo_root);
                        let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'save_checkpoint' requires 'path' (source file), 'symbol_name', and 'semantic_tag'. \
                                You omitted 'path'. \
                                Please call cortex_chronos again with action='save_checkpoint', path='<file>', \
                                symbol_name='<name>', and semantic_tag='pre-refactor' (or any descriptive tag).".to_string()
                            );
                        };
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'save_checkpoint' requires 'path', 'symbol_name', and 'semantic_tag'. \
                                You omitted 'symbol_name'. \
                                Please call cortex_chronos again with action='save_checkpoint', path='<file>', \
                                symbol_name='<name>', and semantic_tag='pre-refactor'.".to_string()
                            );
                        };
                        let tag = args
                            .get("semantic_tag")
                            .and_then(|v| v.as_str())
                            .or_else(|| args.get("tag").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        let namespace = args.get("namespace").and_then(|v| v.as_str());
                        match checkpoint_symbol(
                            &repo_root,
                            &self.workspace_roots,
                            &cfg,
                            p,
                            sym,
                            tag,
                            namespace,
                        ) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("checkpoint_symbol failed: {e}")),
                        }
                    }
                    "list_checkpoints" => {
                        let repo_root = match self.repo_root_from_params(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let cfg = load_config(&repo_root);
                        let namespace = args.get("namespace").and_then(|v| v.as_str());
                        match list_checkpoints(&repo_root, &cfg, namespace) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("list_checkpoints failed: {e}")),
                        }
                    }
                    "compare_checkpoint" => {
                        let repo_root = match self.repo_root_from_params(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let cfg = load_config(&repo_root);
                        let Some(sym) = args.get("symbol_name").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'compare_checkpoint' requires 'symbol_name', 'tag_a', and 'tag_b'. \
                                You omitted 'symbol_name'. \
                                Please call cortex_chronos again with action='compare_checkpoint', \
                                symbol_name='<name>', tag_a='<before-tag>', and tag_b='<after-tag>'. \
                                Tip: call cortex_chronos(action=list_checkpoints) first to see all available tags.".to_string()
                            );
                        };
                        let Some(tag_a) = args.get("tag_a").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'compare_checkpoint' requires 'symbol_name', 'tag_a', and 'tag_b'. \
                                You omitted 'tag_a' (the 'before' snapshot tag). \
                                Please call cortex_chronos again with action='compare_checkpoint', \
                                symbol_name='<name>', tag_a='<before-tag>', and tag_b='<after-tag>'. \
                                Tip: call cortex_chronos(action=list_checkpoints) to see all available tags.".to_string()
                            );
                        };
                        let Some(tag_b) = args.get("tag_b").and_then(|v| v.as_str()) else {
                            return err(
                                "Error: action 'compare_checkpoint' requires 'symbol_name', 'tag_a', and 'tag_b'. \
                                You omitted 'tag_b' (the 'after' snapshot tag). \
                                Please call cortex_chronos again with action='compare_checkpoint', \
                                symbol_name='<name>', tag_a='<before-tag>', and tag_b='<after-tag>'.".to_string()
                            );
                        };
                        let path = args.get("path").and_then(|v| v.as_str());
                        let namespace = args.get("namespace").and_then(|v| v.as_str());
                        if tag_b.trim() == "__live__" && path.is_none() {
                            return err(
                                "Error: tag_b='__live__' requires 'path' (the source file containing the symbol). \
Please call cortex_chronos again with action='compare_checkpoint', symbol_name='<name>', tag_a='<snapshot-tag>', tag_b='__live__', and path='<file>'.".to_string()
                            );
                        }
                        match compare_symbol(
                            &repo_root,
                            &self.workspace_roots,
                            &cfg,
                            sym,
                            tag_a,
                            tag_b,
                            path,
                            namespace,
                        ) {
                            Ok(s) => ok(s),
                            Err(e) => {
                                let msg = e.to_string();
                                if msg.contains("No checkpoint found")
                                    || msg.contains("No checkpoints found")
                                {
                                    err(format!(
                                        "compare_symbol failed: {msg}\n\n\
Tip: run cortex_chronos(action=list_checkpoints) to see valid tag+symbol combinations, then retry.\n\
Common cause: you saved a checkpoint for a different symbol or under a different tag."
                                    ))
                                } else {
                                    err(format!("compare_symbol failed: {msg}"))
                                }
                            }
                        }
                    }
                    "delete_checkpoint" => {
                        let repo_root = match self.repo_root_from_params(&args) {
                            Ok(r) => r,
                            Err(e) => return err(e),
                        };
                        let cfg = load_config(&repo_root);

                        let symbol_name = args
                            .get("symbol_name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let semantic_tag = args
                            .get("semantic_tag")
                            .and_then(|v| v.as_str())
                            .or_else(|| args.get("tag").and_then(|v| v.as_str()))
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty());
                        let path = args.get("path").and_then(|v| v.as_str());
                        let namespace = args.get("namespace").and_then(|v| v.as_str());

                        // Allow namespace-only purge (omit symbol_name + semantic_tag to wipe
                        // an entire namespace in one call, e.g. cleaning up a QC run).
                        // Only reject if ALL of: no namespace context AND no filters.
                        let has_namespace =
                            namespace.map(|s| !s.trim().is_empty()).unwrap_or(false);
                        if symbol_name.is_none()
                            && semantic_tag.is_none()
                            && path.is_none()
                            && !has_namespace
                        {
                            return err(
                                "Error: action 'delete_checkpoint' requires at least one filter: 'symbol_name', 'semantic_tag'/'tag', or 'namespace'. \
Provide 'namespace' alone to purge an entire namespace (e.g. namespace='qa-run-1'). \
Call cortex_chronos with action='list_checkpoints' first to see what exists.".to_string(),
                            );
                        }

                        match crate::chronos::delete_checkpoints(
                            &repo_root,
                            &self.workspace_roots,
                            &cfg,
                            symbol_name,
                            semantic_tag,
                            path,
                            namespace,
                        ) {
                            Ok(s) => ok(s),
                            Err(e) => err(format!("delete_checkpoints failed: {e}")),
                        }
                    }
                    _ => err(format!(
                        "Error: Invalid or missing 'action' for cortex_chronos: received '{action}'. \
                        Choose one of: 'save_checkpoint' (snapshot before edit), 'list_checkpoints' (show all snapshots), \
                        'compare_checkpoint' (AST diff after edit), or 'delete_checkpoint' (remove saved checkpoints). \
                        Example: cortex_chronos with action='save_checkpoint', path='src/main.rs', symbol_name='my_fn', and semantic_tag='pre-refactor'"
                    )),
                }
            }

            // Compat shim — removed from tools/list; kept functional for old clients
            "cortex_run_diagnostics" => {
                let repo_root = match self.repo_root_from_params(&args) {
                    Ok(r) => r,
                    Err(e) => return err(e),
                };
                match run_diagnostics(&repo_root) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("diagnostics failed: {e}")),
                }
            }

            // ── Compatibility shims (not exposed in tool_list) ───────────
            // Keep these aliases so existing clients don't instantly break.
            "map_repo" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("map_overview");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_code_explorer", "arguments": new_args }),
                )
            }
            "get_context_slice" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("deep_slice");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_code_explorer", "arguments": new_args }),
                )
            }
            "read_symbol" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("read_source");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }),
                )
            }
            "find_usages" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("find_usages");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }),
                )
            }
            "call_hierarchy" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("blast_radius");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }),
                )
            }
            "propagation_checklist" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("propagation_checklist");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_symbol_analyzer", "arguments": new_args }),
                )
            }
            "save_checkpoint" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("save_checkpoint");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_chronos", "arguments": new_args }),
                )
            }
            "list_checkpoints" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("list_checkpoints");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_chronos", "arguments": new_args }),
                )
            }
            "compare_checkpoint" => {
                let mut new_args = args.clone();
                if new_args.get("action").is_none() {
                    new_args["action"] = json!("compare_checkpoint");
                }
                self.tool_call(
                    id,
                    &json!({ "name": "cortex_chronos", "arguments": new_args }),
                )
            }

            // Deprecated (kept for now): skeleton reader
            "read_file_skeleton" => {
                let repo_root = match self.repo_root_from_params(&args) {
                    Ok(r) => r,
                    Err(e) => return err(e),
                };
                let Some(p) = args.get("path").and_then(|v| v.as_str()) else {
                    return err("Missing path".to_string());
                };
                let abs = resolve_path(&repo_root, &self.workspace_roots, p);
                match render_skeleton(&abs) {
                    Ok(s) => ok(s),
                    Err(e) => err(format!("skeleton failed: {e}")),
                }
            }

            _ => err(format!("Tool not found: {name}")),
        }
    }

    /// Run vector-search-based slicing (query mode) from the MCP server.
    #[allow(clippy::too_many_arguments)]
    fn run_query_slice(
        &mut self,
        repo_root: &std::path::Path,
        target: &std::path::Path,
        only_dirs: &[String],
        query: &str,
        query_limit: Option<usize>,
        budget_tokens: usize,
        skeleton_only: bool,
        cfg: &crate::config::Config,
    ) -> anyhow::Result<String> {
        let mut exclude_dir_names = vec![
            ".git".into(),
            "node_modules".into(),
            "dist".into(),
            "target".into(),
            cfg.output_dir.to_string_lossy().to_string(),
        ];
        exclude_dir_names.extend(cfg.scan.exclude_dir_names.iter().cloned());

        let opts = ScanOptions {
            repo_root: repo_root.to_path_buf(),
            workspace_roots: self.workspace_roots.clone(),
            target: target.to_path_buf(),
            max_file_bytes: cfg.token_estimator.max_file_bytes,
            exclude_dir_names,
            extra_glob_excludes: Vec::new(),
        };
        let entries = scan_workspace(&opts)?;

        let auto_scope_prefixes = if only_dirs.is_empty() {
            auto_scope_prefixes(repo_root, &self.workspace_roots, target)
        } else {
            only_dirs.to_vec()
        };

        let candidate_entries: Vec<&crate::scanner::FileEntry> = if auto_scope_prefixes.is_empty() {
            entries.iter().collect()
        } else {
            entries
                .iter()
                .filter(|e| {
                    let rel = e.rel_path.to_string_lossy().replace('\\', "/");
                    auto_scope_prefixes
                        .iter()
                        .any(|prefix| rel == *prefix || rel.starts_with(&format!("{prefix}/")))
                })
                .collect()
        };

        let db_dir = crate::config::central_cache_dir(&self.workspace_roots)
            .unwrap_or_else(|| repo_root.join(&cfg.output_dir))
            .join("db");
        let model_id = cfg.vector_search.model.as_str();
        let chunk_lines = cfg.vector_search.chunk_lines;
        let mut index = CodebaseIndex::open(repo_root, &db_dir, model_id, chunk_lines)?;

        let limit = query_limit.unwrap_or_else(|| {
            let budget_based = (budget_tokens / 1_500).clamp(8, 60);
            budget_based
                .min(cfg.vector_search.default_query_limit)
                .max(1)
        });
        let max_candidates = (limit * 12).clamp(80, 400);
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| s.len() >= 2)
            .collect();

        let mut scored: Vec<(i32, usize)> = candidate_entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let rel = e.rel_path.to_string_lossy().replace('\\', "/");
                (score_path(&rel, &terms), i)
            })
            .collect();
        scored.sort_by(|(sa, ia), (sb, ib)| {
            sb.cmp(sa)
                .then_with(|| candidate_entries[*ia].bytes.cmp(&candidate_entries[*ib].bytes))
        });

        let mut to_index: Vec<(String, PathBuf)> = Vec::new();
        for (_score, idx) in scored.iter().take(max_candidates) {
            let e = candidate_entries[*idx];
            let rel = e.rel_path.to_string_lossy().replace('\\', "/");
            if matches!(index.needs_reindex_path(&rel, &e.abs_path), Ok(true)) {
                to_index.push((rel, e.abs_path.clone()));
            }
        }

        let jobs: Vec<IndexJob> = to_index
            .par_iter()
            .filter_map(|(rel, abs)| {
                let bytes = std::fs::read(abs).ok()?;
                let content = String::from_utf8(bytes)
                    .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
                Some(IndexJob {
                    rel_path: rel.clone(),
                    abs_path: abs.clone(),
                    content,
                })
            })
            .collect();

        let rt = tokio::runtime::Runtime::new()?;
        let q_owned = query.to_string();
        let mut rel_paths: Vec<String> = rt.block_on(async move {
            let _ = index.index_jobs(&jobs, || {}).await;
            index.search(&q_owned, limit).await.unwrap_or_default()
        });

        if !auto_scope_prefixes.is_empty() {
            rel_paths.retain(|p| {
                auto_scope_prefixes
                    .iter()
                    .any(|prefix| p == prefix || p.starts_with(&format!("{prefix}/")))
            });
        }

        let (xml, _meta) = if rel_paths.is_empty() {
            slice_to_xml(repo_root, &self.workspace_roots, target, budget_tokens, cfg, skeleton_only)?
        } else {
            slice_paths_to_xml(
                repo_root,
                &self.workspace_roots,
                &rel_paths,
                Some(&auto_scope_prefixes),
                budget_tokens,
                cfg,
                skeleton_only,
            )?
        };
        Ok(xml)
    }
}

fn parse_string_array_arg(
    args: &serde_json::Value,
    plural_key: &str,
    singular_key: &str,
) -> Vec<String> {
    if let Some(arr) = args.get(plural_key).and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    args.get(singular_key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| vec![s])
        .unwrap_or_default()
}

fn effective_workspace_roots(
    repo_root: &std::path::Path,
    workspace_roots: &[PathBuf],
    args: &serde_json::Value,
) -> Vec<PathBuf> {
    if args.get("repoPath").and_then(|v| v.as_str()).is_some() {
        return vec![repo_root.to_path_buf()];
    }
    if workspace_roots.is_empty() {
        vec![repo_root.to_path_buf()]
    } else {
        workspace_roots.to_vec()
    }
}

fn normalize_scope_prefix(
    repo_root: &std::path::Path,
    workspace_roots: &[PathBuf],
    raw: &str,
) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('[') {
        return Some(raw.trim_end_matches('/').replace('\\', "/"));
    }

    let pb = PathBuf::from(raw);
    if pb.is_absolute() {
        return rel_prefix_for_path(repo_root, workspace_roots, &pb);
    }

    Some(raw.trim_end_matches('/').replace('\\', "/"))
}

fn rel_prefix_for_path(
    repo_root: &std::path::Path,
    workspace_roots: &[PathBuf],
    path: &std::path::Path,
) -> Option<String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for root in workspace_roots {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if let Ok(rel) = path.strip_prefix(&root) {
            let root_name = root.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
            let rel = rel.to_string_lossy().replace('\\', "/");
            return Some(if rel.is_empty() {
                format!("[{root_name}]")
            } else {
                format!("[{root_name}]/{rel}")
            });
        }
    }

    path.strip_prefix(repo_root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .filter(|s| !s.is_empty())
}

fn auto_scope_prefixes(
    repo_root: &std::path::Path,
    workspace_roots: &[PathBuf],
    target: &std::path::Path,
) -> Vec<String> {
    let scope_path = if target.is_file() {
        target.parent().unwrap_or(target)
    } else {
        target
    };
    rel_prefix_for_path(repo_root, workspace_roots, scope_path)
        .map(|s| vec![s])
        .unwrap_or_default()
}

/// Resolve a path parameter to an absolute `PathBuf`, understanding the
/// `[FolderName]/path/to/file` convention used by multi-root workspaces.
///
/// Resolution priority:
/// 1. Absolute paths — returned as-is.
/// 2. `[FolderName]/rest` — the `FolderName` is matched (case-sensitive) against
///    the directory names of `workspace_roots`. The remainder of the path is then
///    joined against the matching root. This is the "reverse routing" step that
///    lets the LLM target files in any workspace folder unambiguously.
/// 3. Bare relative paths — joined against `repo_root` (the primary root).
fn resolve_path(repo_root: &std::path::Path, workspace_roots: &[PathBuf], p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        return pb;
    }

    // Decode `[FolderName]/rest` prefix produced by the multi-root scanner.
    if let Some(inner) = p.strip_prefix('[') {
        if let Some((folder_name, tail)) = inner.split_once(']') {
            // Strip any leading path separator (forward-slash or backslash) so
            // `[Folder]/path` and `[Folder]\path` both resolve on all platforms.
            let subpath = tail.trim_start_matches(['/', '\\']);

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
            // No matching root found — fall through to standard join.
        }
    }

    repo_root.join(p)
}

fn score_path(rel_path: &str, terms: &[String]) -> i32 {
    let p = rel_path.to_ascii_lowercase();
    let filename = p.rsplit('/').next().unwrap_or(&p);
    let mut score = 0i32;
    for t in terms {
        if filename.contains(t.as_str()) {
            score += 30;
        } else if p.contains(t.as_str()) {
            score += 10;
        }
    }
    score
}

pub fn run_stdio_server(startup_root: Option<PathBuf>) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let mut state = ServerState::default();
    // ── Bootstrap repo_root before the first tool call arrives ──────────────
    // Priority (first non-None wins; the MCP initialize handler may overwrite
    // this later with the editor's authoritative root):
    //
    //   1. --root <PATH>  / CORTEXAST_ROOT     — explicit config (always wins)
    //   2. VSCODE_WORKSPACE_FOLDER             — VS Code / Cursor / Windsurf
    //   3. VSCODE_CWD                          — VS Code secondary
    //   4. IDEA_INITIAL_DIRECTORY              — JetBrains IDEs
    //   5. PWD / INIT_CWD (≠ $HOME)            — Zed, Neovim, npm runners
    //
    // This is a best-effort bootstrap only. The MCP `initialize` request
    // (capture_init_root) is the canonical, protocol-level source and will
    // overwrite this value when the editor sends it.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let env_root = std::env::var("CORTEXAST_ROOT")
        .ok()
        .or_else(|| std::env::var("VSCODE_WORKSPACE_FOLDER").ok())
        .or_else(|| std::env::var("VSCODE_CWD").ok())
        .or_else(|| std::env::var("IDEA_INITIAL_DIRECTORY").ok())
        .or_else(|| {
            std::env::var("PWD")
                .ok()
                .filter(|v| v.trim() != home.trim())
        })
        .or_else(|| {
            std::env::var("INIT_CWD")
                .ok()
                .filter(|v| v.trim() != home.trim())
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    if let Some(r) = startup_root.or(env_root) {
        state.workspace_roots = vec![r];
    }

    for line in stdin.lock().lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }

        let msg: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // JSON-RPC notifications have no "id" field — don't respond.
        let has_id = msg.get("id").is_some();
        if !has_id {
            // Side-effect-only notifications (initialize ack, cancel, log, etc.) — ignore.
            continue;
        }

        let id = msg.get("id").cloned().unwrap_or(json!(null));
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let reply = match method {
            "initialize" => {
                // Capture workspace root from VS Code's initialize params so subsequent
                // tool calls without repoPath resolve to the correct directory.
                if let Some(p) = msg.get("params") {
                    state.capture_init_root(p);
                }
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": msg.get("params").and_then(|p| p.get("protocolVersion")).cloned().unwrap_or(json!("2024-11-05")),
                        "capabilities": { "tools": { "listChanged": true } },
                        "serverInfo": { "name": "cortexast", "version": env!("CARGO_PKG_VERSION") }
                    }
                })
            }
            "ping" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }),
            "tools/list" => state.tool_list(id),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                state.tool_call(id, &params)
            }
            // Return empty lists for resources/prompts — we don't implement them.
            "resources/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "resources": [] }
            }),
            "prompts/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "prompts": [] }
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }),
        };

        writeln!(stdout, "{}", reply)?;
        stdout.flush()?;
    }

    Ok(())
}

const DEFAULT_MAX_CHARS: usize = 8_000;

fn negotiated_max_chars(args: &serde_json::Value) -> usize {
    args.get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_CHARS)
}

/// Hard inline cap: always truncates in the response body — never writes to disk.
/// Safe for any MCP client; the truncation marker makes partial output obvious.
fn force_inline_truncate(mut content: String, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content;
    }
    let total_len = content.len();
    let mut cut = max_chars.min(content.len());
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    content.truncate(cut);
    content.push_str(&format!(
        "\n\n... ✂️ [TRUNCATED: {max_chars}/{total_len} chars to prevent IDE spill]"
    ));
    content
}
