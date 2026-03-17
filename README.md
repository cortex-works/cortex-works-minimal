# cortex-works (minimal)

> **One binary. 14 tools. Built to make AI agents faster, cheaper, and more precise.**

`cortex-works-minimal` is a lean MCP server for AI-powered IDEs. It replaces slow, token-hungry IDE tooling with a disciplined surface of 14 surgical tools — covering repo mapping, symbol analysis, structural editing, semantic search, filesystem work, and bounded shell execution.

```bash
cargo build --release -p cortex-mcp
./target/release/cortex-mcp   # wire to your IDE via MCP stdio
```

---

## Why Agents Perform Better Here

Standard IDE tools expose the filesystem as a flat text surface. Agents waste tokens reading full files to find the three lines they need, burning context on boilerplate and build artifacts that should never appear in a prompt.

`cortex-works` is designed around the opposite principle: **return exactly what the agent needs, nothing more.**

### Token Efficiency by Design

The stack enforces a layered, progressive-disclosure model:

| Stage | Tool | What the agent learns |
|-------|------|----------------------|
| L1 — Discover | `workspace_topology` | Roots, manifests, language hints — <500 tokens |
| L2 — Map | `map_overview` / `skeleton` | File tree + symbol list for selected dirs only |
| L3 — Read | `read_source` / `deep_slice` | Exact symbol body or focused file slice |

An agent that follows this pattern typically spends 5–10× fewer tokens on codebase orientation than one using raw file reads.

### Structure-Aware Edits, Not Line Patches

Line-number-based edits break as soon as another tool touches the same file. `cortex-works` edits by **name**:

- `cortex_act_edit_ast` — replace a Rust/TS/Python function or struct by symbol name
- `cortex_act_edit_data_graph` — update a JSON or YAML key by JSONPath target
- `cortex_act_edit_markup` — rewrite a Markdown section by heading or HTML node by tag/id
- `cortex_act_sql_surgery` — swap a DDL statement by type and object name

The agent supplies the new content; the tool handles byte offsets, bottom-up ordering, and post-edit validation automatically.

### Built-In Auto-Healer

When an AST edit produces a Rust file with syntax errors, the editor automatically invokes a local LLM (LM Studio / Ollama / llama.cpp) to repair the code before committing the write. The agent never sees a broken file.

### Multi-Root Path Routing, Built-In

VS Code multi-root workspaces, Zed multi-project, and JetBrains polyrepo sessions all work transparently via prefixed path convention: `[FolderName]/path/to/file`. No manual path juggling required.

### One Round-Trip for Sequential Workflows

`cortex_act_batch_execute` collapses multi-step workflows — explore → checkpoint → edit → verify — into a single MCP call, cutting back-and-forth latency by up to an order of magnitude.

---

## The 14 Active Tools

### Intelligence (read-only)

| Tool | Use when |
|------|----------|
| `cortex_code_explorer` | First look at an unfamiliar repo; topology, maps, deep slices |
| `cortex_symbol_analyzer` | You already know the symbol; read source, find usages, blast radius |
| `cortex_chronos` | Save a rollback point before risky refactors; compare before/after |
| `cortex_manage_ast_languages` | A non-core parser is missing (Go, PHP, Ruby, Java, C, C++, C#, Dart) |

### Structural Mutations

| Tool | Use when |
|------|----------|
| `cortex_act_edit_ast` | Editing a Rust, TypeScript, or Python symbol by name |
| `cortex_act_edit_data_graph` | Updating JSON or YAML keys structurally |
| `cortex_act_edit_markup` | Rewriting Markdown sections, HTML/XML nodes by heading/tag/id |
| `cortex_act_sql_surgery` | Replacing a DDL statement (CREATE TABLE, CREATE INDEX, …) |
| `cortex_fs_manage` | Creating, copying, moving, deleting files and dirs; patching `.env`/`.ini` |

### Search, Execution, and Runtime

| Tool | Use when |
|------|----------|
| `cortex_search_exact` | A literal string, regex, or identifier you know exactly |
| `cortex_semantic_code_search` | A concept you don't know the exact name of |
| `cortex_act_shell_exec` | Short diagnostic commands; manifest-aware `run_diagnostics` mode |
| `cortex_act_batch_execute` | Collapsing a sequential workflow (explore → edit → verify) into one call |
| `cortex_mcp_hot_reload` | After rebuilding, reload the worker on the same stdio channel without restarting the IDE |

---

## Quick Start

```bash
# 1. Clone and build
git clone https://github.com/cortex-works/cortex-works-minimal
cd cortex-works-minimal
cargo build --release -p cortex-mcp

# 2. Wire to VS Code (add to mcp.json)
{
  "cortex-works": {
    "type": "stdio",
    "command": "/absolute/path/to/target/release/cortex-mcp"
  }
}
```

---

## Recommended Agent Workflow

```text
1. cortex_code_explorer(workspace_topology)      ← discover roots cheaply
2. cortex_code_explorer(map_overview, target_dirs=[...])  ← inspect only what matters
3. cortex_symbol_analyzer(read_source, ...)      ← read exact code before editing
4. cortex_chronos(save_checkpoint, ...)          ← rollback point before risky changes
5. cortex_act_edit_ast / edit_data_graph / …     ← narrowest structural edit
6. cortex_act_shell_exec(run_diagnostics=true)   ← verify the change compiled
```

---

## Runtime Layout

```text
cortex-mcp  (IDE gateway — MCP stdio transport)
    │
    ├── cortex-ast  (intelligence: topology, slices, symbols, checkpoints, grammar loading)
    │
    ├── cortex-act  (mutations: AST edits, data/markup/SQL edits, search, shell exec, batching)
    │
    └── cortex-db   (persistence: LanceDB semantic index, SQLite project metadata)
```

All four crates compile into one binary (`cortex-mcp`). The IDE talks only to that binary.

---

## Documentation

| Doc | Content |
|-----|---------|
| [docs/ARCH.md](docs/ARCH.md) | Architecture, component roles, multi-root routing, progressive disclosure model |
| [docs/USAGE.md](docs/USAGE.md) | Workflow patterns, path conventions, batch examples, JSON vs YAML rules |
| [docs/DEVELOPING.md](docs/DEVELOPING.md) | Build commands, test suite, schema source-of-truth, dependency management |
