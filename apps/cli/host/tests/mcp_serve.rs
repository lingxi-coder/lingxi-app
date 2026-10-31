//! Process-level coverage for MCP stdio initialization, tool listing, cancellation,
//! and scoped desktop configuration import.

use assert_cmd::Command;
use serde_json::Value;
#[cfg(not(unix))]
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

fn write_permission_settings(temp: &tempfile::TempDir, contents: &str) {
    let config = temp.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), contents).unwrap();
}

#[cfg(not(unix))]
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

#[cfg(not(unix))]
#[test]
fn serve_duplicate_tools_call_id_keeps_original_call_cancellable() {
    let temp = tempfile::tempdir().unwrap();
    write_permission_settings(
        &temp,
        r#"{"disableAutoMode":"disable","permissions":{"allow":["Bash"]}}"#,
    );
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

#[cfg(unix)]
mod cancellation {
    use super::{write_permission_settings, Value};
    use std::fs::{File, OpenOptions};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    enum Event {
        Frame(Value),
        Error(String),
        StdoutClosed,
        StderrClosed,
    }

    /// Keep both pipes open until the notification and lifecycle assertions are
    /// complete. Captured bytes and frames accompany every probe failure.
    struct Probe {
        child: Child,
        temp: tempfile::TempDir,
        gate: File,
        shell_pid: Option<i32>,
        events: mpsc::Receiver<Event>,
        stdout: Arc<Mutex<Vec<u8>>>,
        stderr: Arc<Mutex<Vec<u8>>>,
        readers: Vec<JoinHandle<()>>,
        frames: Vec<Value>,
        stdout_closed: bool,
        stderr_closed: bool,
        background_enabled: bool,
    }

    impl Probe {
        fn start(disable_background: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let project = temp.path().join("project");
            let config = temp.path().join("config");
            std::fs::create_dir_all(&project).unwrap();
            std::fs::create_dir_all(&config).unwrap();
            write_permission_settings(
                &temp,
                r#"{"disableAutoMode":"disable","permissions":{"allow":["Bash"]}}"#,
            );
            let gate_path = project.join("mcp-gate");
            assert!(Command::new("mkfifo")
                .args(["-m", "600"])
                .arg(&gate_path)
                .status()
                .unwrap()
                .success());
            // Holding both ends prevents EOF until the test explicitly releases
            // or cancels the shell's builtin read.
            let gate = OpenOptions::new()
                .read(true)
                .write(true)
                .open(gate_path)
                .unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"));
            command
                .current_dir(&project)
                .env("HOME", temp.path())
                .env("LINGXI_CONFIG_DIR", config)
                .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
                .env("DISABLE_AUTOUPDATER", "1")
                .args(["mcp", "serve"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if disable_background {
                command.env("LINGXI_DISABLE_BACKGROUND_TASKS", "1");
            } else {
                command.env_remove("LINGXI_DISABLE_BACKGROUND_TASKS");
            }
            let mut child = command.spawn().unwrap();
            let stdout_pipe = child.stdout.take().unwrap();
            let stderr_pipe = child.stderr.take().unwrap();
            let stdout = Arc::new(Mutex::new(Vec::new()));
            let stderr = Arc::new(Mutex::new(Vec::new()));
            let (tx, events) = mpsc::channel();
            let stdout_copy = stdout.clone();
            let stdout_tx = tx.clone();
            let stdout_reader = thread::spawn(move || {
                let mut reader = BufReader::new(stdout_pipe);
                loop {
                    let mut line = Vec::new();
                    match reader.read_until(b'\n', &mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            stdout_copy.lock().unwrap().extend_from_slice(&line);
                            match serde_json::from_slice(&line) {
                                Ok(frame) => {
                                    if stdout_tx.send(Event::Frame(frame)).is_err() {
                                        return;
                                    }
                                }
                                Err(error) => {
                                    let _ = stdout_tx.send(Event::Error(format!(
                                        "invalid JSON-RPC stdout frame: {error}"
                                    )));
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            let _ = stdout_tx.send(Event::Error(format!("read stdout: {error}")));
                            return;
                        }
                    }
                }
                let _ = stdout_tx.send(Event::StdoutClosed);
            });
            let stderr_copy = stderr.clone();
            let stderr_reader = thread::spawn(move || {
                let mut reader = stderr_pipe;
                let mut bytes = [0_u8; 8192];
                loop {
                    match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => stderr_copy
                            .lock()
                            .unwrap()
                            .extend_from_slice(&bytes[..count]),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            let _ = tx.send(Event::Error(format!("read stderr: {error}")));
                            return;
                        }
                    }
                }
                let _ = tx.send(Event::StderrClosed);
            });
            Self {
                child,
                temp,
                gate,
                shell_pid: None,
                events,
                stdout,
                stderr,
                readers: vec![stdout_reader, stderr_reader],
                frames: Vec::new(),
                stdout_closed: false,
                stderr_closed: false,
                background_enabled: !disable_background,
            }
        }

        fn capture(&mut self, event: Event) {
            match event {
                Event::Frame(frame) => self.frames.push(frame),
                Event::Error(error) => self.fail(&error),
                Event::StdoutClosed => self.stdout_closed = true,
                Event::StderrClosed => self.stderr_closed = true,
            }
        }

        fn fail(&mut self, message: &str) -> ! {
            while let Ok(event) = self.events.try_recv() {
                match event {
                    Event::Frame(frame) => self.frames.push(frame),
                    Event::StdoutClosed => self.stdout_closed = true,
                    Event::StderrClosed => self.stderr_closed = true,
                    Event::Error(_) => {}
                }
            }
            let status = self.child.try_wait();
            let stdout = String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned();
            let stderr = String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned();
            panic!(
                "{message}; background_enabled={}; shell_pid={:?}; status={status:?}\nframes={:#?}\nstdout={stdout}\nstderr={stderr}",
                self.background_enabled, self.shell_pid, self.frames,
            );
        }

        fn require(&mut self, condition: bool, message: impl AsRef<str>) {
            if !condition {
                self.fail(message.as_ref());
            }
        }

        fn send(&mut self, frame: Value) {
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            let result = self.child.stdin.as_mut().unwrap().write_all(&bytes);
            if let Err(error) = result {
                self.fail(&format!("write JSON-RPC input: {error}"));
            }
        }

        fn receive_until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(frame) = self.frames.iter().find(|frame| predicate(frame)) {
                    return frame.clone();
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.events.recv_timeout(remaining) {
                    Ok(event) => self.capture(event),
                    Err(error) => self.fail(&format!("waiting for JSON-RPC frame: {error}")),
                }
                if self.stdout_closed {
                    self.fail("MCP stdout closed before the expected frame");
                }
            }
        }

        fn initialize(&mut self) {
            self.send(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"}}
            }));
            let frame = self.receive_until(|frame| frame["id"] == 1);
            self.require(
                frame["result"]["protocolVersion"] == "2025-06-18",
                "initialize response was invalid",
            );
            self.send(serde_json::json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
            self.send(
                serde_json::json!({"jsonrpc":"2.0", "id":2, "method":"tools/list", "params":{}}),
            );
            let list = self.receive_until(|frame| frame["id"] == 2);
            let enabled = list["result"]["tools"]
                .as_array()
                .and_then(|tools| tools.iter().find(|tool| tool["name"] == "Bash"))
                .and_then(|tool| tool["inputSchema"]["properties"].as_object())
                .map(|properties| properties.contains_key("run_in_background"));
            self.require(
                enabled == Some(self.background_enabled),
                format!("unexpected Bash background schema: {enabled:?}"),
            );
        }

        fn call(&mut self) {
            self.send(serde_json::json!({
                "jsonrpc":"2.0", "id":7, "method":"tools/call",
                "params": {"name":"Bash", "arguments": {
                    "command":"printf '%s\\n' $$ > mcp-pid; : > mcp-ready; IFS= read -r _ < mcp-gate; : > mcp-complete"
                }, "_meta":{"progressToken":"dup-cancel"}}
            }));
            let ready = self.temp.path().join("project/mcp-ready");
            let pid_path = self.temp.path().join("project/mcp-pid");
            let deadline = Instant::now() + Duration::from_secs(20);
            while !ready.exists() {
                match self.events.recv_timeout(Duration::from_millis(20)) {
                    Ok(event) => self.capture(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(error) => self.fail(&format!("waiting for shell readiness: {error}")),
                }
                if self.frames.iter().any(|frame| frame["id"] == 7) {
                    self.fail("original call ended before its shell became ready");
                }
                if self.stdout_closed || Instant::now() >= deadline {
                    self.fail("shell did not reach its FIFO readiness marker");
                }
            }
            let pid = std::fs::read_to_string(pid_path)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok());
            self.require(
                pid.is_some_and(|pid| {
                    pid > 1 && pid as u32 != self.child.id() && pid as u32 != std::process::id()
                }),
                format!("invalid owned shell PID: {pid:?}"),
            );
            self.shell_pid = pid;
        }

        fn ping(&mut self, id: u32) {
            self.send(serde_json::json!({"jsonrpc":"2.0", "id":id, "method":"ping"}));
            let frame = self.receive_until(|frame| frame["id"] == id);
            self.require(frame["result"] == serde_json::json!({}), "ping failed");
        }

        fn shell_gone(&mut self) {
            let pid = nix::unistd::Pid::from_raw(self.shell_pid.unwrap());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match nix::sys::signal::kill(pid, None) {
                    Err(nix::errno::Errno::ESRCH) => {
                        self.shell_pid = None;
                        return;
                    }
                    Ok(()) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                    result => self.fail(&format!("owned shell was not reaped: {result:?}")),
                }
            }
        }

        fn finish(&mut self) -> ExitStatus {
            self.child.stdin.take();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut status = None;
            while status.is_none() || !self.stdout_closed || !self.stderr_closed {
                if status.is_none() {
                    status = self.child.try_wait().unwrap();
                }
                match self.events.recv_timeout(Duration::from_millis(20)) {
                    Ok(event) => self.capture(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {}
                }
                if Instant::now() >= deadline {
                    self.fail("MCP server or its output readers did not stop after EOF");
                }
            }
            for reader in self.readers.drain(..) {
                reader.join().unwrap();
            }
            status.unwrap()
        }

        fn check_frames(&mut self, duplicate: bool) {
            let results = self
                .frames
                .iter()
                .filter(|f| f["id"] == 7 && f.get("result").is_some())
                .count();
            let errors = self
                .frames
                .iter()
                .filter(|f| f["id"] == 7 && f.get("error").is_some())
                .count();
            self.require(
                results == 1 && errors == usize::from(duplicate),
                format!("unexpected original/duplicate frame counts: {results}/{errors}"),
            );
            let tokens_match = self
                .frames
                .iter()
                .filter(|f| f["method"] == "notifications/progress")
                .all(|f| f["params"]["progressToken"] == "dup-cancel");
            self.require(tokens_match, "progress token changed");
            let stderr = String::from_utf8_lossy(&self.stderr.lock().unwrap()).to_string();
            self.require(
                !stderr.contains(r#""jsonrpc":"2.0""#),
                "protocol frame leaked to stderr",
            );
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            // Release only this test's gate and give the live CLI supervisor a
            // chance to reap the shell. A recorded PID is never used to signal
            // a process: it could have been reused after the shell exited.
            let _ = self.gate.write_all(b"release\n");
            let _ = self.gate.flush();
            if let Some(pid) = self.shell_pid {
                let pid = nix::unistd::Pid::from_raw(pid);
                let deadline = Instant::now() + Duration::from_secs(2);
                while nix::sys::signal::kill(pid, None).is_ok() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            for reader in self.readers.drain(..) {
                let _ = reader.join();
            }
        }
    }

    fn cancellation_probe(disable_background: bool) {
        let mut probe = Probe::start(disable_background);
        probe.initialize();
        probe.call();
        probe.send(serde_json::json!({"jsonrpc":"2.0", "id":7, "method":"tools/call", "params":{"name":"Bash", "arguments":{"command":"true"}}}));
        let duplicate =
            probe.receive_until(|frame| frame["id"] == 7 && frame.get("error").is_some());
        probe.require(
            duplicate["error"]["code"] == -32600
                && duplicate["error"]["message"] == "Duplicate request id",
            "duplicate call was not rejected",
        );
        probe.ping(3);
        probe.require(
            !probe
                .frames
                .iter()
                .any(|frame| frame["id"] == 7 && frame.get("result").is_some()),
            "original completed before cancellation",
        );
        probe.send(serde_json::json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":7}}));
        probe.ping(4);
        let result = probe.receive_until(|frame| frame["id"] == 7 && frame.get("result").is_some());
        probe.require(
            result["result"]["isError"] == true,
            "cancelled original must return an error",
        );
        probe.require(
            result["result"]["content"][0]["text"] == "aborted",
            "started shell must return the dispatcher abort text",
        );
        probe.shell_gone();
        probe.require(
            !probe.temp.path().join("project/mcp-complete").exists(),
            "cancelled shell reached its completion marker",
        );
        probe.ping(5);
        let status = probe.finish();
        probe.require(
            status.success(),
            "MCP server failed after notification cancellation",
        );
        probe.check_frames(true);
    }

    #[test]
    fn serve_duplicate_tools_call_id_keeps_original_call_cancellable() {
        cancellation_probe(false);
    }

    #[test]
    fn serve_duplicate_tools_call_id_cancels_foreground_only_shell() {
        cancellation_probe(true);
    }

    #[test]
    fn serve_wrong_id_does_not_cancel_blocked_call() {
        let mut probe = Probe::start(false);
        probe.initialize();
        probe.call();
        probe.send(serde_json::json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":8}}));
        probe.ping(3);
        probe.require(
            !probe
                .frames
                .iter()
                .any(|frame| frame["id"] == 7 && frame.get("result").is_some()),
            "wrong-ID notification completed the original call",
        );
        probe.gate.write_all(b"release\n").unwrap();
        probe.gate.flush().unwrap();
        let result = probe.receive_until(|frame| frame["id"] == 7 && frame.get("result").is_some());
        probe.require(
            result["result"]["isError"] == false,
            "released shell did not complete successfully",
        );
        probe.require(
            probe.temp.path().join("project/mcp-complete").exists(),
            "released shell did not reach its completion marker",
        );
        probe.shell_gone();
        probe.ping(4);
        let status = probe.finish();
        probe.require(
            status.success(),
            "MCP server failed after the no-cancel control",
        );
        probe.check_frames(false);
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
        config.join("settings.json"),
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
        serde_json::from_slice(&std::fs::read(config.join("settings.json")).unwrap()).unwrap();
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
