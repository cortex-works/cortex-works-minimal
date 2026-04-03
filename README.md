# cortex-works (minimal)

> **One binary. 13 tools. Built to make AI agents faster, cheaper, and more precise.**

`cortex-works-minimal` is a lean MCP server for AI-powered IDEs. It replaces slow, token-hungry IDE tooling with a disciplined surface of 13 surgical tools — covering repo mapping, symbol analysis, structural editing, filesystem work, and bounded shell execution.

✅ **Cross-platform tested:** macOS, Windows, and Ubuntu (other Linux distros may work too).

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

### Fail-Safe AST Editing

`cortex_act_edit_ast` validates syntax after edits and aborts safely if the output is invalid. This avoids writing broken code to disk.

### Multi-Root Path Routing, Built-In

VS Code multi-root workspaces, Zed multi-project, and JetBrains polyrepo sessions all work transparently via prefixed path convention: `[FolderName]/path/to/file`. No manual path juggling required.

### One Round-Trip for Sequential Workflows

`cortex_act_batch_execute` collapses multi-step workflows — explore → checkpoint → edit → verify — into a single MCP call, cutting back-and-forth latency by up to an order of magnitude.

---

## The 17 Active Tools

### Z4-Native Intelligence

| Tool | Use when |
|------|----------|
| `cortex_z4_reg_reader` | You need canonical ids, registry aliases, or one build catalog in low-token form |
| `cortex_z4_hex_bridge` | A z4 source hides mnemonics or tokens inside `DOC:"0x..."` payloads |
| `cortex_z4_unit_scan` | You need build-unit membership or rename safety before touching a z4 label |

### Intelligence (read-only)

| Tool | Use when |
|------|----------|
| `cortex_code_explorer` | First look at an unfamiliar repo; topology, maps, deep slices |
| `cortex_symbol_analyzer` | You already know the symbol; read source, find usages, blast radius |
| `cortex_chronos` | Save a rollback point before risky refactors; compare before/after |
| `cortex_manage_ast_languages` | A non-core parser is missing for non-z4 side work |

### Z4-Native Mutation

| Tool | Use when |
|------|----------|
| `cortex_z4_atomic_sync` | You want a phase-scoped, validated, focused commit after z4 edits |

### Structural Mutations

| Tool | Use when |
|------|----------|
| `cortex_act_edit_ast` | Editing a Rust, TypeScript, or Python symbol by name |
| `cortex_act_edit_data_graph` | Updating JSON or YAML keys structurally |
| `cortex_act_edit_markup` | Rewriting Markdown sections, HTML/XML nodes by heading/tag/id |
| `cortex_act_sql_surgery` | Replacing a DDL statement (CREATE TABLE, CREATE INDEX, …) |
| `cortex_fs_manage` | Creating, copying, moving, deleting files and dirs; default mutation path for `.z4`, `.filelist`, `.project.z4` |

### Search, Execution, and Runtime

| Tool | Use when |
|------|----------|
| `cortex_search_exact` | A literal string, regex, or identifier you know exactly |
| `cortex_act_shell_exec` | Short diagnostic commands; z4-aware or manifest-aware `run_diagnostics` mode |
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
1. cortex_code_explorer(workspace_topology, repoPath="/abs/path")
   ← discover workspace roots cheaply (always pass repoPath)

2. cortex_code_explorer(map_overview, target_dirs=["crates/my-crate/src"])
   ← inspect only the dirs that matter

3. cortex_symbol_analyzer(read_source, path="crates/my-crate/src/lib.rs", symbol_name="MyStruct")
   ← read exact code before editing (use `path`, not `file`)

4. cortex_chronos(save_checkpoint, path="...", symbol_name="...", semantic_tag="pre-refactor")
   ← rollback point before risky changes

5. cortex_act_edit_ast / edit_data_graph / edit_markup / …
   ← narrowest structural edit matching the file type

6. cortex_act_shell_exec(run_diagnostics=true, cwd=".")
   ← verify the change compiled
```

---

## Common Pitfalls

| Wrong | Correct |
|-------|---------|
| `cortex_symbol_analyzer(read_source, file="...")` | `path="..."` (not `file`) |
| `cortex_symbol_analyzer(find_usages, path="...")` | `target_dir="."` (not `path`) |
| `cortex_chronos(compare_checkpoint, semantic_tag="...")` | `tag_a="before"` + `tag_b="after"` (or `tag_b="__live__"`) |
| `cortex_fs_manage(action=read, ...)` | No `read` action — use `cortex_symbol_analyzer` or `deep_slice` |
| `cortex_fs_manage(action=delete, paths=["/tmp/..."])` | Delete is workspace-guarded; use workspace-relative paths |
| `cortex_act_edit_data_graph(edits=[..., value=..., code=...])` | Field is `value`, not `code` |
| `cortex_code_explorer(workspace_topology)` without `repoPath` | Always pass `repoPath` (absolute) — omitting it triggers a safety block |

---

## Runtime Layout

```text
cortex-mcp  (IDE gateway — MCP stdio transport)
    │
    ├── cortex-ast  (intelligence: topology, slices, symbols, checkpoints, grammar loading)
    │
    ├── cortex-act  (mutations: AST edits, data/markup/SQL edits, search, shell exec, batching)
    │
    └── cortex-db   (persistence: local checkpoint storage)
```

All four crates compile into one binary (`cortex-mcp`). The IDE talks only to that binary.

---

## Documentation

| Doc | Content |
|-----|---------|
| [docs/ARCH.md](docs/ARCH.md) | Architecture, component roles, multi-root routing, progressive disclosure model |
| [docs/USAGE.md](docs/USAGE.md) | Workflow patterns, path conventions, batch examples, JSON vs YAML rules |
| [docs/DEVELOPING.md](docs/DEVELOPING.md) | Build commands, test suite, schema source-of-truth, dependency management |
