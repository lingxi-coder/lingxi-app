//! Unix process IO without Tokio's uncancellable blocking stdio workers.

use harness_runtime::headless::io::{HeadlessIo, Output};
use lingxi_async_owned_fd::OwnedAsyncFd;
use rustix::fs::{FileType, OFlags};
use std::io::{self, IsTerminal};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, Interest, ReadBuf};

/// Capture every original flag before modifying any descriptor. stdout and
/// stderr may share one open-file description (`2>&1`); restore only when all
/// three streams have been released, so one writer cannot re-block another.
struct ProcessFds {
    fds: [OwnedFd; 3],
    flags: [OFlags; 3],
    changed: [bool; 3],
}

impl ProcessFds {
    fn new(fds: [OwnedFd; 3], types: [FileType; 3]) -> io::Result<Arc<Self>> {
        let flags = [
            rustix::fs::fcntl_getfl(&fds[0])?,
            rustix::fs::fcntl_getfl(&fds[1])?,
            rustix::fs::fcntl_getfl(&fds[2])?,
        ];
        let mut group = Self {
            fds,
            flags,
            changed: [false; 3],
        };
        for index in 0..3 {
            if types[index] != FileType::RegularFile && !flags[index].contains(OFlags::NONBLOCK) {
                rustix::fs::fcntl_setfl(&group.fds[index], flags[index] | OFlags::NONBLOCK)?;
                group.changed[index] = true;
            }
        }
        Ok(Arc::new(group))
    }
}

impl Drop for ProcessFds {
    fn drop(&mut self) {
        for index in 0..self.fds.len() {
            if !self.changed[index] {
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
                let count = read(fd, buf.initialize_unfilled())?;
                buf.advance(count);
                Poll::Ready(Ok(()))
            }
            Self::Ready(fd) => Pin::new(&mut fd.registered).poll_read(cx, buf),
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
            Self::File(fd) => Poll::Ready(write(fd, bytes)),
            Self::Ready(fd) => Pin::new(&mut fd.registered).poll_write(cx, bytes),
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

pub(crate) fn open() -> io::Result<HeadlessIo> {
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
    let group = ProcessFds::new(fds, types)?;
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
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;

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
}
