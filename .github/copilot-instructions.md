# Cortex-Works — Minimal Branch Rules

## Z4-First Priority

This branch is optimized for z4 work first.

- If `.cortexast.json` enables `z4: true`, prefer the z4-native flow before any generic AST or parser-management flow.
- Primary z4 loop: `cortex_z4_reg_reader` -> `cortex_code_explorer(action=map_overview)` -> `cortex_z4_unit_scan` -> `cortex_symbol_analyzer` -> `cortex_z4_hex_bridge` -> `cortex_fs_manage` -> `cortex_act_shell_exec(run_diagnostics=true)` -> `cortex_z4_atomic_sync`.
- Treat generic AST, markup, SQL, and parser-management tools as secondary support for non-z4 side files.

## Tool Map

| Task | Preferred Tool |
|------|----------------|
| First pass on a z4 repo | `cortex_z4_reg_reader(path="z4.reg")` + `cortex_code_explorer(action=map_overview, target_dirs=["."])` |
| Inspect build-unit membership | `cortex_z4_unit_scan(action=build_units)` |
| Check rename safety for a z4 label or compiler symbol | `cortex_z4_unit_scan(action=rename_guard)` |
| Read one exact z4 label or hex symbol | `cortex_symbol_analyzer(action=read_source)` |
| Find callers / branches for a z4 label | `cortex_symbol_analyzer(action=find_usages)` / `cortex_symbol_analyzer(action=blast_radius)` |
| Decode `DOC:"0x..."` payloads | `cortex_z4_hex_bridge` |
| Save a rollback point before risky refactor | `cortex_chronos(action=save_checkpoint)` |
| Mutate `.z4`, `.filelist`, or `.project.z4` files | `cortex_fs_manage` |
| Validate a z4 repo after edits | `cortex_act_shell_exec(run_diagnostics=true)` |
| Commit a focused z4 change atomically | `cortex_z4_atomic_sync` |
| Collapse a z4 inspect/verify bundle into one call | `cortex_act_batch_execute` |
| Search by exact phase id, build id, label, or opcode | `cortex_search_exact` |
| Non-z4 structured code edits | `cortex_act_edit_ast` / `cortex_act_edit_data_graph` |
| Non-z4 markup or SQL edits | `cortex_act_edit_markup` / `cortex_act_sql_surgery` |
| Check parser inventory for non-z4 side work | `cortex_manage_ast_languages` |
| Restart the rebuilt MCP worker | `cortex_mcp_hot_reload` |

## Path Rules

- In a single-root repo, prefer plain repo-relative paths such as `crates/cortex-mcp/src`.
- Use `[FolderName]/...` only for actual multi-root workspace folders provided by MCP `initialize`.
- Use absolute paths only when you intentionally need to pin work outside the current workspace conventions.
- For ACT tools, `file`, `project_path`, `paths`, and shell `cwd` all accept repo-relative, workspace-prefixed, or absolute paths.

## Tool Selection Priority

- In this workspace, prefer the `cortex-works` MCP tools over generic built-in read/search/edit tools whenever a matching Cortex tool exists.
- In this branch, treat z4 as the primary workload. If the target repo has `z4: true`, prefer `cortex_z4_reg_reader`, `cortex_z4_unit_scan`, `cortex_z4_hex_bridge`, `cortex_code_explorer`, and `cortex_symbol_analyzer` before generic parser or structural-edit flows.
- **Always pass `repoPath` to `cortex_code_explorer`** — without it the tool falls back to `$HOME` and returns a CRITICAL safety error.
- In multi-root sessions, start with `cortex_code_explorer(action=workspace_topology, repoPath=...)` before any broad map or slice call.
- In z4 repos, `target_dirs=["."]` is the normal `map_overview` entry point because z4 mode already hides prose/config noise. Narrow further with `search_filter` on file names, phase ids, build ids, or hex labels.
- In single-root work, prefer repo-relative paths first; reserve `[FolderName]/...` for true multi-root work.
- Once `initialize.workspaceFolders` is present, omit `repoPath` for workspace-wide discovery; pass `repoPath` only when you intentionally want to pin work to one root.
- Treat singular `target_dir` / `only_dir` fields as compatibility shims only; prefer `target_dirs` moving forward.
- `deep_slice` requires a `target` (the primary file or dir to slice). Use `single_file=true` to return only one exact file.
- For `cortex_search_exact` use `regex_pattern` (or `pattern` as alias) and `project_path` (not `search_dir`) to scope the search.
- Start z4 exploration with `cortex_z4_reg_reader` plus `cortex_code_explorer`, then move to `cortex_z4_unit_scan` or `cortex_symbol_analyzer` once you know the unit or exact label.
- Use `cortex_search_exact` for literal strings, regexes, path hunts, phase ids, build ids, and exact hex labels.
- For `.z4`, `.filelist`, and `.project.z4` edits, prefer `cortex_fs_manage` for mutation and `cortex_z4_atomic_sync` for final commit. Generic AST/markup/sql editors are secondary on this branch.
- `cortex_manage_ast_languages` is not part of the normal z4 flow. Reach for it only when you are intentionally working on non-z4 side code that truly needs parser coverage.

