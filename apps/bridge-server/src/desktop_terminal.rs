//! Local interactive terminals. This mode never initializes the model engine.
use anyhow::{bail, Context, Result};
use platform_pty::{spawn_pty_process, ProcessHandle, TerminalSize};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch, Mutex},
};

const MAX_REQUEST: usize = 512 * 1024;
const MAX_TERMINALS: usize = 32;
const MAX_RESPONSE_BACKLOG: usize = 160;

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
    shell_selection: Option<bool>,
    shell_program: Option<String>,
    shell_launch_cwd: Option<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalPhase {
    Running,
    ExitObserved,
    Exited,
    Unknown,
    Closed,
}

struct TerminalState {
    // Input admission and lifecycle transitions share this lock. An input
    // already admitted before a transition may complete; later ones cannot.
    phase: Mutex<TerminalPhase>,
    closed: watch::Sender<bool>,
    output_drained: AtomicBool,
}

impl TerminalState {
    fn new() -> Self {
        let (closed, _) = watch::channel(false);
        Self {
            phase: Mutex::new(TerminalPhase::Running),
            closed,
            output_drained: AtomicBool::new(false),
        }
    }

    async fn close(&self) {
        let mut phase = self.phase.lock().await;
        *phase = TerminalPhase::Closed;
        self.closed.send_replace(true);
    }
}

struct TerminalEntry {
    handle: Arc<ProcessHandle>,
    state: Arc<TerminalState>,
}

// Reserve capacity before taking the lifecycle lock. Once close changes the
// state, no old instance can enqueue another event under a reused terminal ID.
async fn send_terminal_event(
    state: &TerminalState,
    events: &mpsc::Sender<Value>,
    closed: &mut watch::Receiver<bool>,
    event: Value,
    confirmed_exit: bool,
) -> bool {
    if *closed.borrow() {
        return true;
    }
    let permit = tokio::select! {
        result = events.reserve() => match result { Ok(permit) => permit, Err(_) => return false },
        _ = closed.changed() => return true,
    };
    let mut phase = state.phase.lock().await;
    if *phase != TerminalPhase::Closed {
        permit.send(event);
        // Eviction must not cancel a confirmed exit before its frame has been
        // queued. The frame precedes any later create response on this queue.
        if confirmed_exit && *phase == TerminalPhase::ExitObserved {
            *phase = TerminalPhase::Exited;
        }
    }
    true
}

// Output and wait observation are polled independently, so a full event queue
// cannot prevent the broker from closing input admission when wait is unknown.
async fn forward_terminal_events(
    terminal_id: String,
    mut output: mpsc::Receiver<Vec<u8>>,
    exited: oneshot::Receiver<i32>,
    events: mpsc::Sender<Value>,
    state: Arc<TerminalState>,
) -> bool {
    let output_task = async {
        let mut closed = state.closed.subscribe();
        if *closed.borrow() {
            return true;
        }
        let mut utf8 = Utf8Stream::default();
        loop {
            // send_terminal_event may have consumed the close watch change
            // while waiting for capacity. Do not wait for a second change.
            if *closed.borrow() {
                return true;
            }
            tokio::select! {
                bytes = output.recv() => {
                    let (data, ended) = match bytes {
                        Some(bytes) => (utf8.push(&bytes, false), false),
                        None => (utf8.push(&[], true), true),
                    };
                    if !data.is_empty() && !send_terminal_event(&state, &events, &mut closed,
                        json!({"kind":"output","terminalId":terminal_id,"data":data}), false).await {
                        return false;
                    }
                    if ended {
                        // EOF is observed only after the final UTF-8 chunk has
                        // been queued; it is not evidence of process cleanup.
                        state.output_drained.store(true, Ordering::Release);
                        return true;
                    }
                }
                _ = closed.changed() => return true,
                () = events.closed() => return false,
            }
        }
    };
    let exit_task = async {
        let mut closed = state.closed.subscribe();
        if *closed.borrow() {
            return true;
        }
        let result = tokio::select! {
            result = exited => result,
            _ = closed.changed() => return true,
            () = events.closed() => return false,
        };
        let (event, confirmed_exit) = {
            let mut phase = state.phase.lock().await;
            if *phase == TerminalPhase::Closed {
                return true;
            }
            match result {
                Ok(code) if code != -1 => {
                    *phase = TerminalPhase::ExitObserved;
                    (
                        json!({"kind":"exit","terminalId":terminal_id,"exitCode":code}),
                        true,
                    )
                }
                // -1 also represents a successful wait whose code overflows
                // i32, so the SDK value cannot prove either exit or liveness.
                _ => {
                    *phase = TerminalPhase::Unknown;
                    (
                        json!({"kind":"unknown","terminalId":terminal_id,"reason":"exit-observation-unavailable"}),
                        false,
                    )
                }
            }
        };
        send_terminal_event(&state, &events, &mut closed, event, confirmed_exit).await
    };
    let (output_ok, exit_ok) = tokio::join!(output_task, exit_task);
    output_ok && exit_ok
}

fn shell(request: &Request) -> Result<(String, Vec<String>)> {
    #[cfg(unix)]
    {
        if request.shell_selection.is_some()
            || request.shell_program.is_some()
            || request.shell_launch_cwd.is_some()
        {
            bail!("Windows shell selection is not supported on this platform");
        }
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| Path::new(s).is_absolute() && Path::new(s).is_file())
            .unwrap_or_else(|| "/bin/sh".into());
        Ok((shell, vec!["-l".into(), "-i".into()]))
    }
    #[cfg(windows)]
    {
        // Desktop main, not the renderer, selects from the original PATH.
        // The broker's own PATH and cwd remain app-owned for native loading.
        let environment = shell_environment();
        match (
            request.shell_selection,
            request.shell_program.as_deref(),
            request.shell_launch_cwd.as_deref(),
        ) {
            (Some(true), Some(program), None) => {
                if !valid_windows_shell_program(program, |path| path.is_file()) {
                    bail!("invalid Windows terminal shell selection");
                }
                return Ok((program.into(), vec!["-NoLogo".into()]));
            }
            (Some(true), None, Some(cwd)) => {
                let path = packaged_windows_fallback(&environment, Path::new(cwd))?;
                return Ok((path.to_string_lossy().into_owned(), vec![]));
            }
            (None, None, None) => {}
            _ => bail!("invalid Windows terminal shell selection"),
        }
        let paths = shell_path(&environment).unwrap_or_default();
        if let Some(path) = find_windows_shell(paths, |candidate| candidate.is_file()) {
            return Ok((path.to_string_lossy().into_owned(), vec!["-NoLogo".into()]));
        }
        Ok((windows_fallback_shell(&environment).into(), vec![]))
    }
}

