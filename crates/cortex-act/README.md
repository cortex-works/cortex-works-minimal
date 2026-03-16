# cortex-act 🖐️

> **The AI-Native Code Action Backend** — the "hands" of the Cortex ecosystem.
>
> `cortex-ast` sees. `cortex-act` does.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)

---

## Overview

`cortex-act` is a pure-Rust **MCP (Model Context Protocol) server** that provides AI coding agents with surgical write, edit, and execute capabilities. It is designed to achieve "God Hand" precision for both code and complex data/documentation files.

| Project | Role | Capability |
|---------|------|------------|
| `CortexAST` | 👁️ Eyes | Read-only: code analysis, symbol lookup, semantic navigation |
| **`cortex-act`** | ✋ Hands | Write/execute: file edits, data graph patching, shell commands |
| `CortexSync` | 🧠 Brain | Global memory: captures intent/decisions, vectorizes memories |

> [!IMPORTANT]
> **Full Feature Requirement:** To enable full ecosystem features such as automatic task-end memory capture, permanent decision tracking, and cross-project knowledge recall, you **MUST** have `CortexSync` installed and running.

---

## Tools

### 1. ✏️ `cortex_act_edit_ast`
Replace or delete a named symbol (function/class/struct) in any source file. Targets by name, not line number. Auto-heals broken AST via local LLM if validation fails. Use `cortexast map_overview` to discover symbol names first.

### 2. 🕸️ `cortex_act_edit_data_graph`
Advanced, comment-preserving deep-patching for **JSON, YAML, and TOML** (e.g., OpenAPI specs). Uses JSONPath logic over Tree-sitter nodes to guarantee that formatting and comments remain 100% intact.

### 3. 📝 `cortex_act_edit_markup`
Surgical, structural edits for **Markdown, HTML, and XML**. Targeted by heading name, element index (e.g. `table:0`), or tag/id selectors.

### 4. 🔪 `cortex_act_sql_surgery`
Edit DDL structures (like `CREATE TABLE`) inside massive SQL dumps without loading the entire file, using `sqlparser-rs` byte-offsets.

### 5. 🦆 `cortex_act_duckdb_query`
Run SQL directly against **CSV, JSONL, and Parquet** files on disk via DuckDB. Enables lightning-fast discovery and mutations of massive datasets at an extremely low token cost.

### 6. ⚙️ `cortex_patch_file`
Surgical patcher for simple config files (dot-path), markdown sections (heading), or `.env` files. Preferred for lighter tasks.

### 7. ⏳ `cortex_act_run_async` / `check_job` / `kill_job`
Asynchronous job runner for shell commands. Returns a `job_id` and allows non-blocking polling of status and log tails.

---

## Architecture

```
cortex-act/
├── src/
│   ├── main.rs            # MCP stdio server (JSON-RPC 2.0)
│   └── act/
│       ├── mod.rs
│       ├── editor.rs      # AST Semantic Patcher (Rust/Python/TS)
│       ├── data_editor.rs # Tree-sitter Data Patcher (JSON/YAML/TOML)
│       ├── markup_editor.rs # Markup Patcher (MD/HTML/XML)
│       ├── sql_editor.rs  # SQL DDL Patcher
│       ├── data_query.rs  # DuckDB Query Engine
│       ├── auto_healer.rs # LLM-based syntax error repair
│       ├── job_manager.rs # Async background job runner
│       └── ..._patcher.rs # Legacy simple patchers
```

## Design Principles

1. **Surgical Precision** — Never rewrites entire files. Uses byte-level replacement to preserve 100% of comments and formatting.
2. **Token Efficiency** — Optimized for interacting with massive files (10k+ lines) using under 500 tokens via the Map -> Target -> Patch workflow.
3. **Safe Mutation** — Implements safe-swap sequences for data files (writing to temp then renaming) to prevent data loss.
4. **Auto-Healing** — Syntax errors in code edits trigger a local LLM repair loop with a strict 10-second timeout.
5. **CortexSync Dependency** — Integrated with the CortexSync permanent memory layer for end-to-end task accountability.

## Author

**Thanon Aphithanawat** — [thanon@aphithanawat.me](mailto:thanon@aphithanawat.me)

## License

MIT © 2026 Thanon Aphithanawat
