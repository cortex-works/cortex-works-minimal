# Developer Guide

Minimal-branch developer workflow for building, validating, and releasing
`cortex-mcp`.

Recent workspace-aware AST behavior to preserve:

- `cortex_code_explorer(action=workspace_topology)` is the lowest-token orientation step.
- `map_overview` and `skeleton` prefer `target_dirs=[...]` in multi-root sessions.
- `deep_slice` supports focused slicing with `single_file=true`.
- Cross-root file references use the `[FolderName]/path/to/file` convention.
- Both AST and ACT tools must resolve workspace-prefixed paths consistently.
- ACT tools that accept `file`, `project_path`, `paths`, or `cwd` should continue to work with either workspace-prefixed paths or absolute paths.

## Build Targets

Fast local compile:

```bash
cargo check --workspace
```

Production build:

```bash
cargo build --release -p cortex-mcp
```

## Validation Commands

Full test suite across all crates:

```bash
cargo test --workspace
```

Unit tests for the AST schema layer only:

```bash
cargo test -p cortexast tool_schemas
cargo test -p cortexast grammar_manager
```

AST server smoke test (requires a debug build):

```bash
cargo test -p cortexast mcp_stdio_smoke
```

Full MCP gateway smoke test (builds and exercises all 13 tools end-to-end):

```bash
cargo test -p cortex-mcp full_tool_smoke_and_hot_reload
```

## What The Tests Cover

`full_tool_smoke_and_hot_reload` builds the release binary and runs a
comprehensive integration harness that validates:

- the active 13-tool MCP surface from `tools/list`
- AST tool calls through the real MCP transport
- multi-root `initialize.workspaceFolders` handling plus `workspace_topology`
- array-based `target_dirs` flows for workspace-aware AST calls
- ACT tool calls through the real MCP transport, including workspace-prefixed path routing
- filesystem patch semantics including `patch_action`
- supervisor-based `cortex_mcp_hot_reload`

`mcp_stdio_smoke` tests the cortex-ast binary directly without the MCP gateway layer.

## VS Code Config Path

macOS:
```text
$HOME/Library/Application Support/Code/User/mcp.json
```

Linux:
```text
$HOME/.config/Code/User/mcp.json
```

Windows:
```text
%APPDATA%\Code\User\mcp.json
```

## Schema Source of Truth

Tool schemas live in dedicated modules — never inline in `server.rs`:

- `crates/cortex-ast/src/tool_schemas.rs` — `cortex_code_explorer`, `cortex_symbol_analyzer`, `cortex_chronos`
- `crates/cortex-ast/src/grammar_manager.rs` — `cortex_manage_ast_languages` (schema + action constants + runtime handler)
- `crates/cortex-mcp/src/tools/act.rs` — all 9 ACT tool schemas
- `crates/cortex-ast/src/server.rs` — dispatch only; no schema text

The `tool_schemas` test suite (`cargo test -p cortexast tool_schemas`) verifies:
- all 4 AST tools are present in `tools/list`
- each schema has a non-empty name, description, and inputSchema
- `grammar_manager` action constants match the schema enum values

## Dependency Refresh

Refresh the workspace lockfile to the newest versions allowed by current
manifest constraints:

```bash
cargo update --workspace
```

After any lockfile refresh, rerun:

```bash
./scripts/test_cortexworks_all.sh
```

## Release Checklist

Before shipping a release build for the minimal branch:

1. run `cargo build --release -p cortex-mcp`
2. run `cargo test -p cortex-mcp --test full_stack_smoke`
3. confirm ACT tools still accept `[FolderName]/...` for edit, search, filesystem, and shell `cwd` parameters
4. if schemas changed, verify `tools/list` wording stays aligned with real behavior
5. after a rebuild, call `cortex_mcp_hot_reload` before checking updated schemas in the IDE

## Cross-Platform Notes

- Shell exec uses `sh -c` on Unix and `cmd /C` on Windows (handled automatically).
- PATH is augmented on Unix to include `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin` so IDEs that launch with reduced PATH can still find cargo/node/python.
- Timeout kill uses `kill -9 <pid>` on Unix and `taskkill /PID <pid> /F` on Windows.
- Maven/Gradle wrappers use `.cmd`/`.bat` extensions on Windows and `./mvnw`/`./gradlew` on Unix.
- The `permission_guard_catches_readonly` test is guarded with `#[cfg(unix)]` because `std::os::unix::fs::PermissionsExt` is not available on Windows.

## Package Name: `cortexast` vs `cortex-ast`

The Rust *crate directory* is `crates/cortex-ast/` but the Cargo *package name* inside its
`Cargo.toml` is **`cortexast`** (no hyphen). This means all cargo commands that target this
crate by package name must use the exact package name:

```bash
cargo test -p cortexast mcp_stdio_smoke   # correct
cargo test -p cortex-ast mcp_stdio_smoke  # FAILS — no package with that name
```

When adding a new `[dependencies]` entry that pulls in this crate, use:

```toml
cortexast = { path = "../cortex-ast" }
```

## Fast Linux Smoke Gate

For CI or quick Linux validation, run only the two targeted smoke tests
instead of the full workspace suite (which takes several minutes):

```bash
bash scripts/linux_smoke.sh
```

This runs in ~2 s and covers the full 13-tool MCP surface end-to-end.
