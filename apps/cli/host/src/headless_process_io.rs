//! Unix process IO without Tokio's uncancellable blocking stdio workers.

use harness_runtime::headless::io::{HeadlessIo, Output};
use lingxi_async_owned_fd::OwnedAsyncFd;
use rustix::fs::{FileType, OFlags};
use std::io::{self, IsTerminal};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, Interest, ReadBuf};

/// Capture every original flag before modifying any descriptor. stdout and
/// stderr may share one open-file description (`2>&1`); restore only when all
/// three streams have been released, so one writer cannot re-block another.
struct ProcessFds {
    fds: [OwnedFd; 3],
    flags: [OFlags; 3],
    changed: [AtomicBool; 3],
    regular: [bool; 3],
    managed: bool,
    stopping: AtomicBool,
    restored: AtomicBool,
    io_gate: Mutex<()>,
}

impl ProcessFds {
    fn new(
        fds: [OwnedFd; 3],
        types: [FileType; 3],
        managed: bool,
        mcp_group: Option<&mut Option<Arc<Self>>>,
    ) -> io::Result<Arc<Self>> {
        let flags = [
            rustix::fs::fcntl_getfl(&fds[0])?,
            rustix::fs::fcntl_getfl(&fds[1])?,
            rustix::fs::fcntl_getfl(&fds[2])?,
        ];
        let group = Arc::new(Self {
            fds,
            flags,
            changed: std::array::from_fn(|_| AtomicBool::new(false)),
            regular: types.map(|kind| kind == FileType::RegularFile),
            managed,
            stopping: AtomicBool::new(false),
            restored: AtomicBool::new(false),
            io_gate: Mutex::new(()),
        });
        // Register before the first mutation: even unwinding during setup
        // leaves the MCP entrypoint a handle to verify restoration.
        if let Some(slot) = mcp_group {
            *slot = Some(group.clone());
        }
        for index in 0..3 {
            if types[index] != FileType::RegularFile && !flags[index].contains(OFlags::NONBLOCK) {
                group.changed[index].store(true, Ordering::Release);
                if let Err(error) =
                    rustix::fs::fcntl_setfl(&group.fds[index], flags[index] | OFlags::NONBLOCK)
                {
                    if managed {
                        if let Err(restore) = group.restore_changed() {
                            return Err(io::Error::other(format!(
                                "failed to set process IO flags: {error}; flag restoration is unverified: {restore}"
                            )));
                        }
                        group.restored.store(true, Ordering::Release);
                    } else {
                        // Preserve the ordinary path's prior ownership: a
                        // failed set did not claim this flag for restoration.
                        group.changed[index].store(false, Ordering::Release);
                    }
                    return Err(error.into());
                }
            }
        }
        Ok(group)
    }

