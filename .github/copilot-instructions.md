# Cortex-Works — Minimal Branch Rules

## Workspace Structure

```
crates/
  cortex-ast/    # Read-only code intelligence: map, symbol analysis, chronos, grammar loading
  cortex-act/    # Surgical edits, shell execution, exact search, semantic search, filesystem ops
  cortex-db/     # SQLite + LanceDB helpers used by semantic search and indexing
  cortex-mcp/    # Single MCP gateway binary exposed to the IDE
```

The active runtime surface for this branch is intentionally minimal.

- Focus on `cortex-mcp`, `cortex-ast`, `cortex-act`, and `cortex-db`
- Ignore legacy or auxiliary folders unless the task explicitly targets them
- No `cortex-mesh` in the supported MCP tool surface
- No `cortex-proxy` in the supported MCP tool surface
- No `cortex-scout` in the supported MCP tool surface
- No hidden boot-sequence or mutation-gate workflow

## Tool Map

| Task | Preferred Tool |
|------|----------------|
| First look at a workspace | `cortex_code_explorer(action=workspace_topology)` |
| Inspect one or more concrete roots | `cortex_code_explorer(action=map_overview, target_dirs=[...])` |
| Read exact source before editing | `cortex_symbol_analyzer(action=read_source)` |
| Find callers / references | `cortex_symbol_analyzer(action=find_usages)` |
| Check rename/delete impact | `cortex_symbol_analyzer(action=blast_radius)` |
| Save before risky refactor | `cortex_chronos(action=save_checkpoint)` |
| Edit Rust/TS/Python symbol | `cortex_act_edit_ast` |
| Edit JSON / YAML structurally | `cortex_act_edit_data_graph` |
| Edit Markdown / HTML / XML structurally | `cortex_act_edit_markup` |
| Edit SQL schema by statement | `cortex_act_sql_surgery` |
| Run bounded command or diagnostics | `cortex_act_shell_exec` |
| Batch independent operations | `cortex_act_batch_execute` |
| Search by concept | `cortex_semantic_code_search` |
| Search by exact text / regex | `cortex_search_exact` |
| File / directory operations | `cortex_fs_manage` |
| Install non-core parsers | `cortex_manage_ast_languages` |
| Reload rebuilt MCP binary | `cortex_mcp_hot_reload` |

## Tool Selection Priority

- In this workspace, prefer the `cortex-works` MCP tools over generic built-in read/search/edit tools whenever a matching Cortex tool exists.
- In multi-root sessions, start with `cortex_code_explorer(action=workspace_topology)` before any broad map or slice call.
- After topology, prefer explicit `target_dirs=[...]` arrays for `map_overview` and `skeleton` instead of a global `.` scan.
- Use `[FolderName]/path/to/file` when referencing files across workspace roots.
- ACT tools also accept `[FolderName]/...` for `file`, `project_path`, `paths`, and shell `cwd` parameters; do not force absolute paths when a workspace prefix is clearer.
- Once `initialize.workspaceFolders` is present, omit `repoPath` for workspace-wide discovery; pass `repoPath` only when you intentionally want to pin work to one root.
- Treat singular `target_dir` / `only_dir` fields as compatibility shims only; prefer `target_dirs` / `only_dirs` moving forward.
- `deep_slice` requires a `target` (the primary file or dir to slice). `only_dirs=[...]` is an ADDITIONAL optional filter that scopes semantic-search ranking within that slice. Call it as `deep_slice(target='src/foo', only_dirs=['src/foo/sub'])` — not `only_dirs` alone.
- For `cortex_search_exact` use `regex_pattern` (or `pattern` as alias) and `project_path` (not `search_dir`) to scope the search.
- Start repo exploration with #tool:cortex_code_explorer or #tool:cortex_symbol_analyzer before falling back to plain text search.
- Use #tool:cortex_search_exact for literal strings, regexes, path hunts, and symbol names you already know.
- Use #tool:cortex_semantic_code_search only for concept lookup. If it returns no results, immediately retry with #tool:cortex_search_exact or #tool:cortex_code_explorer instead of assuming the code does not exist.
- When calling #tool:cortex_semantic_code_search, pass `project_path` whenever possible so the tool can build or refresh the local symbol index on demand.