#[cfg(any(windows, test))]
fn shell_path(environment: &HashMap<String, String>) -> Option<&str> {
    shell_environment_value(environment, "PATH")
}

#[cfg(any(windows, test))]
fn shell_environment_value<'a>(
    environment: &'a HashMap<String, String>,
    name: &str,
) -> Option<&'a str> {
    environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[cfg(any(windows, test))]
fn valid_windows_shell_program(program: &str, is_file: impl Fn(&Path) -> bool) -> bool {
    let path = Path::new(program);
    let expected_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("pwsh.exe") || name.eq_ignore_ascii_case("powershell.exe")
        });
    path.is_absolute() && expected_name && is_file(path)
}

#[cfg(any(windows, test))]
fn find_windows_shell(paths: &str, is_file: impl Fn(&Path) -> bool) -> Option<std::path::PathBuf> {
    for name in ["pwsh.exe", "powershell.exe"] {
        for entry in std::env::split_paths(paths) {
            let candidate = entry.join(name);
            if is_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(any(windows, test))]
fn windows_fallback_shell(environment: &HashMap<String, String>) -> &str {
    shell_environment_value(environment, "COMSPEC").unwrap_or("cmd.exe")
}

#[cfg(windows)]
fn windows_path_from_launch_cwd(cwd: &Path, path: &Path) -> Result<std::path::PathBuf> {
    use std::path::{Component, Prefix};
    if !cwd.is_absolute() {
        bail!("invalid Windows terminal launch directory");
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let mut components = path.components();
    if let Some(Component::Prefix(prefix)) = components.next() {
        let drive = match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => bail!("invalid relative Windows terminal shell path"),
        };
        let cwd_drive = match cwd.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => Some(drive),
                _ => None,
            },
            _ => None,
        };
        // The broker receives an explicit environment without Windows' hidden
        // per-drive cwd variables. A different drive therefore starts at its
        // root, while the launch drive retains the parent's original cwd.
        let base = if cwd_drive.is_some_and(|current| current.eq_ignore_ascii_case(&drive)) {
            cwd.to_path_buf()
        } else {
            std::path::PathBuf::from(format!("{}:\\", drive as char))
        };
        return Ok(base.join(components.as_path()));
    }
    Ok(cwd.join(path))
}

#[cfg(windows)]
fn packaged_windows_fallback(
    environment: &HashMap<String, String>,
    launch_cwd: &Path,
) -> Result<std::path::PathBuf> {
    let executable = Path::new(windows_fallback_shell(environment));
    if let Some(paths) = shell_path(environment) {
        let extensions = shell_environment_value(environment, "PATHEXT").unwrap_or(".EXE");
        for entry in std::env::split_paths(paths) {
            let candidate = entry.join(executable);
            let resolved = windows_path_from_launch_cwd(launch_cwd, &candidate)?;
            if resolved.exists() {
                return Ok(resolved);
            }
            for extension in std::env::split_paths(extensions) {
                let extension = extension.to_string_lossy();
                if let Some(extension) = extension.get(1..) {
                    let resolved = windows_path_from_launch_cwd(
                        launch_cwd,
                        &candidate.with_extension(extension),
                    )?;
                    if resolved.exists() {
                        return Ok(resolved);
                    }
                }
            }
        }
    }
    let fallback = windows_path_from_launch_cwd(launch_cwd, executable)?;
    // portable-pty searches PATH again when starting the selected program.
    // If the original unqualified fallback does not exist, passing its new
    // absolute spelling could make that second search select a PATHEXT file
    // in the launch cwd that the original search would never have selected.
    if !fallback.exists() {
        bail!("Windows terminal fallback executable not found");
    }
    Ok(fallback)
}

fn shell_environment() -> HashMap<String, String> {
    shell_environment_from(std::env::vars())
}

