# Bundled Native Sidecars

The extension ships the same 13-tool surface through a dedicated native bridge.

`npm run stage:sidecars` stages the host platform. `npm run stage:sidecars:all` stages every supported platform that has already been built under `target/<triple>/release/` or provided through `CORTEX_STAGE_SOURCE_<PLATFORM>` overrides.

Expected layout:

```text
resources/sidecars/<platform>/cortex-extension-bridge
resources/sidecars/<platform>/cortex-mcp
```

Supported `<platform>` keys:

- `darwin-arm64`
- `darwin-x64`
- `linux-arm64`
- `linux-x64`
- `win32-x64`