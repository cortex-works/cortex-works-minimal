# CortexDB

Local storage helpers for the minimal `cortex-works` branch.

`cortex-db` remains in the workspace for one reason: it backs local semantic code search.

## Responsibilities

- Open and manage local SQLite/LanceDB state.
- Store and query semantic code vectors.
- Provide shared helpers used by `cortex-act`.

This crate is no longer described as part of a larger sync/proxy/UI stack in the minimal branch.