## Batch Rules

- `cortex_act_batch_execute` is sequential, not parallel.
- Use it for short z4 bundles such as `cortex_z4_reg_reader` -> `cortex_z4_unit_scan` -> `cortex_symbol_analyzer` -> `cortex_act_shell_exec(run_diagnostics=true)`.
- The tool returns a JSON `BatchSummary` object with `total`, `passed`, `failed`, `skipped`, and `results[]`.
- Each `results[]` entry contains `index`, `tool_name`, `success`, `output`, `output_chars`, and `truncated`.
- `parameters` is optional per operation and defaults to `{}`.
- Never nest `cortex_act_batch_execute` inside itself.
- If `cortex_mcp_hot_reload` appears in a batch, it must be the final operation because it restarts the worker.
- Use `fail_fast=true` when later operations depend on earlier ones.
- Raise `max_chars_per_op` for high-volume tools such as `map_overview`, `deep_slice`, or z4 build-unit scans.

## Situation Guide
- If the repo is `z4: true`, do not start with `cortex_manage_ast_languages`; start with `cortex_z4_reg_reader` and `cortex_code_explorer(action=map_overview, target_dirs=["."])`.
- If you need the canonical id/path map, use `cortex_z4_reg_reader` first.
- If you need build-unit boundaries or rename blast radius, use `cortex_z4_unit_scan` before broader search.
- If you hit `DOC:"0x..."` rows and need the human meaning, use `cortex_z4_hex_bridge`.
- If you already know the exact z4 label or hex symbol, prefer `cortex_symbol_analyzer` over broad repo mapping.
- If you know the exact string or regex, prefer `cortex_search_exact`.
- If you need a physical file operation or a `.z4` mutation, prefer `cortex_fs_manage`; do not use structural editors for raw file creation, copies, or deletes.
- If you need post-edit verification, use `cortex_act_shell_exec(run_diagnostics=true)`; do not use it for watch mode or dev servers.
- If you want the final commit to stay phase-scoped and validated, use `cortex_z4_atomic_sync`.
- Use `cortex_manage_ast_languages` only for non-z4 side code that truly needs parser coverage.

## Best-Practice Workflow
```text
1. cortex_z4_reg_reader(path="z4.reg", repoPath=...)                                      # map ids -> canonical source/build-unit paths
2. cortex_code_explorer(action=map_overview, target_dirs=["."], search_filter="parser|z4c|0x16ab1d44")
3. cortex_z4_unit_scan(action=rename_guard, symbol_name="f16ab1d44000001f9", repoPath=...) # check unit isolation before rename/edit
4. cortex_symbol_analyzer(action=read_source, path="z4c.z4", symbol_name="f16ab1d44000001f9")
5. cortex_z4_hex_bridge(path="parser.z4", symbol_name="fc718ffb200000000", repoPath=...)   # decode nearby DOC payloads when needed
6. cortex_chronos(action=save_checkpoint, path="z4c.z4", symbol_name="f16ab1d44000001f9", tag="pre-edit")
7. mutate with cortex_fs_manage or finish with cortex_z4_atomic_sync                          # prefer z4-aware mutation paths
8. cortex_act_shell_exec(run_diagnostics=true, cwd=...)                                       # verify after edits
```


## Notes For Agents

- This branch is z4-first. Generic language support remains available, but z4-native tools and z4-aware validation are the primary workflow.
- The public surface is the 17 active tools only.
- `cortex_code_explorer(action=workspace_topology, repoPath=...)` is the preferred low-token entry point. **Always pass `repoPath`** — omitting it causes the tool to block with a CRITICAL safety error.
- `map_overview` and `skeleton` accept `target_dirs=[...]`. Use the array form first. In z4 repos, `target_dirs=["."]` is normal because machine-only filtering already shrinks the surface.
- In single-root sessions, plain repo-relative paths are usually the least confusing choice.
- Prefixed paths such as `[cortex-db]/src/lib.rs` are canonical only for real multi-root workspace identifiers.
- z4 repos often expose exact labels as hex-shaped names rather than human-readable identifiers. Use exact symbol text and avoid regex-shaped guesses.
- `cortex_z4_unit_scan` defaults to build-unit catalogs only. Keep `z4.reg` for registry/alias lookups through `cortex_z4_reg_reader`, not rename-guard scans.
- `cortex_z4_hex_bridge` is the normal way to decode embedded DOC payloads; do not paraphrase raw hex when the tool can decode it directly.
- `cortex_act_edit_ast`, `cortex_act_edit_markup`, and `cortex_act_sql_surgery` are not normal z4 source-edit paths.
- If a non-core language parser is missing for non-z4 side code, call `cortex_manage_ast_languages` instead of guessing parser support.
- **Data editing:** JSON supports full upsert (new keys at any depth via `set`). YAML only supports updating existing keys — use `replace` on the parent object to add new keys to YAML.
- **Batch:** `cortex_act_batch_execute` accepts all 17 tool names, returns a `BatchSummary`, and supports omitted `parameters`. Nesting is not allowed. Put `cortex_mcp_hot_reload` last.
- **Reload after rebuild:** if a just-built z4 tool or schema still behaves like the old code, run `cortex_mcp_hot_reload` before trusting the runtime result. Source changes and live MCP behavior can drift until the worker is reloaded.
- **Shell PATH:** on Unix the tool automatically adds `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin` to PATH. `cargo`, `node`, `python3` are available without manual PATH manipulation.

