//! Local interactive terminals. This mode never initializes the model engine.
use anyhow::{bail, Context, Result};
use platform_pty::{spawn_pty_process, ProcessHandle, TerminalSize};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
};

const MAX_REQUEST: usize = 512 * 1024;
const MAX_TERMINALS: usize = 32;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    request_id: String,
    kind: String,
    terminal_id: String,
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    data: Option<String>,
}

#[derive(Default)]
struct Utf8Stream(Vec<u8>);
impl Utf8Stream {
    fn push(&mut self, bytes: &[u8], end: bool) -> String {
        self.0.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.0) {
                Ok(text) => {
                    output.push_str(text);
                    self.0.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    output.push_str(std::str::from_utf8(&self.0[..valid]).unwrap_or_default());
                    self.0.drain(..valid);
                    if let Some(length) = error.error_len() {
                        output.push('\u{fffd}');
                        self.0.drain(..length);
                    } else {
                        if end {
                            output.push('\u{fffd}');
                            self.0.clear();
                        }
                        break;
                    }
                }
            }
        }
        output
    }
}

fn shell() -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| Path::new(s).is_absolute() && Path::new(s).is_file())
            .unwrap_or_else(|| "/bin/sh".into());
        (shell, vec!["-l".into(), "-i".into()])
    }
    #[cfg(windows)]
    {
        let paths = std::env::var_os("PATH").unwrap_or_default();
        for name in ["pwsh.exe", "powershell.exe"] {
            if let Some(path) = std::env::split_paths(&paths)
                .map(|p| p.join(name))
                .find(|p| p.is_file())
            {
                return (path.to_string_lossy().into_owned(), vec!["-NoLogo".into()]);
            }
        }
        (
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
            vec![],
        )
    }
}

fn shell_environment() -> HashMap<String, String> {
    // Deliberate allowlist: model credentials and app transport secrets never
    // cross this boundary. Login shell startup files still behave normally.
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(key, _)| {
            matches!(
                key.to_ascii_uppercase().as_str(),
                "HOME"
                    | "USER"
                    | "LOGNAME"
                    | "SHELL"
                    | "PATH"
                    | "TMPDIR"
                    | "TMP"
                    | "TEMP"
                    | "LANG"
                    | "LANGUAGE"
                    | "TZ"
                    | "SSH_AUTH_SOCK"
                    | "SYSTEMROOT"
                    | "WINDIR"
                    | "COMSPEC"
                    | "PATHEXT"
                    | "USERPROFILE"
                    | "APPDATA"
                    | "LOCALAPPDATA"
                    | "HOMEDRIVE"
                    | "HOMEPATH"
                    | "LC_ALL"
                    | "LC_CTYPE"
                    | "LC_COLLATE"
                    | "LC_MESSAGES"
                    | "LC_MONETARY"
                    | "LC_NUMERIC"
                    | "LC_TIME"
            )
        })
        .collect();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env
}

// Interactive jobs can have a different process group from their login shell.
// Find all members by POSIX session ID even after the login shell exits, then
// terminate the PTY group. Windows' PTY backend already owns a kill-on-close Job Object.
async fn terminate_tree(handle: &ProcessHandle) {
    #[cfg(unix)]
    // `process_group_id()` is the spawn-time child pid, and the sweep has to keep
    // working after the shell exits so reparented `nohup` jobs still die. Refuse
    // the two session ids that are never ours, so a recycled pid can at worst
    // reach a stranger's job — never this broker's own process tree or init.
    if let Some(session_id) = handle.process_group_id() {
        let own_session = platform_pty::process_group::process_session_id(std::process::id())
            .ok()
            .flatten();
        // Once the child is reaped its pid is free for reuse, and both sweeps
        // call this on already-exited handles. If the recorded pid is STILL a
        // live process at that point it cannot be ours — it is a recycled pid,
        // and sweeping its session would `kill -KILL` every member of a
        // stranger's login session (another shell, a tmux server, an IDE
        // terminal). A leader that is genuinely gone is the orphan case the
        // sweep exists for, so that one still proceeds.
        let recycled_pid = handle.has_exited()
            && platform_pty::process_group::process_session_id(session_id)
                .ok()
                .flatten()
                .is_some();
        if session_id <= 1 || Some(session_id) == own_session || recycled_pid {
            handle.terminate();
            return;
        }
        if let Ok(output) = tokio::process::Command::new("/bin/ps")
            .args(["-axo", "pid="])
            .output()
            .await
        {
            let members: Vec<u32> = String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .filter_map(|pid| pid.parse().ok())
                .filter(|&pid| {
                    platform_pty::process_group::process_session_id(pid)
                        .ok()
                        .flatten()
                        == Some(session_id)
                })
                .collect();
            if !members.is_empty() {
                let _ = tokio::process::Command::new("/bin/kill")
                    .arg("-KILL")
                    .args(members.iter().map(u32::to_string))
                    .output()
                    .await;
            }
        }
    }
    handle.terminate();
}

