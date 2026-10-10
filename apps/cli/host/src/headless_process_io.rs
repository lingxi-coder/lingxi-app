//! Unix process IO without Tokio's uncancellable blocking stdio workers.

use harness_runtime::headless::io::{HeadlessIo, Output};
use rustix::fs::{FileType, OFlags};
use std::io::{self, IsTerminal};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, Interest, ReadBuf};

/// Capture every original flag before modifying any descriptor. stdout and
/// stderr may share one open-file description (`2>&1`); restore only when all
/// three streams have been released, so one writer cannot re-block another.
struct ProcessFds {
    fds: [OwnedFd; 3],
    flags: [OFlags; 3],
    changed: [bool; 3],
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

impl AsRawFd for ProcessFd {
    fn as_raw_fd(&self) -> RawFd {
        self.group.fds[self.index].as_raw_fd()
    }
}

impl AsFd for ProcessFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.group.fds[self.index].as_fd()
    }
}

enum ProcessIo {
    Ready(AsyncFd<ProcessFd>),
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
            Self::Ready(fd) => loop {
                let mut guard = ready!(fd.poll_read_ready(cx))?;
                match guard.try_io(|fd| read(fd.get_ref(), buf.initialize_unfilled())) {
                    Ok(Ok(count)) => {
                        buf.advance(count);
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(error)) => return Poll::Ready(Err(error)),
                    Err(_would_block) => continue,
                }
            },
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
            Self::Ready(fd) => loop {
                let mut guard = ready!(fd.poll_write_ready(cx))?;
                match guard.try_io(|fd| write(fd.get_ref(), bytes)) {
                    Ok(result) => return Poll::Ready(result),
                    Err(_would_block) => continue,
                }
            },
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
    let flags = [
        rustix::fs::fcntl_getfl(&fds[0])?,
        rustix::fs::fcntl_getfl(&fds[1])?,
        rustix::fs::fcntl_getfl(&fds[2])?,
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
    let mut group = ProcessFds {
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
    let group = Arc::new(group);
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
            AsyncFd::with_interest(fd, interest).map(ProcessIo::Ready)
        }
    };
    Ok(HeadlessIo {
        input: Box::pin(stream(0, Interest::READABLE)?),
        input_is_terminal: terminals[0],
        stdout: Output::with_terminal(stream(1, Interest::WRITABLE)?, terminals[1]),
        stderr: Output::with_terminal(stream(2, Interest::WRITABLE)?, terminals[2]),
    })
}
