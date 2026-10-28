//! Process-level coverage for MCP stdio initialization, tool listing, cancellation,
//! and scoped desktop configuration import.

use assert_cmd::Command;
use serde_json::Value;

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

fn permission_settings(allow: &[String]) -> Value {
    // Auto mode deliberately strips broad shell permissions. Use the normal
    // policy in this isolated test home and grant only the permissions needed.
    serde_json::json!({ "permissions": { "defaultMode": "default", "allow": allow } })
}

fn exact_shell_rule(tool: &str, command: &str) -> String {
    permission::PermissionRuleValue {
        tool_name: tool.to_owned(),
        rule_content: Some(command.into()),
        original_rule: None,
    }
    .to_rule_string()
}

fn write_user_permissions(temp: &tempfile::TempDir, allow: &[String]) {
    let config = temp.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("settings.json"),
        serde_json::to_vec(&permission_settings(allow)).unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
fn shell_fixture_command(name: &str) -> String {
    assert!(name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
    // Expand the runner shell's PID before starting the fixture. `$PPID`
    // inside the fixture would identify the MCP server if the shell execs it.
    format!("sh ./mcp-process-{name}.sh \"$$\"")
}

#[cfg(unix)]
fn write_shell_fixture(project: &std::path::Path, name: &str, record_completion: bool) -> String {
    // A literal `&` in the tool call triggers the background-operator safety
    // guard even with an exact allow. The test-owned script keeps the real
    // child process while the tool call itself can be allowed exactly.
    let command = shell_fixture_command(name);
    let script = format!(
        "sleep 30 & child=$!; trap 'kill \"$child\" 2>/dev/null || true; wait \"$child\" 2>/dev/null || true' EXIT; printf '%s %s %s\\n' \"$1\" \"$$\" \"$child\" > started-{name}.tmp; mv started-{name}.tmp started-{name}; while [ ! -e gate-{name} ]; do sleep 0.05; done{}",
        if record_completion {
            format!("; printf 'fixture-{name}-completed\\n'; printf finished > finished-{name}")
        } else {
            String::new()
        }
    );
    std::fs::write(project.join(format!("mcp-process-{name}.sh")), script).unwrap();
    command
}

#[cfg(unix)]
fn fixture_process_ids(text: &str, server_pid: u32) -> Option<Vec<i32>> {
    let mut pids: Vec<i32> = text
        .split_whitespace()
        .map(|part| part.parse().expect("fixture PID must be an integer"))
        .collect();
    if pids.len() != 3 {
        return None;
    }
    for &pid in &pids {
        assert!(
            pid > 1 && pid as u32 != server_pid && pid as u32 != std::process::id(),
            "fixture reported a non-owned PID: {pids:?}"
        );
    }
    assert_ne!(pids[0], pids[2], "runner and sleep must be distinct");
    assert_ne!(pids[1], pids[2], "fixture and sleep must be distinct");
    // The runner shell may exec the fixture, in which case both IDs are equal.
    pids.dedup();
    Some(pids)
}

#[cfg(unix)]
#[test]
fn shell_fixture_permission_is_exact_and_keeps_the_policy_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let command = write_shell_fixture(&project, "permission-probe", false);
    let background_command = format!("{command} &");
    write_user_permissions(
        &temp,
        &[
            exact_shell_rule("Bash", &command),
            exact_shell_rule("Bash", &background_command),
        ],
    );
    let config = temp.path().join("config");
    let raw = std::fs::read_to_string(config.join("settings.json")).unwrap();
    let mode = permission::default_mode_from_settings_json(&raw).unwrap();
    assert_eq!(mode, permission::PermissionMode::Default);
    let rules = permission::permission_rules_from_settings_json(
        &raw,
        permission::PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User),
    )
    .unwrap();
    let policy =
        permission::PermissionPolicy::from_rules(mode, rules).with_roots(permission::FsRoots {
            cwd: project,
            home: Some(temp.path().to_path_buf()),
            lingxi_home: config,
        });
    let input = |command: &str| {
        lingxi_core::types::utf16_json::Utf16JsonProjection::from_serde_value(
            &serde_json::json!({ "command": command }),
        )
        .unwrap()
    };
    assert!(matches!(
        policy.authorize("Bash", &input(&command)),
        permission::PermissionResult::Allow { .. }
    ));
    assert!(!matches!(
        policy.authorize("Bash", &input("sh ./mcp-process-other.sh \"$$\"")),
        permission::PermissionResult::Allow { .. }
    ));
    assert!(matches!(
        policy.authorize("Bash", &input(&background_command)),
        permission::PermissionResult::Ask {
            reason: permission::PermissionDecisionReason::SafetyCheck { .. },
            ..
        }
    ));
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
fn serve_echoes_large_negative_and_escaped_ids_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":-9007199254740993,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":-184467440737095516161234567890,\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":\"\\u0061\\ud834\\udd1e\",\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":1e400,\"method\":\"ping\"}\n",
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
    let frames: Vec<_> = stdout.lines().collect();
    assert_eq!(frames.len(), 4, "{stdout}");
    assert!(
        frames[0].contains("\"id\":-9007199254740993"),
        "{}",
        frames[0]
    );
    assert!(
        frames[1].contains("\"id\":-184467440737095516161234567890"),
        "{}",
        frames[1]
    );
    assert!(
        frames[2].contains(r#""id":"\u0061\ud834\udd1e""#),
        "{}",
        frames[2]
    );
    assert!(frames[3].contains("\"id\":1e400"), "{}", frames[3]);
}

#[test]
fn eof_keeps_an_accepted_ordinary_tool_error_response() {
    let temp = tempfile::tempdir().unwrap();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"tools/call\",\"params\":{\"name\":\"not-a-tool\",\"arguments\":{}}}\n",
    );
    let output = isolated_command(&temp)
        .args(["mcp", "serve"])
        .write_stdin(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let frames: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let response = frames
        .iter()
        .find(|frame| frame["id"] == 42)
        .expect("ordinary request response after EOF");
    assert_eq!(response["error"]["code"], -32602);
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
fn serve_negotiates_a_supported_protocol_version() {
    let temp = tempfile::tempdir().unwrap();
    let initialize = |id, params| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": params,
        })
    };
    let frames = [
        initialize(1, serde_json::json!({})),
        initialize(2, serde_json::json!({ "protocolVersion": 123 })),
        initialize(3, serde_json::json!({ "protocolVersion": null })),
        initialize(
            4,
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        ),
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/list" }),
    ];
    let input = frames
        .iter()
        .map(|frame| format!("{frame}\n"))
        .collect::<String>();
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
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("protocol-only stdout"))
        .collect();
    assert_eq!(responses.len(), 5);
    for (index, response) in responses.iter().take(3).enumerate() {
        assert_eq!(response["id"], index + 1);
        assert_eq!(response["error"]["code"], -32602);
    }
    assert_eq!(responses[3]["id"], 4);
    assert_eq!(responses[3]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(responses[4]["id"], 5);
    assert!(responses[4]["result"]["tools"].is_array());
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
fn serve_rejects_duplicate_tool_id_and_suppresses_cancelled_reply() {
    let temp = tempfile::tempdir().unwrap();
    let (tool, command) = if cfg!(windows) {
        ("PowerShell", "Start-Sleep -Seconds 10")
    } else {
        ("Bash", "sleep 10")
    };
    write_user_permissions(&temp, &[exact_shell_rule(tool, command)]);
    let initialize = serde_json::json!({
        "jsonrpc":"2.0", "id":1, "method":"initialize",
        "params":{"protocolVersion":"2025-06-18", "capabilities":{},
                  "clientInfo":{"name":"test", "version":"1"}}
    });
    let call = serde_json::json!({
        "jsonrpc":"2.0", "id":7, "method":"tools/call",
        "params":{"name":tool, "arguments":{"command":command}}
    });
    let input = format!(
        "{initialize}\n{}\n{call}\n{call}\n{}\n",
        serde_json::json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
        serde_json::json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":7}}),
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
        .map(|line| serde_json::from_str(line).expect("protocol-only stdout"))
        .collect();
    assert_eq!(frames[0]["result"]["protocolVersion"], "2025-06-18");
    let duplicate = frames
        .iter()
        .find(|frame| frame["id"] == 7 && frame.get("error").is_some())
        .expect("duplicate request should be rejected");
    assert_eq!(duplicate["error"]["code"], -32600);
    assert_eq!(duplicate["error"]["message"], "Duplicate request id");
    assert!(
        frames
            .iter()
            .all(|frame| !(frame["id"] == 7 && frame.get("result").is_some())),
        "cancelled request must not receive a result: {frames:?}"
    );
}

#[cfg(unix)]
#[test]
fn serve_duplicate_tools_call_id_keeps_original_call_cancellable() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    struct Server {
        child: Child,
        stdin: Option<ChildStdin>,
        frames: Receiver<Value>,
        project: std::path::PathBuf,
        stderr_path: std::path::PathBuf,
        seen: Vec<Value>,
    }

    impl Server {
        fn send(&mut self, frame: Value) {
            writeln!(self.stdin.as_mut().unwrap(), "{frame}").unwrap();
        }

        fn until(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let frame = self.frames.recv_timeout(remaining).unwrap_or_else(|error| {
                    panic!(
                        "missing MCP response: {error}; frames: {:?}; stderr: {}",
                        self.seen,
                        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
                    )
                });
                self.seen.push(frame.clone());
                if predicate(&frame) {
                    return frame;
                }
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            // Release any shell that was mistakenly backgrounded before
            // stopping the server, so a failing test leaves no long-lived job.
            for gate in ["gate-a", "gate-b", "gate-c"] {
                let _ = std::fs::write(self.project.join(gate), b"release");
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn read_started(path: &std::path::Path, server_pid: u32) -> Vec<i32> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Some(pids) = fixture_process_ids(&text, server_pid) {
                    return pids;
                }
            }
            assert!(Instant::now() < deadline, "shell did not start: {path:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn alive(pid: i32) -> bool {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) | Err(nix::errno::Errno::EPERM) => true,
            Err(nix::errno::Errno::ESRCH) => false,
            Err(error) => panic!("checking pid {pid}: {error}"),
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let mut allow = vec!["TaskOutput".to_owned()];
    for name in ["a", "b", "c"] {
        let command = write_shell_fixture(&project, name, true);
        allow.push(exact_shell_rule("Bash", &command));
    }
    write_user_permissions(&temp, &allow);
    let stderr_path = temp.path().join("stderr.log");
    let stderr = std::fs::File::create(&stderr_path).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .current_dir(&project)
        .env("HOME", temp.path())
        .env("LINGXI_CONFIG_DIR", config)
        .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
        .env("DISABLE_AUTOUPDATER", "1")
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let (sender, frames) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.expect("read MCP stdout");
            let frame = serde_json::from_str(&line).expect("protocol-only stdout");
            if sender.send(frame).is_err() {
                break;
            }
        }
    });
    let mut server = Server {
        child,
        stdin,
        frames,
        project,
        stderr_path,
        seen: Vec::new(),
    };
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
    server.send(initialize);
    let initialized = server.until(|frame| frame["id"] == 1);
    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
    server.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server.send(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let catalog = server.until(|frame| frame["id"] == 2);
    let tools = catalog["result"]["tools"].as_array().unwrap();
    let bash = tools
        .iter()
        .find(|tool| tool["name"] == "Bash")
        .expect("Bash is exposed by MCP");
    assert!(bash.get("outputSchema").is_none());
    assert!(tools.iter().any(|tool| tool["name"] == "TaskOutput"));
    assert!(tools.iter().any(|tool| tool["name"] == "TaskStop"));
    let call = |id, suffix: &str| {
        let command = shell_fixture_command(suffix);
        serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "Bash", "arguments": { "command": command },
                "_meta": { "progressToken": format!("token-{suffix}") } }
        })
    };
    let first = call(7, "a");
    server.send(first.clone());
    server.send(call(8, "b"));
    let cancelled_pids = read_started(&server.project.join("started-a"), server.child.id());
    let other_pids = read_started(&server.project.join("started-b"), server.child.id());
    server.send(first);
    let duplicate = server.until(|frame| frame["id"] == 7 && frame.get("error").is_some());
    assert_eq!(duplicate["error"]["code"], -32600);
    assert_eq!(duplicate["error"]["message"], "Duplicate request id");
    server.send(serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 7 }
    }));

    let deadline = Instant::now() + Duration::from_secs(10);
    while cancelled_pids.iter().any(|pid| alive(*pid)) {
        assert!(
            Instant::now() < deadline,
            "cancelled process tree remains alive: {cancelled_pids:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        other_pids.iter().all(|pid| alive(*pid)),
        "unrelated request was cancelled"
    );
    std::fs::write(server.project.join("gate-a"), b"release").unwrap();
    std::fs::write(server.project.join("gate-b"), b"release").unwrap();
    let other = server.until(|frame| frame["id"] == 8 && frame.get("result").is_some());
    assert_eq!(other["result"]["isError"], false);
    assert!(!server.project.join("finished-a").exists());
    assert!(server.project.join("finished-b").exists());

    // A cancellation after a completed response must not affect the server.
    server.send(serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 8 }
    }));
    server.send(serde_json::json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }));
    assert_eq!(
        server.until(|frame| frame["id"] == 9)["result"],
        serde_json::json!({})
    );
    assert!(
        server
            .seen
            .iter()
            .all(|frame| !(frame["id"] == 7 && frame.get("result").is_some())),
        "cancelled request received a result: {:?}",
        server.seen
    );
    assert!(server
        .seen
        .iter()
        .filter(|frame| frame["method"] == "notifications/progress")
        .all(|frame| frame["params"]["progressToken"] == "token-a"
            || frame["params"]["progressToken"] == "token-b"));

    let mut background = call(10, "c");
    background["params"]["arguments"]["run_in_background"] = serde_json::json!(true);
    server.send(background);
    let completed = server.until(|frame| frame["id"] == 10 && frame.get("result").is_some());
    assert_eq!(completed["result"]["isError"], false, "{completed}");
    let handoff = &completed["result"]["structuredContent"];
    let task_ids = handoff["backgroundTaskIds"].as_array().unwrap();
    assert_eq!(task_ids.len(), 1, "{completed}");
    let task_id = task_ids[0].as_str().filter(|id| !id.is_empty()).unwrap();
    assert!(
        completed["result"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| {
                block["type"] == "text"
                    && block["text"]
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .as_ref()
                        == Some(handoff)
            }),
        "structured handoff must also be available to text-only clients: {completed}"
    );
    server.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 12, "method": "tools/call",
        "params": { "name": "TaskOutput", "arguments": { "task_id": task_id, "block": true } }
    }));
    // TaskOutput does not provide a verified owned-process cancellation
    // contract. Its cancellation notification must be ignored, and the
    // original call must still produce its result once the task completes.
    server.send(serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 12 }
    }));
    server.send(serde_json::json!({ "jsonrpc": "2.0", "id": 13, "method": "ping" }));
    assert_eq!(
        server.until(|frame| frame["id"] == 13)["result"],
        serde_json::json!({})
    );
    assert!(server.seen.iter().all(|frame| frame["id"] != 12));
    let committed_pids = read_started(&server.project.join("started-c"), server.child.id());
    server.send(serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 10 }
    }));
    server.send(serde_json::json!({ "jsonrpc": "2.0", "id": 11, "method": "ping" }));
    assert_eq!(
        server.until(|frame| frame["id"] == 11)["result"],
        serde_json::json!({})
    );
    assert!(
        committed_pids.iter().all(|pid| alive(*pid)),
        "committed background was cancelled"
    );
    std::fs::write(server.project.join("gate-c"), b"release").unwrap();
    let task_output = server.until(|frame| frame["id"] == 12);
    assert!(
        task_output.get("error").is_none(),
        "ordinary request should complete after ignored cancellation: {task_output}"
    );
    let result = task_output["result"]
        .as_object()
        .expect("TaskOutput response must contain a result object");
    // MCP treats an omitted isError as false; reject null and non-booleans.
    assert!(
        matches!(result.get("isError"), None | Some(Value::Bool(false))),
        "ordinary request should complete after ignored cancellation: {task_output}"
    );
    let expected_header = format!(
        "<retrieval_status>success</retrieval_status>\n\n<task_id>{task_id}</task_id>\n\n<task_type>local_bash</task_type>\n\n<status>completed</status>\n\n<exit_code>0</exit_code>\n\n<output>\n"
    );
    assert!(
        result
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| blocks.iter().any(|block| {
                block["type"] == "text"
                    && block["text"].as_str().is_some_and(|text| {
                        text.strip_prefix(&expected_header)
                            .and_then(|body| body.split_once("\n</output>"))
                            .is_some_and(|(output, tail)| {
                                tail.trim().is_empty()
                                    && output.lines().any(|line| line == "fixture-c-completed")
                            })
                    })
            })),
        "TaskOutput must contain the completed task's successful output: {task_output}"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while committed_pids.iter().any(|pid| alive(*pid)) {
        assert!(
            Instant::now() < deadline,
            "committed process failed to exit: {committed_pids:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(server.project.join("finished-c").exists());
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

#[cfg(unix)]
mod transport_failure_tests {
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::process::{Child, ChildStdin, ChildStdout, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    struct Server {
        child: Child,
        input: Option<ChildStdin>,
        output: Option<ChildStdout>,
        home: tempfile::TempDir,
        project: std::path::PathBuf,
        received: Vec<u8>,
    }

    impl Server {
        fn start(name: &str) -> Self {
            Self::start_with_catalog_gate(false, Some(name))
        }

        fn start_with_catalog_gate(gated: bool, shell_fixture: Option<&str>) -> Self {
            let home = tempfile::tempdir().unwrap();
            let config = home.path().join("config");
            let project = home.path().join("project");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::create_dir_all(&project).unwrap();
            let mut global = json!({});
            let allow = shell_fixture
                .map(|name| {
                    let command = super::write_shell_fixture(&project, name, false);
                    vec![super::exact_shell_rule("Bash", &command)]
                })
                .unwrap_or_default();
            let mut settings = super::permission_settings(&allow);
            if gated {
                let project_key = std::fs::canonicalize(&project)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                global["projects"] = json!({});
                global["projects"][project_key.as_str()] = json!({"hasTrustDialogAccepted": true});
                let plugin = config.join("plugins/cache/test/catalog-gate/1.0.0");
                std::fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
                std::fs::create_dir_all(plugin.join("hooks")).unwrap();
                std::fs::write(
                    plugin
                        .join(branding::PLUGIN_MANIFEST_DIR)
                        .join("plugin.json"),
                    r#"{"name":"catalog-gate","version":"1.0.0"}"#,
                )
                .unwrap();
                std::fs::write(
                    plugin.join("hooks/hooks.json"),
                    r#"{"modules":["./register.js"]}"#,
                )
                .unwrap();
                std::fs::write(
                    plugin.join("hooks/register.js"),
                    r#"
export function register(on) {
  on('tool.describe', { tool: 'Bash' }, async ($, event, next) => {
    if (await $.fs.exists('catalog-armed')) {
      await $.fs.write('catalog-entered', 'entered');
      while (!await $.fs.exists('catalog-release')) {
        await $.clock.sleep(25);
      }
    }
    return next(event);
  });
}
"#,
                )
                .unwrap();
                settings["enabledPlugins"] = json!({"catalog-gate@test":true});
                std::fs::write(
                    config.join("plugins/installed_plugins.json"),
                    serde_json::to_vec(&json!({
                        "version": 2,
                        "plugins": {"catalog-gate@test": [{
                            "scope": "user", "installPath": plugin,
                            "version": "1.0.0",
                            "installedAt": "2026-01-01T00:00:00.000Z",
                            "lastUpdated": "2026-01-01T00:00:00.000Z"
                        }]}
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            std::fs::write(
                config.join("settings.json"),
                serde_json::to_vec(&settings).unwrap(),
            )
            .unwrap();
            std::fs::write(
                config.join(".lingxi.json"),
                serde_json::to_vec(&global).unwrap(),
            )
            .unwrap();
            let stderr = std::fs::File::create(home.path().join("stderr.log")).unwrap();
            let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
                .current_dir(&project)
                .env("HOME", home.path())
                .env("LINGXI_CONFIG_DIR", &config)
                .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
                .env("DISABLE_AUTOUPDATER", "1")
                .args(["mcp", "serve"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap();
            let input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            for fd in [
                std::os::fd::AsFd::as_fd(&input),
                std::os::fd::AsFd::as_fd(&output),
            ] {
                let flags = rustix::fs::fcntl_getfl(fd).unwrap();
                rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
            }
            let mut server = Self {
                child,
                input: Some(input),
                output: Some(output),
                home,
                project,
                received: Vec::new(),
            };
            server.send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                            "clientInfo": { "name": "transport-test", "version": "1" } }
            }));
            assert_eq!(server.frame()["result"]["protocolVersion"], "2025-06-18");
            server.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
            server
        }

        fn send(&mut self, frame: Value) {
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            self.send_bytes(&bytes);
        }

        fn send_bytes(&mut self, bytes: &[u8]) {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut offset = 0;
            while offset < bytes.len() {
                match self.input.as_mut().unwrap().write(&bytes[offset..]) {
                    Ok(0) => panic!("MCP stdin closed"),
                    Ok(count) => offset += count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "MCP server stopped reading stdin"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("write MCP stdin: {error}"),
                }
            }
        }

        fn frame(&mut self) -> Value {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(end) = self.received.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<_> = self.received.drain(..=end).collect();
                    return serde_json::from_slice(&line).unwrap();
                }
                let mut bytes = [0_u8; 4096];
                match self.output.as_mut().unwrap().read(&mut bytes) {
                    Ok(0) => panic!("MCP stdout closed: {}", self.stderr()),
                    Ok(count) => self.received.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "missing frame: {}",
                            self.stderr()
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("read MCP stdout: {error}"),
                }
            }
        }

        fn start_shell(&mut self, id: u64, name: &str, background: bool) -> Vec<i32> {
            self.start_shell_with_id(json!(id), name, background)
        }

        fn start_shell_with_id(&mut self, id: Value, name: &str, background: bool) -> Vec<i32> {
            let command = super::shell_fixture_command(name);
            self.send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "Bash", "arguments": {
                    "command": command, "run_in_background": background
                }}
            }));
            let path = self.project.join(format!("started-{name}"));
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Some(pids) = super::fixture_process_ids(&text, self.child.id()) {
                        return pids;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "shell did not start: {}",
                    self.stderr()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn block_catalog(&mut self, id: u64) {
            std::fs::write(self.project.join("catalog-armed"), b"armed").unwrap();
            self.send(json!({"jsonrpc":"2.0", "id":id, "method":"tools/list"}));
            let deadline = Instant::now() + Duration::from_secs(20);
            while !self.project.join("catalog-entered").exists() {
                assert!(
                    Instant::now() < deadline,
                    "tool.describe did not enter the controlled gate: {}",
                    self.stderr()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn fill_stdout(&mut self) {
            self.send(
                json!({ "jsonrpc": "2.0", "id": "x".repeat(4 * 1024 * 1024), "method": "ping" }),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut previous = 0;
            let mut stable = 0;
            loop {
                let available = rustix::io::ioctl_fionread(self.output.as_ref().unwrap()).unwrap();
                if available >= 4096 && available == previous {
                    stable += 1;
                    if stable == 5 {
                        return;
                    }
                } else {
                    stable = 0;
                }
                previous = available;
                assert!(
                    Instant::now() < deadline,
                    "stdout did not reach backpressure: {}",
                    self.stderr()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        fn stderr(&self) -> String {
            std::fs::read_to_string(self.home.path().join("stderr.log")).unwrap_or_default()
        }

        fn wait_exit(&mut self) -> ExitStatus {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    return status;
                }
                assert!(
                    Instant::now() < deadline,
                    "MCP server did not exit: {}",
                    self.stderr()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            for name in [
                "full",
                "broken",
                "eof",
                "invalid",
                "catalog-cancel",
                "catalog-eof",
                "catalog-broken",
                "catalog-large-id",
                "escaped-id",
                "duplicate-backpressure",
                "handoff-eof",
            ] {
                let _ = std::fs::write(self.project.join(format!("gate-{name}")), b"release");
            }
            let _ = std::fs::write(self.project.join("catalog-release"), b"release");
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn wait_reaped(pids: &[i32]) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let alive = pids.iter().any(|pid| {
                match nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), None) {
                    Ok(()) | Err(nix::errno::Errno::EPERM) => true,
                    Err(nix::errno::Errno::ESRCH) => false,
                    Err(error) => panic!("checking pid {pid}: {error}"),
                }
            });
            if !alive {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "owned process tree still alive: {pids:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn cancellation_reaps_the_process_while_stdout_is_full() {
        let mut server = Server::start("full");
        let pids = server.start_shell(7, "full", false);
        server.fill_stdout();
        server.send(json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 7 }
        }));
        wait_reaped(&pids);
        drop(server.input.take());
        assert!(
            !server.wait_exit().success(),
            "undrained output must fail closed"
        );
        assert!(
            server.stderr().contains("stdout did not drain"),
            "{}",
            server.stderr()
        );
    }

    #[test]
    fn broken_stdout_reaps_a_background_handle_before_handoff() {
        let mut server = Server::start("broken");
        server.fill_stdout();
        let pids = server.start_shell(7, "broken", true);
        drop(server.output.take());
        wait_reaped(&pids);
        assert!(!server.wait_exit().success());
        assert!(
            server.stderr().contains("write stdout:"),
            "{}",
            server.stderr()
        );
    }

    #[test]
    fn eof_reaps_an_active_process() {
        let mut server = Server::start("eof");
        let pids = server.start_shell(7, "eof", false);
        drop(server.input.take());
        wait_reaped(&pids);
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn visible_background_handoff_survives_eof() {
        let mut server = Server::start("handoff-eof");
        let pids = server.start_shell(7, "handoff-eof", true);
        let response = server.frame();
        assert_eq!(response["id"], 7);
        assert!(response["result"]["structuredContent"]["backgroundTaskIds"].is_array());
        drop(server.input.take());
        assert!(server.wait_exit().success(), "{}", server.stderr());
        for pid in &pids {
            assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), None).is_ok());
        }
        std::fs::write(server.project.join("gate-handoff-eof"), b"release").unwrap();
        wait_reaped(&pids);
    }

    #[test]
    fn queued_ping_id_is_reserved_during_stdout_backpressure() {
        let mut server = Server::start("duplicate-backpressure");
        let pids = server.start_shell(7, "duplicate-backpressure", false);
        server.fill_stdout();
        server.send(json!({"jsonrpc":"2.0", "id":42, "method":"ping"}));
        server.send(json!({"jsonrpc":"2.0", "id":42, "method":"ping"}));
        server.send(json!({"jsonrpc":"2.0", "id":42, "method":"tools/list"}));
        server.send(
            json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":7}}),
        );
        // Reaping proves the server consumed all preceding frames while the
        // first ping was still queued behind the filled stdout pipe.
        wait_reaped(&pids);
        let first = server.frame();
        assert!(first["id"]
            .as_str()
            .is_some_and(|id| id.len() == 4 * 1024 * 1024));
        let ping = server.frame();
        assert_eq!(ping["id"], 42);
        assert_eq!(ping["result"], json!({}));
        for _ in 0..2 {
            let duplicate = server.frame();
            assert_eq!(duplicate["id"], 42);
            assert_eq!(duplicate["error"]["code"], -32600);
        }
        drop(server.input.take());
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn stalled_catalog_does_not_block_cancellation_or_duplicate_ids() {
        let mut server = Server::start_with_catalog_gate(true, Some("catalog-cancel"));
        let pids = server.start_shell(7, "catalog-cancel", false);
        server.send(json!({"jsonrpc":"2.0", "id":7, "method":"ping"}));
        let duplicate = server.frame();
        assert_eq!(duplicate["id"], 7);
        assert_eq!(duplicate["error"]["code"], -32600);
        server.send(json!({"jsonrpc":"2.0", "id":7, "method":"tools/list"}));
        let duplicate = server.frame();
        assert_eq!(duplicate["id"], 7);
        assert_eq!(duplicate["error"]["code"], -32600);
        server.block_catalog(8);
        server.send(json!({
            "jsonrpc":"2.0", "id":8, "method":"tools/call",
            "params":{"name":"Bash", "arguments":{"command":"echo duplicate"}}
        }));
        let duplicate = server.frame();
        assert_eq!(duplicate["id"], 8);
        assert_eq!(duplicate["error"]["code"], -32600);
        server.send(json!({
            "jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":7}
        }));
        wait_reaped(&pids);
        assert!(!server.project.join("catalog-release").exists());
        server.send(json!({"jsonrpc":"2.0", "id":9, "method":"ping"}));
        let ping = server.frame();
        assert_eq!(ping["id"], 9);
        assert_eq!(ping["result"], json!({}));
        std::fs::write(server.project.join("catalog-release"), b"release").unwrap();
        let catalog = server.frame();
        assert_eq!(catalog["id"], 8);
        assert!(catalog["result"]["tools"].is_array(), "{catalog}");
        drop(server.input.take());
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn escaped_string_ids_match_for_duplicates_and_cancellation() {
        let mut server = Server::start("escaped-id");
        let pids = server.start_shell_with_id(json!("shell-a"), "escaped-id", false);
        server.send_bytes(
            concat!(
                r#"{"jsonrpc":"2.0","id":"shell-\u0061","method":"ping"}"#,
                "\n"
            )
            .as_bytes(),
        );
        let duplicate = server.frame();
        assert_eq!(duplicate["id"], "shell-a");
        assert_eq!(duplicate["error"]["code"], -32600);
        server.send_bytes(concat!(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"shell-\u0061"}}"#,
            "\n"
        ).as_bytes());
        wait_reaped(&pids);
        drop(server.input.take());
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn adjacent_large_ids_remain_distinct_for_routing_duplicate_and_cancellation() {
        let mut server = Server::start_with_catalog_gate(true, Some("catalog-large-id"));
        let list_id = 9_007_199_254_740_992_u64;
        let shell_id = 9_007_199_254_740_993_u64;
        let pids = server.start_shell(shell_id, "catalog-large-id", false);
        server.block_catalog(list_id);
        server.send(json!({"jsonrpc":"2.0", "id":shell_id, "method":"tools/list"}));
        let duplicate = server.frame();
        assert_eq!(duplicate["id"], shell_id);
        assert_eq!(duplicate["error"]["code"], -32600);
        // The list ID must not cancel the distinct adjacent shell ID.
        server.send(json!({
            "jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":list_id}
        }));
        server.send(json!({"jsonrpc":"2.0", "id":9, "method":"ping"}));
        assert_eq!(server.frame()["id"], 9);
        for pid in &pids {
            assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), None).is_ok());
        }
        server.send(json!({
            "jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":shell_id}
        }));
        wait_reaped(&pids);
        std::fs::write(server.project.join("catalog-release"), b"release").unwrap();
        let catalog = server.frame();
        assert_eq!(catalog["id"], list_id);
        assert!(catalog["result"]["tools"].is_array(), "{catalog}");
        let tool_id = 9_007_199_254_740_995_u64;
        server.send(json!({
            "jsonrpc":"2.0", "id":tool_id, "method":"tools/call",
            "params":{"name":"not-a-tool", "arguments":{}}
        }));
        let tool_error = server.frame();
        assert_eq!(tool_error["id"], tool_id);
        assert_eq!(tool_error["error"]["code"], -32602);
        drop(server.input.take());
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn stalled_catalog_does_not_block_eof_process_cleanup() {
        let mut server = Server::start_with_catalog_gate(true, Some("catalog-eof"));
        let pids = server.start_shell(7, "catalog-eof", false);
        server.block_catalog(8);
        drop(server.input.take());
        wait_reaped(&pids);
        assert!(!server.project.join("catalog-release").exists());
        let response = server.frame();
        assert_eq!(response["id"], 8);
        assert_eq!(response["error"]["code"], -32603);
        assert_eq!(
            response["error"]["message"],
            "Tool catalog did not finish before EOF"
        );
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn pending_catalog_count_is_bounded_without_blocking_other_requests() {
        let mut server = Server::start_with_catalog_gate(true, None);
        server.block_catalog(100);
        for id in 101..116_u64 {
            server.send(json!({"jsonrpc":"2.0", "id":id, "method":"tools/list"}));
        }
        server.send(json!({"jsonrpc":"2.0", "id":116, "method":"tools/list"}));
        let busy = server.frame();
        assert_eq!(busy["id"], 116);
        assert_eq!(busy["error"]["code"], -32000);
        server.send(json!({"jsonrpc":"2.0", "id":999, "method":"ping"}));
        assert_eq!(server.frame()["id"], 999);
        std::fs::write(server.project.join("catalog-release"), b"release").unwrap();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..16 {
            let response = server.frame();
            assert!(response["result"]["tools"].is_array(), "{response}");
            assert!(ids.insert(response["id"].as_u64().unwrap()), "{response}");
        }
        assert_eq!(ids, (100..116_u64).collect());
        drop(server.input.take());
        assert!(server.wait_exit().success(), "{}", server.stderr());
    }

    #[test]
    fn stalled_catalog_does_not_block_broken_stdout_process_cleanup() {
        let mut server = Server::start_with_catalog_gate(true, Some("catalog-broken"));
        let pids = server.start_shell(7, "catalog-broken", false);
        server.block_catalog(8);
        server.fill_stdout();
        drop(server.output.take());
        wait_reaped(&pids);
        assert!(!server.project.join("catalog-release").exists());
        assert!(!server.wait_exit().success());
        assert!(
            server.stderr().contains("write stdout:"),
            "{}",
            server.stderr()
        );
    }

    #[test]
    fn invalid_input_preserves_the_read_error_and_reaps_the_process() {
        let mut server = Server::start("invalid");
        let pids = server.start_shell(7, "invalid", false);
        server.send_bytes(&[0xff, b'\n']);
        wait_reaped(&pids);
        assert!(!server.wait_exit().success());
        assert!(
            server.stderr().contains("read stdin:"),
            "{}",
            server.stderr()
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_duplicate_call_cancellation_reaps_the_started_process_tree() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    write_user_permissions(&temp, &["PowerShell".to_owned()]);
    let config = temp.path().join("config");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let stderr_path = temp.path().join("stderr.log");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .current_dir(&project)
        .env("HOME", temp.path())
        .env("LINGXI_CONFIG_DIR", config)
        .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
        .env("DISABLE_AUTOUPDATER", "1")
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(std::fs::File::create(&stderr_path).unwrap()))
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, frames) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let frame = serde_json::from_str(&line).expect("protocol-only stdout");
            if sender.send(frame).is_err() {
                break;
            }
        }
    });

    struct Cleanup {
        child: Child,
        pids: Vec<u32>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for pid in &self.pids {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    let mut cleanup = Cleanup {
        child,
        pids: Vec::new(),
    };
    let send = |stdin: &mut std::process::ChildStdin, frame: Value| {
        writeln!(stdin, "{frame}").unwrap();
    };
    let recv = || {
        frames
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|error| {
                panic!(
                    "missing MCP response: {error}; stderr: {}",
                    std::fs::read_to_string(&stderr_path).unwrap_or_default()
                )
            })
    };
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "windows-cancel-test", "version": "1" } }
        }),
    );
    assert_eq!(recv()["result"]["protocolVersion"], "2025-06-18");
    send(
        &mut stdin,
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );

    let command = r#"$child = Start-Process -PassThru -FilePath 'powershell.exe' -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30'; "$PID $($child.Id)" | Set-Content -Encoding ascii -Path 'started-win'; Start-Sleep -Seconds 30"#;
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": "PowerShell", "arguments": { "command": command } }
    });
    send(&mut stdin, call.clone());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(project.join("started-win")) {
            let pids: Vec<u32> = text
                .split_whitespace()
                .map(|part| part.parse().unwrap())
                .collect();
            if pids.len() == 2 {
                cleanup.pids = pids;
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "PowerShell did not start: {}",
            std::fs::read_to_string(&stderr_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    send(&mut stdin, call);
    let duplicate = recv();
    assert_eq!(duplicate["id"], 7);
    assert_eq!(duplicate["error"]["message"], "Duplicate request id");
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 7 }
        }),
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let ids = cleanup
            .pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let script = format!("if (Get-Process -Id {ids} -ErrorAction SilentlyContinue) {{ exit 1 }} else {{ exit 0 }}");
        if std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "cancelled PowerShell tree remains alive: {:?}",
            cleanup.pids
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    cleanup.pids.clear();
    send(
        &mut stdin,
        serde_json::json!({ "jsonrpc": "2.0", "id": 8, "method": "ping" }),
    );
    let reply = recv();
    assert_eq!(reply["id"], 8, "cancelled call must not receive a response");
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = cleanup.child.try_wait().unwrap() {
            assert!(
                status.success(),
                "{}",
                std::fs::read_to_string(&stderr_path).unwrap_or_default()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "MCP server did not exit after EOF"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
#[test]
fn windows_full_stdout_does_not_block_cancellation_or_eof_cleanup() {
    use std::io::{Read, Write};
    use std::process::{Child, Stdio};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    struct Cleanup {
        child: Child,
        pids: Vec<u32>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for pid in &self.pids {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    let temp = tempfile::tempdir().unwrap();
    write_user_permissions(&temp, &["PowerShell".to_owned()]);
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let stderr_path = temp.path().join("stderr-full.log");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .current_dir(&project)
        .env("HOME", temp.path())
        .env("LINGXI_CONFIG_DIR", temp.path().join("config"))
        .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
        .env("DISABLE_AUTOUPDATER", "1")
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(std::fs::File::create(&stderr_path).unwrap()))
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut cleanup = Cleanup {
        child,
        pids: Vec::new(),
    };
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "windows-full-test", "version": "1" } }
        })
    )
    .unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            if stdout.read_exact(&mut byte).is_err() {
                return;
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                let _ = ready_tx.send((stdout, line));
                return;
            }
        }
    });
    let (stdout, line) = ready_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("MCP initialize");
    let frame: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(frame["result"]["protocolVersion"], "2025-06-18");
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        })
    )
    .unwrap();
    let command = r#"$child = Start-Process -PassThru -FilePath 'powershell.exe' -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30'; "$PID $($child.Id)" | Set-Content -Encoding ascii -Path 'started-full'; Start-Sleep -Seconds 30"#;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "PowerShell", "arguments": { "command": command } }
        })
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(project.join("started-full")) {
            let pids: Vec<u32> = text
                .split_whitespace()
                .map(|part| part.parse().unwrap())
                .collect();
            if pids.len() == 2 {
                cleanup.pids = pids;
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "PowerShell did not start: {}",
            std::fs::read_to_string(&stderr_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let large = serde_json::json!({
        "jsonrpc": "2.0", "id": "x".repeat(4 * 1024 * 1024), "method": "ping"
    });
    let (sent_tx, sent_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = writeln!(stdin, "{large}");
        let _ = sent_tx.send((stdin, result));
    });
    let (mut stdin, result) = sent_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("server should continue reading input");
    result.unwrap();

    // Peek runs on a separate test thread because synchronous PeekNamedPipe can
    // itself block. The reader never drains stdout after initialization.
    let stdout = Arc::new(stdout);
    let (bytes_tx, bytes_rx) = mpsc::channel();
    std::thread::spawn({
        let stdout = stdout.clone();
        move || loop {
            let available = platform_windows::stdio_cancel::pipe_bytes_available(&*stdout);
            let failed = available.is_err();
            if bytes_tx.send(available).is_err() || failed {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut previous = 0;
    let mut stable = 0;
    loop {
        let available = bytes_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("stdout pipe occupancy query timed out")
            .unwrap();
        if available >= 4096 && available == previous {
            stable += 1;
            if stable == 5 {
                break;
            }
        } else {
            stable = 0;
        }
        previous = available;
        assert!(
            Instant::now() < deadline,
            "stdout did not reach backpressure"
        );
    }

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 7 }
        })
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let ids = cleanup
            .pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let script = format!("if (Get-Process -Id {ids} -ErrorAction SilentlyContinue) {{ exit 1 }} else {{ exit 0 }}");
        if std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "cancelled PowerShell tree remains alive: {:?}",
            cleanup.pids
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    cleanup.pids.clear();
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = cleanup.child.try_wait().unwrap() {
            assert!(!status.success(), "undrained output must fail closed");
            assert!(std::fs::read_to_string(&stderr_path)
                .unwrap_or_default()
                .contains("stdout did not drain"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "MCP server retained a blocked Windows stdio thread"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(bytes_rx);
    drop(stdout);
}

#[cfg(windows)]
#[test]
fn windows_broken_stdout_cancels_open_stdin_and_reaps_the_running_shell() {
    use std::io::{Read, Write};
    use std::process::{Child, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct Cleanup {
        child: Child,
        pid: Option<u32>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(pid) = self.pid {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    let temp = tempfile::tempdir().unwrap();
    write_user_permissions(&temp, &["PowerShell".to_owned()]);
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let stderr_path = temp.path().join("stderr-broken.log");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .current_dir(&project)
        .env("HOME", temp.path())
        .env("LINGXI_CONFIG_DIR", temp.path().join("config"))
        .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
        .env("DISABLE_AUTOUPDATER", "1")
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(std::fs::File::create(&stderr_path).unwrap()))
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut cleanup = Cleanup { child, pid: None };
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "windows-broken-test", "version": "1" } }
        })
    )
    .unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            if stdout.read_exact(&mut byte).is_err() {
                return;
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                let _ = ready_tx.send((stdout, line));
                return;
            }
        }
    });
    let (stdout, line) = ready_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("MCP initialize");
    assert_eq!(
        serde_json::from_slice::<Value>(&line).unwrap()["result"]["protocolVersion"],
        "2025-06-18"
    );
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        })
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "PowerShell", "arguments": {
                "command": "$PID | Set-Content -Encoding ascii -Path 'started-broken'; Start-Sleep -Seconds 30"
            }}
        })
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(project.join("started-broken")) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                cleanup.pid = Some(pid);
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "PowerShell did not start: {}",
            std::fs::read_to_string(&stderr_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stdout);
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 8, "method": "ping"
        })
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let script = format!(
            "if (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ exit 1 }} else {{ exit 0 }}",
            cleanup.pid.unwrap()
        );
        if std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PowerShell survived broken stdout"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    cleanup.pid = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = cleanup.child.try_wait().unwrap() {
            assert!(!status.success());
            assert!(std::fs::read_to_string(&stderr_path)
                .unwrap_or_default()
                .contains("write stdout:"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "server did not cancel its blocked stdin read"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Retain the parent's stdin handle until after the server has exited.
    drop(stdin);
}

mod bounded_server_tests {
    use serde_json::Value;
    use std::io;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
    use tokio::process::{Child, Command};
    use tokio::task::JoinHandle;
    use tokio::time::{timeout, timeout_at, Instant};

    const MAX_CAPTURE: usize = 1024 * 1024;
    const INPUT: &[u8] = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{}}\n",
    ).as_bytes();

    async fn read_limited(mut reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, String> {
        let mut result = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let count = reader
                .read(&mut chunk)
                .await
                .map_err(|error| error.to_string())?;
            if count == 0 {
                return Ok(result);
            }
            if count > MAX_CAPTURE.saturating_sub(result.len()) {
                return Err(format!("output exceeded {MAX_CAPTURE} bytes"));
            }
            result.extend_from_slice(&chunk[..count]);
        }
    }

    async fn finish_task<T>(
        task: &mut JoinHandle<Result<T, String>>,
        deadline: Instant,
        name: &str,
    ) -> Result<T, String> {
        match timeout_at(deadline, &mut *task).await {
            Ok(Ok(result)) => result.map_err(|error| format!("{name}: {error}")),
            Ok(Err(error)) => Err(format!("{name} task failed: {error}")),
            Err(_) => {
                task.abort();
                // Aborting closes the async pipe. Even the cancellation receipt
                // has its own bound; a stuck task is reported, not awaited forever.
                if timeout(Duration::from_secs(1), &mut *task).await.is_err() {
                    return Err(format!(
                        "{name} exceeded its deadline and cancellation was unverified"
                    ));
                }
                Err(format!("{name} exceeded its deadline"))
            }
        }
    }

    async fn stop_and_reap(child: &mut Child) -> Result<(), String> {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(error) => return Err(format!("child status is unverified: {error}")),
        }
        if let Err(error) = child.start_kill() {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                _ => return Err(format!("could not kill owned child: {error}")),
            }
        }
        match timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(format!("could not reap owned child: {error}")),
            Err(_) => Err("owned child did not reap before the deadline".into()),
        }
    }

    struct Server {
        _home: tempfile::TempDir,
        child: Child,
        stdin: JoinHandle<Result<(), String>>,
        stdout: JoinHandle<Result<Vec<u8>, String>>,
        stderr: JoinHandle<Result<Vec<u8>, String>>,
        status: Option<std::process::ExitStatus>,
    }

    /// Four real servers share a deadline. Outputs and input are drained by
    /// cancellable tasks with caps; child timeout and incomplete reaping fail.
    #[tokio::test]
    async fn concurrent_servers_complete_protocol_and_shutdown() {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut servers = Vec::new();
        let mut errors = Vec::new();
        for _ in 0..4 {
            let setup = || -> io::Result<_> {
                let home = tempfile::tempdir()?;
                let project = home.path().join("project");
                let config = home.path().join("config");
                std::fs::create_dir_all(&project)?;
                std::fs::create_dir_all(&config)?;
                let child = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
                    .current_dir(project)
                    .env("HOME", home.path())
                    .env("LINGXI_CONFIG_DIR", config)
                    .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
                    .env("DISABLE_AUTOUPDATER", "1")
                    .args(["mcp", "serve"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()?;
                Ok((home, child))
            };
            let (home, mut child) = match setup() {
                Ok(value) => value,
                Err(error) => {
                    errors.push(format!("could not start server: {error}"));
                    break;
                }
            };
            let (Some(mut stdin), Some(stdout), Some(stderr)) =
                (child.stdin.take(), child.stdout.take(), child.stderr.take())
            else {
                errors.push("a piped server stream was unavailable".into());
                if let Err(error) = stop_and_reap(&mut child).await {
                    errors.push(error);
                }
                break;
            };
            servers.push(Server {
                _home: home,
                child,
                stdin: tokio::spawn(async move {
                    stdin
                        .write_all(INPUT)
                        .await
                        .map_err(|error| error.to_string())?;
                    stdin.shutdown().await.map_err(|error| error.to_string())
                }),
                stdout: tokio::spawn(read_limited(stdout)),
                stderr: tokio::spawn(read_limited(stderr)),
                status: None,
            });
        }
        if servers.len() != 4 {
            for server in &mut servers {
                if let Err(error) = stop_and_reap(&mut server.child).await {
                    errors.push(error);
                }
            }
        }
        for server in &mut servers {
            match timeout_at(deadline, server.child.wait()).await {
                Ok(Ok(status)) => server.status = Some(status),
                Ok(Err(error)) => {
                    errors.push(format!("server exit status is unverified: {error}"));
                    if let Err(error) = stop_and_reap(&mut server.child).await {
                        errors.push(error);
                    }
                }
                Err(_) => {
                    errors.push("server did not exit before the deadline".into());
                    if let Err(error) = stop_and_reap(&mut server.child).await {
                        errors.push(error);
                    }
                }
            }
        }
        for server in &mut servers {
            if let Err(error) = finish_task(&mut server.stdin, deadline, "stdin").await {
                errors.push(error);
            }
            let stdout = finish_task(&mut server.stdout, deadline, "stdout").await;
            let stderr = finish_task(&mut server.stderr, deadline, "stderr").await;
            if let Err(error) = &stdout {
                errors.push(error.clone());
            }
            if let Err(error) = &stderr {
                errors.push(error.clone());
            }
            if let (Some(status), Ok(stdout), Ok(stderr)) = (server.status, stdout, stderr) {
                if !status.success() {
                    errors.push(format!(
                        "server failed: {status}; stderr: {}",
                        String::from_utf8_lossy(&stderr)
                    ));
                    continue;
                }
                let frames = std::str::from_utf8(&stdout)
                    .map_err(|error| error.to_string())
                    .and_then(|text| {
                        text.lines()
                            .map(serde_json::from_str::<Value>)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|error| error.to_string())
                    });
                match frames {
                    Ok(frames)
                        if frames.len() == 3
                            && frames[0]["id"] == 1
                            && frames[1]
                                == serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
                            && frames[2]["id"] == 3
                            && frames[2]["result"]["tools"]
                                .as_array()
                                .is_some_and(|tools| !tools.is_empty()) => {}
                    Ok(frames) => errors.push(format!("unexpected protocol frames: {frames:?}")),
                    Err(error) => errors.push(format!("invalid protocol output: {error}")),
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("; "));
    }

    /// A second owned process retains the same output writer after the first
    /// child exits, reproducing the inherited-pipe lifetime that must fail.
    #[cfg(unix)]
    #[tokio::test]
    async fn inherited_output_handle_is_bounded_and_reported() {
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;

        let (reader, writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let holder_stdout: OwnedFd = writer.try_clone().unwrap().into();
        let first_stdout: OwnedFd = writer.try_clone().unwrap().into();
        let mut holder = Command::new("/bin/sleep")
            .arg("5")
            .stdout(Stdio::from(holder_stdout))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let first = Command::new("/usr/bin/true")
            .stdout(Stdio::from(first_stdout))
            .kill_on_drop(true)
            .spawn();
        let mut first = match first {
            Ok(first) => first,
            Err(error) => {
                let cleanup = stop_and_reap(&mut holder).await;
                panic!("first child did not start: {error}; holder cleanup: {cleanup:?}");
            }
        };
        drop(writer);
        let status = timeout(Duration::from_secs(2), first.wait()).await;
        let first_cleanup = stop_and_reap(&mut first).await;
        let mut capture = tokio::spawn(read_limited(
            tokio::net::UnixStream::from_std(reader).unwrap(),
        ));
        let result = finish_task(
            &mut capture,
            Instant::now() + Duration::from_millis(150),
            "stdout",
        )
        .await;
        let cleanup = stop_and_reap(&mut holder).await;
        assert!(
            matches!(status, Ok(Ok(status)) if status.success()),
            "first child did not exit cleanly"
        );
        assert!(
            first_cleanup.is_ok(),
            "first child cleanup: {first_cleanup:?}"
        );
        assert!(
            matches!(result, Err(ref error) if error.contains("exceeded its deadline")),
            "{result:?}"
        );
        assert!(cleanup.is_ok(), "holder cleanup: {cleanup:?}");
        assert!(
            holder.try_wait().unwrap().is_some(),
            "holder was not reaped"
        );
    }

    #[tokio::test]
    async fn capture_rejects_output_above_the_limit() {
        let (mut writer, reader) = tokio::io::duplex(8192);
        let mut writer_task = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; MAX_CAPTURE + 1])
                .await
                .map_err(|error| error.to_string())
        });
        let mut capture = tokio::spawn(read_limited(reader));
        let result = finish_task(
            &mut capture,
            Instant::now() + Duration::from_secs(3),
            "stdout",
        )
        .await;
        let writer_result = finish_task(
            &mut writer_task,
            Instant::now() + Duration::from_secs(2),
            "writer",
        )
        .await;
        assert!(
            !matches!(writer_result, Err(ref error) if error.contains("deadline")),
            "{writer_result:?}"
        );
        assert!(
            matches!(result, Err(ref error) if error.contains("output exceeded")),
            "{result:?}"
        );
    }
}