## Best-Practice Workflow

```
1. cortex_code_explorer(action=workspace_topology)                    # discover workspace roots with minimal tokens
2. cortex_code_explorer(action=map_overview, target_dirs=[...])       # inspect one or more specific roots
3. cortex_symbol_analyzer(action=read_source)                         # inspect exact code before changing it
4. cortex_chronos(action=save_checkpoint)                             # before risky refactors
5. use the narrowest edit tool that matches the file type
6. cortex_act_shell_exec(run_diagnostics=true)                        # verify after edits
```

## Behavioral Rules

- Prefer `cortex_code_explorer` and `cortex_symbol_analyzer` over blind grep-style exploration.
- In multi-root workspaces, avoid `target_dir='.'` unless you intentionally want the primary root only.
- Prefer `cortex_code_explorer(action=workspace_topology)` for initial orientation because it lists roots, manifests, and language hints without expanding file trees.
- For cross-repo work, pass arrays such as `target_dirs=["[cortex-ast]", "[cortex-db]"]` or `only_dirs=["[cortex-db]"]` rather than issuing repeated single-root calls.
- For ACT operations inside multi-root workspaces, prefer workspace-prefixed paths over long absolute paths when the target root is already known.
- Use `cortex_search_exact` only when the search term is literal or regex-shaped.
- Use `cortex_semantic_code_search` only when you need concept-based lookup and a local semantic index exists.
- Use `cortex_act_edit_ast` only for Rust, TypeScript, and Python source edits by symbol.
- Use `cortex_act_edit_data_graph` for JSON and YAML only. For TOML rewrites, use `cortex_fs_manage(action=write)`.
- Use `cortex_fs_manage` for physical file operations, not structural code edits.
- For `cortex_fs_manage(action=patch)`, keep the top-level `action` as `patch` and use `patch_action=set|delete` for the key mutation. If `patch_action` is omitted, the tool defaults to `set`.
- `cortex_act_shell_exec` is synchronous and bounded. Do not use it for long-running servers or watch mode.
- `cortex_act_batch_execute` runs sequentially and can mix AST and ACT tools in one round-trip. Do not nest batch calls.
- `cortex_manage_ast_languages(action=add)` downloads Wasm grammars from GitHub tree-sitter releases into `~/.cortex-works/grammars/` and hot-reloads them.
- `cortex_mcp_hot_reload` restarts the worker through the supervisor on the same stdio channel. After restart, re-run `initialize` and refresh `tools/list` if the client needs updated schema.
- Do not reference removed services, removed tools, or old boot-order requirements in this branch.

## Release Validation

```text
1. cargo build --release -p cortex-mcp
2. run MCP smoke tests against target/release/cortex-mcp
3. verify cortex_manage_ast_languages(action=add) with a clean HOME when touching grammar loading
4. after rebuilding the binary, call cortex_mcp_hot_reload and re-initialize the client before checking tools/list
```

## Running The Stack

```bash
# Build the only IDE entry point
cargo build --release -p cortex-mcp

# Run the MCP gateway directly
cargo run --release -p cortex-mcp
```

## Notes For Agents

- The public surface is the 14 active tools only.
- `cortex_code_explorer(action=workspace_topology)` is the preferred low-token entry point for workspace-aware agents.
- `map_overview` and `skeleton` accept `target_dirs=[...]`; `deep_slice` accepts `only_dirs=[...]`. Use the array forms first.
- Prefixed paths such as `[cortex-db]/src/lib.rs` are the canonical cross-root file identifiers.
- If semantic search returns no results, assume the local vector index is missing or stale, then retry with `project_path` or fall back to exact/code-structure tools.
- If a non-core language parser is missing, call `cortex_manage_ast_languages` instead of guessing parser support.
