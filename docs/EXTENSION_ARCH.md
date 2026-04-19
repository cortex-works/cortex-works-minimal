# VS Code Extension Wrapper Architecture

This repository now supports two delivery modes over the same 13-tool Cortex surface:

- `cortex-mcp` remains the portable MCP server for editors and hosts that speak MCP over stdio.
- `extensions/cortex-works-vscode` exposes those exact same tools through an extension-native bridge for VS Code.

## Design Goal

The extension must **not** invent a second tool surface. The tool names, intent, and arguments should stay aligned with `cortex-mcp`. The only thing that changes is the transport and dispatch path inside VS Code.

## Runtime Model

### 1. Shared Tool Surface

The extension contributes the same 13 tool names already used by `cortex-mcp`:

- `cortex_code_explorer`
- `cortex_symbol_analyzer`
- `cortex_chronos`
- `cortex_manage_ast_languages`
- `cortex_act_edit_ast`
- `cortex_act_edit_data_graph`
- `cortex_act_edit_markup`
- `cortex_act_sql_surgery`
- `cortex_act_shell_exec`
- `cortex_act_batch_execute`
- `cortex_search_exact`
- `cortex_mcp_hot_reload`
- `cortex_fs_manage`

### 2. Native Bridge

The extension runs `cortex-extension-bridge`, a lightweight stdio bridge binary that reuses the existing `cortex-mcp` dispatch code directly.

Key point: the extension does **not** call a separate set of extension-only tools. It calls the same Cortex implementation behind a thinner transport.

### 3. MCP Baseline For Regression Testing

The extension also bundles `cortex-mcp` for parity testing. A built-in self-test command compares native-bridge results against the MCP baseline and writes a report to:

```text
target/cortex-works-vscode/extension-self-test.json
```

This makes it possible to catch regressions where the extension path diverges from the canonical MCP path.

## Why This Is Better Than The Previous Wrapper

- No duplicate tool surface in VS Code.
- No extension-only tool names that confuse discovery.
- Lower overhead than routing every extension call back through MCP.
- The same Rust tool implementations remain the source of truth.

## Packaging Strategy

The extension expects sidecars under:

```text
resources/sidecars/<platform>/
```

Each platform folder contains:

- `cortex-extension-bridge`
- `cortex-mcp`

Supported platform keys:

- `darwin-arm64`
- `darwin-x64`
- `linux-arm64`
- `linux-x64`
- `win32-x64`

`npm run stage:sidecars` stages the current host platform. `npm run stage:sidecars:all` stages every supported platform that has already been built locally.

## Result

This architecture keeps the contract stable and moves performance work into the transport layer instead of creating a second behavior layer:

- MCP stays available where MCP is the right integration point.
- The VS Code extension becomes a direct alternative to MCP, not an extra tool family.
- Parity can be tested continuously against the baseline implementation.