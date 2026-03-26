# cortex-works (minimal) Architecture

This document describes the runtime architecture that matters for the minimal branch.

## Scope

The supported MCP runtime is intentionally small:

- `cortex-mcp` is the only IDE-facing binary.
- `cortex-ast` provides read-only code intelligence.
- `cortex-act` provides edits, searches, filesystem actions, and bounded shell execution.
- `cortex-db` provides local storage used by checkpoint features.

Other folders may exist in the monorepo, but they are not part of the active minimal MCP surface unless a task explicitly targets them.

## Design Goals

- keep the public tool surface compact and predictable
- make multi-root workspaces first-class instead of a compatibility layer
- minimize token waste by supporting discovery, mapping, and slicing as separate stages
- make edits precise by operating on symbols or structures instead of line numbers

## Runtime Topology

```text
IDE / MCP Client
        |
        v
  cortex-mcp
        |
        +-------------------+
        |                   |
        v                   v
   cortex-ast          cortex-act
        |                   |
        +---------+---------+
                  |
                  v
              cortex-db
```

## Component Roles

### `cortex-mcp`

- owns the MCP transport over STDIO
- exposes the 13 active tools in `tools/list`
- dispatches read-only AST requests to `ServerState`
- dispatches ACT requests through synchronous handlers and passes workspace root context downstream

### `cortex-ast`

- captures workspace roots from MCP `initialize`
- powers `workspace_topology`, `map_overview`, `deep_slice`, symbol reads, usages, and Chronos checkpoints
- resolves `[FolderName]/...` paths for read-only tool flows
- provides the authoritative workspace-root state used by `cortex-mcp`

### `cortex-act`

- performs AST-backed source edits and structural data/markup/SQL changes
- runs bounded shell commands and exact search helpers
- now resolves `[FolderName]/...` paths for edit, search, filesystem, and shell `cwd` parameters using the workspace roots passed down from `cortex-mcp`
- shell commands execute via `sh -c` on Unix and `cmd /C` on Windows; PATH is augmented on Unix to include `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin`
- timeout kill uses `kill -9` on Unix and `taskkill /F` on Windows
- `cortex_act_edit_data_graph` supports full upsert (insert new keys) for JSON; YAML only supports updating existing keys

### `cortex-db`

- stores semantic-search data and local project metadata
- stores checkpoint data used by Chronos and related flows
- remains an implementation detail behind MCP tools

## Multi-Root Path Routing

Multi-root support in the minimal branch is a two-part contract:

1. `cortex-ast` captures all workspace folders from MCP `initialize`.
2. `cortex-mcp` forwards those roots to ACT handlers so both subsystems resolve the same prefixed paths.

Path rules:

- absolute paths are accepted as-is
- `[FolderName]/path/to/file` resolves against the matching workspace root name
- bare relative paths resolve against the primary workspace root

This means the same prefix convention works across both the “Eyes” and the “Hands” sides of the stack.

## Progressive Disclosure Model

The minimal branch is designed around layered inspection rather than whole-repo dumps:

- `workspace_topology`: discover roots and manifests cheaply
- `map_overview` and `skeleton`: inspect selected roots with `target_dirs=[...]`
- `deep_slice` or `read_source`: read exact files or symbols only when needed

This staged approach keeps tool output smaller and makes multi-root sessions practical for LLMs.

## Tool Surface Summary

### Intelligence

- `cortex_code_explorer`
- `cortex_symbol_analyzer`
- `cortex_chronos`
- `cortex_manage_ast_languages`

### Mutations and Filesystem

- `cortex_act_edit_ast`
- `cortex_act_edit_data_graph`
- `cortex_act_edit_markup`
- `cortex_act_sql_surgery`
- `cortex_fs_manage`

### Search, Execution, and Runtime

- `cortex_act_shell_exec`
- `cortex_act_batch_execute`
- `cortex_search_exact`
- `cortex_mcp_hot_reload`

## Schema Source of Truth

Tool schemas live in dedicated modules — never inline in `server.rs`:

| File | Owns schema for |
|------|-----------------|
| `crates/cortex-ast/src/tool_schemas.rs` | `cortex_code_explorer`, `cortex_symbol_analyzer`, `cortex_chronos` |
| `crates/cortex-ast/src/grammar_manager.rs` | `cortex_manage_ast_languages` (+ action constants + runtime handler) |
| `crates/cortex-mcp/src/tools/act.rs` | all 9 ACT-side tools |
| `crates/cortex-ast/src/server.rs` | dispatch only — no schema text |

Action name constants in `grammar_manager.rs` (`ACTION_STATUS`, `ACTION_ADD`) are shared by both
the JSON schema enum and the runtime match arm, preventing schema-vs-dispatch drift.

## Build and Validation

The production build target for this branch is:

```bash
cargo build --release -p cortex-mcp
```

Recommended validation is documented in [docs/DEVELOPING.md](DEVELOPING.md).
