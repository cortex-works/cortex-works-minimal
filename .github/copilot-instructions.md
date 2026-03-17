# Cortex-Works — Minimal Branch Rules

## Workspace Structure

```
crates/
  cortex-ast/    # Read-only code intelligence: map, symbol analysis, chronos, grammar loading
  cortex-act/    # Surgical edits, shell execution, exact search, semantic search, filesystem ops
  cortex-db/     # SQLite + LanceDB helpers used by semantic search and indexing
  cortex-mcp/    # Single MCP gateway binary exposed to the IDE
```

AST tool schema source of truth:

- `crates/cortex-ast/src/tool_schemas.rs` — `cortex_code_explorer`, `cortex_symbol_analyzer`, `cortex_chronos`
- `crates/cortex-ast/src/grammar_manager.rs` — `cortex_manage_ast_languages`
- `crates/cortex-ast/src/server.rs` — runtime dispatch only; do not duplicate public schema text there

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
| First look at an unfamiliar repo | `cortex_code_explorer(action=workspace_topology, repoPath=...)` |
| Narrow to concrete directories before reading bodies | `cortex_code_explorer(action=map_overview, target_dirs=[...])` |
| Read one exact symbol or a few exact symbols | `cortex_symbol_analyzer(action=read_source)` |
| Find callers / references | `cortex_symbol_analyzer(action=find_usages)` |
| Check rename/delete impact first | `cortex_symbol_analyzer(action=blast_radius)` |
| Save a rollback point before risky refactor | `cortex_chronos(action=save_checkpoint)` |
| Compare current code against a saved checkpoint | `cortex_chronos(action=compare_checkpoint)` |
| Edit a Rust/TS/Python symbol by name | `cortex_act_edit_ast` |
| Edit JSON or YAML keys structurally | `cortex_act_edit_data_graph` |
| Add a new YAML key | `cortex_act_edit_data_graph` with `action=replace` on the parent |
| Rewrite TOML or raw text wholesale | `cortex_fs_manage(action=write)` |
| Edit Markdown / HTML / XML by heading, tag, id, or section | `cortex_act_edit_markup` |
| Edit SQL DDL by statement name | `cortex_act_sql_surgery` |
| Run a short command or diagnostics | `cortex_act_shell_exec` |
| Collapse a short sequential workflow into one tool call | `cortex_act_batch_execute` |
| Search by concept when you do not know the exact symbol name | `cortex_semantic_code_search` |
| Search by exact string, regex, path clue, or identifier | `cortex_search_exact` |
| Create / copy / move / delete files and folders | `cortex_fs_manage` |
| Patch `.env`, `.ini`, or `key=value` files | `cortex_fs_manage(action=patch)` |
| Check or install non-core parsers | `cortex_manage_ast_languages` |
| Restart the rebuilt MCP worker | `cortex_mcp_hot_reload` |

## Path Rules

- In a single-root repo, prefer plain repo-relative paths such as `crates/cortex-mcp/src`.
- Use `[FolderName]/...` only for actual multi-root workspace folders provided by MCP `initialize`.
- Use absolute paths only when you intentionally need to pin work outside the current workspace conventions.
- For ACT tools, `file`, `project_path`, `paths`, and shell `cwd` all accept repo-relative, workspace-prefixed, or absolute paths.

## Tool Selection Priority

