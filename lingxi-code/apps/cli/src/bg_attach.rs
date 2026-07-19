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
    /// Raw terminal input bytes from the attached client.
    Bytes(Vec<u8>),
    /// Terminal resize (`SIGWINCH`) from the attached client.
    Resize { cols: u16, rows: u16 },
    /// User interrupt (`Ctrl-C`).
    Interrupt,
    /// Client-requested detach.
    Detach,
    /// Client EOF (`Ctrl-D`).
    Eof,
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
    /// Per-write timeout on an attached client socket (review #5/#6): a client
    /// that stops draining is dropped rather than blocking the worker forever.
    const ATTACH_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
    const MAX_FRAME_BYTES: usize = 1024 * 1024;
    const FRAME_OUTPUT: u8 = 1;
    const FRAME_INPUT_BYTES: u8 = 2;
    const FRAME_RESIZE: u8 = 3;
    const FRAME_INTERRUPT: u8 = 4;
    const FRAME_DETACH: u8 = 5;
    const FRAME_EOF: u8 = 6;

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
                let ok = write_frame(&mut clients[i].stream, FRAME_OUTPUT, bytes)
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
        // (review #5, #6) Bound EVERY write to this client. Without it, a client
        // that authenticates then stops draining its socket (paused terminal,
        // XOFF flow-control, SIGSTOP) makes the blocking `write_all` in the
        // replay below — and later in `broadcast` while it holds the `clients`
        // Mutex — block indefinitely, wedging the worker's turn output path and
        // its tokio executor thread. With a write timeout the stalled write
        // errors out and the client is dropped (replay: early return; broadcast:
        // the `else { clients.remove(i) }` arm), so one bad local client can no
        // longer hang the live worker.
        let _ = stream.set_write_timeout(Some(ATTACH_WRITE_TIMEOUT));
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
        if !transcript.is_empty() && write_frame(&mut stream, FRAME_OUTPUT, &transcript).is_err() {
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
        loop {
            match read_frame(&mut stream) {
                Ok(Some((FRAME_INPUT_BYTES, payload))) => {
                    let Some(hub) = hub.upgrade() else {
                        return;
                    };
                    hub.send_input(AttachInput::Bytes(payload));
                }
                Ok(Some((FRAME_RESIZE, payload))) => {
                    if payload.len() != 4 {
                        continue;
                    }
                    let Some(hub) = hub.upgrade() else {
                        return;
                    };
                    hub.send_input(AttachInput::Resize {
                        cols: u16::from_be_bytes([payload[0], payload[1]]),
                        rows: u16::from_be_bytes([payload[2], payload[3]]),
                    });
                }
                Ok(Some((FRAME_INTERRUPT, _))) => {
                    let Some(hub) = hub.upgrade() else {
                        return;
                    };
                    hub.send_input(AttachInput::Interrupt);
                }
                Ok(Some((FRAME_DETACH, _))) => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_client(id);
                        hub.send_input(AttachInput::Detach);
                        hub.send_input(AttachInput::ClientDetached);
                    }
                    return;
                }
                Ok(Some((FRAME_EOF, _))) => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_client(id);
                        hub.send_input(AttachInput::Eof);
                        hub.send_input(AttachInput::ClientDetached);
                    }
                    return;
                }
                Ok(Some((_other, _payload))) => {}
                Ok(None) | Err(_) => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_client(id);
                        hub.send_input(AttachInput::ClientDetached);
                    }
                    return;
                }
            }
        }
    }

    fn write_frame(stream: &mut UnixStream, kind: u8, payload: &[u8]) -> io::Result<()> {
        let len = u32::try_from(payload.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "attach frame exceeds u32 length")
        })?;
        stream.write_all(&[kind])?;
        stream.write_all(&len.to_be_bytes())?;
        stream.write_all(payload)
    }

    fn read_frame(stream: &mut UnixStream) -> io::Result<Option<(u8, Vec<u8>)>> {
        let mut header = [0_u8; 5];
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
        let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attach frame too large",
            ));
        }
        let mut payload = vec![0_u8; len];
        stream.read_exact(&mut payload)?;
        Ok(Some((header[0], payload)))
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

    fn is_detach_command(line: &str) -> bool {
        matches!(line.trim(), "/exit" | "/quit")
    }

    #[derive(Default)]
    struct PendingLine {
        bytes: Vec<u8>,
    }

    impl PendingLine {
        fn push_bytes(&mut self, bytes: &[u8]) {
            for byte in bytes {
                match *byte {
                    b'\r' | b'\n' => self.bytes.clear(),
                    0x08 | 0x7f => {
                        self.bytes.pop();
                    }
                    b if b >= 0x20 || b == b'\t' => self.bytes.push(b),
                    _ => {}
                }
            }
        }

        fn as_string(&self) -> String {
            String::from_utf8_lossy(&self.bytes).into_owned()
        }
    }

    fn resize_payload(cols: u16, rows: u16) -> [u8; 4] {
        let [c0, c1] = cols.to_be_bytes();
        let [r0, r1] = rows.to_be_bytes();
        [c0, c1, r0, r1]
    }

    fn encode_key_bytes(key: &crossterm::event::KeyEvent) -> Option<Vec<u8>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let mut out = Vec::new();
        match key.code {
            KeyCode::Char('c') if ctrl => return None,
            KeyCode::Char('d') if ctrl => return None,
            KeyCode::Char(c) => {
                if alt {
                    out.push(0x1b);
                }
                if ctrl && c.is_ascii() {
                    let lower = c.to_ascii_lowercase() as u8;
                    if (b'a'..=b'z').contains(&lower) {
                        out.push(lower - b'a' + 1);
                    } else {
                        out.push(lower);
                    }
                } else {
                    let mut buf = [0_u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
            KeyCode::Enter => out.push(b'\n'),
            KeyCode::Tab => out.push(b'\t'),
            KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
            KeyCode::Backspace => out.push(0x7f),
            KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
            KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
            KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
            KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
            KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
            KeyCode::End => out.extend_from_slice(b"\x1b[F"),
            KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
            KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
            KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
            KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
            KeyCode::Esc => out.push(0x1b),
            _ => return None,
        }
        Some(out)
    }

    /// Connect to a live background worker and run a framed PTY-like pump:
    /// worker output is copied to stdout, while local terminal bytes/control are
    /// forwarded over the same socket.
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
        if let Ok((cols, rows)) = crossterm::terminal::size() {
            let payload = resize_payload(cols, rows);
            let _ = write_frame(&mut stream, FRAME_RESIZE, &payload).and_then(|()| stream.flush());
        }

        let mut stdout = io::stdout().lock();
        let mut prev_output = 0_u8;
        let mut pending_line = PendingLine::default();
        loop {
            match read_frame(&mut stream) {
                Ok(None) => {
                    persist_pending_line(&pending_line.as_string(), fallback);
                    return Ok(());
                }
                Ok(Some((FRAME_OUTPUT, payload))) => {
                    write_terminal_output(&mut stdout, &payload, &mut prev_output);
                }
                Ok(Some((_other, _payload))) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e),
            }

            if event::poll(Duration::from_millis(10))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                write_frame(&mut stream, FRAME_INTERRUPT, &[])?;
                                stream.flush()?;
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                let _ = write_frame(&mut stream, FRAME_EOF, &[]);
                                let _ = stream.flush();
                                let _ = stream.shutdown(Shutdown::Both);
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                let _ = write_frame(&mut stream, FRAME_DETACH, &[]);
                                let _ = stream.flush();
                                let _ = stream.shutdown(Shutdown::Both);
                                return Ok(());
                            }
                            KeyCode::Enter => {
                                if is_detach_command(&pending_line.as_string()) {
                                    let _ = write_frame(&mut stream, FRAME_DETACH, &[]);
                                    let _ = stream.flush();
                                    let _ = stream.shutdown(Shutdown::Both);
                                    return Ok(());
                                }
                                if let Some(bytes) = encode_key_bytes(&key) {
                                    let submitted = pending_line.as_string();
                                    pending_line.push_bytes(&bytes);
                                    if write_frame(&mut stream, FRAME_INPUT_BYTES, &bytes)
                                        .and_then(|()| stream.flush())
                                        .is_err()
                                    {
                                        persist_pending_line(&submitted, fallback);
                                        return Ok(());
                                    }
                                }
                            }
                            _ => {
                                let Some(bytes) = encode_key_bytes(&key) else {
                                    continue;
                                };
                                pending_line.push_bytes(&bytes);
                                if write_frame(&mut stream, FRAME_INPUT_BYTES, &bytes)
                                    .and_then(|()| stream.flush())
                                    .is_err()
                                {
                                    persist_pending_line(&pending_line.as_string(), fallback);
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Event::Paste(data) => {
                        let bytes = data.into_bytes();
                        pending_line.push_bytes(&bytes);
                        if write_frame(&mut stream, FRAME_INPUT_BYTES, &bytes)
                            .and_then(|()| stream.flush())
                            .is_err()
                        {
                            persist_pending_line(&pending_line.as_string(), fallback);
                            return Ok(());
                        }
                    }
                    Event::Resize(cols, rows) => {
                        let payload = resize_payload(cols, rows);
                        let _ = write_frame(&mut stream, FRAME_RESIZE, &payload)
                            .and_then(|()| stream.flush());
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
                // Honest wording: a vanished worker is failed closed and never
                // respawns, so the daemon does NOT re-run this reply — it drains
                // the queue when it reaps the job and records it as undelivered.
                // Do not promise "delivery on respawn" the policy can't keep.
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(
                    b"\r\n[worker unavailable \xe2\x80\x94 reply saved to the job's offline queue]\r\n",
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
            let (kind, payload) = read_frame(&mut client).unwrap().unwrap();
            assert_eq!(kind, FRAME_OUTPUT);
            assert_eq!(payload, b"before\n");

            hub.broadcast(b"after\n");
            let (kind, payload) = read_frame(&mut client).unwrap().unwrap();
            assert_eq!(kind, FRAME_OUTPUT);
            assert_eq!(payload, b"after\n");
        }

        #[test]
        fn attach_socket_forwards_raw_input_and_controls() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mut rx = hub.take_input_rx().unwrap();

            let mut client = UnixStream::connect(&path).unwrap();
            client.write_all(b"token-1\n").unwrap();
            write_frame(&mut client, FRAME_INPUT_BYTES, b"hello worker\n").unwrap();
            write_frame(&mut client, FRAME_RESIZE, &resize_payload(120, 40)).unwrap();
            write_frame(&mut client, FRAME_INTERRUPT, &[]).unwrap();
            write_frame(&mut client, FRAME_DETACH, &[]).unwrap();

            assert_eq!(
                rx.blocking_recv(),
                Some(AttachInput::Bytes(b"hello worker\n".to_vec()))
            );
            assert_eq!(
                rx.blocking_recv(),
                Some(AttachInput::Resize { cols: 120, rows: 40 })
            );
            assert_eq!(rx.blocking_recv(), Some(AttachInput::Interrupt));
            assert_eq!(rx.blocking_recv(), Some(AttachInput::Detach));
            assert_eq!(rx.blocking_recv(), Some(AttachInput::ClientDetached));
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
