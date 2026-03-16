# cortex-works (minimal)

Lean MCP server for AI IDEs. The minimal branch keeps the runtime focused on one Rust binary, `cortex-mcp`, backed by AST intelligence, multi-root path routing, and surgical code-edit tools.

## Why Minimal

This branch is optimized for agent workflows inside editors such as VS Code, Cursor, and Windsurf:

- direct MCP transport with no extra middleware in the hot path
- progressive disclosure for large or multi-root workspaces
- structural edits that avoid brittle line-number or regex workflows
- a compact 14-tool public surface that is easier to reason about and document

The repository may still contain legacy or auxiliary folders, but the supported MCP runtime in this branch is centered on four crates: `cortex-mcp`, `cortex-ast`, `cortex-act`, and `cortex-db`.

## Key Capabilities

- Multi-root native: both AST and ACT tools understand `[FolderName]/path/to/file` and resolve it against workspace roots captured from MCP `initialize`.
- Progressive workspace disclosure: start with `workspace_topology`, then narrow to `target_dirs=[...]` or `only_dirs=[...]` before reading full source bodies.
- Surgical edits: mutate Rust, TypeScript, Python, JSON, YAML, Markdown, HTML, XML, and SQL with structure-aware tools instead of blind text replacement.
- Chronos checkpoints: save and compare AST-aware snapshots before and after refactors.
- Search in two modes: semantic lookup for intent, exact regex search for literal strings, identifiers, and error messages.
- Single-binary deployment: build `cortex-mcp`, point your IDE at it, and the whole tool surface is available over STDIO.

## Runtime Layout

- `cortex-mcp`: MCP gateway and tool registry.
- `cortex-ast`: workspace discovery, symbol analysis, slicing, checkpoints, grammar loading.
- `cortex-act`: edits, filesystem operations, exact search, semantic search, shell execution.
- `cortex-db`: local SQLite and LanceDB helpers for semantic indexing and persistence.

## Active Tool Surface

### Intelligence

- `cortex_code_explorer`
- `cortex_symbol_analyzer`
- `cortex_chronos`
- `cortex_manage_ast_languages`

### Edits and Mutations

- `cortex_act_edit_ast`
- `cortex_act_edit_data_graph`
- `cortex_act_edit_markup`
- `cortex_act_sql_surgery`
- `cortex_fs_manage`

### Search, Execution, and Runtime Control

- `cortex_act_shell_exec`
- `cortex_act_batch_execute`
- `cortex_semantic_code_search`
- `cortex_search_exact`
- `cortex_mcp_hot_reload`

## Quick Start

```bash
git clone https://github.com/cortex-works/cortex-works-minimal
cd cortex-works-minimal

cargo build --release -p cortex-mcp
./target/release/cortex-mcp
```

## Documentation

- [docs/ARCH.md](docs/ARCH.md) — minimal-branch architecture and request flow
- [docs/USAGE.md](docs/USAGE.md) — agent workflow, multi-root patterns, and examples
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — build, validation, and release checks

## GitHub About

Lean MCP server for AI IDEs (Cursor, VS Code). Delivers high-performance AST intelligence, multi-root workspace support, and surgical code editing in pure Rust.
