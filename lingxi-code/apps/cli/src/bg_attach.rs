//! Live background-session attach transport.
//!
//! Claude Code's background-agent roster advertises authenticated
//! rendezvous/PTY sockets so `claude agents` can attach to a worker that is
//! still running instead of spawning a second `--resume` writer. LingXi's
//! worker is still headless, but it now exposes the same live attach primitive:
//! an authenticated Unix-domain socket that replays buffered output, streams
//! subsequent output, and forwards attached terminal input/control back to the
//! live worker.

use crate::agents_registry;
use std::path::{Path, PathBuf};

/// Environment key carrying the worker's live attach socket path.
pub const ATTACH_SOCK_ENV: &str = "LINGXI_BG_ATTACH_SOCK";
/// Environment key carrying the bearer token required by the attach socket.
pub const ATTACH_AUTH_ENV: &str = "LINGXI_BG_ATTACH_AUTH";

/// Where to durably persist a follow-up reply whose LIVE delivery to a worker
/// fails (the worker vanished mid-attach). Lets [`attach_to_socket`] fall back
/// to the offline reply queue instead of dropping the line — the
/// "失败投递无法持久化" (a failed delivery that cannot be persisted) harm.
#[derive(Debug, Clone)]
pub struct ReplyFallback {
    /// Config home whose `jobs/<short>/replies/` queue the reply is written to.
    pub config_home: PathBuf,
    /// The background job short id owning the queue.
    pub short: String,
}

/// Input/control frames received from an attached terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachInput {
    /// A complete user input line, without trailing CR/LF.
    Line(String),
    /// User interrupt (`Ctrl-C`).
    Interrupt,
    /// One attach client disconnected.
    ClientDetached,
}

/// Stable socket location for a background job.
#[must_use]
pub fn socket_path(runtime_dir: &Path, short: &str) -> PathBuf {
    agents_registry::jobs_dir(runtime_dir)
        .join(short)
        .join("attach.sock")
}