async fn dispatch(
    request: &Request,
    terminals: &mut HashMap<String, Arc<ProcessHandle>>,
    events: &mpsc::Sender<Value>,
) -> Result<()> {
    if request.request_id.is_empty()
        || request.request_id.len() > 128
        || request.terminal_id.is_empty()
        || request.terminal_id.len() > 128
    {
        bail!("invalid request or terminal identifier");
    }
    match request.kind.as_str() {
        "create" => {
            // A shell the user exited is not a live terminal: without this the
            // map fills with dead entries and `create` fails permanently. Reap
            // each evicted handle the way `close` does — `ProcessHandle::drop`
            // only kills the shell's own (already dead) process group, so a job
            // the shell reparented (`nohup … &`) would otherwise survive both
            // this eviction and the shutdown sweep, which iterates only the
            // handles still in this map.
            let exited: Vec<String> = terminals
                .iter()
                .filter(|(_, handle)| handle.has_exited())
                .map(|(id, _)| id.clone())
                .collect();
            for id in exited {
                if let Some(handle) = terminals.remove(&id) {
                    terminate_tree(&handle).await;
                }
            }
            if terminals.contains_key(&request.terminal_id) {
                bail!("terminal already exists");
            }
            if terminals.len() >= MAX_TERMINALS {
                bail!("terminal limit reached");
            }
            let cwd = Path::new(request.cwd.as_deref().context("missing cwd")?);
            if !cwd.is_absolute() || !cwd.is_dir() {
                bail!("cwd must be an existing absolute directory");
            }
            let (program, args) = shell();
            let process = spawn_pty_process(
                &program,
                &args,
                cwd,
                &shell_environment(),
                &None,
                dimensions(request)?,
                &[],
            )
            .await?;
            let handle = Arc::new(process.session);
            terminals.insert(request.terminal_id.clone(), handle.clone());
            let terminal_id = request.terminal_id.clone();
            let events = events.clone();
            tokio::spawn(async move {
                let mut output = process.stdout_rx;
                let mut utf8 = Utf8Stream::default();
                while let Some(bytes) = output.recv().await {
                    let data = utf8.push(&bytes, false);
                    if !data.is_empty()
                        && events
                            .send(json!({"kind":"output","terminalId":terminal_id,"data":data}))
                            .await
                            .is_err()
                    {
                        handle.terminate();
                        return;
                    }
                }
                let data = utf8.push(&[], true);
                if !data.is_empty() {
                    let _ = events
                        .send(json!({"kind":"output","terminalId":terminal_id,"data":data}))
                        .await;
                }
                let code = handle.wait().await;
                let _ = events
                    .send(json!({"kind":"exit","terminalId":terminal_id,"exitCode":code}))
                    .await;
            });
        }
        "input" => {
            let handle = terminals
                .get(&request.terminal_id)
                .context("unknown terminal")?;
            if handle.has_exited() {
                bail!("terminal has exited");
            }
            let data = request.data.as_deref().context("missing input")?;
            if data.len() > 64 * 1024 {
                bail!("terminal input exceeds size limit");
            }
            tokio::time::timeout(
                Duration::from_secs(5),
                handle.write(data.as_bytes().to_vec()),
            )
            .await
            .context("terminal input timed out")??;
        }
        "resize" => terminals
            .get(&request.terminal_id)
            .context("unknown terminal")?
            .resize(dimensions(request)?)?,
        "close" => {
            let handle = terminals
                .remove(&request.terminal_id)
                .context("unknown terminal")?;
            terminate_tree(&handle).await;
        }
        _ => bail!("unknown terminal request"),
    }
    Ok(())
}

fn dimensions(request: &Request) -> Result<TerminalSize> {
    let cols = request.cols.context("missing cols")?;
    let rows = request.rows.context("missing rows")?;
    if cols == 0 || rows == 0 || cols > 1000 || rows > 1000 {
        bail!("terminal dimensions out of range");
    }
    Ok(TerminalSize { rows, cols })
}