    /// Admit a managed poll under the same gate used for flag restoration.
    /// Ordinary headless sessions retain their original lock-free behavior.
    fn admit(&self) -> io::Result<Option<MutexGuard<'_, ()>>> {
        if !self.managed {
            return Ok(None);
        }
        if self.stopping.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "MCP process IO is stopping",
            ));
        }
        let guard = self
            .io_gate
            .lock()
            .map_err(|_| io::Error::other("MCP process IO gate is poisoned"))?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "MCP process IO is stopping",
            ));
        }
        Ok(Some(guard))
    }

    fn file_io<T>(&self, index: usize, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let admission = self.admit()?;
        if self.regular[index] {
            // Already-admitted regular file operations may block. Their flags
            // were not changed, so they must not delay pipe flag restoration.
            drop(admission);
            operation()
        } else {
            let _admission = admission;
            operation()
        }
    }

    fn restore_changed(&self) -> io::Result<()> {
        let mut errors = Vec::new();
        for index in 0..self.fds.len() {
            if !self.changed[index].load(Ordering::Acquire) {
                continue;
            }
            let result = (|| -> io::Result<()> {
                let current = rustix::fs::fcntl_getfl(&self.fds[index])?;
                let restored =
                    (current & !OFlags::NONBLOCK) | (self.flags[index] & OFlags::NONBLOCK);
                rustix::fs::fcntl_setfl(&self.fds[index], restored)?;
                let actual = rustix::fs::fcntl_getfl(&self.fds[index])?;
                if actual.contains(OFlags::NONBLOCK) != self.flags[index].contains(OFlags::NONBLOCK)
                {
                    return Err(io::Error::other(
                        "O_NONBLOCK did not match its original value",
                    ));
                }
                Ok(())
            })();
            if let Err(error) = result {
                errors.push(format!("descriptor {index}: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }

    fn stop_and_restore(&self, deadline: Instant) -> io::Result<()> {
        self.stopping.store(true, Ordering::Release);
        let _guard = loop {
            match self.io_gate.try_lock() {
                Ok(guard) => break guard,
                // Poison still returns the exclusively acquired guard. New
                // polls have already been stopped, and this gate carries no
                // protected data, so restoration remains safe and verifiable.
                Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "MCP process IO did not stop before the deadline",
                    ));
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        if !self.restored.load(Ordering::Acquire) {
            self.restore_changed()?;
            self.restored.store(true, Ordering::Release);
        }
        Ok(())
    }
}

impl Drop for ProcessFds {
    fn drop(&mut self) {
        if self.restored.load(Ordering::Acquire) {
            return;
        }
        for index in 0..self.fds.len() {
            if !self.changed[index].load(Ordering::Acquire) {
                continue;
            }
            if let Ok(current) = rustix::fs::fcntl_getfl(&self.fds[index]) {
                let restored =
                    (current & !OFlags::NONBLOCK) | (self.flags[index] & OFlags::NONBLOCK);
                let _ = rustix::fs::fcntl_setfl(&self.fds[index], restored);
            }
        }
    }
}

struct ProcessFd {
    group: Arc<ProcessFds>,
    index: usize,
}

impl AsFd for ProcessFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.group.fds[self.index].as_fd()
    }
}

struct RegisteredProcessFd {
    // Fields are dropped in declaration order: deregister this owned duplicate
    // before the final group owner can restore the shared O_NONBLOCK flags.
    registered: OwnedAsyncFd,
    _owner: ProcessFd,
}

fn register(fd: ProcessFd, interest: Interest) -> io::Result<RegisteredProcessFd> {
    let owned = rustix::io::fcntl_dupfd_cloexec(&fd, 3)?;
    Ok(RegisteredProcessFd {
        registered: OwnedAsyncFd::new(owned, interest)?,
        _owner: fd,
    })
}

enum ProcessIo {
    Ready(RegisteredProcessFd),
    /// Regular files have no readiness registration on epoll. Perform their
    /// ordinary file syscalls directly; no detached blocking task retains them.
    File(ProcessFd),
}

fn read(fd: &ProcessFd, bytes: &mut [u8]) -> io::Result<usize> {
    loop {
        match rustix::io::read(fd, bytes) {
            Err(rustix::io::Errno::INTR) => continue,
            result => return result.map_err(io::Error::from),
        }
    }
}

fn write(fd: &ProcessFd, bytes: &[u8]) -> io::Result<usize> {
    loop {
        match rustix::io::write(fd, bytes) {
            Err(rustix::io::Errno::INTR) => continue,
            result => return result.map_err(io::Error::from),
        }
    }
}

impl AsyncRead for ProcessIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        match self.get_mut() {
            Self::File(fd) => {
                let count = fd
                    .group
                    .file_io(fd.index, || read(fd, buf.initialize_unfilled()))?;
                buf.advance(count);
                Poll::Ready(Ok(()))
            }
            Self::Ready(fd) => {
                let _admission = fd._owner.group.admit()?;
                Pin::new(&mut fd.registered).poll_read(cx, buf)
            }
        }
    }
}

