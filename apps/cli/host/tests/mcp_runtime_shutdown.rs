//! Process-level coverage for the MCP entrypoint's bounded runtime teardown.

#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn non_mcp_cli_exit_codes_are_unchanged() {
    let binary = env!("CARGO_BIN_EXE_lingxi-cli");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success(), "{help:?}");
    assert!(!help.stdout.is_empty());
    let invalid = Command::new(binary)
        .arg("--definitely-invalid-lingxi-option")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
}

#[test]
fn cli_reports_unverified_shutdown_for_a_real_blocked_mod_read() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config");
    let project = home.path().join("project");
    let plugin = config.join("plugins/cache/test/shutdown-gate/1.0.0");
    std::fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
    std::fs::create_dir_all(plugin.join("hooks")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        plugin
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
        r#"{"name":"shutdown-gate","version":"1.0.0"}"#,
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
    if (await $.fs.exists('runtime-armed')) {
      await $.fs.read('runtime-block.fifo');
    }
    return next(event);
  });
}
"#,
    )
    .unwrap();
    std::fs::write(
        config.join("settings.json"),
        r#"{"enabledPlugins":{"shutdown-gate@test":true}}"#,
    )
    .unwrap();
    std::fs::write(
        config.join("plugins/installed_plugins.json"),
        serde_json::to_vec(&json!({
            "version": 2,
            "plugins": {"shutdown-gate@test": [{
                "scope": "user", "installPath": plugin,
                "version": "1.0.0",
                "installedAt": "2026-01-01T00:00:00.000Z",
                "lastUpdated": "2026-01-01T00:00:00.000Z"
            }]}
        }))
        .unwrap(),
    )
    .unwrap();
    let project_key = std::fs::canonicalize(&project)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let mut settings = json!({"projects": {}});
    settings["projects"][project_key.as_str()] = json!({"hasTrustDialogAccepted": true});
    std::fs::write(
        config.join(branding::GLOBAL_CONFIG_FILE),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();

    let fifo = project.join("runtime-block.fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let stderr_path = home.path().join("stderr.log");
    let stderr = std::fs::File::create(&stderr_path).unwrap();
    let (stdin_read, mut stdin) = std::io::pipe().unwrap();
    let stdin_flags = stdin_read.as_fd().try_clone_to_owned().unwrap();
    let (stdout, stdout_write) = std::io::pipe().unwrap();
    let stdout_flags = stdout_write.as_fd().try_clone_to_owned().unwrap();
    let stdin_original = rustix::fs::fcntl_getfl(&stdin_flags).unwrap();
    let stdout_original = rustix::fs::fcntl_getfl(&stdout_flags).unwrap();
    assert!(!stdin_original.contains(rustix::fs::OFlags::NONBLOCK));
    assert!(!stdout_original.contains(rustix::fs::OFlags::NONBLOCK));
    let child = Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .current_dir(&project)
        .env("HOME", home.path())
        .env(branding::CONFIG_DIR_ENV, &config)
        .env("ANTHROPIC_API_KEY", "sk-ant-test-mcp-serve")
        .env("DISABLE_AUTOUPDATER", "1")
        .args(["mcp", "serve"])
        .stdin(Stdio::from(stdin_read))
        .stdout(Stdio::from(stdout_write))
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);
    let (frames_tx, frames_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if frames_tx.send(line).is_err() {
                break;
            }
        }
    });
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "runtime-shutdown-test", "version": "1"}}
    });
    writeln!(stdin, "{initialize}").unwrap();
    let first = frames_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["result"]["protocolVersion"], "2025-06-18");
    assert!(rustix::fs::fcntl_getfl(&stdin_flags)
        .unwrap()
        .contains(rustix::fs::OFlags::NONBLOCK));
    assert!(rustix::fs::fcntl_getfl(&stdout_flags)
        .unwrap()
        .contains(rustix::fs::OFlags::NONBLOCK));
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0", "method":"notifications/initialized"})
    )
    .unwrap();
    std::fs::write(project.join("runtime-armed"), b"armed").unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0", "id":2, "method":"tools/list"})
    )
    .unwrap();

    // A nonblocking write-side open succeeds only once the Mod host has opened
    // the read end. Keep it open and empty so the actual blocking read cannot
    // finish before the CLI exits; this does not depend on a sleep-based guess.
    let deadline = Instant::now() + Duration::from_secs(20);
    let writer = loop {
        match rustix::fs::open(
            &fifo,
            rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(writer) => break writer,
            Err(rustix::io::Errno::NXIO) => {
                assert!(
                    Instant::now() < deadline,
                    "Mod did not enter fs.read: {}",
                    std::fs::read_to_string(&stderr_path).unwrap()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("open controlled FIFO writer: {error}"),
        }
    };
    drop(stdin);

    let response = frames_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], 2, "{response}");
    assert_eq!(response["error"]["code"], -32603, "{response}");
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "CLI did not exit after EOF: {}",
            std::fs::read_to_string(&stderr_path).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let stderr = std::fs::read_to_string(&stderr_path).unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("MCP runtime teardown is unverified"),
        "{stderr}"
    );
    assert_eq!(
        rustix::fs::fcntl_getfl(&stdin_flags).unwrap() & rustix::fs::OFlags::NONBLOCK,
        stdin_original & rustix::fs::OFlags::NONBLOCK,
        "MCP exit left the shared stdin file description nonblocking"
    );
    assert_eq!(
        rustix::fs::fcntl_getfl(&stdout_flags).unwrap() & rustix::fs::OFlags::NONBLOCK,
        stdout_original & rustix::fs::OFlags::NONBLOCK,
        "MCP exit left the shared stdout file description nonblocking"
    );
    drop(writer);
}
