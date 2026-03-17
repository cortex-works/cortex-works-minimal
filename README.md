# cortex-works (minimal)

The smallest useful Cortex runtime for coding agents.

`cortex-works-minimal` gives AI IDEs a tight 14-tool MCP surface for repo mapping, symbol analysis, structural edits, semantic search, filesystem work, and controlled shell execution, all behind one Rust binary: `cortex-mcp`.

Small enough to reason about. Sharp enough to trust.

## Why It Feels Better In Agents

- One binary, one public tool surface, no proxy maze in the middle.
- Structure-aware edits beat brittle line-number patches.
- Multi-root path routing is built in, so agents can work across workspace folders without inventing path conventions.
- Progressive disclosure keeps large repos navigable: topology first, focused slices second, exact source last.
- Batch execution reduces chat round-trips without pretending operations are parallel.

This branch intentionally centers on four crates only: `cortex-mcp`, `cortex-ast`, `cortex-act`, and `cortex-db`.

## What You Get

- Repo intelligence: topology, maps, deep slices, symbol lookups, usage search, blast radius, and AST-aware checkpoints.
- Structural mutations: Rust, TypeScript, Python, JSON, YAML, Markdown, HTML, XML, SQL, and filesystem operations.
- Search in two modes: semantic search for intent and exact search for literals, identifiers, and regexes.
- Bounded execution: short shell commands and manifest-aware diagnostics without turning the tool into a long-running terminal.
- One-round-trip workflows: batch several tool calls into a single sequential `BatchSummary` result.

## Tool Chooser

- Start with `cortex_code_explorer` when the repo is unfamiliar.
- Use `cortex_symbol_analyzer` when you already know the symbol.
- Save a `cortex_chronos` checkpoint before risky refactors.
- Pick `cortex_act_edit_ast` for Rust/TS/Python symbol edits.
- Pick `cortex_act_edit_data_graph` for JSON or YAML keys.
- Pick `cortex_act_edit_markup` for headings, tags, ids, and sections.
- Pick `cortex_act_sql_surgery` for DDL statements.
- Pick `cortex_fs_manage` for raw files, folders, copy/move/delete, and `.env`/`.ini` patching.
- Pick `cortex_search_exact` when you know the string or regex.
- Pick `cortex_semantic_code_search` when you know the idea but not the name.
- Pick `cortex_act_shell_exec` for short commands and diagnostics only.
- Pick `cortex_act_batch_execute` for short sequential workflows such as explore → edit → verify.
- Pick `cortex_mcp_hot_reload` only after rebuilding, and make it the final batch operation if you batch it at all.

## Runtime Layout

- `cortex-mcp`: MCP gateway, schema surface, dispatcher.
- `cortex-ast`: topology, slices, symbol analysis, checkpoints, grammar loading.
- `cortex-act`: structural edits, filesystem ops, exact search, semantic search, shell execution, batching.
- `cortex-db`: local SQLite and LanceDB support for persistence and indexing.

## Quick Start

```bash
git clone https://github.com/cortex-works/cortex-works-minimal
cd cortex-works-minimal

cargo build --release -p cortex-mcp
./target/release/cortex-mcp
```

## Recommended Flow

```text
1. cortex_code_explorer(workspace_topology)
2. cortex_code_explorer(map_overview)
3. cortex_symbol_analyzer(read_source)
4. cortex_chronos(save_checkpoint)
5. edit with the narrowest structural tool
6. cortex_act_shell_exec(run_diagnostics=true)
```

## Documentation

- [docs/ARCH.md](docs/ARCH.md) explains the minimal architecture and request flow.
- [docs/USAGE.md](docs/USAGE.md) shows agent workflows, batch patterns, and multi-root examples.
- [docs/DEVELOPING.md](docs/DEVELOPING.md) covers build, validation, and release checks.

## GitHub About

Lean MCP server for AI IDEs with AST intelligence, structural editing, multi-root path routing, and a disciplined 14-tool surface in pure Rust.
