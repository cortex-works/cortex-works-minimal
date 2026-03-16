# Usage Guide & Workflow

This guide covers the best practices for using the `cortex-works` minimal stack in an AI-powered IDE.

## Core Concepts

### 1. Progressive Disclosure

Do not read the whole codebase at once. Use a three-level approach:

- **L1 (Discover):** `workspace_topology` to see roots, manifests, and language hints.
- **L2 (Map):** `map_overview` or `skeleton` with `target_dirs=[...]` to inspect only the relevant roots.
- **L3 (Slice):** `deep_slice`, `read_source`, or a targeted search once you know where to look.

### 2. Prefixed Paths

In multi-root workspaces, use the `[FolderName]/path/to/file` convention.

This prefix now works across both AST and ACT tools, including:

- `cortex_code_explorer` and `cortex_symbol_analyzer`
- `cortex_act_edit_ast`, `cortex_act_edit_data_graph`, `cortex_act_edit_markup`, `cortex_act_sql_surgery`
- `cortex_search_exact`, `cortex_semantic_code_search`, `cortex_fs_manage`, and `cortex_act_shell_exec(cwd=...)`

If the argument is not absolute and does not use a prefix, it resolves against the primary workspace root.

### 3. Scope Explicitly

For multi-root sessions, prefer arrays when the tool supports them:

- `target_dirs=["[frontend]", "[backend]"]` for `map_overview` and `skeleton`
- `only_dirs=["[backend]"]` for `deep_slice` semantic narrowing
- `project_path="[backend]"` when using ACT search tools against one root

---

## Recommended AI Agent Workflow

A good engineering session follows this loop:

1. **Orientation**
   - `cortex_code_explorer(action="workspace_topology")`
   - `cortex_code_explorer(action="map_overview", target_dirs=["[RootA]", "[RootB]"])`

2. **Analysis**
   - `cortex_symbol_analyzer(action="read_source", path="[RootA]/src/lib.rs", symbol_name="MyStruct")`
   - `cortex_symbol_analyzer(action="find_usages", symbol_name="MyStruct", target_dir="[RootA]")`

3. **Safety**
   - `cortex_chronos(action="save_checkpoint", path="[RootA]/src/lib.rs", symbol_name="MyStruct", tag="pre-refactor")`

4. **Execution**
   - use `cortex_act_edit_ast` for code changes
   - use `cortex_fs_manage` for file creation, rename, move, copy, or delete

5. **Verification**
   - `cortex_act_shell_exec(run_diagnostics=true, cwd="[RootA]")`
   - `cortex_chronos(action="compare_checkpoint", path="[RootA]/src/lib.rs", symbol_name="MyStruct", tag_a="pre-refactor", tag_b="__live__")`

---

## Example Scenarios

### Scenario: Implementing a feature across two crates

If you need to update a trait in `crate-a` and implement it in `crate-b`:

```json
{
  "name": "cortex_code_explorer",
  "arguments": {
    "action": "map_overview",
    "target_dirs": ["[crate-a]"],
    "search_filter": "trait"
  }
}

{
  "name": "cortex_code_explorer",
  "arguments": {
    "action": "deep_slice",
    "target": "[crate-b]/src/impl.rs",
    "query": "impl MyTrait",
    "only_dirs": ["[crate-b]"]
  }
}
```

### Scenario: Refactoring a shared data structure

When changing a shared JSON config in one root:

```json
{
  "name": "cortex_chronos",
  "arguments": {
    "action": "save_checkpoint",
    "path": "[config-root]/config.json",
    "symbol_name": "root",
    "tag": "before-json-fix"
  }
}

{
  "name": "cortex_act_edit_data_graph",
  "arguments": {
    "file": "[config-root]/config.json",
    "edits": [{ "target": "$.v2_enabled", "action": "set", "value": "true" }]
  }
}
```

### Scenario: Verifying ACT path routing

When editing and validating with the ACT tools only:

```json
{
  "name": "cortex_act_edit_ast",
  "arguments": {
    "file": "[backend]/src/lib.rs",
    "edits": [{
      "target": "function:handle_login",
      "action": "replace",
      "code": "pub fn handle_login() -> bool {\n    true\n}\n"
    }]
  }
}
```

