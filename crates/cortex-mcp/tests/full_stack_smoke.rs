use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

const PROTOCOL_VERSION: &str = "2024-11-05";

struct ToolReply {
    is_error: bool,
    text: String,
}

struct RpcClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    workspace_root: PathBuf,
}

impl RpcClient {
    fn spawn(bin: &Path, home: &Path, workspace_root: &Path, worker_mode: bool) -> Self {
        let mut cmd = Command::new(bin);
        cmd.current_dir(workspace_root)
            .env("HOME", home)
            .env("NO_COLOR", "1")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        if worker_mode {
            cmd.env("CORTEX_WORKER_MODE", "1");
        }

        let mut child = cmd.spawn().expect("spawn cortex-mcp");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));

        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            workspace_root: workspace_root.to_path_buf(),
        };
        client.initialize();
        client
    }

    fn initialize(&mut self) {
        let id = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "rootUri": path_to_uri(&self.workspace_root),
                "workspaceFolders": [{
                    "uri": path_to_uri(&self.workspace_root),
                    "name": "fixture"
                }]
            }),
        );
        let response = self.read_response(id);
        let server_name = response["result"]["serverInfo"]["name"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(server_name, "cortex-mcp", "initialize must return cortex-mcp");
        self.notify("notifications/initialized", json!({}));
    }

    fn tools_list(&mut self) -> Vec<String> {
        let id = self.request("tools/list", json!({}));
        let response = self.read_response(id);
        response["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(|s| s.to_string()))
            .collect()
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> ToolReply {
        let id = self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        );
        let response = self.read_response(id);
        let result = &response["result"];
        let text = result["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|entry| entry["text"].as_str())
            .unwrap_or_default()
            .to_string();
        ToolReply {
            is_error: result["isError"].as_bool().unwrap_or(false),
            text,
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&payload).expect("serialize notification");
        self.stdin.write_all(line.as_bytes()).expect("write notification");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush notification");
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&payload).expect("serialize request");
        self.stdin.write_all(line.as_bytes()).expect("write request");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush request");
        id
    }

    fn read_response(&mut self, expected_id: u64) -> Value {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read response line");
            assert!(read > 0, "unexpected EOF waiting for response id {expected_id}");

            if line.trim().is_empty() {
                continue;
            }

            let parsed: Value = serde_json::from_str(line.trim()).expect("parse response JSON");
            match parsed.get("id").and_then(|v| v.as_u64()) {
                Some(id) if id == expected_id => return parsed,
                _ => continue,
            }
        }
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn full_tool_smoke_and_hot_reload() {
    let sandbox = TempDir::new().expect("sandbox tempdir");
    let home = sandbox.path().join("home");
    let workspace = sandbox.path().join("fixture-workspace");
    fs::create_dir_all(&home).expect("create home");
    create_fixture_workspace(&workspace);

    let bin = cortex_mcp_bin();
    let workspace_prefix = format!(
        "[{}]",
        workspace.file_name().and_then(|s| s.to_str()).expect("workspace basename")
    );
    let prefixed = |suffix: &str| {
        if suffix.is_empty() {
            workspace_prefix.clone()
        } else {
            format!("{workspace_prefix}/{suffix}")
        }
    };

    let mut client = RpcClient::spawn(&bin, &home, &workspace, true);
    let tool_names = client.tools_list();
    let tool_set: HashSet<String> = tool_names.iter().cloned().collect();
    let expected: HashSet<String> = [
        "cortex_code_explorer",
        "cortex_symbol_analyzer",
        "cortex_chronos",
        "cortex_manage_ast_languages",
        "cortex_act_edit_ast",
        "cortex_act_edit_data_graph",
        "cortex_act_edit_markup",
        "cortex_act_sql_surgery",
        "cortex_act_shell_exec",
        "cortex_act_batch_execute",
        "cortex_search_exact",
        "cortex_fs_manage",
        "cortex_mcp_hot_reload",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(tool_set, expected, "tools/list must expose exactly the 13 active tools");

    let languages = client.call_tool("cortex_manage_ast_languages", json!({ "action": "status" }));
    assert_ok(&languages, "rust");
    assert!(languages.text.contains("typescript"));
    assert!(languages.text.contains("python"));

    let add_language = client.call_tool(
        "cortex_manage_ast_languages",
        json!({ "action": "add", "languages": ["go"] }),
    );
    assert_err_contains(&add_language, "not supported in this build");

    let repo_map = client.call_tool(
        "cortex_code_explorer",
        json!({
            "action": "map_overview",
            "repoPath": workspace,
            "target_dirs": ["."]
        }),
    );
    assert_ok(&repo_map, "src/");
    assert!(repo_map.text.contains("lib.rs"));

    let project_skeleton = client.call_tool(
        "cortex_code_explorer",
        json!({
            "action": "skeleton",
            "repoPath": workspace,
            "target_dirs": ["src"]
        }),
    );
    assert_ok(&project_skeleton, "src/lib.rs");
    assert!(project_skeleton.text.contains("Greeter"));

    let deep_slice = client.call_tool(
        "cortex_code_explorer",
        json!({
            "action": "deep_slice",
            "repoPath": workspace,
            "target": "src/lib.rs",
            "single_file": true,
            "skeleton_only": false
        }),
    );
    assert_ok(&deep_slice, "greet");

    let lib_path = workspace.join("src/lib.rs");

    let read_source = client.call_tool(
        "cortex_symbol_analyzer",
        json!({
            "action": "read_source",
            "repoPath": workspace,
            "path": lib_path,
            "symbol_name": "greet"
        }),
    );
    assert_ok(&read_source, "pub fn greet");

    let find_usages = client.call_tool(
        "cortex_symbol_analyzer",
        json!({
            "action": "find_usages",
            "repoPath": workspace,
            "symbol_name": "greet",
            "target_dir": "."
        }),
    );
    assert_ok(&find_usages, "wrapper");

    let find_implementations = client.call_tool(
        "cortex_symbol_analyzer",
        json!({
            "action": "find_implementations",
            "repoPath": workspace,
            "symbol_name": "Greeter",
            "target_dir": "."
        }),
    );
    assert_ok(&find_implementations, "DefaultGreeter");

    let blast_radius = client.call_tool(
        "cortex_symbol_analyzer",
        json!({
            "action": "blast_radius",
            "repoPath": workspace,
            "symbol_name": "greet",
            "target_dir": "."
        }),
    );
    assert_ok(&blast_radius, "greet");

    let propagation = client.call_tool(
        "cortex_symbol_analyzer",
        json!({
            "action": "propagation_checklist",
            "repoPath": workspace,
            "symbol_name": "Greeter",
            "target_dir": "."
        }),
    );
    assert_ok(&propagation, "Propagation Checklist");
    assert!(propagation.text.contains("src/lib.rs"));

    let checkpoint = client.call_tool(
        "cortex_chronos",
        json!({
            "action": "save_checkpoint",
            "repoPath": workspace,
            "path": lib_path,
            "symbol_name": "greet",
            "semantic_tag": "pre-edit"
        }),
    );
    assert_ok(&checkpoint, "pre-edit");

    let checkpoint_list = client.call_tool(
        "cortex_chronos",
        json!({
            "action": "list_checkpoints",
            "repoPath": workspace
        }),
    );
    assert_ok(&checkpoint_list, "pre-edit");

    let edit_ast = client.call_tool(
        "cortex_act_edit_ast",
        json!({
            "file": prefixed("src/lib.rs"),
            "edits": [{
                "target": "function:greet",
                "action": "replace",
                "code": "pub fn greet() -> &'static str {\n    \"updated-by-edit-ast\"\n}\n"
            }]
        }),
    );
    assert_ok(&edit_ast, "Applied 1 edit");
    assert!(fs::read_to_string(&lib_path).expect("read lib after edit").contains("updated-by-edit-ast"));

    let compare = client.call_tool(
        "cortex_chronos",
        json!({
            "action": "compare_checkpoint",
            "repoPath": workspace,
            "path": lib_path,
            "symbol_name": "greet",
            "tag_a": "pre-edit",
            "tag_b": "__live__"
        }),
    );
    assert!(!compare.is_error, "compare_checkpoint must succeed: {}", compare.text);

    let exact_search = client.call_tool(
        "cortex_search_exact",
        json!({
            "project_path": prefixed(""),
            "regex_pattern": "updated-by-edit-ast",
            "include_pattern": "src/**"
        }),
    );
    assert_ok(&exact_search, "updated-by-edit-ast");

    let data_edit = client.call_tool(
        "cortex_act_edit_data_graph",
        json!({
            "file": prefixed("config/sample.json"),
            "edits": [{
                "target": "$.name",
                "action": "replace",
                "value": "patched-name"
            }]
        }),
    );
    assert_ok(&data_edit, "Successfully patched");
    assert!(fs::read_to_string(workspace.join("config/sample.json")).expect("read json").contains("patched-name"));

    let markup_edit = client.call_tool(
        "cortex_act_edit_markup",
        json!({
            "file": prefixed("docs/sample.md"),
            "edits": [{
                "target": "heading:Intro",
                "action": "insert_after",
                "code": "\nInserted from markup tool.\n"
            }]
        }),
    );
    assert_ok(&markup_edit, "Successfully patched");
    assert!(fs::read_to_string(workspace.join("docs/sample.md")).expect("read markdown").contains("Inserted from markup tool."));

    let sql_edit = client.call_tool(
        "cortex_act_sql_surgery",
        json!({
            "file": prefixed("db/schema.sql"),
            "edits": [{
                "target": "create_table:users",
                "action": "replace",
                "code": "CREATE TABLE users (id INT, name TEXT);"
            }]
        }),
    );
    assert_ok(&sql_edit, "Successfully patched");
    assert!(fs::read_to_string(workspace.join("db/schema.sql")).expect("read sql").contains("name TEXT"));

    let mkdir = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "mkdir",
            "paths": [prefixed("generated/nested")]
        }),
    );
    assert_ok(&mkdir, "Successfully created");

    let write = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "write",
            "paths": [prefixed("generated/nested/note.txt")],
            "content": "hello-from-fs-manage"
        }),
    );
    assert_ok(&write, "Written");

    let patch = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "patch",
            "paths": [prefixed(".env")],
            "type": "env",
            "target": "API_KEY",
            "value": "patched-secret"
        }),
    );
    assert_ok(&patch, "API_KEY");
    assert!(fs::read_to_string(workspace.join(".env")).expect("read env").contains("API_KEY=patched-secret"));

    let patch_delete = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "patch",
            "paths": [prefixed(".env")],
            "type": "env",
            "patch_action": "delete",
            "target": "API_KEY"
        }),
    );
    assert_ok(&patch_delete, "API_KEY");
    assert!(!fs::read_to_string(workspace.join(".env")).expect("read env after delete").contains("API_KEY="));

    let copy = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "copy",
            "paths": [
                prefixed("generated/nested/note.txt"),
                prefixed("generated/nested/note-copy.txt")
            ]
        }),
    );
    assert_ok(&copy, "Copied");

    let move_file = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "move",
            "paths": [
                prefixed("generated/nested/note-copy.txt"),
                prefixed("generated/nested/note-moved.txt")
            ]
        }),
    );
    assert_ok(&move_file, "Renamed:");

    let delete = client.call_tool(
        "cortex_fs_manage",
        json!({
            "action": "delete",
            "paths": [prefixed("generated/nested/note-moved.txt")]
        }),
    );
    assert_ok(&delete, "Successfully deleted");
    assert!(!workspace.join("generated/nested/note-moved.txt").exists());

    let shell = client.call_tool(
        "cortex_act_shell_exec",
        json!({
            "command": "printf hello-shell",
            "cwd": prefixed("")
        }),
    );
    assert_ok(&shell, "hello-shell");

    let diagnostics = client.call_tool(
        "cortex_act_shell_exec",
        json!({
            "run_diagnostics": true,
            "cwd": prefixed("")
        }),
    );
    assert!(!diagnostics.is_error, "run_diagnostics must succeed: {}", diagnostics.text);
    assert!(!diagnostics.text.contains("error:"), "diagnostics should not report compiler errors: {}", diagnostics.text);

    let batch_contract = client.call_tool(
        "cortex_act_batch_execute",
        json!({
            "fail_fast": true,
            "max_chars_per_op": 120,
            "operations": [
                {
                    "tool_name": "cortex_search_exact",
                    "parameters": {
                        "project_path": prefixed(""),
                        "regex_pattern": "pub fn",
                        "include_pattern": "src/**"
                    }
                },
                {
                    "tool_name": "cortex_act_batch_execute"
                },
                {
                    "tool_name": "cortex_symbol_analyzer",
                    "parameters": {
                        "action": "read_source",
                        "repoPath": workspace,
                        "path": lib_path,
                        "symbol_name": "wrapper"
                    }
                }
            ]
        }),
    );
    let batch_contract_json = parse_batch_summary(&batch_contract);
    assert_eq!(batch_contract_json["total"].as_u64(), Some(3));
    assert_eq!(batch_contract_json["passed"].as_u64(), Some(1));
    assert_eq!(batch_contract_json["failed"].as_u64(), Some(1));
    assert_eq!(batch_contract_json["skipped"].as_u64(), Some(1));
    let contract_results = batch_contract_json["results"].as_array().expect("batch results array");
    assert_eq!(contract_results.len(), 2, "fail_fast should stop before the third operation runs");
    assert!(contract_results[0]["truncated"].as_bool().unwrap_or(false), "first operation should be truncated: {batch_contract_json}");
    assert_eq!(
        contract_results[1]["output"].as_str(),
        Some("Nested cortex_act_batch_execute is not allowed"),
        "nested batch calls must be rejected with a clear error"
    );

    let full_batch = client.call_tool(
        "cortex_act_batch_execute",
        json!({
            "fail_fast": true,
            "max_chars_per_op": 8000,
            "operations": [
                {
                    "tool_name": "cortex_manage_ast_languages",
                    "parameters": {
                        "action": "status"
                    }
                },
                {
                    "tool_name": "cortex_code_explorer",
                    "parameters": {
                        "action": "workspace_topology",
                        "repoPath": workspace
                    }
                },
                {
                    "tool_name": "cortex_symbol_analyzer",
                    "parameters": {
                        "action": "read_source",
                        "repoPath": workspace,
                        "path": lib_path,
                        "symbol_name": "greet"
                    }
                },
                {
                    "tool_name": "cortex_chronos",
                    "parameters": {
                        "action": "list_checkpoints",
                        "repoPath": workspace
                    }
                },
                {
                    "tool_name": "cortex_act_edit_ast",
                    "parameters": {
                        "file": prefixed("src/lib.rs"),
                        "edits": [{
                            "target": "function:wrapper",
                            "action": "replace",
                            "code": "pub fn wrapper() -> &'static str {\n    \"updated-by-batch\"\n}\n"
                        }]
                    }
                },
                {
                    "tool_name": "cortex_act_edit_data_graph",
                    "parameters": {
                        "file": prefixed("config/sample.json"),
                        "edits": [{
                            "target": "$.flag",
                            "action": "replace",
                            "value": "false"
                        }]
                    }
                },
                {
                    "tool_name": "cortex_act_edit_markup",
                    "parameters": {
                        "file": prefixed("docs/sample.md"),
                        "edits": [{
                            "target": "heading:Intro",
                            "action": "insert_after",
                            "code": "\nBatch follow-up paragraph.\n"
                        }]
                    }
                },
                {
                    "tool_name": "cortex_act_sql_surgery",
                    "parameters": {
                        "file": prefixed("db/schema.sql"),
                        "edits": [{
                            "target": "create_table:users",
                            "action": "replace",
                            "code": "CREATE TABLE users (id INT, name TEXT, email TEXT);"
                        }]
                    }
                },
                {
                    "tool_name": "cortex_fs_manage",
                    "parameters": {
                        "action": "write",
                        "paths": [prefixed("generated/batch-note.txt")],
                        "content": "written-from-batch"
                    }
                },
                {
                    "tool_name": "cortex_act_shell_exec",
                    "parameters": {
                        "command": "printf batch-shell",
                        "cwd": prefixed("")
                    }
                },
                {
                    "tool_name": "cortex_search_exact",
                    "parameters": {
                        "project_path": prefixed(""),
                        "regex_pattern": "updated-by-edit-ast",
                        "include_pattern": "src/**"
                    }
                }
            ]
        }),
    );
    let full_batch_json = parse_batch_summary(&full_batch);
    assert_eq!(full_batch_json["total"].as_u64(), Some(11));
    assert_eq!(full_batch_json["passed"].as_u64(), Some(11));
    assert_eq!(full_batch_json["failed"].as_u64(), Some(0));
    assert_eq!(full_batch_json["skipped"].as_u64(), Some(0));
    let full_batch_results = full_batch_json["results"].as_array().expect("full batch results array");
    assert_eq!(full_batch_results.len(), 11);
    assert!(full_batch_results.iter().all(|entry| entry["success"].as_bool().unwrap_or(false)), "all batched operations must succeed: {full_batch_json}");
    let tool_names_in_batch: HashSet<String> = full_batch_results
        .iter()
        .filter_map(|entry| entry["tool_name"].as_str().map(str::to_string))
        .collect();
    let expected_batched_tools: HashSet<String> = [
        "cortex_manage_ast_languages",
        "cortex_code_explorer",
        "cortex_symbol_analyzer",
        "cortex_chronos",
        "cortex_act_edit_ast",
        "cortex_act_edit_data_graph",
        "cortex_act_edit_markup",
        "cortex_act_sql_surgery",
        "cortex_fs_manage",
        "cortex_act_shell_exec",
        "cortex_search_exact",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(tool_names_in_batch, expected_batched_tools, "batch should cover every non-terminal tool once");
    assert!(fs::read_to_string(&lib_path).expect("read lib after batched edit").contains("updated-by-batch"));
    assert!(fs::read_to_string(workspace.join("config/sample.json")).expect("read json after batched edit").contains("false"));
    assert!(fs::read_to_string(workspace.join("docs/sample.md")).expect("read markdown after batched edit").contains("Batch follow-up paragraph."));
    assert!(fs::read_to_string(workspace.join("db/schema.sql")).expect("read sql after batched edit").contains("email TEXT"));
    assert_eq!(fs::read_to_string(workspace.join("generated/batch-note.txt")).expect("read batched fs output"), "written-from-batch");

    drop(client);

    let mut supervisor = RpcClient::spawn(&bin, &home, &workspace, false);
    let hot_reload = supervisor.call_tool(
        "cortex_mcp_hot_reload",
        json!({ "reason": "integration-smoke" }),
    );
    assert_ok(&hot_reload, "restart with the new binary");
    thread::sleep(Duration::from_millis(1200));
    drop(supervisor);

    let mut supervisor = RpcClient::spawn(&bin, &home, &workspace, false);
    let reloaded_tools = supervisor.tools_list();
    let reloaded_set: HashSet<String> = reloaded_tools.into_iter().collect();
    assert_eq!(reloaded_set, expected, "hot reload must leave the rebuilt MCP worker usable with the same 13-tool surface");

    let hot_reload_batch = supervisor.call_tool(
        "cortex_act_batch_execute",
        json!({
            "fail_fast": true,
            "operations": [
                {
                    "tool_name": "cortex_search_exact",
                    "parameters": {
                        "project_path": prefixed(""),
                        "regex_pattern": "updated-by-batch",
                        "include_pattern": "src/**"
                    }
                },
                {
                    "tool_name": "cortex_mcp_hot_reload",
                    "parameters": { "reason": "integration-smoke-batch" }
                }
            ]
        }),
    );
    let hot_reload_batch_json = parse_batch_summary(&hot_reload_batch);
    assert_eq!(hot_reload_batch_json["total"].as_u64(), Some(2));
    assert_eq!(hot_reload_batch_json["passed"].as_u64(), Some(2));
    assert_eq!(hot_reload_batch_json["failed"].as_u64(), Some(0));
    let hot_reload_results = hot_reload_batch_json["results"].as_array().expect("hot reload batch results");
    assert_eq!(hot_reload_results.len(), 2);
    assert!(
        hot_reload_results[1]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("restart with the new binary"),
        "batched hot reload should report a restart message: {hot_reload_batch_json}"
    );
}

