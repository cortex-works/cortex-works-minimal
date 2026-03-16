# CortexAST MCP Setup

CortexAST is a **Pure Rust MCP server** (stdio JSON-RPC). No editor-side add-on required.

## 1) Get the Binary

**Option A — Download pre-built binary** (recommended):

Visit [Releases](https://github.com/DevsHero/CortexAST/releases/latest) and download the binary for your OS. Make it executable on macOS/Linux:

```bash
chmod +x cortexast-macos-aarch64   # adjust filename for your platform
```

**Option B — Build from source**:

```bash
git clone https://github.com/DevsHero/CortexAST.git
cd CortexAST
cargo build --release
# binary: ./target/release/cortexast
```

## 2) Connect an MCP Client

### Recommended: pass `--root` for reliable workspace detection

CortexAST needs to know where your project lives. When VS Code or Claude Desktop spawns the binary it uses `$HOME` as the working directory, so the server cannot auto-detect your project. Pass `--root` to fix this.

```json
{
  "mcpServers": {
    "cortexast": {
      "command": "/absolute/path/to/cortexast",
      "args": ["mcp", "--root", "/absolute/path/to/your/project"]
    }
  }
}
```

Alternatively, set the `CORTEXAST_ROOT` environment variable (useful for Claude Desktop `env` blocks):

```json
{
  "mcpServers": {
    "cortexast": {
      "command": "/absolute/path/to/cortexast",
      "args": ["mcp"],
      "env": { "CORTEXAST_ROOT": "/absolute/path/to/your/project" }
    }
  }
}
```

> **VS Code Copilot users**: add `--root` to the `args` array in your `settings.json` `github.copilot.chat.mcpServers` entry, pointing to the workspace folder you want CortexAST to target.

Fallback priority when `--root` / `CORTEXAST_ROOT` are omitted:
1. Per-call `repoPath` argument (always works)
2. `workspaceFolders[0].uri` from MCP `initialize` params
3. `VSCODE_WORKSPACE_FOLDER` — VS Code / Cursor / Windsurf
4. `VSCODE_CWD` — VS Code secondary
5. `IDEA_INITIAL_DIRECTORY` — JetBrains (IntelliJ, GoLand, WebStorm, …)
6. `PWD` / `INIT_CWD` — POSIX shell / Zed / Neovim (skipped if equal to `$HOME`)
7. **Find-up heuristic** — if a tool call includes `path` / `target` / `target_dir` / `target_dirs` / `only_dir` / `only_dirs`, CortexAST walks ancestor directories looking for a project root marker (`.git`, `Cargo.toml`, `package.json`)
8. `cwd` (usually `$HOME` in some IDEs). If `cwd` resolves to `$HOME` or OS root, CortexAST returns a **CRITICAL** error and refuses to proceed.

Restart your MCP client after editing the config.

### Reloading after binary update (Seamless Rebirth)

After rebuilding (`cargo build --release`) or downloading a new binary, call `cortex_mcp_hot_reload` from the agent. The worker exits with code 42 and the supervisor restarts the new binary on the same stdio channel. After restart, call `initialize` again and refresh `tools/list` if the client needs the latest schema.

If you still hit stale-schema errors like **"must be equal to one of the allowed values"**, use VS Code Command Palette → **"MCP: Restart Server"** as fallback. `Developer: Reload Window` is last resort only.

## 3) MCP Tools

CortexAST exposes **4 Megatools** (preferred) with `action` enums.
Legacy tool names are accepted as compatibility shims but are deprecated.

```
Megatools (preferred):

├─ cortex_code_explorer(action, ...)
│  ├─ action=workspace_topology(max_chars?, repoPath?)
│  ├─ action=map_overview(target_dirs? | target_dir?, search_filter?, max_chars?, ignore_gitignore?, repoPath?)
│  ├─ action=deep_slice(target, budget_tokens?, query?, query_limit?, only_dirs? | only_dir?, skeleton_only?, max_chars?, repoPath?)
│  │  └─ Returns: token-budget-aware XML slice (optionally skeleton-only)
│  └─ action=skeleton(target_dirs? | target_dir?, max_chars?, ignore_gitignore?, repoPath?)

├─ cortex_symbol_analyzer(action, ...)
│  ├─ action=read_source(path, symbol_name? | symbol_names?, skeleton_only?, max_chars?, repoPath?)
│  ├─ action=find_usages(target_dir, symbol_name, max_chars?, repoPath?)
│  ├─ action=find_implementations(target_dir, symbol_name, max_chars?, repoPath?)
│  ├─ action=blast_radius(target_dir, symbol_name, max_chars?, repoPath?)
│  └─ action=propagation_checklist(symbol_name, aliases?, target_dir?, ignore_gitignore?, max_chars?, repoPath?)

├─ cortex_chronos(action, ...)
│  ├─ action=save_checkpoint(path, symbol_name, semantic_tag, repoPath?)
│  ├─ action=list_checkpoints(repoPath?)
│  ├─ action=compare_checkpoint(symbol_name, tag_a, tag_b, path?, repoPath?)
│  │  └─ Magic: tag_b="__live__" compares tag_a against current filesystem state (requires path)
│  └─ action=delete_checkpoint(symbol_name?, semantic_tag?/tag?, path?, repoPath?)

└─ run_diagnostics(repoPath, max_chars?)
  └─ Returns: compiler errors pinned to file:line with code context
```

Output safety:
- All tools support `max_chars` (default **8000**). The server truncates at this limit and appends a `✂️ [TRUNCATED]` marker. VS Code Copilot spills responses larger than ~8 KB to workspace storage, so keep `max_chars` ≤ 8000 for Copilot sessions.
- **Chronos namespaces:** All Chronos actions accept an optional `namespace` parameter (default: `"default"`). Use distinct names like `"qa-run-1"` per session, then purge all checkpoints at once with `action=delete_checkpoint, namespace="qa-run-1"` (omit `symbol_name` and `semantic_tag`).

Multi-root conventions:

- Start with `cortex_code_explorer(action="workspace_topology")` to list roots, manifest kinds, and language hints.
- Use `[FolderName]/path/to/file` for cross-root paths.
- In multi-root MCP sessions, let `initialize.workspaceFolders` drive workspace discovery. Pass `repoPath` only when you intentionally want to pin a call to one root.
- Prefer `target_dirs=["[Backend]", "[Frontend]"]` over a single `target_dir="."` when multiple roots are present.
- Prefer `only_dirs=["[Backend]"]` to keep `deep_slice` semantic ranking scoped and token-efficient.

## 4) Optional Repo Config

CortexAST reads `.cortexast.json` from the target repo root.
It only accepts `.cortexast.json`.

Note on real-world usage:

- For MCP usage, `.cortexast.json` is re-read on every tool call, so config edits take effect on the next request (no server restart required).
- If you change `vector_search.model` or `vector_search.chunk_lines`, CortexAST will automatically reset/rebuild the local vector index on the next query.

Example:

```json
{
  "output_dir": ".cortexast",
  "scan": {
    "exclude_dir_names": ["generated", "tmp", "fixtures"]
  },
  "skeleton_mode": true,
  "vector_search": {
    "model": "minishlab/potion-base-8M",
    "chunk_lines": 40,
    "default_query_limit": 30
  },
  "token_estimator": {
    "chars_per_token": 4,
    "max_file_bytes": 1048576
  }
}
```