#[cfg(unix)]
mod unix {
    use super::{AttachInput, ATTACH_AUTH_ENV, ATTACH_SOCK_ENV};
    use crate::output::OutputSink;
    use async_trait::async_trait;
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::fs;
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, Weak};
    use std::thread;
    use std::time::Duration;
    use tokio::sync::mpsc;

    const MAX_REPLAY_BYTES: usize = 1024 * 1024;

    struct Client {
        id: u64,
        stream: UnixStream,
    }

    struct AttachHubInner {
        path: PathBuf,
        auth: String,
        clients: Mutex<Vec<Client>>,
        transcript: Mutex<Vec<u8>>,
        input_tx: mpsc::UnboundedSender<AttachInput>,
        input_rx: Mutex<Option<mpsc::UnboundedReceiver<AttachInput>>>,
        next_client_id: AtomicU64,
        stop: AtomicBool,
    }

    impl Drop for AttachHubInner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = fs::remove_file(&self.path);
        }
    }

    /// Output fan-out for one live background worker.
    #[derive(Clone)]
    pub struct AttachHub {
        inner: Arc<AttachHubInner>,
    }

    impl AttachHub {
        /// Start an authenticated Unix-domain attach socket.
        pub fn start(path: PathBuf, auth: String) -> io::Result<Self> {
            if auth.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty attach auth token",
                ));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let _ = fs::remove_file(&path);
            let listener = UnixListener::bind(&path)?;
            listener.set_nonblocking(true)?;
            let (input_tx, input_rx) = mpsc::unbounded_channel();

            let inner = Arc::new(AttachHubInner {
                path,
                auth,
                clients: Mutex::new(Vec::new()),
                transcript: Mutex::new(Vec::new()),
                input_tx,
                input_rx: Mutex::new(Some(input_rx)),
                next_client_id: AtomicU64::new(1),
                stop: AtomicBool::new(false),
            });
            let weak = Arc::downgrade(&inner);
            thread::Builder::new()
                .name("lingxi-bg-attach".to_string())
                .spawn(move || accept_loop(listener, weak))?;
            Ok(Self { inner })
        }

        /// Start from the daemon-provided worker environment. Missing env means
        /// the worker was not spawned as an attachable background worker.
        pub fn start_from_env() -> io::Result<Option<Self>> {
            let Ok(path) = std::env::var(ATTACH_SOCK_ENV) else {
                return Ok(None);
            };
            let auth = std::env::var(ATTACH_AUTH_ENV).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{ATTACH_AUTH_ENV} missing while {ATTACH_SOCK_ENV} is set"),
                )
            })?;
            Self::start(PathBuf::from(path), auth).map(Some)
        }

        /// Take the worker-side input receiver. There is only one consumer: the
        /// live background worker turn loop.
        pub fn take_input_rx(&self) -> Option<mpsc::UnboundedReceiver<AttachInput>> {
            self.inner.input_rx.lock().unwrap().take()
        }

        /// Whether at least one attach client is currently connected.
        #[must_use]
        pub fn has_clients(&self) -> bool {
            !self.inner.clients.lock().unwrap().is_empty()
        }

        /// Broadcast bytes to future and already-attached clients.
        pub fn broadcast(&self, bytes: &[u8]) {
            if bytes.is_empty() {
                return;
            }
            {
                let mut transcript = self.inner.transcript.lock().unwrap();
                transcript.extend_from_slice(bytes);
                if transcript.len() > MAX_REPLAY_BYTES {
                    let overflow = transcript.len() - MAX_REPLAY_BYTES;
                    transcript.drain(..overflow);
                }
            }

            let mut clients = self.inner.clients.lock().unwrap();
            let mut i = 0;
            while i < clients.len() {
                let ok = clients[i]
                    .stream
                    .write_all(bytes)
                    .and_then(|()| clients[i].stream.flush())
                    .is_ok();
                if ok {
                    i += 1;
                } else {
                    clients.remove(i);
                }
            }
        }
    }

    impl AttachHubInner {
        fn remove_client(&self, id: u64) {
            self.clients
                .lock()
                .unwrap()
                .retain(|client| client.id != id);
        }

        fn send_input(&self, input: AttachInput) {
            let _ = self.input_tx.send(input);
        }
    }

    fn accept_loop(listener: UnixListener, inner: Weak<AttachHubInner>) {
        loop {
            let Some(hub) = inner.upgrade() else {
                return;
            };
            if hub.stop.load(Ordering::Relaxed) {
                return;
            }
            match listener.accept() {
                Ok((stream, _addr)) => {
                    accept_client(&hub, stream);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    drop(hub);
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return,
            }
        }
    }

    fn accept_client(hub: &Arc<AttachHubInner>, mut stream: UnixStream) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(auth_line) = read_auth_line(&mut stream) else {
            return;
        };
        if auth_line.trim_end_matches(['\r', '\n']) != hub.auth {
            let _ = stream.write_all(b"unauthorized\n");
            let _ = stream.flush();
            return;
        }
        let _ = stream.set_read_timeout(None);

        let transcript = hub.transcript.lock().unwrap().clone();
        if !transcript.is_empty() && stream.write_all(&transcript).is_err() {
            return;
        }
        let _ = stream.flush();
        let Ok(reader_stream) = stream.try_clone() else {
            return;
        };
        let id = hub.next_client_id.fetch_add(1, Ordering::Relaxed);
        hub.clients.lock().unwrap().push(Client { id, stream });

        let weak = Arc::downgrade(hub);
        let _ = thread::Builder::new()
            .name("lingxi-bg-attach-input".to_string())
            .spawn(move || client_input_loop(weak, id, reader_stream));
    }

    fn read_auth_line(stream: &mut UnixStream) -> io::Result<String> {
        let mut bytes = Vec::new();
        let mut buf = [0_u8; 1];
        while bytes.len() < 4096 {
            stream.read_exact(&mut buf)?;
            bytes.push(buf[0]);
            if buf[0] == b'\n' {
                return Ok(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "attach auth line too long",
        ))
    }

    fn client_input_loop(hub: Weak<AttachHubInner>, id: u64, mut stream: UnixStream) {
        let mut line = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_client(id);
                        hub.send_input(AttachInput::ClientDetached);
                    }
                    return;
                }
                Ok(n) => {
                    let Some(hub) = hub.upgrade() else {
                        return;
                    };
                    for byte in &buf[..n] {
                        match *byte {
                            b'\n' => {
                                let input = String::from_utf8_lossy(&line).into_owned();
                                line.clear();
                                hub.send_input(AttachInput::Line(input));
                            }
                            b'\r' => {}
                            0x03 => hub.send_input(AttachInput::Interrupt), // Ctrl-C
                            0x04 => {
                                hub.remove_client(id);
                                hub.send_input(AttachInput::ClientDetached);
                                return;
                            }
                            0x08 | 0x7f => {
                                line.pop();
                            }
                            b => line.push(b),
                        }
                    }
                }
                Err(_) => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_client(id);
                        hub.send_input(AttachInput::ClientDetached);
                    }
                    return;
                }
            }
        }
    }

    /// Sink that mirrors plain CLI output to the live attach hub.
    pub struct AttachSink {
        hub: AttachHub,
    }

    impl AttachSink {
        #[must_use]
        pub fn new(hub: AttachHub) -> Self {
            Self { hub }
        }
    }

    #[async_trait]
    impl OutputSink for AttachSink {
        async fn text(&self, s: &str) {
            self.hub.broadcast(s.as_bytes());
        }

        async fn turn_start(&self) {}

        async fn turn_end(&self, _stop_reason: &str, _usd: f64, _in_tokens: u64, _out_tokens: u64) {
        }

        async fn tool_call(&self, tool: &str, _input: &serde_json::Value) {
            self.hub.broadcast(format!("[tool: {tool}]\n").as_bytes());
        }

        async fn tool_result(&self, _tool: &str, _result: &serde_json::Value) {}

        async fn command_output(&self, _name: &str, display: &str) {
            self.hub.broadcast(format!("{display}\n").as_bytes());
        }

        async fn error(&self, _code: &str, message: &str) {
            self.hub
                .broadcast(format!("lingxi-cli: {message}\n").as_bytes());
        }
    }

    struct RawModeGuard {
        enabled: bool,
    }

    impl RawModeGuard {
        fn enable() -> io::Result<Self> {
            enable_raw_mode()?;
            Ok(Self { enabled: true })
        }
    }

    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            if self.enabled {
                let _ = disable_raw_mode();
            }
        }
    }

    fn write_terminal_output(stdout: &mut io::StdoutLock<'_>, bytes: &[u8], prev: &mut u8) {
        for byte in bytes {
            if *byte == b'\n' && *prev != b'\r' {
                let _ = stdout.write_all(b"\r\n");
            } else {
                let _ = stdout.write_all(&[*byte]);
            }
            *prev = *byte;
        }
        let _ = stdout.flush();
    }

    fn echo_char(stdout: &mut io::StdoutLock<'_>, c: char) {
        let mut buf = [0_u8; 4];
        let _ = stdout.write_all(c.encode_utf8(&mut buf).as_bytes());
        let _ = stdout.flush();
    }

    fn echo_backspace(stdout: &mut io::StdoutLock<'_>) {
        let _ = stdout.write_all(b"\x08 \x08");
        let _ = stdout.flush();
    }

    fn echo_newline(stdout: &mut io::StdoutLock<'_>) {
        let _ = stdout.write_all(b"\r\n");
        let _ = stdout.flush();
    }

    fn is_detach_command(line: &str) -> bool {
        matches!(line.trim(), "/exit" | "/quit")
    }

    /// Connect to a live background worker and run a small PTY-like pump: worker
    /// output is copied to stdout, while local key events are line-edited and
    /// forwarded as input/control frames over the same socket.
    ///
    /// If the worker vanishes mid-attach and a pending input line therefore fails
    /// to deliver, that line is persisted to the durable offline reply queue
    /// (`fallback`) instead of being dropped — so a later worker respawn still
    /// delivers it. The same applies to a line still being edited when the socket
    /// closes.
    pub fn attach_to_socket(
        path: &std::path::Path,
        auth: &str,
        fallback: Option<&super::ReplyFallback>,
    ) -> io::Result<()> {
        let mut stream = UnixStream::connect(path)?;
        stream.write_all(auth.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        stream.set_read_timeout(Some(Duration::from_millis(10)))?;
        let _raw = RawModeGuard::enable()?;

        let mut stdout = io::stdout().lock();
        let mut socket_buf = [0_u8; 8192];
        let mut line = String::new();
        let mut prev_output = 0_u8;
        loop {
            match stream.read(&mut socket_buf) {
                // Worker closed the socket. If a line was mid-edit, persist it so
                // it is not lost.
                Ok(0) => {
                    persist_pending_line(&line, fallback);
                    return Ok(());
                }
                Ok(n) => write_terminal_output(&mut stdout, &socket_buf[..n], &mut prev_output),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e),
            }

            if event::poll(Duration::from_millis(10))? {
                let Event::Key(key) = event::read()? else {
                    continue;
                };
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        stream.write_all(&[0x03])?;
                        stream.flush()?;
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let _ = stream.shutdown(Shutdown::Both);
                        return Ok(());
                    }
                    KeyCode::Char(c) => {
                        line.push(c);
                        echo_char(&mut stdout, c);
                    }
                    KeyCode::Backspace => {
                        if !line.is_empty() {
                            line.pop();
                            echo_backspace(&mut stdout);
                        }
                    }
                    KeyCode::Enter => {
                        echo_newline(&mut stdout);
                        if is_detach_command(&line) {
                            let _ = stream.shutdown(Shutdown::Both);
                            return Ok(());
                        }
                        // A failed live delivery (BrokenPipe → the worker
                        // vanished) is persisted to the offline queue instead of
                        // aborting the attach with an error.
                        let sent = stream
                            .write_all(line.as_bytes())
                            .and_then(|()| stream.write_all(b"\n"))
                            .and_then(|()| stream.flush());
                        if sent.is_err() {
                            persist_pending_line(&line, fallback);
                            return Ok(());
                        }
                        line.clear();
                    }
                    KeyCode::Esc => {
                        let _ = stream.shutdown(Shutdown::Both);
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }

    /// Persist a non-empty pending input `line` to the durable offline reply
    /// queue when a `fallback` target is known. Best-effort: a queue-write error
    /// is logged, not surfaced (the attach itself is already ending).
    fn persist_pending_line(line: &str, fallback: Option<&super::ReplyFallback>) {
        if line.trim().is_empty() {
            return;
        }
        let Some(fb) = fallback else {
            return;
        };
        match crate::bg_reply_queue::enqueue_reply(&fb.config_home, &fb.short, line) {
            Ok(_) => {
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(
                    b"\r\n[worker unavailable \xe2\x80\x94 reply queued for delivery on respawn]\r\n",
                );
                let _ = stdout.flush();
            }
            Err(e) => tracing::warn!("lingxi-cli attach: could not queue offline reply: {e}"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Read;
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn short_socket_path() -> PathBuf {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            PathBuf::from(format!("/tmp/lx-bg-{}-{n}.sock", std::process::id()))
        }

        #[test]
        fn attach_socket_replays_prior_output_and_streams_future_output() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            hub.broadcast(b"before\n");

            let mut client = UnixStream::connect(&path).unwrap();
            client.write_all(b"token-1\n").unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buf = [0_u8; 7];
            client.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, b"before\n");

            hub.broadcast(b"after\n");
            let mut buf = [0_u8; 6];
            client.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, b"after\n");
        }

        #[test]
        fn attach_socket_forwards_input_lines_and_interrupts() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mut rx = hub.take_input_rx().unwrap();

            let mut client = UnixStream::connect(&path).unwrap();
            client.write_all(b"token-1\n").unwrap();
            client.write_all(b"hello worker\n").unwrap();
            client.write_all(&[0x03]).unwrap();

            assert_eq!(
                rx.blocking_recv(),
                Some(AttachInput::Line("hello worker".to_string()))
            );
            assert_eq!(rx.blocking_recv(), Some(AttachInput::Interrupt));
        }
    }
}

