# Developer Guide

Minimal-branch developer workflow for building, validating, and releasing
`cortex-mcp`.

Recent workspace-aware AST behavior to preserve:

- `cortex_code_explorer(action=workspace_topology)` is the lowest-token orientation step.
- `map_overview` and `skeleton` prefer `target_dirs=[...]` in multi-root sessions.
- `deep_slice` can scope semantic ranking with `only_dirs=[...]`.
- Cross-root file references use the `[FolderName]/path/to/file` convention.
- Both AST and ACT tools must resolve workspace-prefixed paths consistently.
- ACT tools that accept `file`, `project_path`, `paths`, or `cwd` should continue to work with either workspace-prefixed paths or absolute paths.

## Build Targets

Fast local compile:

```bash
cargo check --workspace
cargo build --profile release-fast -p cortex-mcp
```

Production build:

```bash
cargo build --release -p cortex-mcp
```

## Validation Commands

Canonical production validation:

```bash
./scripts/test_cortexworks_all.sh
```

Release smoke only:

```bash
./scripts/test_cortexworks_release.sh
```

VS Code MCP config verification only:

```bash
./scripts/test_cortexworks_all.sh --config-only
```

## What The Validation Covers

`scripts/test_cortexworks_release.sh` builds the release binary and runs
`crates/cortex-mcp/tests/full_stack_smoke.rs` against
`target/release/cortex-mcp`.

The smoke harness validates:

- the active 14-tool MCP surface from `tools/list`
- AST tool calls through the real MCP transport
- multi-root `initialize.workspaceFolders` handling plus `workspace_topology`
- array-based `target_dirs` / `only_dirs` flows for workspace-aware AST calls
- ACT tool calls through the real MCP transport, including workspace-prefixed path routing
- semantic search with seeded local index data
- filesystem patch semantics including `patch_action`
- supervisor-based `cortex_mcp_hot_reload`

`scripts/verify_cortexworks_mcp_setup.py` validates that the configured VS Code
`cortex-works` server entry:

- exists in the selected `mcp.json`
- uses `type = stdio`
- points to an executable command
- responds correctly to `initialize`
- exposes exactly the expected 14 active tools

## VS Code Config Path

Default macOS VS Code user MCP config:

```text
$HOME/Library/Application Support/Code/User/mcp.json
```

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
