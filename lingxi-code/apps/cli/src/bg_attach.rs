//! Authenticated live background-session terminal transport.
//!
//! Protocol v2 deliberately carries terminal bytes, not decoded key events or
//! newline-delimited messages.  A background worker owns the PTY and this
//! module provides a single-controller rendezvous point for it.  Disconnecting
//! the controller never implies that the PTY child should exit.

use crate::agents_registry;
use std::io;
use std::path::{Path, PathBuf};

/// Environment key carrying the worker's live attach endpoint.
pub const ATTACH_SOCK_ENV: &str = "LINGXI_BG_ATTACH_SOCK";
/// Environment key carrying the bearer token required by the attach endpoint.
pub const ATTACH_AUTH_ENV: &str = "LINGXI_BG_ATTACH_AUTH";

fn decode_exit_frame(payload: &[u8]) -> io::Result<()> {
    let bytes: [u8; 4] = payload.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "background PTY sent an invalid EXIT frame",
        )
    })?;
    let code = i32::from_be_bytes(bytes);
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "background PTY exited with code {code}"
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalInputEnd {
    Active,
    DetachSent,
    Failed,
}

fn closed_connection_result(state: LocalInputEnd) -> io::Result<()> {
    match state {
        LocalInputEnd::DetachSent => Ok(()),
        LocalInputEnd::Failed => Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "local terminal input stopped while attached",
        )),
        LocalInputEnd::Active => Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "background PTY connection closed without EXIT or local detach",
        )),
    }
}

/// Input/control frames received from the attached terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachInput {
    /// Raw terminal input.  This includes Esc, Ctrl-C and Ctrl-D.
    Bytes(Vec<u8>),
    /// Terminal resize (`SIGWINCH`/ConPTY resize) from the attached client.
    Resize { cols: u16, rows: u16 },
    /// The controller explicitly detached (Ctrl-]).
    Detach,
    /// The controller connection ended, cleanly or unexpectedly.
    ClientDetached,
}

#[cfg(test)]
mod common_tests {
    use super::*;

