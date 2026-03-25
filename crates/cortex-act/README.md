# cortex-act

Write-side tool runtime for Cortex-Works minimal.

## Scope

`cortex-act` powers the mutation and execution tools exposed by `cortex-mcp`:

- `cortex_act_edit_ast`
- `cortex_act_edit_data_graph`
- `cortex_act_edit_markup`
- `cortex_act_sql_surgery`
- `cortex_act_shell_exec`
- `cortex_act_batch_execute`
- `cortex_search_exact`
- `cortex_fs_manage`
- `cortex_mcp_hot_reload`

## Design Notes

- Symbol/structure-aware edits are preferred over line-number patches.
- `cortex_act_shell_exec` is bounded and synchronous, with optional diagnostics mode.
- Batch execution is sequential and forbids nested `cortex_act_batch_execute` calls.
- Filesystem operations support workspace-prefixed paths (`[FolderName]/...`) via the MCP gateway.

## Repository Layout

```text
crates/cortex-act/src/act/
	dispatch.rs       # tool routing
	editor.rs         # AST edits for Rust/TS/Python
	data_editor.rs    # JSON/YAML structural edits
	markup_editor.rs  # Markdown/HTML/XML structural edits
	sql_editor.rs     # DDL statement surgery
	shell_exec.rs     # bounded shell execution + diagnostics
	batch_executor.rs # sequential batch orchestration
	fs_manage.rs      # write/patch/mkdir/delete/rename/move/copy
	search_exact.rs   # deterministic regex search
	hot_reload.rs     # supervisor restart signal payload
```
