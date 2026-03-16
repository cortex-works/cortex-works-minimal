# CortexAST

Read-only code intelligence for the minimal `cortex-works` branch.

`cortex-ast` is the source of truth for the four analysis tools exposed by `cortex-mcp`:

- `cortex_code_explorer`
- `cortex_symbol_analyzer`
- `cortex_chronos`
- `cortex_manage_ast_languages`

## What It Does

- Maps repository structure by symbols instead of raw files.
- Extracts exact symbol bodies and reference locations.
- Computes blast radius before refactors.
- Stores AST-aware checkpoints for before/after comparison.
- Loads extra Tree-sitter Wasm grammars on demand.

## Tool Guidance

### `cortex_code_explorer`

Use this first when the repo is unfamiliar.

- `map_overview`: fastest repo orientation.
- `deep_slice`: fetches relevant bodies and context for a file or query.
- `skeleton`: signatures-only repo summary.

### `cortex_symbol_analyzer`

Use this when you already know the symbol or need exact impact analysis.

- `read_source`: exact code before editing.
- `find_usages`: references across the repo.
- `find_implementations`: trait/interface implementation lookup.
- `blast_radius`: callers + callees before rename/delete.
- `propagation_checklist`: shared-type update checklist.

### `cortex_chronos`

Use before risky refactors.

- `save_checkpoint`
- `list_checkpoints`
- `compare_checkpoint`
- `delete_checkpoint`

### `cortex_manage_ast_languages`

Installs extra Tree-sitter Wasm grammars from GitHub releases, caches them in `~/.cortex-works/grammars/`, and hot-reloads them without restarting the MCP server.

Core languages:

- Rust
- TypeScript
- Python

Downloadable languages:

- Go
- PHP
- C
- C++
- C# (`c_sharp`)
- Java
- Ruby
- Dart

## Build

```bash
cargo build -p cortex-ast
```