    #[test]
    fn exit_frame_preserves_success_and_failure_status() {
        assert!(decode_exit_frame(&0_i32.to_be_bytes()).is_ok());
        let error = decode_exit_frame(&17_i32.to_be_bytes()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("17"));
        assert_eq!(
            decode_exit_frame(&[0, 1]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn only_explicit_local_detach_makes_bare_eof_successful() {
        assert!(closed_connection_result(LocalInputEnd::DetachSent).is_ok());
        assert_eq!(
            closed_connection_result(LocalInputEnd::Active)
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert_eq!(
            closed_connection_result(LocalInputEnd::Failed)
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
    }
}

/// Stable endpoint location for a background job.
#[must_use]
pub fn socket_path(runtime_dir: &Path, short: &str) -> PathBuf {
    #[cfg(windows)]
    {
        // Named pipes are global kernel objects, so include a deterministic
        // hash of the config home to isolate parallel users/installations.
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in runtime_dir.as_os_str().to_string_lossy().bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        PathBuf::from(format!(r"\\.\pipe\lingxi-{hash:016x}-{short}"))
    }
    #[cfg(not(windows))]
    {
        agents_registry::jobs_dir(runtime_dir)
            .join(short)
            .join("attach.sock")
    }
}

#[cfg(unix)]
mod unix {
    use super::{
        closed_connection_result, decode_exit_frame, AttachInput, LocalInputEnd, ATTACH_AUTH_ENV,
        ATTACH_SOCK_ENV,
    };
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use nix::poll::{poll, PollFd, PollFlags};
    use std::fs;
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex, Weak};
    use std::thread;
    use std::time::Duration;
    use tokio::sync::mpsc;

    const PROTOCOL_MAGIC: &[u8; 8] = b"LXPTY2\0\0";
    const PROTOCOL_VERSION: u16 = 2;
    const MAX_AUTH_BYTES: usize = 4096;
    const MAX_FRAME_BYTES: usize = 1024 * 1024;
    const INPUT_CHANNEL_CAPACITY: usize = 128;
    const OUTPUT_CHANNEL_CAPACITY: usize = 128;
    const OUTPUT_CHUNK_BYTES: usize = 8192;
    const ATTACH_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
    const CLIENT_POLL_INTERVAL: Duration = Duration::from_millis(40);
    const DETACH_BYTE: u8 = 0x1d; // Ctrl-]

    const FRAME_OUTPUT: u8 = 1;
    const FRAME_INPUT_BYTES: u8 = 2;
    const FRAME_RESIZE: u8 = 3;
    const FRAME_DETACH: u8 = 5;
    const FRAME_READY: u8 = 7;
    const FRAME_EXIT: u8 = 8;
    const FRAME_ERROR: u8 = 9;

    #[derive(Debug)]
    struct Client {
        id: u64,
        writer: Arc<Mutex<UnixStream>>,
        shutdown: UnixStream,
    }

    #[derive(Debug)]
    enum Controller {
        Vacant,
        Reserved { id: u64 },
        Active(Client),
    }

    struct ServerFrame {
        controller_id: u64,
        kind: u8,
        payload: Vec<u8>,
        close_after: bool,
        completion: Option<std_mpsc::SyncSender<()>>,
    }

    struct AttachHubInner {
        path: PathBuf,
        auth: String,
        session_label: String,
        controller: Mutex<Controller>,
        input_tx: mpsc::Sender<AttachInput>,
        input_rx: Mutex<Option<mpsc::Receiver<AttachInput>>>,
        output_tx: std_mpsc::SyncSender<ServerFrame>,
        next_client_id: AtomicU64,
        stop: AtomicBool,
    }

    impl Drop for AttachHubInner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Ok(controller) = self.controller.get_mut() {
                if let Controller::Active(client) =
                    std::mem::replace(controller, Controller::Vacant)
                {
                    let _ = client.shutdown.shutdown(Shutdown::Both);
                }
            }
            let _ = fs::remove_file(&self.path);
        }
    }

    /// Output/control surface for one live background PTY worker.
    #[derive(Clone)]
    pub struct AttachHub {
        inner: Arc<AttachHubInner>,
    }

    impl AttachHub {
        /// Start a protocol-v2 authenticated Unix-domain attach socket.
        pub fn start(path: PathBuf, auth: String) -> io::Result<Self> {
            if auth.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty attach auth token",
                ));
            }
            if auth.len() > MAX_AUTH_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "attach auth token is too long",
                ));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            remove_stale_socket(&path)?;
            let listener = UnixListener::bind(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
            let (output_tx, output_rx) = std_mpsc::sync_channel(OUTPUT_CHANNEL_CAPACITY);
            let session_label = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("background")
                .to_owned();

            let inner = Arc::new(AttachHubInner {
                path,
                auth,
                session_label,
                controller: Mutex::new(Controller::Vacant),
                input_tx,
                input_rx: Mutex::new(Some(input_rx)),
                output_tx,
                next_client_id: AtomicU64::new(1),
                stop: AtomicBool::new(false),
            });
            let weak = Arc::downgrade(&inner);
            thread::Builder::new()
                .name("lingxi-bg-attach".to_string())
                .spawn(move || accept_loop(listener, weak))?;
            let weak = Arc::downgrade(&inner);
            thread::Builder::new()
                .name("lingxi-bg-attach-output".to_string())
                .spawn(move || output_loop(output_rx, weak))?;
            Ok(Self { inner })
        }

        /// Start from the daemon-provided worker environment.
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

        /// Take the sole, bounded worker-side input receiver.
        pub fn take_input_rx(&self) -> Option<mpsc::Receiver<AttachInput>> {
            self.inner.input_rx.lock().unwrap().take()
        }

        /// Whether a controller is currently connected.
        #[must_use]
        pub fn has_clients(&self) -> bool {
            !matches!(*self.inner.controller.lock().unwrap(), Controller::Vacant)
        }

        /// Send raw PTY output to the controller, if attached.
        pub fn broadcast(&self, bytes: &[u8]) {
            for chunk in bytes.chunks(OUTPUT_CHUNK_BYTES) {
                if !chunk.is_empty()
                    && !self
                        .inner
                        .enqueue_frame(FRAME_OUTPUT, chunk.to_vec(), false)
                {
                    break;
                }
            }
        }

        /// Announce that the PTY/TUI is ready. The accept handshake also sends
        /// READY, so this method is safe to call before or after attachment.
        pub fn ready(&self) {
            let label = self.inner.session_label.as_bytes();
            self.inner.enqueue_frame(FRAME_READY, label.to_vec(), false);
        }

        /// Report a terminal worker error without mixing it into the PTY byte stream.
        pub fn error(&self, message: &str) {
            self.inner
                .enqueue_frame(FRAME_ERROR, message.as_bytes().to_vec(), false);
        }

        /// Report child exit after output has been drained and close the controller.
        pub fn exit(&self, code: i32) {
            let (completion_tx, completion_rx) = std_mpsc::sync_channel(0);
            if self.inner.enqueue_terminal_frame(
                FRAME_EXIT,
                code.to_be_bytes().to_vec(),
                completion_tx,
            ) {
                let _ = completion_rx.recv_timeout(ATTACH_WRITE_TIMEOUT);
            }
        }
    }

    impl AttachHubInner {
        fn reserve_controller(&self, id: u64) -> bool {
            let Ok(mut controller) = self.controller.lock() else {
                return false;
            };
            match *controller {
                Controller::Vacant => {
                    *controller = Controller::Reserved { id };
                    true
                }
                Controller::Reserved { .. } | Controller::Active(_) => false,
            }
        }

        fn activate_controller(
            &self,
            id: u64,
            writer: Arc<Mutex<UnixStream>>,
            shutdown: UnixStream,
        ) -> bool {
            let Ok(mut controller) = self.controller.lock() else {
                return false;
            };
            match *controller {
                Controller::Reserved { id: reserved_id } if reserved_id == id => {
                    *controller = Controller::Active(Client {
                        id,
                        writer,
                        shutdown,
                    });
                    true
                }
                _ => false,
            }
        }

        fn release_reservation(&self, id: u64) {
            let Ok(mut controller) = self.controller.lock() else {
                return;
            };
            if matches!(*controller, Controller::Reserved { id: reserved_id } if reserved_id == id)
            {
                *controller = Controller::Vacant;
            }
        }

        fn enqueue_frame(&self, kind: u8, payload: Vec<u8>, close_after: bool) -> bool {
            let controller_id = match &*self.controller.lock().unwrap() {
                Controller::Active(client) => client.id,
                Controller::Vacant | Controller::Reserved { .. } => return true,
            };
            match self.output_tx.try_send(ServerFrame {
                controller_id,
                kind,
                payload,
                close_after,
                completion: None,
            }) {
                Ok(()) => true,
                Err(std_mpsc::TrySendError::Full(_))
                | Err(std_mpsc::TrySendError::Disconnected(_)) => {
                    // The PTY reader must never wait behind a slow terminal.
                    self.remove_controller(controller_id, false);
                    false
                }
            }
        }

        fn enqueue_terminal_frame(
            &self,
            kind: u8,
            payload: Vec<u8>,
            completion: std_mpsc::SyncSender<()>,
        ) -> bool {
            let controller_id = match &*self.controller.lock().unwrap() {
                Controller::Active(client) => client.id,
                _ => return false,
            };
            self.output_tx
                .try_send(ServerFrame {
                    controller_id,
                    kind,
                    payload,
                    close_after: true,
                    completion: Some(completion),
                })
                .is_ok()
        }

        fn send_frame_to(&self, id: u64, kind: u8, payload: &[u8]) -> bool {
            let writer = {
                let controller = self.controller.lock().unwrap();
                let Controller::Active(client) = &*controller else {
                    return false;
                };
                if client.id != id {
                    return false;
                }
                Arc::clone(&client.writer)
            };
            let Ok(mut writer) = writer.lock() else {
                return false;
            };
            write_frame(&mut writer, kind, payload)
                .and_then(|()| writer.flush())
                .is_ok()
        }

        fn remove_controller(&self, id: u64, explicit_detach: bool) {
            let removed = {
                let mut controller = self.controller.lock().unwrap();
                match &*controller {
                    Controller::Active(client) if client.id == id => {
                        Some(std::mem::replace(&mut *controller, Controller::Vacant))
                    }
                    Controller::Reserved { id: reserved_id } if *reserved_id == id => {
                        Some(std::mem::replace(&mut *controller, Controller::Vacant))
                    }
                    _ => None,
                }
            };
            if let Some(Controller::Active(client)) = removed {
                let _ = client.shutdown.shutdown(Shutdown::Both);
                if explicit_detach {
                    self.try_send_input(AttachInput::Detach);
                }
                self.try_send_input(AttachInput::ClientDetached);
            }
        }

        fn try_send_input(&self, input: AttachInput) -> bool {
            match self.input_tx.try_send(input) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => false,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        }
    }

    fn remove_stale_socket(path: &PathBuf) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to replace non-socket attach endpoint {}",
                    path.display()
                ),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn accept_loop(listener: UnixListener, inner: Weak<AttachHubInner>) {
        loop {
            let Some(hub) = inner.upgrade() else {
                return;
            };
            if hub.stop.load(Ordering::Acquire) {
                return;
            }
            match listener.accept() {
                Ok((stream, _addr)) => accept_client(&hub, stream),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    drop(hub);
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return,
            }
        }
    }

    fn output_loop(output_rx: std_mpsc::Receiver<ServerFrame>, inner: Weak<AttachHubInner>) {
        while let Ok(frame) = output_rx.recv() {
            let Some(hub) = inner.upgrade() else {
                return;
            };
            if !hub.send_frame_to(frame.controller_id, frame.kind, &frame.payload) {
                hub.remove_controller(frame.controller_id, false);
                if let Some(completion) = frame.completion {
                    let _ = completion.send(());
                }
                continue;
            }
            if frame.close_after {
                hub.remove_controller(frame.controller_id, false);
            }
            if let Some(completion) = frame.completion {
                let _ = completion.send(());
            }
        }
    }

    fn accept_client(hub: &Arc<AttachHubInner>, mut stream: UnixStream) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        let _ = stream.set_write_timeout(Some(ATTACH_WRITE_TIMEOUT));
        let handshake = match read_handshake(&mut stream) {
            Ok(handshake) => handshake,
            Err(error) => {
                let _ = write_frame(&mut stream, FRAME_ERROR, error.to_string().as_bytes());
                return;
            }
        };
        if handshake.version != PROTOCOL_VERSION {
            let message = format!(
                "unsupported attach protocol {} (server requires {PROTOCOL_VERSION})",
                handshake.version
            );
            let _ = write_frame(&mut stream, FRAME_ERROR, message.as_bytes());
            return;
        }
        if !constant_time_eq(handshake.auth.as_bytes(), hub.auth.as_bytes()) {
            let _ = write_frame(&mut stream, FRAME_ERROR, b"unauthorized");
            return;
        }
        if handshake.session_label != hub.session_label {
            let _ = write_frame(
                &mut stream,
                FRAME_ERROR,
                b"attach session identifier mismatch",
            );
            return;
        }
        if handshake.cols == 0 || handshake.rows == 0 {
            let _ = write_frame(&mut stream, FRAME_ERROR, b"terminal size must be non-zero");
            return;
        }
        let _ = stream.set_read_timeout(None);

        let id = hub.next_client_id.fetch_add(1, Ordering::Relaxed);
        if !hub.reserve_controller(id) {
            let _ = write_frame(
                &mut stream,
                FRAME_ERROR,
                b"background PTY is already attached",
            );
            let _ = stream.flush();
            return;
        }

        let writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(_) => {
                hub.release_reservation(id);
                return;
            }
        };
        let shutdown = match writer.try_clone() {
            Ok(shutdown) => shutdown,
            Err(_) => {
                hub.release_reservation(id);
                return;
            }
        };

        if write_frame(&mut stream, FRAME_READY, hub.session_label.as_bytes())
            .and_then(|()| stream.flush())
            .is_err()
        {
            hub.release_reservation(id);
            return;
        }
        if !hub.activate_controller(id, Arc::new(Mutex::new(writer)), shutdown) {
            hub.release_reservation(id);
            return;
        }

        // Ctrl-L is the TUI's structured-history repaint command. Inject it
        // internally instead of replaying an arbitrary ANSI tail.
        if !hub.try_send_input(AttachInput::Bytes(vec![0x0c]))
            || !hub.try_send_input(AttachInput::Resize {
                cols: handshake.cols,
                rows: handshake.rows,
            })
        {
            let _ = hub.send_frame_to(id, FRAME_ERROR, b"attach input queue is full");
            hub.remove_controller(id, false);
            return;
        }

        let weak = Arc::downgrade(hub);
        let _ = thread::Builder::new()
            .name("lingxi-bg-attach-input".to_string())
            .spawn(move || client_input_loop(weak, id, stream));
    }

    struct Handshake {
        version: u16,
        auth: String,
        session_label: String,
        cols: u16,
        rows: u16,
    }

    fn write_handshake(
        stream: &mut UnixStream,
        auth: &str,
        session_label: &str,
        cols: u16,
        rows: u16,
    ) -> io::Result<()> {
        let auth_len = u16::try_from(auth.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "attach auth token is too long")
        })?;
        let label_len = u16::try_from(session_label.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "attach session identifier is too long",
            )
        })?;
        stream.write_all(PROTOCOL_MAGIC)?;
        stream.write_all(&PROTOCOL_VERSION.to_be_bytes())?;
        stream.write_all(&auth_len.to_be_bytes())?;
        stream.write_all(&label_len.to_be_bytes())?;
        stream.write_all(&cols.to_be_bytes())?;
        stream.write_all(&rows.to_be_bytes())?;
        stream.write_all(auth.as_bytes())?;
        stream.write_all(session_label.as_bytes())?;
        stream.flush()
    }

    fn read_handshake(stream: &mut UnixStream) -> io::Result<Handshake> {
        let mut header = [0_u8; 18];
        stream.read_exact(&mut header)?;
        if &header[..8] != PROTOCOL_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid attach protocol magic",
            ));
        }
        let version = u16::from_be_bytes([header[8], header[9]]);
        let auth_len = usize::from(u16::from_be_bytes([header[10], header[11]]));
        let label_len = usize::from(u16::from_be_bytes([header[12], header[13]]));
        if auth_len > MAX_AUTH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attach auth token is too long",
            ));
        }
        if label_len == 0 || label_len > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid attach session identifier length",
            ));
        }
        let cols = u16::from_be_bytes([header[14], header[15]]);
        let rows = u16::from_be_bytes([header[16], header[17]]);
        let mut auth = vec![0_u8; auth_len];
        stream.read_exact(&mut auth)?;
        let mut session_label = vec![0_u8; label_len];
        stream.read_exact(&mut session_label)?;
        let auth = String::from_utf8(auth).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "attach auth token is not UTF-8")
        })?;
        let session_label = String::from_utf8(session_label).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "attach session identifier is not UTF-8",
            )
        })?;
        Ok(Handshake {
            version,
            auth,
            session_label,
            cols,
            rows,
        })
    }

    fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
        let mut difference = left.len() ^ right.len();
        let max_len = left.len().max(right.len());
        for index in 0..max_len {
            let l = left.get(index).copied().unwrap_or(0);
            let r = right.get(index).copied().unwrap_or(0);
            difference |= usize::from(l ^ r);
        }
        difference == 0
    }

    fn client_input_loop(hub: Weak<AttachHubInner>, id: u64, mut stream: UnixStream) {
        loop {
            match read_frame(&mut stream) {
                Ok(Some((FRAME_INPUT_BYTES, payload))) => {
                    let Some(hub) = hub.upgrade() else {
                        return;
                    };
                    if !hub.try_send_input(AttachInput::Bytes(payload)) {
                        let _ = hub.send_frame_to(id, FRAME_ERROR, b"attach input queue is full");
                        hub.remove_controller(id, false);
                        return;
                    }
                }
                Ok(Some((FRAME_RESIZE, payload))) => {
                    if payload.len() != 4 {
                        if let Some(hub) = hub.upgrade() {
                            let _ = hub.send_frame_to(id, FRAME_ERROR, b"invalid resize frame");
                            hub.remove_controller(id, false);
                        }
                        return;
                    }
                    let cols = u16::from_be_bytes([payload[0], payload[1]]);
                    let rows = u16::from_be_bytes([payload[2], payload[3]]);
                    let Some(hub) = hub.upgrade() else {
                        return;
                    };
                    if cols == 0
                        || rows == 0
                        || !hub.try_send_input(AttachInput::Resize { cols, rows })
                    {
                        let _ = hub.send_frame_to(
                            id,
                            FRAME_ERROR,
                            b"invalid resize or input queue full",
                        );
                        hub.remove_controller(id, false);
                        return;
                    }
                }
                Ok(Some((FRAME_DETACH, payload))) if payload.is_empty() => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_controller(id, true);
                    }
                    return;
                }
                Ok(Some((_other, _payload))) => {
                    if let Some(hub) = hub.upgrade() {
                        let _ = hub.send_frame_to(id, FRAME_ERROR, b"unexpected client frame");
                        hub.remove_controller(id, false);
                    }
                    return;
                }
                Ok(None) | Err(_) => {
                    if let Some(hub) = hub.upgrade() {
                        hub.remove_controller(id, false);
                    }
                    return;
                }
            }
        }
    }

    fn write_frame(stream: &mut UnixStream, kind: u8, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "attach frame too large",
            ));
        }
        let len = u32::try_from(payload.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "attach frame exceeds u32 length",
            )
        })?;
        stream.write_all(&[kind])?;
        stream.write_all(&len.to_be_bytes())?;
        stream.write_all(payload)
    }

    fn read_frame(stream: &mut UnixStream) -> io::Result<Option<(u8, Vec<u8>)>> {
        let mut header = [0_u8; 5];
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
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

    struct TerminalGuard;

    impl TerminalGuard {
        fn enter() -> io::Result<Self> {
            enable_raw_mode()?;
            let mut stdout = io::stdout().lock();
            if let Err(error) = stdout
                // Isolate the remote TUI from the caller's scrollback. The
                // background child itself remains inline inside its PTY; the
                // attach client owns this local alternate-screen lifetime.
                .write_all(b"\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l")
                .and_then(|()| stdout.flush())
            {
                let _ = disable_raw_mode();
                return Err(error);
            }
            Ok(Self)
        }
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = stdout.flush();
            let _ = disable_raw_mode();
        }
    }

    fn resize_payload(cols: u16, rows: u16) -> [u8; 4] {
        let [c0, c1] = cols.to_be_bytes();
        let [r0, r1] = rows.to_be_bytes();
        [c0, c1, r0, r1]
    }

    fn split_at_detach(bytes: &[u8]) -> (&[u8], bool) {
        match bytes.iter().position(|byte| *byte == DETACH_BYTE) {
            Some(index) => (&bytes[..index], true),
            None => (bytes, false),
        }
    }

    /// Connect to a live background PTY. Local stdin is read as raw bytes; the
    /// only locally-consumed byte is Ctrl-] (`0x1d`).
    pub fn attach_to_socket(
        path: &std::path::Path,
        auth: &str,
        session_label: &str,
    ) -> io::Result<()> {
        let (initial_cols, initial_rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        stream.set_write_timeout(Some(ATTACH_WRITE_TIMEOUT))?;
        write_handshake(
            &mut stream,
            auth,
            session_label,
            initial_cols.max(1),
            initial_rows.max(1),
        )?;
        match read_frame(&mut stream)? {
            Some((FRAME_READY, _session_label)) => {}
            Some((FRAME_ERROR, payload)) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    String::from_utf8_lossy(&payload).into_owned(),
                ));
            }
            Some((_kind, _payload)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "attach server did not send READY",
                ));
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "attach server closed during handshake",
                ));
            }
        }

        let _terminal = TerminalGuard::enter()?;
        stream.set_read_timeout(Some(CLIENT_POLL_INTERVAL))?;
        let reader = stream;
        let writer = Arc::new(Mutex::new(reader.try_clone()?));
        let stopped = Arc::new(AtomicBool::new(false));
        let local_input_end = Arc::new(Mutex::new(LocalInputEnd::Active));

        let input_writer = Arc::clone(&writer);
        let input_stopped = Arc::clone(&stopped);
        let input_end = Arc::clone(&local_input_end);
        let input_thread = thread::Builder::new()
            .name("lingxi-bg-attach-stdin".to_string())
            .spawn(move || {
                let mut stdin = io::stdin().lock();
                let mut buffer = [0_u8; 8192];
                let mut last_size = (initial_cols.max(1), initial_rows.max(1));
                while !input_stopped.load(Ordering::Acquire) {
                    let ready = {
                        let mut descriptors = [PollFd::new(&stdin, PollFlags::POLLIN)];
                        match poll(
                            &mut descriptors,
                            i32::try_from(CLIENT_POLL_INTERVAL.as_millis()).unwrap_or(40),
                        ) {
                            Ok(0) => false,
                            Ok(_) => descriptors[0]
                                .revents()
                                .is_some_and(|events| events.contains(PollFlags::POLLIN)),
                            Err(nix::errno::Errno::EINTR) => false,
                            Err(_) => {
                                if let Ok(mut end) = input_end.lock() {
                                    *end = LocalInputEnd::Failed;
                                }
                                if let Ok(writer) = input_writer.lock() {
                                    let _ = writer.shutdown(Shutdown::Both);
                                }
                                break;
                            }
                        }
                    };
                    // Resize polling lives on the stdin/control thread, not the
                    // server-output loop. A continuously streaming model can
                    // otherwise keep `read_frame` ready forever and starve
                    // SIGWINCH delivery until output pauses.
                    if let Ok((cols, rows)) = crossterm::terminal::size() {
                        let size = (cols.max(1), rows.max(1));
                        if size != last_size {
                            let payload = resize_payload(size.0, size.1);
                            let sent = input_writer
                                .lock()
                                .map_err(|_| ())
                                .and_then(|mut writer| {
                                    write_frame(&mut writer, FRAME_RESIZE, &payload)
                                        .and_then(|()| writer.flush())
                                        .map_err(|_| ())
                                })
                                .is_ok();
                            if !sent {
                                if let Ok(mut end) = input_end.lock() {
                                    *end = LocalInputEnd::Failed;
                                }
                                if let Ok(writer) = input_writer.lock() {
                                    let _ = writer.shutdown(Shutdown::Both);
                                }
                                break;
                            }
                            last_size = size;
                        }
                    }
                    if !ready {
                        continue;
                    }
                    let count = match stdin.read(&mut buffer) {
                        Ok(0) => {
                            let detached = input_end.lock().is_ok_and(|mut end| {
                                let detached = input_writer.lock().is_ok_and(|mut writer| {
                                    let sent = write_frame(&mut writer, FRAME_DETACH, &[])
                                        .and_then(|()| writer.flush())
                                        .is_ok();
                                    let _ = writer.shutdown(Shutdown::Write);
                                    sent
                                });
                                *end = if detached {
                                    LocalInputEnd::DetachSent
                                } else {
                                    LocalInputEnd::Failed
                                };
                                detached
                            });
                            if !detached {
                                if let Ok(writer) = input_writer.lock() {
                                    let _ = writer.shutdown(Shutdown::Both);
                                }
                            }
                            break;
                        }
                        Ok(count) => count,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            if let Ok(mut end) = input_end.lock() {
                                *end = LocalInputEnd::Failed;
                            }
                            if let Ok(writer) = input_writer.lock() {
                                let _ = writer.shutdown(Shutdown::Both);
                            }
                            break;
                        }
                    };
                    let (bytes, detach) = split_at_detach(&buffer[..count]);
                    if !bytes.is_empty() {
                        let sent = input_writer
                            .lock()
                            .map_err(|_| ())
                            .and_then(|mut writer| {
                                write_frame(&mut writer, FRAME_INPUT_BYTES, bytes)
                                    .and_then(|()| writer.flush())
                                    .map_err(|_| ())
                            })
                            .is_ok();
                        if !sent {
                            if let Ok(mut end) = input_end.lock() {
                                *end = LocalInputEnd::Failed;
                            }
                            if let Ok(writer) = input_writer.lock() {
                                let _ = writer.shutdown(Shutdown::Both);
                            }
                            break;
                        }
                    }
                    if detach {
                        let detached = input_end.lock().is_ok_and(|mut end| {
                            let detached = input_writer.lock().is_ok_and(|mut writer| {
                                let sent = write_frame(&mut writer, FRAME_DETACH, &[])
                                    .and_then(|()| writer.flush())
                                    .is_ok();
                                let _ = writer.shutdown(Shutdown::Write);
                                sent
                            });
                            *end = if detached {
                                LocalInputEnd::DetachSent
                            } else {
                                LocalInputEnd::Failed
                            };
                            detached
                        });
                        if !detached {
                            if let Ok(writer) = input_writer.lock() {
                                let _ = writer.shutdown(Shutdown::Both);
                            }
                        }
                        break;
                    }
                }
            })?;

        let mut reader = reader;
        let mut stdout = io::stdout().lock();
        let result = loop {
            match read_frame(&mut reader) {
                Ok(Some((FRAME_OUTPUT, payload))) => {
                    stdout.write_all(&payload)?;
                    stdout.flush()?;
                }
                Ok(Some((FRAME_READY, _payload))) => {}
                Ok(Some((FRAME_EXIT, payload))) => break decode_exit_frame(&payload),
                Ok(Some((FRAME_ERROR, payload))) => {
                    break Err(io::Error::other(format!(
                        "background PTY: {}",
                        String::from_utf8_lossy(&payload)
                    )));
                }
                Ok(Some((_kind, _payload))) => {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected attach server frame",
                    ));
                }
                Ok(None) => {
                    let state = local_input_end
                        .lock()
                        .map_or(LocalInputEnd::Failed, |state| *state);
                    break closed_connection_result(state);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    let state = local_input_end
                        .lock()
                        .map_or(LocalInputEnd::Failed, |state| *state);
                    if state == LocalInputEnd::Failed {
                        break closed_connection_result(state);
                    }
                }
                Err(error) => break Err(error),
            }
        };

        stopped.store(true, Ordering::Release);
        if let Ok(writer) = writer.lock() {
            let _ = writer.shutdown(Shutdown::Both);
        }
        let _ = input_thread.join();
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn short_socket_path() -> PathBuf {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            PathBuf::from(format!("/tmp/lx-bg-{}-{n}.sock", std::process::id()))
        }

        fn connect_client(path: &PathBuf, auth: &str) -> UnixStream {
            let mut client = UnixStream::connect(path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let label = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap();
            write_handshake(&mut client, auth, label, 120, 40).unwrap();
            let (kind, _payload) = read_frame(&mut client).unwrap().unwrap();
            assert_eq!(kind, FRAME_READY);
            client
        }

        fn drain_initial_repaint(rx: &mut mpsc::Receiver<AttachInput>) {
            assert_eq!(rx.blocking_recv(), Some(AttachInput::Bytes(vec![0x0c])));
            assert_eq!(
                rx.blocking_recv(),
                Some(AttachInput::Resize {
                    cols: 120,
                    rows: 40
                })
            );
        }

        fn active_writer(hub: &AttachHub) -> Arc<Mutex<UnixStream>> {
            let controller = hub.inner.controller.lock().unwrap();
            match &*controller {
                Controller::Active(client) => Arc::clone(&client.writer),
                state => panic!("expected active controller, got {state:?}"),
            }
        }

        #[test]
        fn attach_socket_streams_raw_output_after_v2_handshake() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let _rx = hub.take_input_rx().unwrap();
            let mut client = connect_client(&path, "token-1");

            hub.broadcast(b"\x1b[2Jraw\r\n");
            let (kind, payload) = read_frame(&mut client).unwrap().unwrap();
            assert_eq!(kind, FRAME_OUTPUT);
            assert_eq!(payload, b"\x1b[2Jraw\r\n");
        }

        #[test]
        fn attach_socket_forwards_control_bytes_without_interpreting_them() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mut rx = hub.take_input_rx().unwrap();
            let mut client = connect_client(&path, "token-1");
            drain_initial_repaint(&mut rx);

            let raw = [0x1b, 0x03, 0x04, b'x'];
            write_frame(&mut client, FRAME_INPUT_BYTES, &raw).unwrap();
            write_frame(&mut client, FRAME_RESIZE, &resize_payload(90, 30)).unwrap();
            write_frame(&mut client, FRAME_DETACH, &[]).unwrap();

            assert_eq!(rx.blocking_recv(), Some(AttachInput::Bytes(raw.to_vec())));
            assert_eq!(
                rx.blocking_recv(),
                Some(AttachInput::Resize { cols: 90, rows: 30 })
            );
            assert_eq!(rx.blocking_recv(), Some(AttachInput::Detach));
            assert_eq!(rx.blocking_recv(), Some(AttachInput::ClientDetached));
        }

        #[test]
        fn attach_socket_allows_only_one_controller() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let _rx = hub.take_input_rx().unwrap();
            let _first = connect_client(&path, "token-1");

            let mut second = UnixStream::connect(&path).unwrap();
            second
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let label = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap();
            write_handshake(&mut second, "token-1", label, 80, 24).unwrap();
            let (kind, payload) = read_frame(&mut second).unwrap().unwrap();
            assert_eq!(kind, FRAME_ERROR);
            assert!(String::from_utf8_lossy(&payload).contains("already attached"));
        }

        #[test]
        fn attach_socket_rejects_bad_auth() {
            let path = short_socket_path();
            let _hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mut client = UnixStream::connect(&path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let label = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap();
            write_handshake(&mut client, "wrong", label, 80, 24).unwrap();
            let (kind, payload) = read_frame(&mut client).unwrap().unwrap();
            assert_eq!(kind, FRAME_ERROR);
            assert_eq!(payload, b"unauthorized");
        }

        #[test]
        fn attach_socket_rejects_protocol_downgrade() {
            let path = short_socket_path();
            let _hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mut client = UnixStream::connect(&path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let label = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap();
            client.write_all(PROTOCOL_MAGIC).unwrap();
            client
                .write_all(&(PROTOCOL_VERSION - 1).to_be_bytes())
                .unwrap();
            client
                .write_all(&u16::try_from("token-1".len()).unwrap().to_be_bytes())
                .unwrap();
            client
                .write_all(&u16::try_from(label.len()).unwrap().to_be_bytes())
                .unwrap();
            client.write_all(&80_u16.to_be_bytes()).unwrap();
            client.write_all(&24_u16.to_be_bytes()).unwrap();
            client.write_all(b"token-1").unwrap();
            client.write_all(label.as_bytes()).unwrap();
            client.flush().unwrap();

            let (kind, payload) = read_frame(&mut client).unwrap().unwrap();
            assert_eq!(kind, FRAME_ERROR);
            assert!(String::from_utf8_lossy(&payload).contains("unsupported attach protocol"));
        }

        #[test]
        fn attach_frames_enforce_the_maximum_size_in_both_directions() {
            let (mut writer, mut reader) = UnixStream::pair().unwrap();
            let oversized = vec![0_u8; MAX_FRAME_BYTES + 1];
            assert_eq!(
                write_frame(&mut writer, FRAME_INPUT_BYTES, &oversized)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );

            writer.write_all(&[FRAME_INPUT_BYTES]).unwrap();
            writer
                .write_all(&u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_be_bytes())
                .unwrap();
            writer.flush().unwrap();
            assert_eq!(
                read_frame(&mut reader).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }

        #[test]
        fn attach_socket_reports_error_and_exit_frames() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let _rx = hub.take_input_rx().unwrap();
            let mut client = connect_client(&path, "token-1");

            hub.error("failed");
            assert_eq!(
                read_frame(&mut client).unwrap().unwrap(),
                (FRAME_ERROR, b"failed".to_vec())
            );
            hub.exit(17);
            drop(hub);
            assert_eq!(
                read_frame(&mut client).unwrap().unwrap(),
                (FRAME_EXIT, 17_i32.to_be_bytes().to_vec())
            );
        }

        #[test]
        fn blocked_writer_detaches_without_blocking_broadcast() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mut rx = hub.take_input_rx().unwrap();
            let _client = connect_client(&path, "token-1");
            drain_initial_repaint(&mut rx);

            let writer = active_writer(&hub);
            let _blocked_writer = writer.lock().unwrap();
            let payload = vec![b'x'; OUTPUT_CHUNK_BYTES * (OUTPUT_CHANNEL_CAPACITY + 2)];
            let (done_tx, done_rx) = std_mpsc::channel();
            let broadcast_hub = hub.clone();
            thread::spawn(move || {
                broadcast_hub.broadcast(&payload);
                let _ = done_tx.send(());
            });

            done_rx
                .recv_timeout(Duration::from_millis(500))
                .expect("broadcast should detach instead of blocking behind a stuck writer");
            assert!(!hub.has_clients(), "slow writer should be detached");
            assert_eq!(rx.blocking_recv(), Some(AttachInput::ClientDetached));
        }

        #[test]
        fn ctrl_right_bracket_is_the_only_locally_consumed_byte() {
            let bytes = [0x1b, 0x03, 0x04, b'a', DETACH_BYTE, b'b'];
            let (forwarded, detach) = split_at_detach(&bytes);
            assert_eq!(forwarded, &[0x1b, 0x03, 0x04, b'a']);
            assert!(detach);
            let (forwarded, detach) = split_at_detach(&bytes[..4]);
            assert_eq!(forwarded, &bytes[..4]);
            assert!(!detach);
        }

        #[test]
        fn socket_permissions_are_owner_only_and_non_socket_is_not_replaced() {
            let path = short_socket_path();
            let hub = AttachHub::start(path.clone(), "token-1".to_string()).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            drop(hub);
            for _ in 0..20 {
                if !path.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            assert!(!path.exists(), "socket endpoint is removed after hub drop");
            fs::write(&path, b"do not replace").unwrap();
            let error = AttachHub::start(path.clone(), "token-1".to_string())
                .err()
                .expect("non-socket must be rejected");
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            fs::remove_file(path).unwrap();
        }
    }
}