- In this workspace, prefer the `cortex-works` MCP tools over generic built-in read/search/edit tools whenever a matching Cortex tool exists.
- **Always pass `repoPath` to `cortex_code_explorer`** — without it the tool falls back to `$HOME` and returns a CRITICAL safety error.
- In multi-root sessions, start with `cortex_code_explorer(action=workspace_topology, repoPath=...)` before any broad map or slice call.
- After topology, prefer explicit `target_dirs=[...]` arrays for `map_overview` and `skeleton` instead of a global `.` scan.
- In single-root work, prefer repo-relative paths first; reserve `[FolderName]/...` for true multi-root work.
- Once `initialize.workspaceFolders` is present, omit `repoPath` for workspace-wide discovery; pass `repoPath` only when you intentionally want to pin work to one root.
- Treat singular `target_dir` / `only_dir` fields as compatibility shims only; prefer `target_dirs` / `only_dirs` moving forward.
- `deep_slice` requires a `target` (the primary file or dir to slice). `only_dirs=[...]` is an ADDITIONAL optional filter that scopes semantic-search ranking within that slice. Call it as `deep_slice(target='src/foo', only_dirs=['src/foo/sub'])` — not `only_dirs` alone.
- For `cortex_search_exact` use `regex_pattern` (or `pattern` as alias) and `project_path` (not `search_dir`) to scope the search.
- Start repo exploration with `cortex_code_explorer` or `cortex_symbol_analyzer` before falling back to plain text search.
- Use `cortex_search_exact` for literal strings, regexes, path hunts, and exact symbol names.
- Use `cortex_semantic_code_search` only for concept lookup. If it returns no results, immediately retry with `cortex_search_exact` or `cortex_code_explorer` — do not assume the code does not exist.
- When calling `cortex_semantic_code_search`, pass `project_path` whenever possible so the tool can build or refresh the local symbol index on demand.

## Batch Rules

- `cortex_act_batch_execute` is sequential, not parallel.
- Use it for short workflows such as inspect → edit → verify, or several independent reads in one round-trip.
- The tool returns a JSON `BatchSummary` object with `total`, `passed`, `failed`, `skipped`, and `results[]`.
- Each `results[]` entry contains `index`, `tool_name`, `success`, `output`, `output_chars`, and `truncated`.
- `parameters` is optional per operation and defaults to `{}`.
- Never nest `cortex_act_batch_execute` inside itself.
- If `cortex_mcp_hot_reload` appears in a batch, it must be the final operation because it restarts the worker.
- Use `fail_fast=true` when later operations depend on earlier ones.
- Raise `max_chars_per_op` for high-volume tools such as `map_overview` or `deep_slice`.

## Situation Guide

- If a language is already supported and you need code or symbols now, do not call `cortex_manage_ast_languages`; go straight to `cortex_code_explorer`, `cortex_symbol_analyzer`, or search.
- If a repo contains a non-core language and AST-aware tools are missing coverage, call `cortex_manage_ast_languages(action=status)` first, then `action=add` only for the missing parser.
- If you just added a parser and plan to re-run semantic or AST-heavy tools on one repo, pass `repoPath` so cached semantic records for that repo can be invalidated immediately.
- If you know the exact symbol, prefer `cortex_symbol_analyzer` over `cortex_code_explorer`.
- If you know the exact string or regex, prefer `cortex_search_exact` over `cortex_semantic_code_search`.
- If you need a physical file operation, prefer `cortex_fs_manage`; do not use structural editors for raw file creation, copies, or deletes.
- If you need a short command or diagnostics, use `cortex_act_shell_exec`; do not use it for watch mode or dev servers.

## Best-Practice Workflow

```
1. cortex_code_explorer(action=workspace_topology, repoPath=...)      # discover roots — always pass repoPath
2. cortex_code_explorer(action=map_overview, target_dirs=[...])       # inspect one or more specific roots
3. cortex_symbol_analyzer(action=read_source)                         # inspect exact code before changing it
4. cortex_chronos(action=save_checkpoint)                             # before risky refactors
5. use the narrowest edit tool that matches the file type
6. cortex_act_shell_exec(run_diagnostics=true, cwd=...)               # verify after edits
```

## Behavioral Rules

