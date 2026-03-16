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
