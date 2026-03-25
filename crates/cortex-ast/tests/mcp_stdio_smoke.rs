use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

// DEFAULT_MAX_CHARS in server.rs — keep in sync.
const EXPECTED_DEFAULT_MAX_CHARS: usize = 8_000;

#[test]
fn mcp_stdio_smoke() {
    // `cargo test` sets this for integration tests.
    let bin = env!("CARGO_BIN_EXE_cortexast");
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let db_root = repo_root
        .parent()
        .expect("cortex-ast has parent dir")
        .join("cortex-db");
    assert!(db_root.exists(), "expected sibling cortex-db crate for multi-root smoke test");

    let mut child = Command::new(bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cortexast mcp");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");

        // Keep each JSON-RPC message on one line (server reads by lines()).
        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "workspaceFolders": [
                        {
                            "uri": format!("file://{}", repo_root.display()),
                            "name": "cortex-ast"
                        },
                        {
                            "uri": format!("file://{}", db_root.display()),
                            "name": "cortex-db"
                        }
                    ]
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "cortex_code_explorer",
                    "arguments": { "action": "workspace_topology" }
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "cortex_code_explorer",
                    "arguments": {
                        "action": "map_overview",
                        "target_dirs": ["[cortex-ast]", "[cortex-db]"],
                        "max_chars": 2000
                    }
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "cortex_code_explorer",
                    "arguments": {
                        "action": "deep_slice",
                        "target": "[cortex-db]/src/lib.rs",
                        "single_file": true,
                        "max_chars": 3000
                    }
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "cortex_symbol_analyzer",
                    "arguments": {
                        "action": "read_source",
                        "path": "[cortex-db]/src/lib.rs",
                        "symbol_name": "LanceDb"
                    }
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "cortex_symbol_analyzer",
                    "arguments": {
                        "action": "find_implementations",
                        "symbol_name": "LanguageDriver",
                        "target_dir": "[cortex-ast]"
                    }
                }
            })
        )
        .unwrap();

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "cortex_chronos",
                    "arguments": { "repoPath": repo_root, "action": "delete_checkpoint", "symbol_name": "__smoke_test_nonexistent__", "semantic_tag": "__smoke_test_nonexistent__" }
                }
            })
        )
        .unwrap();
    }

    // Close stdin so the server loop can exit.
    drop(child.stdin.take());

    let stdout = child.stdout.take().expect("child stdout");
    let reader = BufReader::new(stdout);

    let mut replies_by_id: HashMap<i64, serde_json::Value> = HashMap::new();

    for line in reader.lines() {
        let line = line.expect("read stdout line");
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).expect("stdout is json");
        let id = v
            .get("id")
            .and_then(|x| x.as_i64())
            .expect("json-rpc response id");
        replies_by_id.insert(id, v);
        if replies_by_id.len() >= 8 {
            break;
        }
    }

    let status = child.wait().expect("wait child");
    assert!(status.success(), "mcp process should exit cleanly");

    // initialize
    {
        let v = replies_by_id.get(&1).expect("initialize reply");
        assert_eq!(v.get("jsonrpc").and_then(|x| x.as_str()), Some("2.0"));
        let result = v.get("result").expect("initialize result");
        assert!(result.get("capabilities").is_some());
    }

    // tools/list
    {
        let v = replies_by_id.get(&2).expect("tools/list reply");
        let tools = v
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .expect("tools array");
        let names: std::collections::HashSet<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        for required in [
            "cortex_code_explorer",
            "cortex_symbol_analyzer",
            "cortex_chronos",
            "cortex_manage_ast_languages",
        ] {
            assert!(names.contains(required), "missing tool: {required}");
        }
    }

    // workspace_topology
    {
        let v = replies_by_id.get(&3).expect("workspace_topology reply");
        let result = v.get("result").expect("tools/call result");
        assert_eq!(
            result.get("isError").and_then(|x| x.as_bool()),
            Some(false),
            "workspace_topology should not error"
        );
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .expect("workspace_topology text");
        assert!(
            text.contains("# WORKSPACE_TOPOLOGY"),
            "workspace_topology should return a topology header"
        );
        assert!(
            text.contains("[cortex-ast]") && text.contains("[cortex-db]"),
            "workspace_topology should list both initialized workspace roots"
        );
    }

    // map_overview with target_dirs
    {
        let v = replies_by_id.get(&4).expect("map_overview reply");
        let result = v.get("result").expect("tools/call result");
        assert_eq!(result.get("isError").and_then(|x| x.as_bool()), Some(false));
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .expect("map_overview text");
        assert!(
            text.contains("# MULTI_ROOT_MAP") && text.contains("## cortex-ast"),
            "map_overview should return the multi-root aggregate map"
        );
    }

    // deep_slice with single_file
    {
        let v = replies_by_id.get(&5).expect("deep_slice reply");
        let result = v.get("result").expect("tools/call result");
        assert_eq!(result.get("isError").and_then(|x| x.as_bool()), Some(false));
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .expect("deep_slice text");
        assert!(
            text.contains("src/lib.rs") && text.contains("LanceDb"),
            "deep_slice should stay scoped to cortex-db and include the requested symbol context"
        );
    }

    // read_source with prefixed path
    {
        let v = replies_by_id.get(&6).expect("read_source reply");
        let result = v.get("result").expect("tools/call result");
        assert_eq!(result.get("isError").and_then(|x| x.as_bool()), Some(false));
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .expect("read_source text");
        assert!(
            text.contains("LanceDb"),
            "read_source should resolve prefixed paths across workspace roots"
        );
    }

    // find_implementations — must not return an action-enum error
    {
        let v = replies_by_id.get(&7).expect("find_implementations reply");
        let result = v.get("result").expect("tools/call result");
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        assert!(
            !text.contains("must be equal to one of the allowed values"),
            "find_implementations was rejected by schema enum validation — action not registered: {text}"
        );
    }

    // delete_checkpoint — sentinel symbol/tag guaranteed to not exist, so it returns a
    // "No checkpoints matched" message (isError=false) rather than an enum error.
    {
        let v = replies_by_id.get(&8).expect("delete_checkpoint reply");
        let result = v.get("result").expect("tools/call result");
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("text"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        assert!(
            !text.contains("must be equal to one of the allowed values"),
            "delete_checkpoint was rejected by schema enum validation — action not registered: {text}"
        );
    }
}
/// Verifies that the default output truncation is ≤ EXPECTED_DEFAULT_MAX_CHARS +
/// a small overhead for the truncation suffix message (~200 chars).
/// This guards against the spill-to-disk regression where DEFAULT_MAX_CHARS was
/// 15 000 — larger than VS Code Copilot's inline-display threshold (~8 KB).
#[test]
fn default_truncation_caps_output() {
    let bin = env!("CARGO_BIN_EXE_cortexast");
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut child = Command::new(bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cortexast mcp");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");

        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2024-11-05" }
            })
        )
        .unwrap();

        // deep_slice on the whole repo without specifying max_chars —
        // should be capped at EXPECTED_DEFAULT_MAX_CHARS by default.
        writeln!(
            stdin,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "cortex_code_explorer",
                    "arguments": {
                        "repoPath": repo_root,
                        "action":   "deep_slice",
                        "target":   "src"
                    }
                }
            })
        )
        .unwrap();
    }

    drop(child.stdin.take());

    let stdout = child.stdout.take().expect("child stdout");
    let reader = BufReader::new(stdout);
    let mut text_output: Option<String> = None;

    for line in reader.lines() {
        let line = line.expect("read stdout line");
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).expect("stdout is json");
        if v.get("id").and_then(|x| x.as_i64()) == Some(2) {
            text_output = v
                .get("result")
                .and_then(|r| r.get("content"))
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("text"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            break;
        }
    }

    let _ = child.wait();

    let text = text_output.expect("deep_slice should return text content");
    // Allow some overhead for the truncation suffix message.
    let truncation_overhead = 250;
    assert!(
        text.len() <= EXPECTED_DEFAULT_MAX_CHARS + truncation_overhead,
        "default deep_slice output ({} chars) exceeds the safe inline threshold ({} chars). \
         This causes IDE spill-to-disk. Lower DEFAULT_MAX_CHARS in server.rs.",
        text.len(),
        EXPECTED_DEFAULT_MAX_CHARS + truncation_overhead
    );
    // Also confirm the truncation marker is present when content overflows.
    if text.len() > EXPECTED_DEFAULT_MAX_CHARS {
        assert!(
            text.contains("✂️") || text.contains("OUTPUT TRUNCATED"),
            "truncated output should contain the ✂️ marker"
        );
    }
}
