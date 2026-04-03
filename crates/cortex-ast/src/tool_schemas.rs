use serde_json::{Value, json};

pub fn all_tool_schemas() -> Vec<Value> {
    vec![
        code_explorer_schema(),
        symbol_analyzer_schema(),
        chronos_schema(),
        z4_reg_reader_schema(),
        z4_hex_bridge_schema(),
        z4_unit_scan_schema(),
        crate::grammar_manager::tool_schema(),
    ]
}

pub fn code_explorer_schema() -> Value {
    json!({
        "name": "cortex_code_explorer",
        "description": "Map a repo with a z4-first workflow: use workspace_topology for root discovery, map_overview for low-token machine-facing maps, deep_slice when you need bodies, and skeleton when signatures are enough. In z4=true repos start with target_dirs=['.'] or ['build']; map_overview automatically prefers registry aliases and hides prose/config noise. In single-root repos use plain repo-relative paths; use [FolderName]/... only for real multi-root workspaces.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["workspace_topology", "map_overview", "deep_slice", "skeleton"],
                    "description": "workspace_topology: ultra-low-token root and manifest summary. map_overview: bird's-eye map of selected dirs; in z4=true repos it becomes machine-only and works best with target_dirs=['.'] plus search_filter on file names, phase ids, build ids, or hex labels. deep_slice: token-budgeted XML with bodies for one file or dir; use single_file=true for an exact file. skeleton: signatures-only map when bodies are unnecessary."
                },
                "repoPath": { "type": "string", "description": "Absolute path to the primary repo root. Use this for workspace_topology or when you want to pin z4 exploration to a specific root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path from network map. Overrides repoPath." },
                "target_dirs": { "type": "array", "items": { "type": "string" }, "description": "(map_overview, skeleton) One or more dirs to analyze. Use repo-relative paths in single-root repos. In z4 repos common choices are ['.'] for the whole machine-visible surface or ['build'] for build-unit catalogs. In multi-root workspaces, prefer explicit prefixes such as ['[Host]','[Daemon]'] or ['[Host]/src','[Daemon]/src']." },
                "target_dir": { "type": "string", "description": "Deprecated singular form of target_dirs. Kept for compatibility; prefer target_dirs=[...] moving forward." },
                "search_filter": { "type": "string", "description": "(map_overview) Case-insensitive substring filter. OR via 'foo|bar'. In z4 repos filter by file names, phase ids, build ids, or hex labels such as 'parser|0x16ab1d44|f16ab1d44000001f9'." },
                "max_chars": { "type": "integer", "description": "Max output chars. Default 8000." },
                "ignore_gitignore": { "type": "boolean", "description": "(map_overview) Include git-ignored files. In z4 repos keep this false unless you intentionally need ignored artifacts." },
                "exclude": { "type": "array", "items": { "type": "string" }, "description": "Dir names to skip (e.g. ['node_modules','build'])." },
                "target": { "type": "string", "description": "(deep_slice) Repo-relative path to a file or dir. In z4 repos common exact-file targets are 'z4c.z4' or 'parser.z4'. In multi-root workspaces prefix with [FolderName]/, e.g. '[AnvilSynth]/src/main.rs'." },
                "budget_tokens": { "type": "integer", "exclusiveMinimum": 0, "description": "(deep_slice) Token budget. Default 32000." },
                "skeleton_only": { "type": "boolean", "description": "(deep_slice) Strip function bodies, return signatures only." },
                "single_file": { "type": "boolean", "description": "(deep_slice) Return only the exact target file (no directory expansion)." },
                "max_files": { "type": "integer", "description": "(skeleton) Max source files to include (default 200, hard cap 500)." },
                "extensions": { "type": "array", "items": { "type": "string" }, "description": "(skeleton) Optional list of file extensions to include (e.g. ['rs','ts']). On z4 repos usually omit this and let machine-only filtering drive the view." }
            },
            "required": ["action"]
        }
    })
}