#[cfg(unix)]
pub use unix::{attach_to_socket, AttachHub};

#[cfg(windows)]
mod windows {
    use super::{
        closed_connection_result, decode_exit_frame, AttachInput, LocalInputEnd, ATTACH_AUTH_ENV,
        ATTACH_SOCK_ENV,
    };
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex, Weak};
    use std::thread;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::sync::mpsc;

    const PROTOCOL_MAGIC: &[u8; 8] = b"LXPTY2\0\0";
    const PROTOCOL_VERSION: u16 = 2;
    const MAX_AUTH_BYTES: usize = 4096;
    const MAX_FRAME_BYTES: usize = 1024 * 1024;
    const INPUT_CHANNEL_CAPACITY: usize = 128;
    const OUTPUT_CHANNEL_CAPACITY: usize = 128;
    const OUTPUT_CHUNK_BYTES: usize = 8192;
    const CLIENT_POLL_INTERVAL: Duration = Duration::from_millis(20);
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
    const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
    const DETACH_BYTE: u8 = 0x1d;

    const FRAME_OUTPUT: u8 = 1;
    const FRAME_INPUT_BYTES: u8 = 2;
    const FRAME_RESIZE: u8 = 3;
    const FRAME_DETACH: u8 = 5;
    const FRAME_READY: u8 = 7;
    const FRAME_EXIT: u8 = 8;
    const FRAME_ERROR: u8 = 9;

    #[derive(Debug)]
    struct ServerFrame {
        kind: u8,
        payload: Vec<u8>,
        close_after: bool,
        completion: Option<std_mpsc::SyncSender<()>>,
    }

    struct Controller {
        id: u64,
        output: mpsc::Sender<ServerFrame>,
    }

    struct AttachHubInner {
        pipe_name: String,
        auth: String,
        session_label: String,
        controller: Mutex<Option<Controller>>,
        input_tx: mpsc::Sender<AttachInput>,
        input_rx: Mutex<Option<mpsc::Receiver<AttachInput>>>,
        next_client_id: AtomicU64,
        stop: AtomicBool,
    }

    impl Drop for AttachHubInner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Ok(controller) = self.controller.get_mut() {
                controller.take();
            }
        }
    }

    /// Single-controller protocol-v2 named-pipe hub for a ConPTY worker.
    #[derive(Clone)]
    pub struct AttachHub {
        inner: Arc<AttachHubInner>,
    }

    impl AttachHub {
        pub fn start(path: PathBuf, auth: String) -> io::Result<Self> {
            if auth.is_empty() || auth.len() > MAX_AUTH_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid attach auth token",
                ));
            }
            let pipe_name = path.to_string_lossy().into_owned();
            if !pipe_name.starts_with(r"\\.\pipe\") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "background attach endpoint is not a local named pipe",
                ));
            }
            let session_label = pipe_name
                .rsplit('-')
                .next()
                .unwrap_or("background")
                .to_string();
            let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
            let inner = Arc::new(AttachHubInner {
                pipe_name,
                auth,
                session_label,
                controller: Mutex::new(None),
                input_tx,
                input_rx: Mutex::new(Some(input_rx)),
                next_client_id: AtomicU64::new(1),
                stop: AtomicBool::new(false),
            });
            let weak = Arc::downgrade(&inner);
            thread::Builder::new()
                .name("lingxi-bg-attach-pipe".to_string())
                .spawn(move || run_pipe_server(weak))?;
            Ok(Self { inner })
        }

        pub fn start_from_env() -> io::Result<Option<Self>> {
            let Ok(path) = std::env::var(ATTACH_SOCK_ENV) else {
                return Ok(None);
            };
            let auth = std::env::var(ATTACH_AUTH_ENV).map_err(|_| {
                io::Error::new(io::ErrorKind::PermissionDenied, "missing attach auth token")
            })?;
            Self::start(PathBuf::from(path), auth).map(Some)
        }

        pub fn take_input_rx(&self) -> Option<mpsc::Receiver<AttachInput>> {
            self.inner.input_rx.lock().ok()?.take()
        }

        #[must_use]
        pub fn has_clients(&self) -> bool {
            self.inner
                .controller
                .lock()
                .is_ok_and(|controller| controller.is_some())
        }

        pub fn broadcast(&self, bytes: &[u8]) {
            for chunk in bytes.chunks(OUTPUT_CHUNK_BYTES) {
                if !chunk.is_empty() && !self.inner.enqueue(FRAME_OUTPUT, chunk.to_vec(), false) {
                    break;
                }
            }
        }

        pub fn ready(&self) {
            self.inner.enqueue(
                FRAME_READY,
                self.inner.session_label.as_bytes().to_vec(),
                false,
            );
        }

        pub fn error(&self, message: &str) {
            self.inner
                .enqueue(FRAME_ERROR, message.as_bytes().to_vec(), false);
        }

        pub fn exit(&self, code: i32) {
            let (completion_tx, completion_rx) = std_mpsc::sync_channel(0);
            if self
                .inner
                .enqueue_terminal(FRAME_EXIT, code.to_be_bytes().to_vec(), completion_tx)
            {
                let _ = completion_rx.recv_timeout(WRITE_TIMEOUT);
            }
        }
    }

    impl AttachHubInner {
        fn enqueue(&self, kind: u8, payload: Vec<u8>, close_after: bool) -> bool {
            let mut controller = match self.controller.lock() {
                Ok(controller) => controller,
                Err(_) => return false,
            };
            let Some(active) = controller.as_ref() else {
                return true;
            };
            match active.output.try_send(ServerFrame {
                kind,
                payload,
                close_after,
                completion: None,
            }) {
                Ok(()) => true,
                Err(_) => {
                    controller.take();
                    false
                }
            }
        }

        fn enqueue_terminal(
            &self,
            kind: u8,
            payload: Vec<u8>,
            completion: std_mpsc::SyncSender<()>,
        ) -> bool {
            let mut controller = match self.controller.lock() {
                Ok(controller) => controller,
                Err(_) => return false,
            };
            let Some(active) = controller.as_ref() else {
                return false;
            };
            if active
                .output
                .try_send(ServerFrame {
                    kind,
                    payload,
                    close_after: true,
                    completion: Some(completion),
                })
                .is_ok()
            {
                true
            } else {
                controller.take();
                false
            }
        }

        fn remove_controller(&self, id: u64, explicit_detach: bool) {
            let removed = self.controller.lock().ok().and_then(|mut controller| {
                if controller.as_ref().is_some_and(|active| active.id == id) {
                    controller.take()
                } else {
                    None
                }
            });
            if removed.is_some() {
                if explicit_detach {
                    let _ = self.input_tx.try_send(AttachInput::Detach);
                }
                let _ = self.input_tx.try_send(AttachInput::ClientDetached);
            }
        }
    }

    fn run_pipe_server(weak: Weak<AttachHubInner>) {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async move {
            let mut first = true;
            loop {
                let Some(hub) = weak.upgrade() else {
                    return;
                };
                if hub.stop.load(Ordering::Acquire) {
                    return;
                }
                let mut options = ServerOptions::new();
                options
                    .first_pipe_instance(first)
                    .reject_remote_clients(true)
                    .in_buffer_size(64 * 1024)
                    .out_buffer_size(64 * 1024);
                let Ok(server) =
                    platform_pty::create_current_user_named_pipe(&options, &hub.pipe_name)
                else {
                    return;
                };
                first = false;
                drop(hub);
                // Keep one server instance continuously present. Dropping and
                // recreating it on a timer would open a namespace-substitution
                // window after the first-instance guard is released.
                if server.connect().await.is_err() {
                    continue;
                }
                let connection_hub = weak.clone();
                tokio::spawn(async move {
                    handle_connection(server, connection_hub).await;
                });
            }
        });
    }

    async fn handle_connection(mut pipe: NamedPipeServer, weak: Weak<AttachHubInner>) {
        let Some(hub) = weak.upgrade() else {
            return;
        };
        let handshake =
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, read_handshake(&mut pipe)).await {
                Ok(Ok(handshake)) => handshake,
                Ok(Err(_)) | Err(_) => return,
            };
        if handshake.version != PROTOCOL_VERSION {
            let _ = write_async_frame(&mut pipe, FRAME_ERROR, b"unsupported attach protocol").await;
            return;
        }
        if !constant_time_eq(handshake.auth.as_bytes(), hub.auth.as_bytes()) {
            let _ = write_async_frame(&mut pipe, FRAME_ERROR, b"unauthorized").await;
            return;
        }
        if handshake.session_label != hub.session_label {
            let _ = write_async_frame(
                &mut pipe,
                FRAME_ERROR,
                b"attach session identifier mismatch",
            )
            .await;
            return;
        }
        if handshake.cols == 0 || handshake.rows == 0 {
            let _ = write_async_frame(&mut pipe, FRAME_ERROR, b"invalid terminal size").await;
            return;
        }
        let id = hub.next_client_id.fetch_add(1, Ordering::Relaxed);
        let (output, mut output_rx) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
        let claimed = {
            let mut controller = match hub.controller.lock() {
                Ok(controller) => controller,
                Err(_) => return,
            };
            if controller.is_some() {
                false
            } else {
                *controller = Some(Controller { id, output });
                true
            }
        };
        if !claimed {
            let _ = write_async_frame(
                &mut pipe,
                FRAME_ERROR,
                b"background PTY is already attached",
            )
            .await;
            return;
        }
        if write_async_frame(&mut pipe, FRAME_READY, hub.session_label.as_bytes())
            .await
            .is_err()
        {
            hub.remove_controller(id, false);
            return;
        }
        if hub
            .input_tx
            .try_send(AttachInput::Bytes(vec![0x0c]))
            .is_err()
            || hub
                .input_tx
                .try_send(AttachInput::Resize {
                    cols: handshake.cols,
                    rows: handshake.rows,
                })
                .is_err()
        {
            let _ = write_async_frame(
                &mut pipe,
                FRAME_ERROR,
                b"background PTY input queue is full",
            )
            .await;
            hub.remove_controller(id, false);
            return;
        }

        loop {
            tokio::select! {
                frame = read_async_frame(&mut pipe) => {
                    match frame {
                        Ok(Some((FRAME_INPUT_BYTES, payload))) if !payload.is_empty() => {
                            if hub.input_tx.try_send(AttachInput::Bytes(payload)).is_err() {
                                let _ = write_async_frame(&mut pipe, FRAME_ERROR, b"background PTY input queue is full").await;
                                break;
                            }
                        }
                        Ok(Some((FRAME_RESIZE, payload))) if payload.len() == 4 => {
                            let cols = u16::from_be_bytes([payload[0], payload[1]]);
                            let rows = u16::from_be_bytes([payload[2], payload[3]]);
                            if cols == 0 || rows == 0 || hub.input_tx.try_send(AttachInput::Resize { cols, rows }).is_err() {
                                let _ = write_async_frame(&mut pipe, FRAME_ERROR, b"invalid resize or full input queue").await;
                                break;
                            }
                        }
                        Ok(Some((FRAME_DETACH, payload))) if payload.is_empty() => {
                            hub.remove_controller(id, true);
                            return;
                        }
                        Ok(None) | Err(_) => break,
                        _ => {
                            let _ = write_async_frame(&mut pipe, FRAME_ERROR, b"unexpected client frame").await;
                            break;
                        }
                    }
                }
                frame = output_rx.recv() => {
                    let Some(frame) = frame else { break; };
                    let write_result = write_async_frame(&mut pipe, frame.kind, &frame.payload).await;
                    if let Some(completion) = frame.completion {
                        let _ = completion.send(());
                    }
                    if write_result.is_err() {
                        break;
                    }
                    if frame.close_after {
                        break;
                    }
                }
            }
        }
        hub.remove_controller(id, false);
    }

    struct Handshake {
        version: u16,
        auth: String,
        session_label: String,
        cols: u16,
        rows: u16,
    }

    async fn read_handshake(pipe: &mut NamedPipeServer) -> io::Result<Handshake> {
        let mut fixed = [0_u8; 18];
        pipe.read_exact(&mut fixed).await?;
        if &fixed[..8] != PROTOCOL_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid attach magic",
            ));
        }
        let version = u16::from_be_bytes([fixed[8], fixed[9]]);
        let auth_len = usize::from(u16::from_be_bytes([fixed[10], fixed[11]]));
        let label_len = usize::from(u16::from_be_bytes([fixed[12], fixed[13]]));
        let cols = u16::from_be_bytes([fixed[14], fixed[15]]);
        let rows = u16::from_be_bytes([fixed[16], fixed[17]]);
        if auth_len == 0 || auth_len > MAX_AUTH_BYTES || label_len == 0 || label_len > 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid attach handshake lengths",
            ));
        }
        let mut auth = vec![0; auth_len];
        let mut label = vec![0; label_len];
        pipe.read_exact(&mut auth).await?;
        pipe.read_exact(&mut label).await?;
        Ok(Handshake {
            version,
            auth: String::from_utf8(auth).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "attach auth is not utf-8")
            })?,
            session_label: String::from_utf8(label).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "attach session id is not utf-8")
            })?,
            cols,
            rows,
        })
    }

    async fn read_async_frame(pipe: &mut NamedPipeServer) -> io::Result<Option<(u8, Vec<u8>)>> {
        let mut header = [0_u8; 5];
        match pipe.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        }
        let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attach frame too large",
            ));
        }
        let mut payload = vec![0; len];
        pipe.read_exact(&mut payload).await?;
        Ok(Some((header[0], payload)))
    }

    async fn write_async_frame(
        pipe: &mut NamedPipeServer,
        kind: u8,
        payload: &[u8],
    ) -> io::Result<()> {
        match tokio::time::timeout(WRITE_TIMEOUT, write_async_frame_inner(pipe, kind, payload))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "attach client write timed out",
            )),
        }
    }

    async fn write_async_frame_inner(
        pipe: &mut NamedPipeServer,
        kind: u8,
        payload: &[u8],
    ) -> io::Result<()> {
        if payload.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "attach frame too large",
            ));
        }
        pipe.write_all(&[kind]).await?;
        pipe.write_all(
            &u32::try_from(payload.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "attach frame too large"))?
                .to_be_bytes(),
        )
        .await?;
        pipe.write_all(payload).await?;
        pipe.flush().await
    }

    fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
        let mut diff = left.len() ^ right.len();
        for index in 0..left.len().max(right.len()) {
            diff |= usize::from(
                left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
            );
        }
        diff == 0
    }

    fn write_handshake(
        stream: &mut File,
        auth: &str,
        label: &str,
        cols: u16,
        rows: u16,
    ) -> io::Result<()> {
        let auth_len = u16::try_from(auth.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "attach auth is too long"))?;
        let label_len = u16::try_from(label.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "attach session id is too long")
        })?;
        stream.write_all(PROTOCOL_MAGIC)?;
        stream.write_all(&PROTOCOL_VERSION.to_be_bytes())?;
        stream.write_all(&auth_len.to_be_bytes())?;
        stream.write_all(&label_len.to_be_bytes())?;
        stream.write_all(&cols.to_be_bytes())?;
        stream.write_all(&rows.to_be_bytes())?;
        stream.write_all(auth.as_bytes())?;
        stream.write_all(label.as_bytes())?;
        stream.flush()
    }

    fn write_frame(stream: &mut File, kind: u8, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "attach frame too large",
            ));
        }
        stream.write_all(&[kind])?;
        stream.write_all(
            &u32::try_from(payload.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "attach frame too large"))?
                .to_be_bytes(),
        )?;
        stream.write_all(payload)?;
        stream.flush()
    }

    fn read_frame(stream: &mut File) -> io::Result<Option<(u8, Vec<u8>)>> {
        let mut header = [0_u8; 5];
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        }
        let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attach frame too large",
            ));
        }
        let mut payload = vec![0; len];
        stream.read_exact(&mut payload)?;
        Ok(Some((header[0], payload)))
    }

    fn resize_payload(cols: u16, rows: u16) -> [u8; 4] {
        let [c0, c1] = cols.to_be_bytes();
        let [r0, r1] = rows.to_be_bytes();
        [c0, c1, r0, r1]
    }

    fn split_at_detach(bytes: &[u8]) -> (&[u8], bool) {
        bytes
            .iter()
            .position(|byte| *byte == DETACH_BYTE)
            .map_or((bytes, false), |index| (&bytes[..index], true))
    }

    struct TerminalGuard;

    impl TerminalGuard {
        fn enter() -> io::Result<Self> {
            enable_raw_mode()?;
            let mut stdout = io::stdout().lock();
            if let Err(error) = stdout
                .write_all(b"\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l")
                .and_then(|()| stdout.flush())
            {
                let _ = disable_raw_mode();
                return Err(error);
            }
            Ok(Self)
        }
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = stdout.flush();
            let _ = disable_raw_mode();
        }
    }

    pub fn attach_to_socket(path: &Path, auth: &str, session_label: &str) -> io::Result<()> {
        let mut stream = open_pipe(path)?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        write_handshake(&mut stream, auth, session_label, cols.max(1), rows.max(1))?;
        match read_frame(&mut stream)? {
            Some((FRAME_READY, _)) => {}
            Some((FRAME_ERROR, payload)) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    String::from_utf8_lossy(&payload).into_owned(),
                ))
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "attach server did not send READY",
                ))
            }
        }

        let _terminal = TerminalGuard::enter()?;
        let pump = platform_pty::WindowsStdinPump::start()?;
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let stopped = Arc::new(AtomicBool::new(false));
        let local_input_end = Arc::new(Mutex::new(LocalInputEnd::Active));
        let input_writer = Arc::clone(&writer);
        let input_stopped = Arc::clone(&stopped);
        let input_end = Arc::clone(&local_input_end);
        let input_thread = thread::Builder::new()
            .name("lingxi-bg-attach-input".to_string())
            .spawn(move || {
                let mut last_size = (cols.max(1), rows.max(1));
                while !input_stopped.load(Ordering::Acquire) {
                    if pump.failure_message().is_some() {
                        if let Ok(mut end) = input_end.lock() {
                            *end = LocalInputEnd::Failed;
                        }
                        // Wake the blocking server-output read by explicitly
                        // releasing the controller lease. This is an error,
                        // not a successful local detach; the atomic above
                        // preserves that distinction after the server closes.
                        if let Ok(mut writer) = input_writer.lock() {
                            let _ = write_frame(&mut writer, FRAME_DETACH, &[]);
                        }
                        break;
                    }
                    match pump.try_recv() {
                        Ok(bytes) => {
                            let (bytes, detach) = split_at_detach(&bytes);
                            if !bytes.is_empty() {
                                let sent = input_writer.lock().is_ok_and(|mut writer| {
                                    write_frame(&mut writer, FRAME_INPUT_BYTES, bytes).is_ok()
                                });
                                if !sent {
                                    if let Ok(mut end) = input_end.lock() {
                                        *end = LocalInputEnd::Failed;
                                    }
                                    break;
                                }
                            }
                            if detach {
                                let _ = input_end.lock().map(|mut end| {
                                    *end = if input_writer.lock().is_ok_and(|mut writer| {
                                        write_frame(&mut writer, FRAME_DETACH, &[]).is_ok()
                                    }) {
                                        LocalInputEnd::DetachSent
                                    } else {
                                        LocalInputEnd::Failed
                                    };
                                });
                                break;
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            if let Ok(mut end) = input_end.lock() {
                                *end = LocalInputEnd::Failed;
                            }
                            if let Ok(mut writer) = input_writer.lock() {
                                let _ = write_frame(&mut writer, FRAME_DETACH, &[]);
                            }
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                    if let Ok((next_cols, next_rows)) = crossterm::terminal::size() {
                        let next = (next_cols.max(1), next_rows.max(1));
                        if next != last_size {
                            let sent = input_writer.lock().is_ok_and(|mut writer| {
                                write_frame(
                                    &mut writer,
                                    FRAME_RESIZE,
                                    &resize_payload(next.0, next.1),
                                )
                                .is_ok()
                            });
                            if !sent {
                                if let Ok(mut end) = input_end.lock() {
                                    *end = LocalInputEnd::Failed;
                                }
                                break;
                            }
                            last_size = next;
                        }
                    }
                    thread::sleep(CLIENT_POLL_INTERVAL);
                }
            })?;

        let mut stdout = io::stdout().lock();
        let result = loop {
            match read_frame(&mut stream) {
                Ok(Some((FRAME_OUTPUT, payload))) => {
                    stdout.write_all(&payload)?;
                    stdout.flush()?;
                }
                Ok(Some((FRAME_READY, _))) => {}
                Ok(Some((FRAME_EXIT, payload))) => break decode_exit_frame(&payload),
                Ok(Some((FRAME_ERROR, payload))) => {
                    break Err(io::Error::other(format!(
                        "background PTY: {}",
                        String::from_utf8_lossy(&payload)
                    )))
                }
                Ok(None) => {
                    let state = local_input_end
                        .lock()
                        .map_or(LocalInputEnd::Failed, |state| *state);
                    break closed_connection_result(state);
                }
                Ok(Some(_)) => {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected attach server frame",
                    ))
                }
                Err(error) => break Err(error),
            }
        };
        stopped.store(true, Ordering::Release);
        drop(writer);
        let _ = input_thread.join();
        result
    }

    fn open_pipe(path: &Path) -> io::Result<File> {
        let mut last_error = None;
        for _ in 0..100 {
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(pipe) => return Ok(pipe),
                Err(error) if error.raw_os_error() == Some(231) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out opening background attach pipe",
            )
        }))
    }
}