```json
{
  "name": "cortex_act_shell_exec",
  "arguments": {
    "run_diagnostics": true,
    "cwd": "[backend]"
  }
}
```

### Scenario: Exact Text Search vs Semantic Search

- Use `cortex_search_exact` when you know the string: `TODO(hero)`, `Deprecated`, or a specific error message.
- Use `cortex_semantic_code_search` when you have a concept: "How do we handle database migrations?" or "Where is the authentication middleware?"
- When either search can be scoped to one root, pass `project_path="[FolderName]"`.
- If semantic search returns no results, immediately fall back to `cortex_search_exact` or `cortex_code_explorer`. Do not assume the code does not exist.

---

## Data Editing: JSON vs YAML Rules

`cortex_act_edit_data_graph` behaves differently for JSON and YAML:

| Operation | JSON | YAML |
|-----------|------|------|
| Update existing key (`set`/`replace`) | ✅ | ✅ |
| Insert a new top-level key (`set`) | ✅ (upserts) | ❌ use `replace` on parent |
| Insert a nested key (`set`) | ✅ (upserts) | ❌ use `replace` on parent |
| Delete a key (`delete`) | ✅ | ✅ |

**Adding a new key to YAML:** use `action=replace` targeting the parent object and supply the full updated object as the value.

**TOML:** not supported by `cortex_act_edit_data_graph`. Use `cortex_fs_manage(action=write)` to rewrite the whole file.

### Example: Insert a new top-level key in JSON

```json
{
  "name": "cortex_act_edit_data_graph",
  "arguments": {
    "file": "[config-root]/db.json",
    "edits": [{ "target": "$.ssl", "action": "set", "value": "true" }]
  }
}
```

### Example: Add a new key to YAML (use replace on parent)

```json
{
  "name": "cortex_act_edit_data_graph",
  "arguments": {
    "file": "[config-root]/docker-compose.yaml",
    "edits": [{
      "target": "$.services.app",
      "action": "replace",
      "value": "{\"image\": \"myapp:v2\", \"ports\": [\"8080:80\"], \"restart\": \"always\"}"
    }]
  }
}
```

---

## Batch Execute Patterns

Use `cortex_act_batch_execute` to collapse multiple round-trips into one. It runs operations sequentially and is ideal for:

- Parallel independent reads (topology + map_overview + search)
- Edit + verify patterns (edit → run_diagnostics)
- Explore + checkpoint + edit sequences

**Rules:**
- Do not nest `cortex_act_batch_execute` inside itself.
- Increase `max_chars_per_op` (default 4000) when an operation returns large output (e.g. `map_overview`, `deep_slice`).
- Use `fail_fast=true` when later operations depend on earlier ones succeeding.

### Example: Explore + Edit + Verify

```json
{
  "name": "cortex_act_batch_execute",
  "arguments": {
    "fail_fast": true,
    "max_chars_per_op": 8000,
    "operations": [
      {
        "tool_name": "cortex_code_explorer",
        "parameters": { "action": "map_overview", "repoPath": "/path/to/repo", "target_dirs": ["crates/cortex-act/src/act"] }
      },
      {
        "tool_name": "cortex_act_edit_ast",
        "parameters": {
          "file": "[cortex-act]/src/act/shell_exec.rs",
          "edits": [{ "target": "augment_unix_path", "action": "replace", "code": "fn augment_unix_path(...) { ... }" }]
        }
      },
      {
        "tool_name": "cortex_act_shell_exec",
        "parameters": { "run_diagnostics": true, "cwd": "/path/to/repo", "timeout_secs": 60 }
      }
    ]
  }
}
```

---

## Shell Exec: PATH and Diagnostics

- On Unix, `cortex_act_shell_exec` automatically prepends `~/.cargo/bin`, `~/.local/bin`, and `/usr/local/bin` to `PATH` so tools like `cargo`, `node`, and `python3` work even when launched from an IDE with a reduced PATH.
- `run_diagnostics=true` auto-detects the build system from the `cwd` manifest and runs the appropriate compiler check. Supported: cargo, tsc (tsconfig.json), go (go.mod), Maven (pom.xml), Gradle (build.gradle).
- The hard `timeout_secs` kill is cross-platform: `kill -9` on Unix, `taskkill /F` on Windows.
