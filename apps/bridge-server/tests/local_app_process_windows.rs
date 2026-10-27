#![cfg(windows)]

//! Process-level checks use the real bridge binary and Windows pipes/Jobs.
//! Harness separately tests the suspended-spawn boundary and Job failures.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

const OWNER: &str = "real-helper-test";

struct Helper {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bridge-server"))
            .arg("--local-app-process")
            .env("HOST_ONLY_SENTINEL", "must-not-reach-payload")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn bridge helper");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take().expect("stderr pipe");
        thread::spawn(move || {
            let _ = BufReader::new(stderr).read_to_end(&mut Vec::new());
        });
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, frame: Value) {
        let input = self.stdin.as_mut().expect("open helper stdin");
        serde_json::to_writer(&mut *input, &frame).expect("write control frame");
        input.write_all(b"\n").expect("control newline");
        input.flush().expect("flush control frame");
    }

    fn frames(&mut self) -> Receiver<Result<Value, String>> {
        let stdout = self.stdout.take().expect("stdout pipe");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) if line.len() <= 16 * 1024 => {
                        let frame = serde_json::from_slice(&line).map_err(|e| e.to_string());
                        if tx.send(frame).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {
                        let _ = tx.send(Err("oversized frame".into()));
                        break;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        });
        rx
    }

    fn close_input(&mut self) {
        self.stdin.take();
    }

    fn wait(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait helper") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "helper did not exit within {timeout:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stdin.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .expect("SystemRoot")
}

fn start(command_line: &str) -> Value {
    let root = system_root();
    json!({
        "version": 1, "type": "start", "id": OWNER,
        "command": root.join("System32/cmd.exe").to_str().expect("cmd path"),
        "args": ["/D", "/C", command_line],
        "cwd": std::env::temp_dir().to_str().expect("temporary directory"),
        "env": {
            "SystemRoot": root.to_str().expect("system root"),
            "PATH": root.join("System32").to_str().expect("system path"),
            "EXPECTED_VALUE": "child-only"
        }
    })
}

fn next(rx: &Receiver<Result<Value, String>>) -> Value {
    let frame = rx
        .recv_timeout(Duration::from_secs(25))
        .expect("helper frame deadline")
        .expect("valid frame");
    assert_eq!(frame["version"], 1);
    assert_eq!(frame["id"], OWNER);
    frame
}

fn bytes(frame: &Value) -> Vec<u8> {
    STANDARD
        .decode(frame["data"].as_str().expect("base64 data"))
        .expect("decode payload")
}

/// The fixture publishes a PID while it is alive. A separate process opens a
/// kernel process handle before EOF, then waits on that same object; this is
/// test observation only, never a production PID-based termination mechanism.
fn watch_payload(path: &Path) -> Child {
    let deadline = Instant::now() + Duration::from_secs(15);
    let pid: u32 = loop {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(value) = contents.trim().parse() {
                break value;
            }
        }
        assert!(
            Instant::now() < deadline,
            "payload did not publish its identity"
        );
        thread::sleep(Duration::from_millis(25));
    };
    let script = format!(
        "$p = [System.Diagnostics.Process]::GetProcessById({pid}); \
         $h = $p.SafeHandle; if ($p.HasExited) {{ exit 4 }}; \
         [Console]::Out.WriteLine('WATCHING'); [Console]::Out.Flush(); \
         if (-not $p.WaitForExit(60000)) {{ exit 5 }}; $h.Dispose()"
    );
    let mut watcher =
        Command::new(system_root().join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn handle watcher");
    let stdout = watcher.stdout.take().expect("watcher stdout");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = tx.send(result);
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(15))
            .expect("watcher did not open process handle")
            .expect("watcher stdout")
            .trim(),
        "WATCHING"
    );
    watcher
}

