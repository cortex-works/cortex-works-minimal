use serde_json::{Value, json};

pub fn all_tool_schemas() -> Vec<Value> {
    vec![
        code_explorer_schema(),
        symbol_analyzer_schema(),
        chronos_schema(),
        crate::grammar_manager::tool_schema(),
    ]
}

pub fn code_explorer_schema() -> Value {
    json!({
        "name": "cortex_code_explorer",
        "description": "Map an unfamiliar repo, inspect workspace topology, deep-slice relevant files, or emit a signatures-only skeleton. Start with workspace_topology or map_overview for orientation; use deep_slice when you need bodies and context. In single-root repos use plain repo-relative paths such as 'crates/cortex-mcp/src'. Use [FolderName]/... only when MCP initialize provided multiple workspace roots.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["workspace_topology", "map_overview", "deep_slice", "skeleton"],
                    "description": "workspace_topology: ultra-low-token workspace summary listing only discovered projects, manifests, and language hints. map_overview: bird's-eye symbol map of one or more dirs; in multi-root mode prefer target_dirs=['[Host]','[Daemon]'] instead of '.'. deep_slice: token-budgeted XML with bodies for a concrete target file or dir; use single_file=true for one exact file and query for semantic ranking. skeleton: project-wide YAML signatures-only constitution for one or more explicit target dirs."
                },
                "repoPath": { "type": "string", "description": "Absolute path to the primary repo root. Use this for workspace_topology or when you want to pin exploration to a specific root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path from network map. Overrides repoPath." },
                "target_dirs": { "type": "array", "items": { "type": "string" }, "description": "(map_overview, skeleton) One or more dirs to analyze. Use repo-relative paths in single-root repos. In multi-root workspaces, prefer explicit prefixes such as ['[Host]','[Daemon]'] or ['[Host]/src','[Daemon]/src']." },
                "target_dir": { "type": "string", "description": "Deprecated singular form of target_dirs. Kept for compatibility; prefer target_dirs=[...] moving forward." },
                "search_filter": { "type": "string", "description": "(map_overview) Case-insensitive substring filter. OR via 'foo|bar'." },
                "max_chars": { "type": "integer", "description": "Max output chars. Default 8000." },
                "ignore_gitignore": { "type": "boolean", "description": "(map_overview) Include git-ignored files." },
                "exclude": { "type": "array", "items": { "type": "string" }, "description": "Dir names to skip (e.g. ['node_modules','build'])." },
                "target": { "type": "string", "description": "(deep_slice) Repo-relative path to a file or dir. In multi-root workspaces prefix with [FolderName]/, e.g. '[AnvilSynth]/src/main.rs'." },
                "budget_tokens": { "type": "integer", "exclusiveMinimum": 0, "description": "(deep_slice) Token budget. Default 32000." },
                "skeleton_only": { "type": "boolean", "description": "(deep_slice) Strip function bodies, return signatures only." },
                "query": { "type": "string", "description": "(deep_slice) Semantic query for vector-ranked file selection." },
                "query_limit": { "type": "integer", "description": "(deep_slice) Max files returned in query mode." },
                "single_file": { "type": "boolean", "description": "(deep_slice) Skip vector search; return only the exact target file." },
                "only_dirs": { "type": "array", "items": { "type": "string" }, "description": "(deep_slice) Restrict semantic search to one or more dirs. Use repo-relative paths in single-root repos or [FolderName]/... in multi-root workspaces." },
                "only_dir": { "type": "string", "description": "Deprecated singular form of only_dirs. Kept for compatibility; prefer only_dirs=[...] moving forward." },
                "max_files": { "type": "integer", "description": "(skeleton) Max source files to include (default 200, hard cap 500)." },
                "extensions": { "type": "array", "items": { "type": "string" }, "description": "(skeleton) Optional list of file extensions to include (e.g. ['rs','ts']). Omit for all supported languages." }
            },
            "required": ["action"]
        }
    })
}

