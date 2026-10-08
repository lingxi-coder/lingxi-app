//! Process adapter regressions: every headless format resolves session source
//! before a slash command can execute or an API request can start.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn every_print_format_rejects_a_missing_resume_session_before_dispatch() {
    let session_id = "00000000-0000-4000-8000-000000000001";
    for format in ["text", "json", "stream-json"] {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::cargo_bin("lingxi-cli").unwrap();
        command
            .env("LINGXI_CONFIG_DIR", home.path())
            .env("ANTHROPIC_API_KEY", "sk-test-fake")
            .args(["--print", "--verbose", "--output-format", format])
            .args(["--resume", session_id, "/version"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains(format!(
                "No conversation found with session ID: {session_id}"
            )))
            .stdout(predicate::str::contains("lingxi-cli ").not());
    }
}

#[test]
fn every_print_format_rejects_continue_when_the_project_has_no_session() {
    for format in ["text", "json", "stream-json"] {
        let home = tempfile::tempdir().unwrap();
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .env("LINGXI_CONFIG_DIR", home.path())
            .env("ANTHROPIC_API_KEY", "sk-test-fake")
            .args(["--print", "--verbose", "--output-format", format])
            .args(["--continue", "/version"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains(
                "No conversation found to continue",
            ))
            .stdout(predicate::str::contains("lingxi-cli ").not());
    }
}

#[cfg(unix)]
mod process_io {
    use std::io::{Read, Write};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    struct HeadlessChild {
        child: Child,
        _home: tempfile::TempDir,
        output: Vec<u8>,
    }

    impl HeadlessChild {
        fn spawn() -> Self {
            let home = tempfile::tempdir().unwrap();
            let child = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
                .current_dir(home.path())
                .env("LINGXI_CONFIG_DIR", home.path())
                .env("ANTHROPIC_API_KEY", "sk-test-fake")
                .args([
                    "--print",
                    "--verbose",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            for fd in [
                std::os::fd::AsFd::as_fd(child.stdin.as_ref().unwrap()),
                std::os::fd::AsFd::as_fd(child.stdout.as_ref().unwrap()),
            ] {
                let flags = rustix::fs::fcntl_getfl(fd).unwrap();
                rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
            }
            Self {
                child,
                _home: home,
                output: Vec::new(),
            }
        }

        fn send(&mut self, frame: serde_json::Value) {
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut sent = 0;
            while sent < bytes.len() {
                match self.child.stdin.as_mut().unwrap().write(&bytes[sent..]) {
                    Ok(0) => panic!("child stdin closed"),
                    Ok(count) => sent += count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "child stopped consuming control input"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("write child control input: {error}"),
                }
            }
        }

        fn response(&mut self, request_id: &str) -> serde_json::Value {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                while let Some(end) = self.output.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = self.output.drain(..=end).collect();
                    let frame: serde_json::Value = serde_json::from_slice(&line).unwrap();
                    if frame["type"] == "control_response"
                        && frame["response"]["request_id"] == request_id
                    {
                        return frame;
                    }
                }
                let mut bytes = [0_u8; 8192];
                match self.child.stdout.as_mut().unwrap().read(&mut bytes) {
                    Ok(0) => panic!("child closed stdout before control receipt {request_id}"),
                    Ok(count) => self.output.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "missing control receipt {request_id}"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("read child control receipt: {error}"),
                }
            }
        }

        fn initialize(&mut self) {
            self.send(serde_json::json!({
                "type": "control_request", "request_id": "ready",
                "request": {"subtype": "initialize"},
            }));
            assert_eq!(self.response("ready")["response"]["subtype"], "success");
        }

        fn exited(&mut self) -> ExitStatus {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    return status;
                }
                assert!(
                    Instant::now() < deadline,
                    "headless process retained blocked IO after shutdown"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for HeadlessChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn end_session_exits_with_the_parent_still_holding_stdin_open() {
        let mut process = HeadlessChild::spawn();
        process.initialize();
        process.send(serde_json::json!({
            "type": "control_request", "request_id": "stop",
            "request": {"subtype": "end_session"},
        }));
        assert_eq!(process.response("stop")["response"]["subtype"], "success");
        // The parent deliberately retains ChildStdin through the exit check.
        assert!(process.child.stdin.is_some());
        assert_eq!(process.exited().code(), Some(0));
    }

    #[test]
    fn sigterm_exits_while_stdout_has_backpressure_and_stdin_is_open() {
        let mut process = HeadlessChild::spawn();
        process.initialize();
        // Echoed request IDs produce several MiB of receipts without model
        // calls. Retain the stdout reader, but never drain any of these frames.
        let padding = "x".repeat(16 * 1024);
        for index in 0..256 {
            process.send(serde_json::json!({
                "type": "control_request", "request_id": format!("full-{index}-{padding}"),
                "request": {"subtype": "get_binary_version"},
            }));
        }
        let stdout = process.child.stdout.as_ref().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut previous = 0;
        let mut stable = 0;
        loop {
            let available = rustix::io::ioctl_fionread(stdout).unwrap();
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
                "stdout never reached stable backpressure"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(process.child.try_wait().unwrap().is_none());
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(process.child.id()).unwrap()),
            nix::sys::signal::Signal::SIGTERM,
        )
        .unwrap();
        assert_eq!(process.exited().code(), Some(143));
    }

    #[test]
    fn normal_exit_restores_flags_when_stdout_and_stderr_share_one_description() {
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        let home = tempfile::tempdir().unwrap();
        let (output, mut reader) = UnixStream::pair().unwrap();
        let flags = rustix::fs::fcntl_getfl(&output).unwrap();
        let stdout: OwnedFd = output.try_clone().unwrap().into();
        let stderr: OwnedFd = output.try_clone().unwrap().into();
        let child = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
            .current_dir(home.path())
            .env("LINGXI_CONFIG_DIR", home.path())
            .env("ANTHROPIC_API_KEY", "sk-test-fake")
            .arg("/version")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap();
        let mut process = HeadlessChild {
            child,
            _home: home,
            output: Vec::new(),
        };
        let status = process.exited();
        assert!(status.success());
        let restored = rustix::fs::fcntl_getfl(&output).unwrap();
        // Darwin exposes FWASWRITTEN after the child writes to this shared
        // description. It is kernel state, not an F_SETFL-restorable flag:
        // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/fcntl.h
        #[cfg(target_vendor = "apple")]
        let (restored, flags) = {
            let was_written = rustix::fs::OFlags::from_bits_retain(0x0001_0000);
            (restored & !was_written, flags & !was_written)
        };
        assert_eq!(restored, flags);
        reader
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = [0_u8; 8192];
        let count = reader.read(&mut bytes).unwrap();
        assert!(String::from_utf8_lossy(&bytes[..count]).contains("lingxi-cli "));
    }

    #[test]
    fn print_mode_reads_and_writes_redirected_regular_files() {
        let home = tempfile::tempdir().unwrap();
        let mut input = tempfile::NamedTempFile::new().unwrap();
        input.write_all(b"/version\n").unwrap();
        let output = tempfile::NamedTempFile::new().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
            .current_dir(home.path())
            .env("LINGXI_CONFIG_DIR", home.path())
            .env("ANTHROPIC_API_KEY", "sk-test-fake")
            .arg("--print")
            .stdin(Stdio::from(input.reopen().unwrap()))
            .stdout(Stdio::from(output.reopen().unwrap()))
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut process = HeadlessChild {
            child,
            _home: home,
            output: Vec::new(),
        };
        assert!(process.exited().success());
        assert!(std::fs::read_to_string(output.path())
            .unwrap()
            .starts_with("lingxi-cli "));
    }
}