fn shell_environment_from(
    source: impl Iterator<Item = (String, String)>,
) -> HashMap<String, String> {
    // Deliberate allowlist: model credentials and app transport secrets never
    // cross this boundary. Login shell startup files still behave normally.
    let source: HashMap<String, String> = source.collect();
    let mut env: HashMap<String, String> = source
        .iter()
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
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(present) = source.get("DESKTOP_TERMINAL_SHELL_PATH_PRESENT") {
        // Keep the original value intact so a maximal Windows PATH does not
        // overflow the per-variable limit when it enters the broker.
        env.retain(|key, _| !key.eq_ignore_ascii_case("PATH"));
        if present == "1" {
            if let Some(path) = source.get("DESKTOP_TERMINAL_SHELL_PATH") {
                env.insert("PATH".into(), path.clone());
            }
        }
    }
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
    terminals: &mut HashMap<String, TerminalEntry>,
    events: &mpsc::Sender<Value>,
) -> Result<()> {
    if request.kind != "create"
        && (request.shell_selection.is_some()
            || request.shell_program.is_some()
            || request.shell_launch_cwd.is_some())
    {
        bail!("shell selection is only valid for terminal creation");
    }
    if request.request_id.is_empty()
        || request.request_id.len() > 128
        || request.terminal_id.is_empty()
        || request.terminal_id.len() > 128
    {
        bail!("invalid request or terminal identifier");
    }
    match request.kind.as_str() {
        "create" => {
            if terminals.contains_key(&request.terminal_id) {
                bail!("terminal already exists");
            }
            // Keep exited output available while its tab remains open. At the
            // limit, a confirmed exit AND drained output permit eviction. The
            // SDK's has_exited flag is also set on failed/overflowed waits.
            if terminals.len() >= MAX_TERMINALS {
                let mut exited = Vec::new();
                for (id, entry) in terminals.iter() {
                    if *entry.state.phase.lock().await == TerminalPhase::Exited
                        && entry.state.output_drained.load(Ordering::Acquire)
                    {
                        exited.push(id.clone());
                    }
                }
                for id in exited {
                    if let Some(entry) = terminals.remove(&id) {
                        entry.state.close().await;
                        terminate_tree(&entry.handle).await;
                    }
                }
            }
            if terminals.len() >= MAX_TERMINALS {
                bail!("terminal limit reached");
            }
            let cwd = Path::new(request.cwd.as_deref().context("missing cwd")?);
            if !cwd.is_absolute() || !cwd.is_dir() {
                bail!("cwd must be an existing absolute directory");
            }
            let (program, args) = shell(request)?;
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
            let state = Arc::new(TerminalState::new());
            terminals.insert(
                request.terminal_id.clone(),
                TerminalEntry {
                    handle: handle.clone(),
                    state: state.clone(),
                },
            );
            let terminal_id = request.terminal_id.clone();
            let events = events.clone();
            // The map owns the PTY. The forwarding task must not keep it alive:
            // closing the last handle is what lets ConPTY's output pipe reach EOF.
            let handle = Arc::downgrade(&handle);
            tokio::spawn(async move {
                if !forward_terminal_events(
                    terminal_id,
                    process.stdout_rx,
                    process.exit_rx,
                    events,
                    state,
                )
                .await
                {
                    if let Some(handle) = handle.upgrade() {
                        handle.terminate();
                    }
                }
            });
        }
        "input" => {
            let entry = terminals
                .get(&request.terminal_id)
                .context("unknown terminal")?;
            let phase = entry.state.phase.lock().await;
            match *phase {
                TerminalPhase::ExitObserved | TerminalPhase::Exited => bail!("terminal has exited"),
                TerminalPhase::Unknown | TerminalPhase::Closed => {
                    bail!("terminal status is unknown")
                }
                TerminalPhase::Running if entry.handle.has_exited() => {
                    bail!("terminal status is unavailable")
                }
                TerminalPhase::Running => {}
            }
            let data = request.data.as_deref().context("missing input")?;
            if data.len() > 64 * 1024 {
                bail!("terminal input exceeds size limit");
            }
            tokio::time::timeout(
                Duration::from_secs(5),
                entry.handle.write(data.as_bytes().to_vec()),
            )
            .await
            .context("terminal input timed out")??;
        }
        "resize" => {
            let entry = terminals
                .get(&request.terminal_id)
                .context("unknown terminal")?;
            let phase = entry.state.phase.lock().await;
            match *phase {
                TerminalPhase::ExitObserved | TerminalPhase::Exited => bail!("terminal has exited"),
                TerminalPhase::Unknown | TerminalPhase::Closed => {
                    bail!("terminal status is unknown")
                }
                TerminalPhase::Running if entry.handle.has_exited() => {
                    bail!("terminal status is unavailable")
                }
                TerminalPhase::Running => {}
            }
            entry.handle.resize(dimensions(request)?)?;
        }
        "close" => {
            let entry = terminals
                .remove(&request.terminal_id)
                .context("unknown terminal")?;
            entry.state.close().await;
            terminate_tree(&entry.handle).await;
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

fn forward_terminal_stdin<R: std::io::Read>(
    input: &mut R,
    input_tx: mpsc::Sender<Vec<u8>>,
    stdin_eof: &AtomicBool,
) {
    let mut buffer = [0_u8; 8192];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => {
                stdin_eof.store(true, Ordering::Release);
                break;
            }
            Err(_) => break,
            Ok(count) => {
                if input_tx.blocking_send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
}

/// Run the isolated newline-delimited terminal protocol on standard I/O.
pub async fn run() -> Result<()> {
    // Tokio's stdin uses a non-cancellable blocking task, which keeps runtime
    // shutdown waiting for EOF after SIGTERM. A detached standard thread can
    // remain blocked in the OS read without holding the runtime alive.
    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(8);
    let stdin_eof = Arc::new(AtomicBool::new(false));
    let stdin_eof_reader = stdin_eof.clone();
    std::thread::Builder::new()
        .name("desktop-terminal-stdin".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            forward_terminal_stdin(&mut input, input_tx, &stdin_eof_reader);
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
    let result = serve(input, output, Some(stdin_eof)).await;
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
    output: W,
    stdin_eof: Option<Arc<AtomicBool>>,
) -> Result<()> {
    serve_until(input, output, stdin_eof, shutdown_signal()).await
}

async fn complete_terminal_cleanup(mut cleanup: tokio::task::JoinSet<()>) -> Result<()> {
    let mut failed = false;
    while let Some(result) = cleanup.join_next().await {
        failed |= result.is_err();
    }
    if failed {
        bail!("terminal cleanup action failed");
    }
    Ok(())
}

async fn serve_until<R, W, S>(
    input: R,
    mut output: W,
    stdin_eof: Option<Arc<AtomicBool>>,
    shutdown: S,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
    S: std::future::Future<Output = ()>,
{
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
    // Keep request processing independent of a blocked stdout writer. This
    // bounded queue permits close/EOF to run even while output is stalled;
    // exhausting it is an explicit transport failure, not cleanup proof.
    let (responses, mut response_rx) = mpsc::channel::<Value>(MAX_RESPONSE_BACKLOG);
    let response_events = events.clone();
    let mut response_pump = tokio::spawn(async move {
        while let Some(response) = response_rx.recv().await {
            response_events
                .send(response)
                .await
                .context("terminal output disconnected")?;
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
            let response = json!({"kind":"response","requestId":request.request_id,"terminalId":request.terminal_id,"error":error});
            match responses.try_send(response) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    bail!("terminal response queue disconnected")
                }
                Err(mpsc::error::TrySendError::Full(response)) => {
                    // Give a healthy writer a chance to drain a burst. A
                    // persistently full queue is an explicit transport error,
                    // not evidence that any process has been cleaned up.
                    match tokio::time::timeout(Duration::from_millis(250), responses.send(response))
                        .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => bail!("terminal response queue disconnected"),
                        Err(_) => bail!("terminal response queue stalled"),
                    }
                }
            }
        }
        Ok(())
    };
    let (mut result, observed_eof): (Result<()>, bool) = tokio::select! {
        result = operation => (result, true),
        () = shutdown => (Ok(()), false),
    };
    drop(responses);
    let mut cleanup = tokio::task::JoinSet::new();
    for (_, entry) in terminals {
        cleanup.spawn(async move {
            entry.state.close().await;
            terminate_tree(&entry.handle).await;
        });
    }
    let cleanup_result = complete_terminal_cleanup(cleanup).await;
    let cleanup_completed = cleanup_result.is_ok();
    if let Err(error) = cleanup_result {
        if result.is_ok() {
            result = Err(error);
        }
    }
    let responses_delivered =
        match tokio::time::timeout(Duration::from_secs(1), &mut response_pump).await {
            Ok(Ok(Ok(()))) => true,
            Ok(_) => false,
            Err(_) => {
                response_pump.abort();
                false
            }
        };
    if !responses_delivered && result.is_ok() {
        result = Err(anyhow::anyhow!(
            "terminal response delivery could not be confirmed"
        ));
    }
    if observed_eof
        && result.is_ok()
        && cleanup_completed
        && stdin_eof
            .as_ref()
            .is_none_or(|eof| eof.load(Ordering::Acquire))
    {
        // This confirms EOF and completion of the cleanup actions, not that
        // every descendant has been reaped. Keep a blocked parent bounded.
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            events.send(json!({"kind":"shutdown","reason":"eof"})),
        )
        .await;
    }
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

    #[tokio::test]
    async fn exit_does_not_wait_for_pty_eof_and_late_output_is_preserved() {
        let (output_tx, output_rx) = mpsc::channel(2);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (events, mut receiver) = mpsc::channel(4);
        let state = Arc::new(TerminalState::new());
        let task = tokio::spawn(forward_terminal_events(
            "test".into(),
            output_rx,
            exit_rx,
            events,
            state.clone(),
        ));

        // The stream may remain open while a ConPTY handle is retained. A
        // partial character also checks that the decoder survives an exit.
        output_tx.send(vec![0xe4, 0xb8]).await.unwrap();
        exit_tx.send(7).unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            exit,
            json!({"kind":"exit","terminalId":"test","exitCode":7})
        );
        assert!(!state.output_drained.load(Ordering::Acquire));
        assert!(!task.is_finished());
        output_tx.send(vec![0xad]).await.unwrap();
        let tail = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tail,
            json!({"kind":"output","terminalId":"test","data":"中"})
        );
        drop(output_tx);
        assert!(task.await.unwrap());
        assert!(state.output_drained.load(Ordering::Acquire));
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn output_eof_does_not_substitute_for_child_exit() {
        let (output_tx, output_rx) = mpsc::channel(1);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (events, mut receiver) = mpsc::channel(2);
        let task = tokio::spawn(forward_terminal_events(
            "test".into(),
            output_rx,
            exit_rx,
            events,
            Arc::new(TerminalState::new()),
        ));
        output_tx.send(vec![0xe4]).await.unwrap();
        drop(output_tx);
        let tail = receiver.recv().await.unwrap();
        assert_eq!(
            tail,
            json!({"kind":"output","terminalId":"test","data":"�"})
        );
        assert!(!task.is_finished());
        exit_tx.send(23).unwrap();
        assert_eq!(
            receiver.recv().await.unwrap(),
            json!({"kind":"exit","terminalId":"test","exitCode":23})
        );
        assert!(task.await.unwrap());
    }

    #[tokio::test]
    async fn lost_or_ambiguous_exit_stays_unknown_and_keeps_output() {
        for lost in [true, false] {
            let (output_tx, output_rx) = mpsc::channel(2);
            let (exit_tx, exit_rx) = oneshot::channel();
            let (events, mut receiver) = mpsc::channel(4);
            let state = Arc::new(TerminalState::new());
            let task = tokio::spawn(forward_terminal_events(
                "test".into(),
                output_rx,
                exit_rx,
                events,
                state.clone(),
            ));
            output_tx.send(b"before".to_vec()).await.unwrap();
            assert_eq!(receiver.recv().await.unwrap()["data"], "before");
            if lost {
                drop(exit_tx);
            } else {
                exit_tx.send(-1).unwrap();
            }
            assert_eq!(
                receiver.recv().await.unwrap(),
                json!({"kind":"unknown","terminalId":"test","reason":"exit-observation-unavailable"})
            );
            assert_eq!(*state.phase.lock().await, TerminalPhase::Unknown);
            output_tx.send(b"after".to_vec()).await.unwrap();
            assert_eq!(receiver.recv().await.unwrap()["data"], "after");
            drop(output_tx);
            assert!(task.await.unwrap());
            assert!(receiver.recv().await.is_none());
        }
    }

    #[tokio::test]
    async fn disconnected_parent_does_not_fabricate_exit() {
        let (_output_tx, output_rx) = mpsc::channel(1);
        let (_exit_tx, exit_rx) = oneshot::channel();
        let (events, receiver) = mpsc::channel(2);
        drop(receiver);
        assert!(!tokio::time::timeout(
            Duration::from_secs(2),
            forward_terminal_events(
                "test".into(),
                output_rx,
                exit_rx,
                events,
                Arc::new(TerminalState::new())
            )
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn full_output_queue_does_not_block_unknown_admission_or_leak_after_close() {
        let (output_tx, output_rx) = mpsc::channel(1);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (events, mut receiver) = mpsc::channel(1);
        events.send(json!({"kind":"occupied"})).await.unwrap();
        let state = Arc::new(TerminalState::new());
        let task = tokio::spawn(forward_terminal_events(
            "reused".into(),
            output_rx,
            exit_rx,
            events.clone(),
            state.clone(),
        ));
        output_tx.send(b"old output".to_vec()).await.unwrap();
        exit_tx.send(-1).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while *state.phase.lock().await != TerminalPhase::Unknown {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while output_tx.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        state.close().await;
        assert!(tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(receiver.recv().await.unwrap()["kind"], "occupied");
        assert!(
            receiver.try_recv().is_err(),
            "old instance emitted after close"
        );
    }

    #[tokio::test]
    async fn exit_is_not_evictable_before_notification_is_queued() {
        let (_output_tx, output_rx) = mpsc::channel(1);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (events, mut receiver) = mpsc::channel(1);
        events.send(json!({"kind":"occupied"})).await.unwrap();
        let state = Arc::new(TerminalState::new());
        let task = tokio::spawn(forward_terminal_events(
            "test".into(),
            output_rx,
            exit_rx,
            events,
            state.clone(),
        ));
        exit_tx.send(7).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while *state.phase.lock().await != TerminalPhase::ExitObserved {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(receiver.recv().await.unwrap()["kind"], "occupied");
        assert_eq!(
            receiver.recv().await.unwrap(),
            json!({"kind":"exit","terminalId":"test","exitCode":7})
        );
        assert_eq!(*state.phase.lock().await, TerminalPhase::Exited);
        state.close().await;
        assert!(task.await.unwrap());
    }

    #[test]
    fn credentials_are_not_inherited() {
        let env = shell_environment();
        assert!(!env
            .keys()
            .any(|key| key.contains("API_KEY") || key.contains("TOKEN") || key.contains("SECRET")));
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
    }

    #[test]
    fn shell_path_is_restored_without_native_loader_environment() {
        let env = shell_environment_from(
            [
                ("PATH", "C:\\app\\gstreamer\\bin;C:\\user\\bin"),
                ("DESKTOP_TERMINAL_SHELL_PATH", "C:\\user\\bin"),
                ("DESKTOP_TERMINAL_SHELL_PATH_PRESENT", "1"),
                ("LD_LIBRARY_PATH", "/injected"),
                ("DYLD_INSERT_LIBRARIES", "/injected.dylib"),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value.into())),
        );
        assert_eq!(env.get("PATH").map(String::as_str), Some("C:\\user\\bin"));
        assert!(!env.contains_key("DESKTOP_TERMINAL_SHELL_PATH"));
        assert!(!env.contains_key("DESKTOP_TERMINAL_SHELL_PATH_PRESENT"));
        assert!(!env.contains_key("LD_LIBRARY_PATH"));
        assert!(!env.contains_key("DYLD_INSERT_LIBRARIES"));
        assert_eq!(shell_path(&env), Some("C:\\user\\bin"));

        let mixed_case = shell_environment_from(
            [("Path", "C:\\user\\tools")]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        assert_eq!(shell_path(&mixed_case), Some("C:\\user\\tools"));

        for (marker, expected) in [("0", None), ("1", Some(""))] {
            let env = shell_environment_from(
                [
                    ("PATH", "broker-only"),
                    ("DESKTOP_TERMINAL_SHELL_PATH_PRESENT", marker),
                    ("DESKTOP_TERMINAL_SHELL_PATH", ""),
                ]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
            );
            assert_eq!(env.get("PATH").map(String::as_str), expected);
        }
    }

    #[test]
    fn windows_shell_selection_uses_original_directory_and_safe_fallback() {
        let base = std::env::temp_dir().join("lingxi-original-working-directory");
        let relative = std::path::PathBuf::from("relative-tools");
        let expected = relative.join("pwsh.exe");
        let paths = std::env::join_paths([relative]).unwrap();
        let selected = find_windows_shell(&paths.to_string_lossy(), |path| path == expected);
        assert_eq!(selected.as_deref(), Some(expected.as_path()));
        assert_eq!(
            find_windows_shell(&paths.to_string_lossy(), |_| false),
            None
        );

        let empty = HashMap::new();
        assert_eq!(shell_path(&empty), None);
        assert_eq!(windows_fallback_shell(&empty), "cmd.exe");
        let comspec = [("ComSpec".into(), "custom-cmd.exe".into())]
            .into_iter()
            .collect();
        assert_eq!(windows_fallback_shell(&comspec), "custom-cmd.exe");

        let allowed = base.join("PWSH.EXE");
        let wrong_name = base.join("other.exe");
        assert!(valid_windows_shell_program(
            &allowed.to_string_lossy(),
            |_| true
        ));
        assert!(!valid_windows_shell_program("pwsh.exe", |_| true));
        assert!(!valid_windows_shell_program(
            &wrong_name.to_string_lossy(),
            |_| true
        ));
        assert!(!valid_windows_shell_program(
            &allowed.to_string_lossy(),
            |_| false
        ));
    }

    #[cfg(windows)]
    #[test]
    fn unpackaged_windows_shell_search_preserves_drive_relative_paths() {
        for entry in ["C:tools", "D:tools", ""] {
            let expected = Path::new(entry).join("pwsh.exe");
            assert_eq!(
                find_windows_shell(entry, |candidate| candidate == expected),
                Some(expected)
            );
        }
        let quoted = r#""C:\with;semicolon";C:tools"#;
        let expected = Path::new(r"C:\with;semicolon").join("pwsh.exe");
        assert_eq!(
            find_windows_shell(quoted, |candidate| candidate == expected),
            Some(expected)
        );
    }

    #[cfg(windows)]
    #[test]
    fn unpackaged_windows_shell_search_resolves_current_drive_relative_path() {
        use std::path::{Component, Prefix};

        // Use the existing working directory so this native regression does
        // not mutate the process-wide cwd while other tests may be running.
        let cwd = std::env::current_dir().unwrap();
        let drive = match cwd.components().next().unwrap() {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive as char,
                _ => panic!("test requires a local Windows working directory"),
            },
            _ => panic!("test requires a Windows drive prefix"),
        };
        let directory = tempfile::tempdir_in(".").unwrap();
        let program = directory.path().join("pwsh.exe");
        std::fs::write(&program, b"test shell").unwrap();
        let entry = format!(
            "{drive}:{}",
            directory.path().file_name().unwrap().to_string_lossy()
        );
        let selected = find_windows_shell(&entry, |candidate| candidate.is_file()).unwrap();
        assert!(!selected.is_absolute());
        assert_eq!(
            selected.canonicalize().unwrap(),
            program.canonicalize().unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn packaged_windows_fallback_resolves_original_cwd_and_user_path() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path();
        let tools = cwd.join("tools");
        std::fs::create_dir(&tools).unwrap();
        let system_root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
        let system_cmd = Path::new(&system_root).join("System32").join("cmd.exe");
        let cwd_cmd = cwd.join("cmd.exe");
        let tools_cmd = tools.join("cmd.exe");
        std::fs::copy(&system_cmd, &cwd_cmd).unwrap();
        std::fs::copy(&system_cmd, &tools_cmd).unwrap();

        let mut environment = HashMap::new();
        // A missing PATH and COMSPEC must use the original cwd, not resources/bin.
        let selected = packaged_windows_fallback(&environment, cwd).unwrap();
        assert_eq!(selected, cwd_cmd);
        let status = std::process::Command::new(&selected)
            .args(["/d", "/c", "exit 0"])
            .current_dir(&tools)
            .env_clear()
            .env("SystemRoot", &system_root)
            .status()
            .unwrap();
        assert!(status.success());

        environment.insert("Path".into(), "tools".into());
        assert_eq!(
            packaged_windows_fallback(&environment, cwd).unwrap(),
            tools_cmd
        );

        let nested = cwd.join("search").join("tools");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::copy(&system_cmd, nested.join("cmd.exe")).unwrap();
        environment.insert("ComSpec".into(), "tools\\cmd.exe".into());
        environment.insert("Path".into(), "search".into());
        assert_eq!(
            packaged_windows_fallback(&environment, cwd).unwrap(),
            nested.join("cmd.exe")
        );
        environment.remove("Path");
        assert_eq!(
            packaged_windows_fallback(&environment, cwd).unwrap(),
            tools_cmd
        );

        environment.insert("Path".into(), "".into());
        environment.insert("ComSpec".into(), "alternate".into());
        environment.insert("PATHEXT".into(), ".EXE;.COM".into());
        let alternate = cwd.join("alternate.EXE");
        std::fs::copy(&system_cmd, &alternate).unwrap();
        assert_eq!(
            packaged_windows_fallback(&environment, cwd).unwrap(),
            alternate
        );
        environment.remove("Path");
        assert!(!cwd.join("alternate").exists());
        assert!(packaged_windows_fallback(&environment, cwd).is_err());
        environment.insert("Path".into(), "missing".into());
        assert!(packaged_windows_fallback(&environment, cwd).is_err());
        std::fs::copy(&system_cmd, cwd.join("alternate")).unwrap();
        assert_eq!(
            packaged_windows_fallback(&environment, cwd).unwrap(),
            cwd.join("alternate")
        );

        environment.insert("ComSpec".into(), system_cmd.to_string_lossy().into_owned());
        assert_eq!(
            packaged_windows_fallback(&environment, cwd).unwrap(),
            system_cmd
        );
        assert!(packaged_windows_fallback(&environment, Path::new("relative")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn packaged_windows_fallback_preserves_drive_relative_semantics() {
        let cwd = Path::new(r"C:\original\working");
        assert_eq!(
            windows_path_from_launch_cwd(cwd, Path::new(r"C:tools\cmd.exe")).unwrap(),
            cwd.join(r"tools\cmd.exe")
        );
        assert_eq!(
            windows_path_from_launch_cwd(cwd, Path::new(r"D:tools\cmd.exe")).unwrap(),
            Path::new(r"D:\tools\cmd.exe")
        );
        assert_eq!(
            windows_path_from_launch_cwd(cwd, Path::new(r"\tools\cmd.exe")).unwrap(),
            Path::new(r"C:\tools\cmd.exe")
        );
        let verbatim = Path::new(r"\\?\C:\original\working");
        assert_eq!(
            windows_path_from_launch_cwd(verbatim, Path::new(r"C:tools\cmd.exe")).unwrap(),
            verbatim.join(r"tools\cmd.exe")
        );
        let unc = Path::new(r"\\server\share\working");
        assert_eq!(
            windows_path_from_launch_cwd(unc, Path::new(r"D:tools\cmd.exe")).unwrap(),
            Path::new(r"D:\tools\cmd.exe")
        );
    }

    #[tokio::test]
    async fn shell_selection_fields_are_rejected_outside_create() {
        let (events, _receiver) = mpsc::channel(1);
        for field in [
            json!({"shellSelection":true}),
            json!({"shellProgram":"pwsh.exe"}),
            json!({"shellLaunchCwd":"C:\\original"}),
        ] {
            let mut frame = json!({"kind":"input", "requestId":"request", "terminalId":"terminal"});
            frame
                .as_object_mut()
                .unwrap()
                .extend(field.as_object().unwrap().clone());
            let request: Request = serde_json::from_value(frame).unwrap();
            let error = dispatch(&request, &mut HashMap::new(), &events)
                .await
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("only valid for terminal creation"));
        }
        let unknown = json!({"kind":"create", "requestId":"request", "terminalId":"terminal", "shellPath":"pwsh.exe"});
        assert!(serde_json::from_value::<Request>(unknown).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_rejects_windows_shell_selection_fields() {
        let mut create = request("create", "test", Path::new("/"), None);
        create.shell_selection = Some(true);
        create.shell_program = Some("/tmp/pwsh.exe".into());
        assert!(shell(&create)
            .unwrap_err()
            .to_string()
            .contains("not supported"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_accepts_only_an_absolute_existing_expected_shell_file() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("pwsh.exe");
        std::fs::write(&program, b"test shell").unwrap();
        let mut create = request("create", "test", directory.path(), None);
        create.shell_selection = Some(true);
        create.shell_program = Some(program.to_string_lossy().into_owned());
        assert_eq!(
            shell(&create).unwrap().0,
            program.to_string_lossy().into_owned()
        );
        create.shell_program = Some("pwsh.exe".into());
        assert!(shell(&create).is_err());
        let other = directory.path().join("other.exe");
        std::fs::write(&other, b"wrong name").unwrap();
        create.shell_program = Some(other.to_string_lossy().into_owned());
        assert!(shell(&create).is_err());
        let not_file = directory.path().join("directory").join("pwsh.exe");
        std::fs::create_dir_all(&not_file).unwrap();
        create.shell_program = Some(not_file.to_string_lossy().into_owned());
        assert!(shell(&create).is_err());
        create.shell_selection = None;
        assert!(shell(&create).is_err());
        create.shell_selection = Some(true);
        create.shell_program = None;
        assert!(shell(&create).is_err());
        create.shell_launch_cwd = Some("relative".into());
        assert!(shell(&create).is_err());
        create.shell_launch_cwd = Some(directory.path().to_string_lossy().into_owned());
        assert!(Path::new(&shell(&create).unwrap().0).is_absolute());
        create.shell_program = Some(program.to_string_lossy().into_owned());
        assert!(shell(&create).is_err());
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
            shell_selection: None,
            shell_program: None,
            shell_launch_cwd: None,
        }
    }

    #[tokio::test]
    async fn unknown_blocks_broker_input_even_while_notification_is_backpressured() {
        let directory = tempfile::tempdir().unwrap();
        let (events, _output) = mpsc::channel(32);
        let mut terminals = HashMap::new();
        dispatch(
            &request("create", "live", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        let state = terminals["live"].state.clone();
        let (_output_tx, output_rx) = mpsc::channel(1);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (blocked_events, mut receiver) = mpsc::channel(1);
        blocked_events
            .send(json!({"kind":"occupied"}))
            .await
            .unwrap();
        let task = tokio::spawn(forward_terminal_events(
            "live".into(),
            output_rx,
            exit_rx,
            blocked_events,
            state.clone(),
        ));
        exit_tx.send(-1).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while *state.phase.lock().await != TerminalPhase::Unknown {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for kind in ["input", "resize"] {
            let error = dispatch(
                &request(kind, "live", directory.path(), Some("blocked")),
                &mut terminals,
                &events,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("status is unknown"));
        }
        assert_eq!(receiver.recv().await.unwrap()["kind"], "occupied");
        assert_eq!(receiver.recv().await.unwrap()["kind"], "unknown");
        dispatch(
            &request("close", "live", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        assert!(task.await.unwrap());
    }

    #[tokio::test]
    async fn unknown_is_not_evicted_by_sdk_exit_flag_and_rejects_operations() {
        let directory = tempfile::tempdir().unwrap();
        let (events, _output) = mpsc::channel(128);
        let mut terminals = HashMap::new();
        dispatch(
            &request("create", "old", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        dispatch(
            &request(
                "input",
                "old",
                directory.path(),
                Some(if cfg!(windows) {
                    "exit 0\r"
                } else {
                    "exit 0\n"
                }),
            ),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        let handle = terminals["old"].handle.clone();
        let exit_code = tokio::time::timeout(Duration::from_secs(15), handle.wait())
            .await
            .unwrap();
        assert_eq!(
            exit_code, 0,
            "the fixture shell must actually exit successfully"
        );
        tokio::time::timeout(Duration::from_secs(15), async {
            while *terminals["old"].state.phase.lock().await != TerminalPhase::Exited {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(handle.has_exited());
        *terminals["old"].state.phase.lock().await = TerminalPhase::Unknown;
        for index in 1..MAX_TERMINALS {
            let state = Arc::new(TerminalState::new());
            *state.phase.lock().await = TerminalPhase::Unknown;
            terminals.insert(
                format!("fake-{index}"),
                TerminalEntry {
                    handle: handle.clone(),
                    state,
                },
            );
        }
        for kind in ["input", "resize"] {
            let error = dispatch(
                &request(kind, "old", directory.path(), Some("ignored")),
                &mut terminals,
                &events,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("status is unknown"));
        }
        let create = request("create", "new", directory.path(), None);
        assert!(dispatch(&create, &mut terminals, &events)
            .await
            .unwrap_err()
            .to_string()
            .contains("limit"));
        assert!(terminals.contains_key("old"));
        // Observing a genuine exit is insufficient until its frame is queued.
        *terminals["fake-1"].state.phase.lock().await = TerminalPhase::ExitObserved;
        assert!(dispatch(&create, &mut terminals, &events).await.is_err());
        let retained = Arc::new(TerminalState::new());
        terminals.get_mut("fake-1").unwrap().state = retained.clone();
        let (tail_tx, tail_rx) = mpsc::channel(1);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (tail_events, mut tail_frames) = mpsc::channel(2);
        let tail_task = tokio::spawn(forward_terminal_events(
            "fake-1".into(),
            tail_rx,
            exit_rx,
            tail_events,
            retained.clone(),
        ));
        exit_tx.send(7).unwrap();
        assert_eq!(tail_frames.recv().await.unwrap()["kind"], "exit");
        // A full-capacity create must not cancel the delayed tail.
        assert!(dispatch(&create, &mut terminals, &events).await.is_err());
        tail_tx.send(b"late tail".to_vec()).await.unwrap();
        assert_eq!(tail_frames.recv().await.unwrap()["data"], "late tail");
        drop(tail_tx);
        assert!(tail_task.await.unwrap());
        dispatch(&create, &mut terminals, &events).await.unwrap();
        assert!(terminals.contains_key("old"));
        assert!(!terminals.contains_key("fake-1"));
        terminals.retain(|id, _| id == "old" || id == "new");
        dispatch(
            &request("close", "old", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
        dispatch(
            &request("close", "new", directory.path(), None),
            &mut terminals,
            &events,
        )
        .await
        .unwrap();
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
        let first = terminals["one"].handle.clone();
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
        assert!(!terminals["two"].handle.has_exited());
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
        let task = tokio::spawn(serve(input, tokio::io::sink(), None));
        client
            .write_all(&vec![b'x'; MAX_REQUEST + 1])
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(serve(&b"{}\n"[..], tokio::io::sink(), None).await.is_err());
    }

    #[tokio::test]
    async fn cleanup_task_panic_is_reported_after_other_actions_finish() {
        let finished = Arc::new(AtomicBool::new(false));
        let mut cleanup = tokio::task::JoinSet::new();
        cleanup.spawn(async { panic!("fixture cleanup failure") });
        let other = finished.clone();
        cleanup.spawn(async move {
            other.store(true, Ordering::Release);
        });
        let error = complete_terminal_cleanup(cleanup).await.unwrap_err();
        assert!(error.to_string().contains("cleanup action failed"));
        assert!(
            finished.load(Ordering::Acquire),
            "other cleanup actions must still run"
        );
    }

    #[tokio::test]
    async fn draining_output_handles_a_large_response_burst() {
        let (server_input, mut input) = tokio::io::duplex(128 * 1024);
        let (server_output, mut output) = tokio::io::duplex(4096);
        let task = tokio::spawn(serve_until(
            server_input,
            server_output,
            None,
            std::future::pending(),
        ));
        let reader = tokio::spawn(async move {
            let mut frames = String::new();
            output.read_to_string(&mut frames).await.unwrap();
            frames
        });
        let mut requests = String::new();
        for index in 0..500 {
            requests.push_str(&format!("{}\n", json!({"kind":"resize","requestId":format!("burst-{index}"),"terminalId":"missing","cols":80,"rows":24})));
        }
        input.write_all(requests.as_bytes()).await.unwrap();
        input.shutdown().await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_ok());
        let output = reader.await.unwrap();
        let frames: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame["kind"] == "response")
                .count(),
            500
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame["kind"] == "shutdown")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_output_does_not_prevent_real_close_or_stdin_eof_cleanup() {
        async fn alive(pid: u32) -> bool {
            tokio::process::Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .output()
                .await
                .unwrap()
                .status
                .success()
        }
        async fn send(input: &mut tokio::io::DuplexStream, value: Value) {
            input
                .write_all(format!("{value}\n").as_bytes())
                .await
                .unwrap();
        }
        for mode in ["close", "eof", "overflow"] {
            let directory = tempfile::tempdir().unwrap();
            let (server_input, mut input) = tokio::io::duplex(128 * 1024);
            let (server_output, output) = tokio::io::duplex(256);
            let mut output = BufReader::new(output);
            let mut task = tokio::spawn(serve_until(
                server_input,
                server_output,
                None,
                std::future::pending(),
            ));
            send(&mut input, json!({"kind":"create","requestId":"create","terminalId":"live","cwd":directory.path(),"cols":80,"rows":24})).await;
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    line.clear();
                    output.read_line(&mut line).await.unwrap();
                    let event: Value = serde_json::from_str(&line).unwrap();
                    if event["requestId"] == "create" {
                        assert!(event["error"].is_null());
                        break;
                    }
                }
            })
            .await
            .unwrap();
            send(&mut input, json!({"kind":"input","requestId":"start","terminalId":"live","data":"printf '__LX_%s__%s\\n' PID $$; exec sleep 30\n"})).await;
            let mut text = String::new();
            let pid = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    line.clear();
                    output.read_line(&mut line).await.unwrap();
                    let event: Value = serde_json::from_str(&line).unwrap();
                    if let Some(data) = event["data"].as_str() {
                        text.push_str(data);
                    }
                    if let Some((_, tail)) = text.split_once("__LX_PID__") {
                        let digits: String =
                            tail.chars().take_while(char::is_ascii_digit).collect();
                        if !digits.is_empty() {
                            break digits.parse::<u32>().unwrap();
                        }
                    }
                }
            })
            .await
            .unwrap();
            assert!(
                alive(pid).await,
                "the fixture process must actually be running"
            );
            // Stop reading stdout. Responses exceed the event queue and make
            // its writer block on the tiny duplex stream. Overflow also fills
            // the bounded response queue and must fail explicitly.
            let count = if mode == "overflow" {
                MAX_RESPONSE_BACKLOG + 64
            } else {
                40
            };
            let mut requests = String::new();
            for index in 0..count {
                requests.push_str(&format!("{}\n", json!({"kind":"resize","requestId":format!("blocked-{index}"),"terminalId":"missing","cols":80,"rows":24})));
            }
            input.write_all(requests.as_bytes()).await.unwrap();
            if mode == "close" {
                send(
                    &mut input,
                    json!({"kind":"close","requestId":"close","terminalId":"live"}),
                )
                .await;
            } else if mode == "eof" {
                input.shutdown().await.unwrap();
            }
            let stopped = tokio::time::timeout(Duration::from_secs(6), async {
                while alive(pid).await {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await;
            if stopped.is_err() {
                task.abort();
            }
            stopped.expect("the owned shell process must actually stop despite blocked output");
            if mode == "close" {
                assert!(!task.is_finished(), "stdin is still open after close");
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        line.clear();
                        output.read_line(&mut line).await.unwrap();
                        let event: Value = serde_json::from_str(&line).unwrap();
                        if event["requestId"] == "close" {
                            assert!(event["error"].is_null());
                            break;
                        }
                    }
                })
                .await
                .unwrap();
                input.shutdown().await.unwrap();
                // Keep consuming so the EOF acknowledgement can be delivered.
                tokio::spawn(async move {
                    let mut sink = Vec::new();
                    let _ = output.read_to_end(&mut sink).await;
                });
                assert!(tokio::time::timeout(Duration::from_secs(5), &mut task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_ok());
            } else {
                let result = tokio::time::timeout(Duration::from_secs(5), &mut task)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    result.is_err(),
                    "stalled or overflowed responses must not be acknowledged as clean EOF"
                );
                if mode == "overflow" {
                    assert!(result.unwrap_err().to_string().contains("response queue"));
                }
            }
        }
    }

    #[tokio::test]
    async fn eof_ack_requires_confirmed_clean_input_and_not_signal_shutdown() {
        async fn capture(input: &[u8], eof: Option<Arc<AtomicBool>>) -> (Result<()>, String) {
            let (output, mut client) = tokio::io::duplex(4096);
            let result = serve(input, output, eof).await;
            let mut frames = String::new();
            client.read_to_string(&mut frames).await.unwrap();
            (result, frames)
        }
        let (result, frames) = capture(b"", Some(Arc::new(AtomicBool::new(true)))).await;
        assert!(result.is_ok());
        assert_eq!(
            serde_json::from_str::<Value>(frames.trim()).unwrap(),
            json!({"kind":"shutdown","reason":"eof"})
        );
        let (result, frames) = capture(b"", Some(Arc::new(AtomicBool::new(false)))).await;
        assert!(result.is_ok());
        assert!(frames.is_empty());
        let (result, frames) = capture(b"{}\n", Some(Arc::new(AtomicBool::new(true)))).await;
        assert!(result.is_err());
        assert!(frames.is_empty());

        let (input, _open_input) = tokio::io::duplex(64);
        let (output, mut client) = tokio::io::duplex(4096);
        assert!(serve_until(input, output, None, std::future::ready(()))
            .await
            .is_ok());
        let mut frames = String::new();
        client.read_to_string(&mut frames).await.unwrap();
        assert!(frames.is_empty());
    }

    #[test]
    fn stdin_reader_distinguishes_eof_error_and_receiver_close() {
        struct DataThenError(bool);
        impl std::io::Read for DataThenError {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    Err(std::io::Error::other("fixture read error"))
                } else {
                    self.0 = true;
                    buffer[..4].copy_from_slice(b"data");
                    Ok(4)
                }
            }
        }
        let (sender, mut receiver) = mpsc::channel(1);
        let eof = AtomicBool::new(false);
        forward_terminal_stdin(&mut std::io::Cursor::new(b"data"), sender, &eof);
        assert_eq!(receiver.try_recv().unwrap(), b"data");
        assert!(eof.load(Ordering::Acquire));

        let (sender, mut receiver) = mpsc::channel(1);
        let eof = AtomicBool::new(false);
        forward_terminal_stdin(&mut DataThenError(false), sender, &eof);
        assert_eq!(receiver.try_recv().unwrap(), b"data");
        assert!(!eof.load(Ordering::Acquire));

        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let eof = AtomicBool::new(false);
        forward_terminal_stdin(&mut std::io::Cursor::new(b"data"), sender, &eof);
        assert!(!eof.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn eof_closes_real_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let request = json!({"requestId":"1","kind":"create","terminalId":"eof","cwd":directory.path(),"cols":80,"rows":24});
        let input = format!("{request}\n");
        tokio::time::timeout(
            Duration::from_secs(5),
            serve(input.as_bytes(), tokio::io::sink(), None),
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
            tokio::time::timeout(Duration::from_secs(5), terminals["jobs"].handle.wait())
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
