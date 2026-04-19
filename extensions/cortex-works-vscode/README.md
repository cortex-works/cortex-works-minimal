# Cortex Works VS Code Extension

This extension exposes the same 13 Cortex tools that the MCP server exposes, but through an extension-native bridge instead of a user-configured MCP server entry.

## What Changes

- No extension-only tool names are added.
- The tool surface stays identical to `cortex-mcp`.
- VS Code talks to `cortex-extension-bridge`, which reuses the existing Cortex dispatch directly.
- `cortex-mcp` is still bundled for parity testing and regression comparison.

## Validation

Run the built-in parity test command after installing or updating the extension:

```text
Cortex Works: Run Parity Self-Test
```

It writes a report to `target/cortex-works-vscode/extension-self-test.json` in the workspace root.

## Development

```bash
cd extensions/cortex-works-vscode
npm install
npm run compile
```

Stage local Rust binaries into the extension before packaging:

```bash
npm run stage:sidecars
```

Stage every supported platform that has already been built:

```bash
npm run stage:sidecars:all
```

Package the extension:

```bash
npm run package:vsix
```

## Expected Sidecars

The staging script copies these binaries into `resources/sidecars/<platform>/`:

- `cortex-extension-bridge`
- `cortex-mcp`

Windows builds use `.exe` suffixes automatically.