/// Run the isolated newline-delimited terminal protocol on standard I/O.
pub async fn run() -> Result<()> {
    // Tokio's stdin uses a non-cancellable blocking task, which keeps runtime
    // shutdown waiting for EOF after SIGTERM. A detached standard thread can
    // remain blocked in the OS read without holding the runtime alive.
    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(8);
    std::thread::Builder::new()
        .name("desktop-terminal-stdin".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            let mut buffer = [0_u8; 8192];
            loop {
                match std::io::Read::read(&mut input, &mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if input_tx.blocking_send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
    let (input, mut input_writer) = tokio::io::duplex(8192);
    let pump = tokio::spawn(async move {
        while let Some(bytes) = input_rx.recv().await {
            if input_writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    // Keep a blocked stdout pipe off Tokio's non-cancellable blocking pool
    // too: shutdown must still finish if the parent stops consuming output.
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(8);
    let (output_done_tx, output_done) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("desktop-terminal-stdout".into())
        .spawn(move || {
            let mut output = std::io::stdout().lock();
            while let Some(bytes) = output_rx.blocking_recv() {
                if std::io::Write::write_all(&mut output, &bytes).is_err() {
                    break;
                }
            }
            let _ = std::io::Write::flush(&mut output);
            let _ = output_done_tx.send(());
        })?;
    let (mut output_reader, output) = tokio::io::duplex(8192);
    let mut output_pump = tokio::spawn(async move {
        let mut buffer = [0_u8; 8192];
        loop {
            match output_reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if output_tx.send(buffer[..count].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let result = serve(input, output).await;
    pump.abort();
    if tokio::time::timeout(Duration::from_secs(1), &mut output_pump)
        .await
        .is_err()
    {
        output_pump.abort();
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), output_done).await;
    result
}

async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin + Send + 'static>(
    input: R,
    mut output: W,
) -> Result<()> {
    let (events, mut receiver) = mpsc::channel::<Value>(32);
    let mut writer = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            let mut line = serde_json::to_vec(&event)?;
            line.push(b'\n');
            output.write_all(&line).await?;
            output.flush().await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let mut terminals = HashMap::new();
    let mut reader = BufReader::new(input);
    let operation = async {
        loop {
            // `take` bounds allocations even when stdin has no newline.
            let mut line = Vec::new();
            let mut limited = (&mut reader).take((MAX_REQUEST + 1) as u64);
            let count = tokio::select! {
                result = limited.read_until(b'\n', &mut line) => result?,
                _ = &mut writer => bail!("terminal output disconnected"),
            };
            if count == 0 {
                break;
            }
            if count > MAX_REQUEST {
                bail!("terminal request exceeds size limit");
            }
            let request: Request =
                serde_json::from_slice(&line).context("invalid terminal request")?;
            let error = dispatch(&request, &mut terminals, &events)
                .await
                .err()
                .map(|e| e.to_string());
            events.send(json!({"kind":"response","requestId":request.request_id,"terminalId":request.terminal_id,"error":error})).await.context("terminal output disconnected")?;
        }
        Ok(())
    };
    let result: Result<()> = tokio::select! {
        result = operation => result,
        () = shutdown_signal() => Ok(()),
    };
    let mut cleanup = tokio::task::JoinSet::new();
    for (_, handle) in terminals {
        cleanup.spawn(async move { terminate_tree(&handle).await });
    }
    while cleanup.join_next().await.is_some() {}
    drop(events);
    // A parent that stopped reading must not prevent child process cleanup.
    if !writer.is_finished()
        && tokio::time::timeout(Duration::from_secs(2), &mut writer)
            .await
            .is_err()
    {
        writer.abort();
    }
    result
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = signal.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_keeps_split_chinese_and_replaces_invalid_bytes() {
        let mut decoder = Utf8Stream::default();
        let mut output = String::new();
        for byte in "中文🌸".as_bytes() {
            output.push_str(&decoder.push(&[*byte], false));
        }
        assert_eq!(output, "中文🌸");
        assert_eq!(decoder.push(&[0xff, 0xe4], false), "�");
        assert_eq!(decoder.push(&[], true), "�");
    }

    #[test]
    fn credentials_are_not_inherited() {
        let env = shell_environment();
        assert!(!env
            .keys()
            .any(|key| key.contains("API_KEY") || key.contains("TOKEN") || key.contains("SECRET")));
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
    }

    fn request(kind: &str, id: &str, cwd: &Path, data: Option<&str>) -> Request {
        Request {
            request_id: uuid::Uuid::new_v4().to_string(),
            kind: kind.into(),
            terminal_id: id.into(),
            cwd: Some(cwd.to_string_lossy().into()),
            cols: Some(90),
            rows: Some(31),
            data: data.map(Into::into),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_shell_unicode_resize_interrupt_isolation_and_close() {
        let directory = tempfile::tempdir().unwrap();
        let (events, mut output) = mpsc::channel(32);
        let mut terminals = HashMap::new();
        dispatch(
            &request("create", "one", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        dispatch(
            &request("create", "two", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        dispatch(
            &request("resize", "one", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        dispatch(
            &request(
                "input",
                "one",
                directory.path(),
                Some("PS1='TEST_READY> '; printf '\\344\\270\\255\\346\\226\\207'; stty size; sh -c 'printf \"__%s__\" READY; exec sleep 30'\n"),
            ),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = output.recv().await {
                if event["terminalId"] == "one" {
                    text.push_str(event["data"].as_str().unwrap_or_default());
                }
                if text.contains("中文") && text.contains("31 90") && text.contains("__READY__") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        dispatch(
            &request("input", "one", directory.path(), Some("\u{3}")),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        // The terminal line discipline flushes queued input on SIGINT. Wait
        // for the shell prompt before sending the next user command.
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = output.recv().await {
                if event["terminalId"] == "one"
                    && event["data"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("TEST_READY>")
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        dispatch(
            &request(
                "input",
                "one",
                directory.path(),
                Some("printf 'INTERRUPTED_OK\\n'\n"),
            ),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = output.recv().await {
                if event["terminalId"] == "one" {
                    text.push_str(event["data"].as_str().unwrap_or_default());
                }
                if text.contains("INTERRUPTED_OK\r\n") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        let first = terminals["one"].clone();
        dispatch(
            &request("close", "one", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), first.wait())
            .await
            .unwrap();
        assert!(!terminals["two"].has_exited());
        dispatch(
            &request("close", "two", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn oversize_and_malformed_frames_terminate_protocol() {
        let (input, mut client) = tokio::io::duplex(MAX_REQUEST + 2);
        let task = tokio::spawn(serve(input, tokio::io::sink()));
        client
            .write_all(&vec![b'x'; MAX_REQUEST + 1])
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(serve(&b"{}\n"[..], tokio::io::sink()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn eof_closes_real_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let request = json!({"requestId":"1","kind":"create","terminalId":"eof","cwd":directory.path(),"cols":80,"rows":24});
        let input = format!("{request}\n");
        tokio::time::timeout(
            Duration::from_secs(5),
            serve(input.as_bytes(), tokio::io::sink()),
        )
        .await
        .unwrap()
        .unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn closing_shell_kills_background_job_with_separate_process_group() {
        assert_background_cleanup(false).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_exited_shell_kills_reparented_nohup_job() {
        assert_background_cleanup(true).await;
    }

    #[cfg(unix)]
    async fn assert_background_cleanup(shell_exits: bool) {
        let directory = tempfile::tempdir().unwrap();
        let (events, mut output) = mpsc::channel(32);
        let drain = tokio::spawn(async move { while output.recv().await.is_some() {} });
        let mut terminals = HashMap::new();
        dispatch(
            &request("create", "jobs", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        dispatch(
            &request(
                "input",
                "jobs",
                directory.path(),
                Some(if shell_exits {
                    "(nohup sleep 300 >/dev/null 2>&1 & echo $! > child.pid); exit\n"
                } else {
                    "sleep 300 & echo $! > child.pid\n"
                }),
            ),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(directory.path().join("child.pid")) {
                    if let Ok(pid) = pid.trim().parse::<u32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        if shell_exits {
            tokio::time::timeout(Duration::from_secs(5), terminals["jobs"].wait())
                .await
                .unwrap();
            assert!(tokio::process::Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .output()
                .await
                .unwrap()
                .status
                .success());
        }
        dispatch(
            &request("close", "jobs", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !tokio::process::Command::new("/bin/kill")
                    .args(["-0", &pid.to_string()])
                    .output()
                    .await
                    .unwrap()
                    .status
                    .success()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        drop(events);
        drain.abort();
    }
}