fn wait_watcher(watcher: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(status) = watcher.try_wait().expect("wait for process handle") {
            assert!(status.success(), "watched payload did not exit: {status}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "payload process handle was not signaled"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn separate_streams_and_explicit_environment_are_delivered_before_verified() {
    let mut helper = Helper::spawn();
    let rx = helper.frames();
    helper.send(start("echo OUT_MARKER & echo ERR_MARKER 1>&2 & echo VALUE-%EXPECTED_VALUE% & if defined HOST_ONLY_SENTINEL (echo LEAKED) else (echo CLEAN)"));
    assert_eq!(next(&rx)["type"], "started");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut saw_leader = false;
    loop {
        let frame = next(&rx);
        match frame["type"].as_str().unwrap() {
            "output" => match frame["stream"].as_str().unwrap() {
                "stdout" => stdout.extend(bytes(&frame)),
                "stderr" => stderr.extend(bytes(&frame)),
                other => panic!("unexpected stream {other}"),
            },
            "leader-exit" => {
                assert_eq!(frame["code"], 0);
                saw_leader = true;
            }
            "cleanup-result" => {
                assert_eq!(frame["attemptId"], Value::Null);
                assert_eq!(frame["verified"], true);
                break;
            }
            other => panic!("unexpected control frame {other}"),
        }
    }
    assert!(saw_leader);
    let stdout = String::from_utf8(stdout).unwrap();
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(stdout.contains("OUT_MARKER"));
    assert!(stdout.contains("VALUE-child-only"));
    assert!(stdout.contains("CLEAN"));
    assert!(!stdout.contains("LEAKED"));
    assert!(stderr.contains("ERR_MARKER"));
    helper.close_input();
    assert!(helper.wait(Duration::from_secs(25)).success());
}

#[test]
fn spawn_error_with_empty_job_has_separate_startup_cleanup_proof() {
    let mut helper = Helper::spawn();
    let rx = helper.frames();
    let dir = tempfile::tempdir().expect("isolated directory");
    let mut request = start("echo unused");
    request["command"] = json!(dir.path().join("missing.exe").to_str().unwrap());
    helper.send(request);
    let frame = next(&rx);
    assert_eq!(frame["type"], "start-error");
    assert_eq!(frame["noChildStarted"], false);
    let proof = next(&rx);
    assert_eq!(proof["type"], "startup-cleanup");
    assert_eq!(proof["verified"], true);
    assert!(helper.wait(Duration::from_secs(25)).success());
    while let Ok(frame) = rx.recv_timeout(Duration::from_secs(1)) {
        let frame = frame.expect("valid frame");
        assert_ne!(frame["type"], "leader-exit");
        assert_ne!(frame["type"], "output");
        assert!(frame["type"] != "cleanup-result" || frame["verified"] != true);
    }
}

#[test]
fn each_stop_attempt_gets_its_own_verified_receipt() {
    let mut helper = Helper::spawn();
    let rx = helper.frames();
    helper.send(start("echo CHILD_READY & ping -n 30 127.0.0.1 >NUL"));
    assert_eq!(next(&rx)["type"], "started");
    let mut ready = Vec::new();
    while !String::from_utf8_lossy(&ready).contains("CHILD_READY") {
        let event = next(&rx);
        assert_eq!(event["type"], "output");
        ready.extend(bytes(&event));
    }
    for sequence in 1..=130 {
        let attempt = format!("{OWNER}:{sequence}");
        helper.send(json!({ "version": 1, "type": "stop", "id": OWNER, "attemptId": &attempt }));
        loop {
            let event = next(&rx);
            if event["type"] == "cleanup-result"
                && event["attemptId"].as_str() == Some(attempt.as_str())
            {
                assert_eq!(event["verified"], true);
                break;
            }
        }
    }
    helper.close_input();
    assert!(helper.wait(Duration::from_secs(25)).success());
}

#[test]
fn stdin_eof_stops_a_running_owned_tree() {
    let mut helper = Helper::spawn();
    let rx = helper.frames();
    helper.send(start("echo CHILD_READY & ping -n 30 127.0.0.1 >NUL"));
    assert_eq!(next(&rx)["type"], "started");
    let mut ready = Vec::new();
    while !String::from_utf8_lossy(&ready).contains("CHILD_READY") {
        let event = next(&rx);
        assert_eq!(event["type"], "output");
        ready.extend(bytes(&event));
    }
    helper.close_input();
    let mut saw_leader = false;
    loop {
        let event = next(&rx);
        match event["type"].as_str().unwrap() {
            "output" => {}
            "leader-exit" => saw_leader = true,
            "cleanup-result" => {
                assert_eq!(event["verified"], true);
                break;
            }
            other => panic!("unexpected frame {other}"),
        }
    }
    assert!(saw_leader);
    assert!(helper.wait(Duration::from_secs(25)).success());
}

#[test]
fn eof_still_terminates_when_parent_does_not_drain_stdout() {
    let mut helper = Helper::spawn();
    let dir = tempfile::tempdir().expect("isolated directory");
    let leader_file = dir.path().join("leader.pid");
    let descendant_file = dir.path().join("descendant.pid");
    let mut request = start("unused");
    request["command"] = json!(system_root()
        .join("System32/WindowsPowerShell/v1.0/powershell.exe")
        .to_str()
        .unwrap());
    request["args"] = json!(["-NoProfile", "-NonInteractive", "-Command",
        "$c = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep 60' -WindowStyle Hidden -PassThru; \
         Set-Content -Encoding ascii -LiteralPath $env:DESCENDANT_PID_FILE -Value $c.Id; \
         Set-Content -Encoding ascii -LiteralPath $env:LEADER_PID_FILE -Value $PID; \
         while ($true) { [Console]::Out.WriteLine(('X' * 4096)) }"]);
    request["env"]["LEADER_PID_FILE"] = json!(leader_file.to_str().unwrap());
    request["env"]["DESCENDANT_PID_FILE"] = json!(descendant_file.to_str().unwrap());
    helper.send(request);
    let mut leader = watch_payload(&leader_file);
    let mut descendant = watch_payload(&descendant_file);
    // PeekNamedPipe can block synchronously: isolate it from the bounded test
    // thread, and keep the read end open without consuming the output.
    let stdout = Arc::new(helper.stdout.take().expect("stdout pipe"));
    let probe = stdout.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut prior = 0;
        let mut stable = 0;
        let result = loop {
            let count = match platform_windows::stdio_cancel::pipe_bytes_available(&*probe) {
                Ok(count) => count,
                Err(error) => break Err(error.to_string()),
            };
            stable = if count >= 4096 && count == prior {
                stable + 1
            } else {
                0
            };
            if stable >= 10 {
                break Ok(());
            }
            if Instant::now() >= deadline {
                break Err("stdout did not reach backpressure".into());
            }
            prior = count;
            thread::sleep(Duration::from_millis(50));
        };
        let _ = tx.send(result);
    });
    rx.recv_timeout(Duration::from_secs(18))
        .expect("pipe probe blocked")
        .expect("observable stdout backpressure");
    helper.close_input();
    wait_watcher(&mut leader);
    wait_watcher(&mut descendant);
    // Both held process objects have signaled. The stream cannot be certified
    // as delivered, so the helper must report failure after Job verification.
    assert!(!helper.wait(Duration::from_secs(25)).success());
    drop(stdout);
}