- Prefer `cortex_code_explorer` and `cortex_symbol_analyzer` over blind grep-style exploration.
- In multi-root workspaces, avoid `target_dir='.'` unless you intentionally want the primary root only.
- Prefer `cortex_code_explorer(action=workspace_topology, repoPath=...)` for initial orientation because it lists roots, manifests, and language hints without expanding file trees. **`repoPath` is required** — omitting it causes a CRITICAL safety block.
- Use `cortex_symbol_analyzer` when you already know the symbol; use `cortex_code_explorer` when you still need to discover the right file or region.
- For cross-repo work, pass arrays such as `target_dirs=["[cortex-ast]", "[cortex-db]"]` or `only_dirs=["[cortex-db]"]` rather than issuing repeated single-root calls.
- For ACT operations inside multi-root workspaces, prefer workspace-prefixed paths over long absolute paths when the target root is already known.
- Use `cortex_search_exact` only when the search term is literal or regex-shaped.
- Use `cortex_semantic_code_search` only when you need concept-based lookup and a local semantic index exists.
- Use `cortex_act_edit_ast` only for Rust, TypeScript, and Python source edits by symbol.
- Use `cortex_act_edit_data_graph` for JSON and YAML. For TOML, use `cortex_fs_manage(action=write)`.
  - JSON: `action=set` can upsert (insert) any new key at any depth, including top-level (`$.newKey`).
  - YAML: `action=set` / `action=replace` only works on **existing** keys. To add a new key to YAML, use `action=replace` on the **parent** object and supply the full updated object.
- Use `cortex_fs_manage(action=patch)` only for `.env`, `.ini`, and `key=value` format files — not for JSON or YAML.
- Use `cortex_fs_manage(action=write)` to create or fully overwrite any file (TOML, plain text, etc.).
- For `cortex_fs_manage(action=patch)`, keep the top-level `action` as `patch` and use `patch_action=set|delete` for the key mutation. If `patch_action` is omitted, the tool defaults to `set`.
- `cortex_act_shell_exec` is synchronous and bounded. Do not use it for long-running servers or watch mode.
  - On Unix, PATH is automatically augmented to include `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin`.
  - `run_diagnostics=true` auto-detects the manifest in `cwd` and runs the appropriate compiler check.
  - `problem_matcher` values: `cargo`, `tsc`, `eslint`, `go`, `python`.
- `cortex_act_batch_execute` can mix all 14 Cortex tools in one round-trip, but `cortex_mcp_hot_reload` should only appear as the last operation.
- `cortex_manage_ast_languages(action=add)` downloads Wasm grammars from GitHub tree-sitter releases into `~/.cortex-works/grammars/` and hot-reloads them.
- `cortex_manage_ast_languages(action=status)` should usually be called before `action=add` so the agent does not re-install parsers that are already active.
- `cortex_manage_ast_languages(action=add)` returns structured JSON text and surfaces partial failures as an error result. Pass `repoPath` when you want semantic cache invalidation scoped to one repo.
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
- `cortex_code_explorer(action=workspace_topology, repoPath=...)` is the preferred low-token entry point. **Always pass `repoPath`** — omitting it causes the tool to block with a CRITICAL safety error.
- `map_overview` and `skeleton` accept `target_dirs=[...]`; `deep_slice` accepts `only_dirs=[...]`. Use the array forms first.
- In single-root sessions, plain repo-relative paths are usually the least confusing choice.
- Prefixed paths such as `[cortex-db]/src/lib.rs` are canonical only for real multi-root workspace identifiers.
- If semantic search returns no results, assume the local vector index is missing or stale, then retry with `project_path` or fall back to exact/code-structure tools.
- If a non-core language parser is missing, call `cortex_manage_ast_languages` instead of guessing parser support.
- **Data editing:** JSON supports full upsert (new keys at any depth via `set`). YAML only supports updating existing keys — use `replace` on the parent object to add new keys to YAML.
- **Batch:** `cortex_act_batch_execute` accepts all 14 tool names, returns a `BatchSummary`, and supports omitted `parameters`. Nesting is not allowed. Put `cortex_mcp_hot_reload` last.
- **Reload after rebuild:** if a just-built tool still behaves like the old code, run `cortex_mcp_hot_reload` before trusting the runtime result. Source changes and live MCP behavior can drift until the worker is reloaded.
- **Shell PATH:** on Unix the tool automatically adds `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin` to PATH. `cargo`, `node`, `python3` are available without manual PATH manipulation.
