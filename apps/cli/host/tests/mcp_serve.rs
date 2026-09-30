use assert_cmd::Command;
use serde_json::Value;
use std::process::Output;

fn isolated_command(temp: &tempfile::TempDir) -> Command {
    let config = temp.path().join("config");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let mut command = Command::cargo_bin("lingxi-cli").unwrap();
    command
        .current_dir(project)
        .env("HOME", temp.path())
        .env("LINGXI_CONFIG_DIR", config)
        .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
        .env("DISABLE_AUTOUPDATER", "1");
    command
}

fn write_global_config(temp: &tempfile::TempDir, contents: &str) {
    let config = temp.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".lingxi.json"), contents).unwrap();
}

fn stdout_frames(output: &Output) -> Vec<Value> {
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout must contain only JSON-RPC frames"))
        .collect()
}

#[test]
fn serve_processes_initialize_ping_and_tools_list_before_eof() {
    let temp = tempfile::tempdir().unwrap();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{}}\n",
    );
    let output = isolated_command(&temp)
        .args(["mcp", "serve"])
        .write_stdin(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let frames: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout must contain only JSON-RPC frames"))
        .collect();
    assert_eq!(frames.len(), 3, "unexpected stdout: {stdout}");
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(frames[0]["result"]["serverInfo"]["name"], "LingXi");
    assert_eq!(
        frames[1],
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
    );
    assert_eq!(frames[2]["id"], 3);
    let tools = frames[2]["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert!(tools.iter().any(|tool| tool["name"] == "Read"));
    assert!(tools.iter().all(|tool| tool.get("inputSchema").is_some()));
}

#[test]
fn serve_starts_without_anthropic_api_key() {
    let temp = tempfile::tempdir().unwrap();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
    );
    let output = isolated_command(&temp)
        .env_remove("ANTHROPIC_API_KEY")
        .args(["mcp", "serve"])
        .write_stdin(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("protocol-only stdout"))
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(frames[1]["result"]["tools"].is_array());
}

#[test]
fn serve_rejects_duplicate_initialize_before_initialized_notification() {
    let temp = tempfile::tempdir().unwrap();
    let request = |id| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        })
        .to_string()
    };
    let input = format!("{}\n{}\n", request(1), request(2));
    let output = isolated_command(&temp)
        .args(["mcp", "serve"])
        .write_stdin(input)
        .output()
        .unwrap();
    assert!(output.status.success());
    let frames: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("protocol-only stdout"))
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[1]["id"], 2);
    assert_eq!(frames[1]["error"]["code"], -32600);
    assert_eq!(
        frames[1]["error"]["message"],
        "Server is already initialized"
    );
}

#[test]
fn serve_duplicate_tools_call_id_keeps_original_call_cancellable() {
    let temp = tempfile::tempdir().unwrap();
    write_global_config(&temp, r#"{"permissions":{"allow":["Bash"]}}"#);
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1" }
        }
    });
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "Bash",
            "arguments": { "command": "sleep 30" },
            "_meta": { "progressToken": "dup-cancel" }
        }
    });
    let cancel = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 7 }
    });
    let input = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        initialize,
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        call,
        call,
        cancel
    );
    let output = isolated_command(&temp)
        .args(["mcp", "serve"])
        .write_stdin(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(r#""jsonrpc":"2.0""#),
        "protocol frames must not leak to stderr: {stderr}"
    );

    let frames = stdout_frames(&output);
    assert!(
        frames.len() >= 3,
        "expected initialize, duplicate error, and aborted tool result; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[0]["result"]["protocolVersion"], "2025-06-18");

    let duplicate = frames
        .iter()
        .find(|frame| frame["id"] == 7 && frame.get("error").is_some())
        .expect("duplicate request should return an error frame");
    assert_eq!(duplicate["error"]["code"], -32600);
    assert_eq!(duplicate["error"]["message"], "Duplicate request id");

    let aborted = frames
        .iter()
        .find(|frame| frame["id"] == 7 && frame.get("result").is_some())
        .expect("original call should still complete after cancellation");
    assert_eq!(aborted["result"]["isError"], true);
    let text = aborted["result"]["content"][0]["text"]
        .as_str()
        .expect("tool result text");
    assert!(
        text == "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed."
            || text.contains("Tool execution failed: aborted"),
        "unexpected cancelled tool result: {text}"
    );

    for progress in frames
        .iter()
        .filter(|frame| frame["method"] == "notifications/progress")
    {
        assert_eq!(progress["params"]["progressToken"], "dup-cancel");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn desktop_import_skips_cross_scope_conflicts_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let desktop = temp
        .path()
        .join("Library/Application Support/Claude/claude_desktop_config.json");
    std::fs::create_dir_all(desktop.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join(".lingxi.json"),
        r#"{"mcpServers":{"shared":{"type":"stdio","command":"keep-me"}}}"#,
    )
    .unwrap();
    std::fs::write(
        desktop,
        r#"{"mcpServers":{"shared":{"command":"overwrite-me"},"fresh":{"command":"new-server"}}}"#,
    )
    .unwrap();

    let output = isolated_command(&temp)
        .args(["mcp", "add-from-claude-desktop", "--scope", "local"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stored: Value =
        serde_json::from_slice(&std::fs::read(config.join(".lingxi.json")).unwrap()).unwrap();
    assert_eq!(stored["mcpServers"]["shared"]["command"], "keep-me");
    assert_eq!(
        stored["projects"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["mcpServers"]["fresh"]["command"],
        "new-server"
    );
}