impl AsyncWrite for ProcessIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::File(fd) => Poll::Ready(fd.group.file_io(fd.index, || write(fd, bytes))),
            Self::Ready(fd) => {
                let _admission = fd._owner.group.admit()?;
                Pin::new(&mut fd.registered).poll_write(cx, bytes)
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Each write is already a syscall, with no userspace buffered bytes.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct McpProcessFds {
    group: Option<Arc<ProcessFds>>,
    armed: bool,
    stopped: bool,
}

// Keep a strong reference even if a runtime task retains the last stream.
// Only MCP opts into explicit teardown; ordinary CLI sessions use open().
static MCP_PROCESS_FDS: Mutex<McpProcessFds> = Mutex::new(McpProcessFds {
    group: None,
    armed: false,
    stopped: false,
});

pub(crate) fn open() -> io::Result<HeadlessIo> {
    open_inner(None)
}

/// Arm the process-entrypoint scope before running an MCP server.
/// Direct library callers that do not arm it retain ordinary IO lifetimes.
pub fn begin_mcp_stdio() -> io::Result<()> {
    let mut registry = MCP_PROCESS_FDS
        .lock()
        .map_err(|_| io::Error::other("MCP process IO registry is poisoned"))?;
    if registry.armed || registry.stopped || registry.group.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "MCP process IO scope was already armed or stopped",
        ));
    }
    registry.armed = true;
    Ok(())
}

/// Open process stdio, retaining a shutdown handle only for an armed scope.
pub fn open_mcp() -> io::Result<HeadlessIo> {
    let mut registry = MCP_PROCESS_FDS
        .lock()
        .map_err(|_| io::Error::other("MCP process IO registry is poisoned"))?;
    if !registry.armed {
        drop(registry);
        return open_inner(None);
    }
    if registry.stopped || registry.group.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "MCP process IO was already opened or stopped",
        ));
    }
    open_inner(Some(&mut registry.group))
}

/// Stop new MCP pipe I/O and verify restoration of the flags this process set.
/// A timeout or any failed flag operation means restoration is unverified.
pub fn stop_and_restore_mcp(timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::other("MCP process IO deadline overflow"))?;
    let mut registry = loop {
        match MCP_PROCESS_FDS.try_lock() {
            Ok(guard) => break guard,
            // Setup may unwind while holding the registry. The group is
            // registered before flags change, so it can still be recovered.
            Err(TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "MCP process IO registration did not stop before the deadline",
                ));
            }
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    };
    if !registry.armed {
        return Ok(());
    }
    registry.stopped = true;
    let group = registry.group.clone();
    drop(registry);
    if let Some(group) = group {
        group.stop_and_restore(deadline)?;
    }
    Ok(())
}