#[cfg(windows)]
pub use windows::{attach_to_socket, AttachHub};

// Windows needs a named-pipe transport rather than pretending a filesystem path
// is a Unix socket.  The API is kept cfg-clean here so the ConPTY integration can
// supply the same v2 framing without changing callers.
#[cfg(not(any(unix, windows)))]
mod non_unix {
    use super::{ATTACH_AUTH_ENV, ATTACH_SOCK_ENV};
    use std::io;
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc;

    #[derive(Clone)]
    pub struct AttachHub;

    impl AttachHub {
        pub fn start(_path: PathBuf, _auth: String) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "protocol-v2 background attach requires the Windows named-pipe backend",
            ))
        }

        pub fn start_from_env() -> io::Result<Option<Self>> {
            if std::env::var(ATTACH_SOCK_ENV).is_ok() || std::env::var(ATTACH_AUTH_ENV).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "protocol-v2 background attach requires the Windows named-pipe backend",
                ));
            }
            Ok(None)
        }

        pub fn take_input_rx(&self) -> Option<mpsc::Receiver<super::AttachInput>> {
            None
        }

        #[must_use]
        pub fn has_clients(&self) -> bool {
            false
        }

        pub fn broadcast(&self, _bytes: &[u8]) {}
        pub fn ready(&self) {}
        pub fn error(&self, _message: &str) {}
        pub fn exit(&self, _code: i32) {}
    }

    pub fn attach_to_socket(_path: &Path, _auth: &str, _session_label: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "protocol-v2 background attach requires the Windows named-pipe backend",
        ))
    }
}

#[cfg(not(any(unix, windows)))]
pub use non_unix::{attach_to_socket, AttachHub};