fn create_fixture_workspace(workspace: &Path) {
    fs::create_dir_all(workspace.join("src")).expect("create src dir");
    fs::create_dir_all(workspace.join("config")).expect("create config dir");
    fs::create_dir_all(workspace.join("docs")).expect("create docs dir");
    fs::create_dir_all(workspace.join("db")).expect("create db dir");

    fs::write(
        workspace.join("Cargo.toml"),
        r#"[package]
name = "fixture-workspace"
version = "0.2.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");

    fs::write(
        workspace.join("src/lib.rs"),
        r#"pub trait Greeter {
    fn render(&self) -> &'static str;
}

pub struct DefaultGreeter;

impl Greeter for DefaultGreeter {
    fn render(&self) -> &'static str {
        greet()
    }
}

pub fn greet() -> &'static str {
    "hello"
}

pub fn wrapper() -> &'static str {
    greet()
}
"#,
    )
    .expect("write src/lib.rs");

    fs::write(
        workspace.join("config/sample.json"),
        "{\n  \"name\": \"old-name\",\n  \"flag\": true\n}\n",
    )
    .expect("write sample.json");

    fs::write(
        workspace.join("docs/sample.md"),
        "# Intro\n\nFixture body.\n",
    )
    .expect("write sample.md");

    fs::write(
        workspace.join("db/schema.sql"),
        "CREATE TABLE users (id INT);\n",
    )
    .expect("write schema.sql");

    fs::write(workspace.join(".env"), "API_KEY=initial\n").expect("write .env");
}

fn cortex_mcp_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("CORTEX_MCP_BIN") {
        return PathBuf::from(path);
    }
    PathBuf::from(std::env::var_os("CARGO_BIN_EXE_cortex-mcp").expect("CARGO_BIN_EXE_cortex-mcp"))
}

fn path_to_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace(' ', "%20");
    format!("file://{path}")
}

fn assert_ok(reply: &ToolReply, needle: &str) {
    assert!(!reply.is_error, "tool returned error: {}", reply.text);
    assert!(reply.text.contains(needle), "expected `{needle}` in tool output, got: {}", reply.text);
}

fn assert_err_contains(reply: &ToolReply, needle: &str) {
    assert!(reply.is_error, "expected tool error containing `{needle}`, got success: {}", reply.text);
    assert!(reply.text.contains(needle), "expected `{needle}` in tool error, got: {}", reply.text);
}

fn parse_batch_summary(reply: &ToolReply) -> Value {
    assert!(!reply.is_error, "batch tool returned error: {}", reply.text);
    serde_json::from_str(&reply.text).expect("parse batch summary JSON")
}