fn open_inner(mcp_group: Option<&mut Option<Arc<ProcessFds>>>) -> io::Result<HeadlessIo> {
    let fds = [
        rustix::io::fcntl_dupfd_cloexec(std::io::stdin(), 3)?,
        rustix::io::fcntl_dupfd_cloexec(std::io::stdout(), 3)?,
        rustix::io::fcntl_dupfd_cloexec(std::io::stderr(), 3)?,
    ];
    let types = [
        FileType::from_raw_mode(rustix::fs::fstat(&fds[0])?.st_mode),
        FileType::from_raw_mode(rustix::fs::fstat(&fds[1])?.st_mode),
        FileType::from_raw_mode(rustix::fs::fstat(&fds[2])?.st_mode),
    ];
    let terminals = [
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
    ];
    let group = ProcessFds::new(fds, types, mcp_group.is_some(), mcp_group)?;
    let stream = |index, interest| -> io::Result<ProcessIo> {
        let fd = ProcessFd {
            group: group.clone(),
            index,
        };
        if types[index] == FileType::RegularFile
            || (types[index] == FileType::CharacterDevice && !terminals[index])
        {
            // `/dev/null` cannot be registered on epoll either; its direct
            // nonblocking syscall completes immediately.
            Ok(ProcessIo::File(fd))
        } else {
            register(fd, interest).map(ProcessIo::Ready)
        }
    };
    Ok(HeadlessIo {
        input: Box::pin(stream(0, Interest::READABLE)?),
        input_is_terminal: terminals[0],
        stdout: Output::with_terminal(stream(1, Interest::WRITABLE)?, terminals[1]),
        stderr: Output::with_terminal(stream(2, Interest::WRITABLE)?, terminals[2]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;

    struct ChildGuard(std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    #[tokio::test]
    async fn shared_pipe_flags_restore_after_the_last_registered_stream_drops() -> io::Result<()> {
        let (reader, writer) = std::io::pipe()?;
        let original_reader_flags = rustix::fs::fcntl_getfl(&reader)?;
        let original_writer_flags = rustix::fs::fcntl_getfl(&writer)?;
        let group = ProcessFds::new(
            [
                reader.as_fd().try_clone_to_owned()?,
                writer.as_fd().try_clone_to_owned()?,
                writer.as_fd().try_clone_to_owned()?,
            ],
            [FileType::Fifo; 3],
            false,
            None,
        )?;
        let mut input = ProcessIo::Ready(register(
            ProcessFd {
                group: group.clone(),
                index: 0,
            },
            Interest::READABLE,
        )?);
        let stdout = ProcessIo::Ready(register(
            ProcessFd {
                group: group.clone(),
                index: 1,
            },
            Interest::WRITABLE,
        )?);
        let mut stderr = ProcessIo::Ready(register(
            ProcessFd {
                group: group.clone(),
                index: 2,
            },
            Interest::WRITABLE,
        )?);
        drop(group);
        drop(stdout);
        assert!(rustix::fs::fcntl_getfl(&writer)?.contains(OFlags::NONBLOCK));
        stderr.write_all(b"ok").await?;
        let mut bytes = [0; 2];
        timeout(Duration::from_secs(2), input.read_exact(&mut bytes)).await??;
        assert_eq!(&bytes, b"ok");
        drop(stderr);
        assert!(rustix::fs::fcntl_getfl(&writer)?.contains(OFlags::NONBLOCK));
        drop(input);
        // Restore only the flag owned by ProcessFds. Other flags can change
        // independently (File::write enables no-SIGPIPE on macOS).
        assert_eq!(
            rustix::fs::fcntl_getfl(&reader)? & OFlags::NONBLOCK,
            original_reader_flags & OFlags::NONBLOCK
        );
        assert_eq!(
            rustix::fs::fcntl_getfl(&writer)? & OFlags::NONBLOCK,
            original_writer_flags & OFlags::NONBLOCK
        );
        Ok(())
    }

    #[tokio::test]
    async fn managed_stop_times_out_without_restoring_across_an_active_poll_gate() -> io::Result<()>
    {
        let (reader, writer) = std::io::pipe()?;
        let group = ProcessFds::new(
            [
                reader.as_fd().try_clone_to_owned()?,
                writer.as_fd().try_clone_to_owned()?,
                writer.as_fd().try_clone_to_owned()?,
            ],
            [FileType::Fifo; 3],
            true,
            None,
        )?;
        let mut output = ProcessIo::Ready(register(
            ProcessFd {
                group: group.clone(),
                index: 1,
            },
            Interest::WRITABLE,
        )?);
        let active = group.io_gate.lock().unwrap();
        let error = group
            .stop_and_restore(Instant::now() + Duration::from_millis(30))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(rustix::fs::fcntl_getfl(&writer)?.contains(OFlags::NONBLOCK));
        drop(active);
        assert_eq!(
            output.write(b"x").await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        group.stop_and_restore(Instant::now() + Duration::from_secs(1))?;
        assert!(!rustix::fs::fcntl_getfl(&writer)?.contains(OFlags::NONBLOCK));
        Ok(())
    }

    #[test]
    fn admitted_regular_file_io_does_not_hold_the_restoration_gate() -> io::Result<()> {
        let file = std::fs::File::open(std::env::current_exe()?)?;
        let (reader, writer) = std::io::pipe()?;
        let group = ProcessFds::new(
            [
                file.as_fd().try_clone_to_owned()?,
                reader.as_fd().try_clone_to_owned()?,
                writer.as_fd().try_clone_to_owned()?,
            ],
            [FileType::RegularFile, FileType::Fifo, FileType::Fifo],
            true,
            None,
        )?;
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn({
            let group = group.clone();
            move || {
                group.file_io(0, || {
                    let _ = entered_tx.send(());
                    let _ = release_rx.recv();
                    Ok(())
                })
            }
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        group.stop_and_restore(Instant::now() + Duration::from_secs(1))?;
        assert!(!rustix::fs::fcntl_getfl(&reader)?.contains(OFlags::NONBLOCK));
        assert!(!rustix::fs::fcntl_getfl(&writer)?.contains(OFlags::NONBLOCK));
        assert_eq!(
            group.file_io(0, || Ok(())).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        release_tx.send(()).unwrap();
        worker.join().unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn managed_stdio_restores_shared_parent_pipes_with_live_output() -> io::Result<()> {
        const CHILD: &str = "MCP_TEST_SHARED_PIPE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            begin_mcp_stdio()?;
            let io = open_mcp()?;
            let mut input = io.input;
            let retained_output = io.stdout;
            drop(io.stderr);
            println!("MCP_SHARED_PIPES_READY");
            std::io::stdout().flush()?;
            let mut proceed = [0];
            input.read_exact(&mut proceed).await?;
            assert_eq!(proceed, [b'x']);
            stop_and_restore_mcp(Duration::from_secs(1))?;
            assert_eq!(
                retained_output
                    .write_all(b"must not be written")
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::BrokenPipe
            );
            // Keep the output alive through exit: restoration must not depend
            // on its Arc being dropped by the runtime.
            std::hint::black_box(&retained_output);
            println!("MCP_SHARED_PIPES_RESTORED");
            std::io::stdout().flush()?;
            std::process::exit(0);
        }

        for initially_nonblocking in [false, true] {
            let (input_reader, mut input_writer) = std::io::pipe()?;
            let (output_reader, output_writer) = std::io::pipe()?;
            if initially_nonblocking {
                for fd in [input_reader.as_fd(), output_writer.as_fd()] {
                    let flags = rustix::fs::fcntl_getfl(fd)?;
                    rustix::fs::fcntl_setfl(fd, flags | OFlags::NONBLOCK)?;
                }
            }
            let original_input = rustix::fs::fcntl_getfl(&input_reader)? & OFlags::NONBLOCK;
            let original_output = rustix::fs::fcntl_getfl(&output_writer)? & OFlags::NONBLOCK;
            let child = Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "headless_process_io::tests::managed_stdio_restores_shared_parent_pipes_with_live_output",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .stdin(Stdio::from(input_reader.as_fd().try_clone_to_owned()?))
                .stdout(Stdio::from(output_writer.as_fd().try_clone_to_owned()?))
                .stderr(Stdio::null())
                .spawn()?;
            let mut child = ChildGuard(child);
            let (lines_tx, lines_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for line in io::BufReader::new(output_reader).lines() {
                    if lines_tx.send(line).is_err() {
                        break;
                    }
                }
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let ready = loop {
                match lines_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(Ok(line)) if line.contains("MCP_SHARED_PIPES_READY") => break true,
                    Ok(Ok(_)) => {}
                    _ => break false,
                }
            };
            if !ready {
                panic!("shared-pipe child did not become ready");
            }
            assert!(rustix::fs::fcntl_getfl(&input_reader)?.contains(OFlags::NONBLOCK));
            assert!(rustix::fs::fcntl_getfl(&output_writer)?.contains(OFlags::NONBLOCK));
            input_writer.write_all(b"x")?;
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.0.try_wait()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    panic!("shared-pipe child did not exit");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(status.success(), "shared-pipe child failed: {status}");
            assert_eq!(
                rustix::fs::fcntl_getfl(&input_reader)? & OFlags::NONBLOCK,
                original_input
            );
            assert_eq!(
                rustix::fs::fcntl_getfl(&output_writer)? & OFlags::NONBLOCK,
                original_output
            );
            drop(output_writer);
            let deadline = Instant::now() + Duration::from_secs(2);
            let restored = loop {
                match lines_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(Ok(line)) if line.contains("MCP_SHARED_PIPES_RESTORED") => break true,
                    Ok(Ok(_)) => {}
                    _ => break false,
                }
            };
            assert!(restored, "shared-pipe child did not confirm restoration");
        }
        Ok(())
    }

    #[test]
    fn unarmed_library_open_and_panicking_managed_open_restore_flags() -> io::Result<()> {
        const CHILD: &str = "MCP_TEST_OPEN_PANIC_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let streams = open_mcp()?;
                assert!(rustix::fs::fcntl_getfl(std::io::stdout())?.contains(OFlags::NONBLOCK));
                drop(streams);
                assert!(!rustix::fs::fcntl_getfl(std::io::stdout())?.contains(OFlags::NONBLOCK));
                Ok::<_, io::Error>(())
            })?;
            drop(runtime);

            begin_mcp_stdio()?;
            // Registering a pipe without a Tokio reactor panics after flags
            // have been set. The shutdown path must recover the poisoned
            // registry and still verify its retained group's restoration.
            assert!(std::panic::catch_unwind(open_mcp).is_err());
            assert!(rustix::fs::fcntl_getfl(std::io::stdout())?.contains(OFlags::NONBLOCK));
            stop_and_restore_mcp(Duration::from_secs(1))?;
            assert!(!rustix::fs::fcntl_getfl(std::io::stdout())?.contains(OFlags::NONBLOCK));
            std::process::exit(0);
        }

        let (output_reader, output_writer) = std::io::pipe()?;
        let original = rustix::fs::fcntl_getfl(&output_writer)? & OFlags::NONBLOCK;
        assert!(!original.contains(OFlags::NONBLOCK));
        let child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "headless_process_io::tests::unarmed_library_open_and_panicking_managed_open_restore_flags",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(output_writer.as_fd().try_clone_to_owned()?))
            .stderr(Stdio::null())
            .spawn()?;
        let mut child = ChildGuard(child);
        std::thread::spawn(move || {
            let _ = io::copy(&mut io::BufReader::new(output_reader), &mut io::sink());
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                panic!("setup-panic child did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "setup-panic child failed: {status}");
        assert_eq!(
            rustix::fs::fcntl_getfl(&output_writer)? & OFlags::NONBLOCK,
            original
        );
        Ok(())
    }

    #[test]
    fn poisoned_poll_gate_restores_shared_parent_pipe() -> io::Result<()> {
        const CHILD: &str = "MCP_TEST_POISONED_POLL_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            begin_mcp_stdio()?;
            let streams = runtime.block_on(async { open_mcp() })?;
            let group = MCP_PROCESS_FDS
                .lock()
                .unwrap()
                .group
                .as_ref()
                .unwrap()
                .clone();
            assert!(rustix::fs::fcntl_getfl(std::io::stdout())?.contains(OFlags::NONBLOCK));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                group.file_io(1, || -> io::Result<()> { panic!("controlled poll panic") })
            }));
            assert!(result.is_err());
            stop_and_restore_mcp(Duration::from_secs(1))?;
            assert!(!rustix::fs::fcntl_getfl(std::io::stdout())?.contains(OFlags::NONBLOCK));
            assert_eq!(
                runtime
                    .block_on(streams.stdout.write_all(b"must not be written"))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::BrokenPipe
            );
            std::hint::black_box(&streams.stdout);
            std::process::exit(0);
        }

        let (output_reader, output_writer) = std::io::pipe()?;
        let original = rustix::fs::fcntl_getfl(&output_writer)? & OFlags::NONBLOCK;
        assert!(!original.contains(OFlags::NONBLOCK));
        let child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "headless_process_io::tests::poisoned_poll_gate_restores_shared_parent_pipe",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(output_writer.as_fd().try_clone_to_owned()?))
            .stderr(Stdio::null())
            .spawn()?;
        let mut child = ChildGuard(child);
        std::thread::spawn(move || {
            let _ = io::copy(&mut io::BufReader::new(output_reader), &mut io::sink());
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                panic!("poisoned-poll child did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let after = rustix::fs::fcntl_getfl(&output_writer)? & OFlags::NONBLOCK;
        assert!(
            status.success() && after == original,
            "poisoned-poll child status={status}, before_nonblock={}, after_nonblock={}",
            original.contains(OFlags::NONBLOCK),
            after.contains(OFlags::NONBLOCK)
        );
        Ok(())
    }
}