#[cfg(unix)]
pub use unix::{attach_to_socket, AttachHub, AttachSink};

#[cfg(not(unix))]
mod non_unix {
    use super::{ATTACH_AUTH_ENV, ATTACH_SOCK_ENV};
    use crate::output::OutputSink;
    use async_trait::async_trait;
    use std::io;
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc;

    #[derive(Clone)]
    pub struct AttachHub;

    impl AttachHub {
        pub fn start(_path: PathBuf, _auth: String) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "live background attach is only supported on Unix",
            ))
        }

        pub fn start_from_env() -> io::Result<Option<Self>> {
            if std::env::var(ATTACH_SOCK_ENV).is_ok() || std::env::var(ATTACH_AUTH_ENV).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "live background attach is only supported on Unix",
                ));
            }
            Ok(None)
        }

        pub fn take_input_rx(&self) -> Option<mpsc::UnboundedReceiver<super::AttachInput>> {
            None
        }

        #[must_use]
        pub fn has_clients(&self) -> bool {
            false
        }

        pub fn broadcast(&self, _bytes: &[u8]) {}
    }

    pub struct AttachSink;

    impl AttachSink {
        #[must_use]
        pub fn new(_hub: AttachHub) -> Self {
            Self
        }
    }

    #[async_trait]
    impl OutputSink for AttachSink {
        async fn text(&self, _s: &str) {}
        async fn turn_start(&self) {}
        async fn turn_end(&self, _stop_reason: &str, _usd: f64, _in_tokens: u64, _out_tokens: u64) {
        }
        async fn tool_call(&self, _tool: &str, _input: &serde_json::Value) {}
        async fn tool_result(&self, _tool: &str, _result: &serde_json::Value) {}
        async fn command_output(&self, _name: &str, _display: &str) {}
        async fn error(&self, _code: &str, _message: &str) {}
    }

    pub fn attach_to_socket(
        _path: &Path,
        _auth: &str,
        _fallback: Option<&super::ReplyFallback>,
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "live background attach is only supported on Unix",
        ))
    }
}

#[cfg(not(unix))]
pub use non_unix::{attach_to_socket, AttachHub, AttachSink};