pub fn symbol_analyzer_schema() -> Value {
    json!({
        "name": "cortex_symbol_analyzer",
        "description": "Exact symbol analysis for reading source, finding usages, implementations, blast radius, and propagation checklists. Use this when you already know the symbol or need precise impact analysis instead of broad repo mapping.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read_source", "find_usages", "find_implementations", "blast_radius", "propagation_checklist"],
                    "description": "read_source (needs path+symbol_name). find_usages (needs symbol_name+target_dir). find_implementations. blast_radius (run before rename/delete). propagation_checklist (shared type update checklist)."
                },
                "repoPath": { "type": "string", "description": "Absolute path to the primary repo root when you want to pin analysis to one workspace root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path. Overrides repoPath." },
                "symbol_name": { "type": "string", "description": "Target symbol name (exact, no regex)." },
                "target_dir": { "type": "string", "description": "Scope dir ('.' = whole repo). Use repo-relative paths in single-root repos. In multi-root workspaces prefix with [FolderName]/ to target a specific root, e.g. '[Host]/src'. Required for find_usages/blast_radius." },
                "ignore_gitignore": { "type": "boolean", "description": "(propagation_checklist) Include git-ignored files." },
                "max_chars": { "type": "integer", "description": "Max output chars. Default 8000." },
                "only_dir": { "type": "string", "description": "(propagation_checklist) Restrict scan to this subdir. Use repo-relative paths in single-root repos or [FolderName]/... in multi-root workspaces." },
                "aliases": { "type": "array", "items": { "type": "string" }, "description": "(propagation_checklist) Alternative names across language boundaries." },
                "path": { "type": "string", "description": "(read_source) Source file. Use a repo-relative path in single-root repos or [FolderName]/path/to/file in multi-root workspaces. Required." },
                "symbol_names": { "type": "array", "items": { "type": "string" }, "description": "(read_source) Batch: extract multiple symbols from path." },
                "skeleton_only": { "type": "boolean", "description": "(read_source) Return signatures only, strip bodies." },
                "instance_index": { "type": "integer", "description": "(read_source) 0-based index when symbol has multiple definitions in the file." },
                "changed_path": { "type": "string", "description": "(propagation_checklist) Contract file path (e.g. .proto) — overrides symbol mode." },
                "max_symbols": { "type": "integer", "description": "(propagation_checklist) Max extracted symbols. Default 20." }
            },
            "required": ["action"]
        }
    })
}

pub fn chronos_schema() -> Value {
    json!({
        "name": "cortex_chronos",
        "description": "AST-aware checkpointing for risky refactors. Save a checkpoint before editing, then compare against live code afterward without being distracted by formatting-only diffs.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["save_checkpoint", "list_checkpoints", "compare_checkpoint", "delete_checkpoint"],
                    "description": "save_checkpoint: snapshot symbol before edit (needs path+symbol_name+tag). list_checkpoints: list all saved tags. compare_checkpoint: AST diff between two tags (needs symbol_name+tag_a+tag_b; tag_b='__live__' for on-disk state). delete_checkpoint: remove by namespace/symbol/tag."
                },
                "repoPath": { "type": "string", "description": "Absolute path to the primary repo root when you want to pin checkpoints to one workspace root." },
                "namespace": { "type": "string", "description": "Checkpoint group (default 'default'). delete_checkpoint with namespace only purges the whole group." },
                "max_chars": { "type": "integer", "description": "Max output chars. Default 8000." },
                "path": { "type": "string", "description": "Source file. Use a repo-relative path in single-root repos or [FolderName]/path/to/file in multi-root workspaces. Required for save; optional for compare." },
                "symbol_name": { "type": "string", "description": "Target symbol name." },
                "semantic_tag": { "type": "string", "description": "Tag name (e.g. 'pre-refactor')." },
                "tag": { "type": "string", "description": "Alias for semantic_tag." },
                "tag_a": { "type": "string", "description": "(compare) First tag." },
                "tag_b": { "type": "string", "description": "(compare) Second tag. '__live__' = current file on disk." }
            },
            "required": ["action"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_schemas_are_present_once() {
        let names: Vec<String> = all_tool_schemas()
            .into_iter()
            .filter_map(|schema| schema["name"].as_str().map(str::to_string))
            .collect();

        assert_eq!(names, vec![
            "cortex_code_explorer".to_string(),
            "cortex_symbol_analyzer".to_string(),
            "cortex_chronos".to_string(),
            "cortex_manage_ast_languages".to_string(),
        ]);
    }

    /// Every schema must have a non-empty name, description, and inputSchema.
    /// This guards against accidentally shipping a bare Null or empty placeholder.
    #[test]
    fn all_schemas_are_structurally_complete() {
        for schema in all_tool_schemas() {
            let name = schema["name"].as_str().unwrap_or("");
            assert!(!name.is_empty(), "Schema is missing 'name'");

            let desc = schema["description"].as_str().unwrap_or("");
            assert!(!desc.is_empty(), "Schema '{name}' is missing 'description'");

            assert!(
                schema["inputSchema"].is_object(),
                "Schema '{name}' is missing 'inputSchema'"
            );
        }
    }

    /// The action constants in grammar_manager must match the schema enum values so that
    /// adding a new action to the schema cannot silently leave the dispatcher un-updated.
    #[test]
    fn grammar_manager_action_constants_match_schema_enum() {
        let schema = crate::grammar_manager::tool_schema();
        let enum_values: Vec<&str> = schema["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .expect("action must have an enum constraint")
            .iter()
            .map(|v| v.as_str().expect("enum value must be a string"))
            .collect();

        assert!(
            enum_values.contains(&crate::grammar_manager::ACTION_STATUS),
            "ACTION_STATUS '{}' not found in schema enum {:?}",
            crate::grammar_manager::ACTION_STATUS,
            enum_values
        );
        assert!(
            enum_values.contains(&crate::grammar_manager::ACTION_ADD),
            "ACTION_ADD '{}' not found in schema enum {:?}",
            crate::grammar_manager::ACTION_ADD,
            enum_values
        );
    }
}