pub fn symbol_analyzer_schema() -> Value {
    json!({
        "name": "cortex_symbol_analyzer",
        "description": "Exact symbol analysis for reading source, finding usages, implementations, blast radius, and propagation checklists. On z4 repos use exact labels or hex symbols, for example 'f16ab1d44000001f9', rather than regex-shaped guesses. Use this after map_overview or cortex_z4_unit_scan when you need precise impact analysis.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read_source", "find_usages", "find_implementations", "blast_radius", "propagation_checklist"],
                    "description": "read_source (needs path+symbol_name). find_usages (needs symbol_name+target_dir). find_implementations. blast_radius (run before rename/delete, especially for z4 labels or branch targets). propagation_checklist (shared type update checklist)."
                },
                "repoPath": { "type": "string", "description": "Absolute path to the primary repo root when you want to pin analysis to one workspace root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path. Overrides repoPath." },
                "symbol_name": { "type": "string", "description": "Target symbol name (exact, no regex). On z4 repos pass the exact label text without '@', e.g. 'f16ab1d44000001f9'." },
                "target_dir": { "type": "string", "description": "Scope dir ('.' = whole repo). Use repo-relative paths in single-root repos. In z4 repos common scopes are '.' or 'build'. In multi-root workspaces prefix with [FolderName]/ to target a specific root, e.g. '[Host]/src'. Required for find_usages/blast_radius." },
                "ignore_gitignore": { "type": "boolean", "description": "(propagation_checklist) Include git-ignored files." },
                "max_chars": { "type": "integer", "description": "Max output chars. Default 8000." },
                "only_dir": { "type": "string", "description": "(propagation_checklist) Restrict scan to this subdir. Use repo-relative paths in single-root repos or [FolderName]/... in multi-root workspaces." },
                "aliases": { "type": "array", "items": { "type": "string" }, "description": "(propagation_checklist) Alternative names across language boundaries." },
                "path": { "type": "string", "description": "(read_source) Source file. In z4 repos common paths are 'z4c.z4', 'parser.z4', or another exact .z4 file. Use repo-relative paths in single-root repos or [FolderName]/path/to/file in multi-root workspaces. Required." },
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
        "description": "Checkpointing for risky refactors. Save a checkpoint before editing a hot z4 label, compiler routine, or adjacent support code, then compare against live code afterward without being distracted by formatting-only diffs.",
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
                "path": { "type": "string", "description": "Source file. In z4 repos use an exact .z4 path such as 'z4c.z4'. Use a repo-relative path in single-root repos or [FolderName]/path/to/file in multi-root workspaces. Required for save; optional for compare." },
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

pub fn z4_reg_reader_schema() -> Value {
    json!({
        "name": "cortex_z4_reg_reader",
        "description": "Primary z4 orientation tool. Read z4.reg to map ids to source/build-unit paths, or inspect .filelist/.project.z4 catalogs as low-token machine-facing tables or JSON. Use this before broader exploration when you need canonical ids and path aliases.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repoPath": { "type": "string", "description": "Absolute repo root when you want to pin the lookup to one workspace root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path. Overrides repoPath." },
                "path": { "type": "string", "description": "Optional path to z4.reg, .filelist, or .project.z4. Omit to read z4.reg under the repo root; pass a specific catalog when you want one build unit only." },
                "output_format": { "type": "string", "enum": ["table", "json"], "description": "table: low-token text output for agent-facing z4 work. json: structured machine-readable output for follow-up processing.", "default": "table" },
                "max_entries": { "type": "integer", "description": "Maximum number of records to emit. Default 64." }
            }
        }
    })
}

pub fn z4_hex_bridge_schema() -> Value {
    json!({
        "name": "cortex_z4_hex_bridge",
        "description": "Decode z4 DATA/DEFINE DOC hex into compact escaped text. Use this when parser, emitter, or compiler code stores mnemonics or protocol strings as DOC rows; optionally scope to one symbol to avoid dumping unrelated constants.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repoPath": { "type": "string", "description": "Absolute repo root when you want to pin the lookup to one workspace root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path. Overrides repoPath." },
                "path": { "type": "string", "description": "Path to a .z4 source file. Required unless doc_hex is provided. Common targets are 'parser.z4' or 'emitter.z4'." },
                "symbol_name": { "type": "string", "description": "Optional symbol scope when path is provided. Limits decoding to DOC literals inside that exact z4 label range." },
                "doc_hex": { "type": "string", "description": "Optional raw DOC literal such as '0x41444400'. When present, path is not required." },
                "max_entries": { "type": "integer", "description": "Maximum number of decoded DOC rows to emit when scanning a file. Default 32." }
            }
        }
    })
}

pub fn z4_unit_scan_schema() -> Value {
    json!({
        "name": "cortex_z4_unit_scan",
        "description": "Primary z4 build-boundary tool. Use build_units to turn .filelist/.project.z4 catalogs into a unit-to-file map, or rename_guard before renaming a hex label, compiler routine, or phase symbol. When path is omitted the repo scan includes build-unit catalogs only, not z4.reg.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["build_units", "rename_guard"],
                    "description": "build_units: render build-unit membership from .filelist/.project.z4 catalogs. rename_guard: summarize which units define/use a z4 symbol before broad rename work, especially for hex labels or compiler entry points."
                },
                "repoPath": { "type": "string", "description": "Absolute repo root when you want to pin the scan to one workspace root." },
                "target_project": { "type": "string", "description": "Cross-project: ID or abs path. Overrides repoPath." },
                "path": { "type": "string", "description": "Optional path to one .filelist/.project.z4 catalog or a directory to scan. When omitted, the repo is scanned for build-unit catalogs only." },
                "symbol_name": { "type": "string", "description": "Required for action=rename_guard. Exact symbol to analyze across discovered build units, e.g. 'f16ab1d44000001f9'." },
                "max_units": { "type": "integer", "description": "Maximum number of build units to emit. Default 32." },
                "max_entries": { "type": "integer", "description": "Maximum number of file entries to show per unit when action=build_units. Default 16." }
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
            "cortex_z4_reg_reader".to_string(),
            "cortex_z4_hex_bridge".to_string(),
            "cortex_z4_unit_scan".to_string(),
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