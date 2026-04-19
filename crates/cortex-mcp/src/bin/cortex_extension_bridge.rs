use anyhow::Result;
use cortex_mcp::{MCP_PROTOCOL_VERSION, tools};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default = "null_id")]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    id: Value,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn null_id() -> Value {
    Value::Null
}

fn response(id: Value, ok: bool, result: Option<Value>, error: Option<String>) -> Response {
    Response {
        id,
        ok,
        result,
        error,
    }
}

fn write_response<W: Write>(out: &mut W, response: Response) -> Result<()> {
    let mut line = serde_json::to_string(&response)?;
    line.push('\n');
    out.write_all(line.as_bytes())?;
    out.flush()?;
    Ok(())
}

fn rpc_text(resp: &Value) -> String {
    resp.get("result")
        .and_then(|result| result.get("content"))
        .and_then(|content| content.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .or_else(|| {
            resp.get("error")
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| serde_json::to_string(resp).unwrap_or_default())
}

fn main() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout());
    let mut ast_state = cortexast::server::ServerState::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                write_response(
                    &mut stdout,
                    response(
                        Value::Null,
                        false,
                        None,
                        Some(format!("Failed to read request: {err}")),
                    ),
                )?;
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                write_response(
                    &mut stdout,
                    response(
                        Value::Null,
                        false,
                        None,
                        Some(format!("Invalid request JSON: {err}")),
                    ),
                )?;
                continue;
            }
        };

        let response = match request.method.as_str() {
            "initialize" => {
                ast_state.capture_init_root(&request.params);
                response(
                    request.id,
                    true,
                    Some(json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "serverInfo": {
                            "name": "cortex-extension-bridge",
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "toolCount": tools::all_schemas(&ast_state).len()
                    })),
                    None,
                )
            }
            "list_tools" => response(
                request.id,
                true,
                Some(json!({
                    "tools": tools::all_schemas(&ast_state)
                })),
                None,
            ),
            "call_tool" => {
                match request.params.get("name").and_then(|value| value.as_str()) {
                    None => response(
                        request.id,
                        false,
                        None,
                        Some("'name' required".to_string()),
                    ),
                    Some(name) => {
                        let args = request
                            .params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));

                        if name == "cortex_mcp_hot_reload" {
                            match cortex_act::act::hot_reload::run(&args) {
                                Ok(text) => response(
                                    request.id,
                                    true,
                                    Some(json!({ "text": text })),
                                    None,
                                ),
                                Err(err) => response(
                                    request.id,
                                    false,
                                    None,
                                    Some(format!("cortex_mcp_hot_reload failed: {err}")),
                                ),
                            }
                        } else {
                            let rpc = tools::dispatch(
                                &mut ast_state,
                                request.id.clone(),
                                &json!({
                                    "name": name,
                                    "arguments": args
                                }),
                            );
                            let text = rpc_text(&rpc);
                            let is_error = rpc
                                .get("result")
                                .and_then(|result| result.get("isError"))
                                .and_then(|value| value.as_bool())
                                .unwrap_or_else(|| rpc.get("error").is_some());

                            if is_error {
                                response(request.id, false, None, Some(text))
                            } else {
                                response(request.id, true, Some(json!({ "text": text })), None)
                            }
                        }
                    }
                }
            }
            "ping" => response(
                request.id,
                true,
                Some(json!({ "pong": true })),
                None,
            ),
            "shutdown" => {
                write_response(
                    &mut stdout,
                    response(
                        request.id,
                        true,
                        Some(json!({ "status": "bye" })),
                        None,
                    ),
                )?;
                break;
            }
            other => response(
                request.id,
                false,
                None,
                Some(format!("Unknown method: {other}")),
            ),
        };

        write_response(&mut stdout, response)?;
    }

    Ok(())
}