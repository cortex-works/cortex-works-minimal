# CortexAct: Complex Non-Code File Patching

- [x] Add Tree-sitter dependencies for YAML, JSON, TOML, Markdown, HTML to `cortex-act` (v0.26.5 alignment)
- [x] Implement `cortex_act_edit_data_graph` (JSON/YAML/TOML)
  - [x] Build path-to-node resolution logic (JSONPath to Tree-sitter node)
  - [x] Build byte-level replacement logic (preserving comments/formatting)
- [x] Implement `cortex_act_edit_markup` (MD/HTML/XML)
  - [x] Build structural target resolution (Headings/Tables/Tags)
  - [x] Build byte-level replacement logic
- [x] Implement `cortex_act_sql_surgery` (sqlparser DDL)
- [x] Implement `cortex_act_duckdb_query` (Discovery/Mutation)
- [x] Register new "God Hand" tools in MCP `main.rs`
- [x] Update `cortex-act/README.md` with new schemas and English descriptions
- [x] Resolve Tree-sitter versioning issues across all grammars (transmute workaround)

## Next Steps
- [ ] Thoroughly test all surgery tools with large real-world samples.
- [ ] Refine DuckDB value extraction (handle non-string types better).
- [ ] Implement robust XPath/CSS selectors in `markup_editor`.