## Parameter Reference — Required Fields Per Action

This section lists the exact required parameters for every action. Using wrong names returns an error immediately.

### `cortex_code_explorer`

| action | Required params | Key optional params |
|--------|-----------------|---------------------|
| `workspace_topology` | `repoPath` (absolute path) | — |
| `map_overview` | `target_dirs` (array) | `repoPath`, `search_filter`, `max_chars` |
| `deep_slice` | `target` (single file or dir) | `single_file`, `budget_tokens`, `skeleton_only` |
| `skeleton` | `target_dirs` (array) | `repoPath`, `max_files`, `extensions` |

### `cortex_symbol_analyzer`

| action | Required params | Notes |
|--------|-----------------|-------|
| `read_source` | `path` (source file path) + `symbol_name` | Batch: `symbol_names` array. NOT `file`. |
| `find_usages` | `symbol_name` + `target_dir` | Use `target_dir='.'` for whole repo |
| `blast_radius` | `symbol_name` + `target_dir` | Run before rename or delete |
| `find_implementations` | `symbol_name` + `target_dir` | — |
| `propagation_checklist` | `symbol_name` + `target_dir` | `aliases`, `only_dir`, `changed_path` optional |

### `cortex_chronos`

| action | Required params | Notes |
|--------|-----------------|-------|
| `save_checkpoint` | `path` + `symbol_name` + `semantic_tag` | Saves one symbol at a time |
| `list_checkpoints` | — | Returns all namespaced tags |
| `compare_checkpoint` | `symbol_name` + `tag_a` + `tag_b` | Use `tag_b='__live__'` to diff against current file |
| `delete_checkpoint` | `semantic_tag` (or `namespace` to purge group) | — |

### `cortex_act_edit_ast`

```
file:  repo-relative or absolute path to .rs / .ts / .py file
edits: [{target: "symbol_name" or "kind:name", action: "replace"|"delete", code: "..."}]
```

### `cortex_act_edit_data_graph`

```
file:  path to .json or .yaml file
edits: [{target: "$.key.path", action: "set"|"replace"|"delete", value: "new_value"}]
```
Note: `value` field (not `code`). For YAML, `set`/`replace` update existing keys only; use `replace` on the parent to add a new key.

### `cortex_act_edit_markup`

```
file:  path to .md / .html / .xml file
edits: [{target: "heading:Name"|"tag:div"|"id:app"|"table:0", action: "replace"|"delete"|"insert_before"|"insert_after", code: "..."}]
```

### `cortex_act_sql_surgery`

```
file:  path to .sql file
edits: [{target: "create_table:tablename"|"create_index:indexname", action: "replace"|"delete", code: "..."}]
```

### `cortex_act_shell_exec`

```
command:         shell command string (required unless run_diagnostics=true)
cwd:             working directory (repo-relative or absolute)
run_diagnostics: true — auto-detect manifest and run compiler check
problem_matcher: "cargo" | "tsc" | "eslint" | "go" | "python"
timeout_secs:    integer (default 30)
```

### `cortex_search_exact`

```
regex_pattern:  string (literal text or regex); alias: pattern
project_path:   absolute path to scope the search root
file_types:     ["rs", "ts", ...] — restrict by extension
```

### `cortex_fs_manage`

Supported actions: `write`, `patch`, `mkdir`, `delete`, `rename`, `move`, `copy`. **No `read` action.**

```
write:  path + content
patch:  path + patches (for .env/.ini/key=value only)
mkdir:  paths (array)
delete: paths (array) — BLOCKED for paths outside workspace roots (safety guard)
rename: source + destination
move:   source + destination
copy:   source + destination
```

### `cortex_act_batch_execute`

```
operations: [{tool_name: "...", parameters: {...}}]   # parameters defaults to {}
fail_fast:       true | false
max_chars_per_op: integer (raise for map_overview or deep_slice)
```
`cortex_mcp_hot_reload`, if included, must be the **last** operation.

### `cortex_manage_ast_languages`

```
action: "status"  — lists active core languages and available-to-download parsers
action: "add"     — NOT supported in this build (returns error)
```

### `cortex_mcp_hot_reload`

```
reason: optional description string
```
Triggers `exit(42)` in ~500 ms so the supervisor restarts the worker. After restart, re-run `initialize` and `tools/list`.