//! Event-driven I/O for an exclusively owned, nonblocking Unix descriptor.

#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, Interest, ReadBuf};

/// An owned descriptor registered with the Tokio reactor.
///
/// The underlying descriptor is private and cannot be replaced or closed by
/// callers while registered. The descriptor must already be nonblocking.
pub struct OwnedAsyncFd {
    inner: AsyncFd<File>,
}

impl OwnedAsyncFd {
    /// Registers a nonblocking owned descriptor with the current reactor.
    ///
    /// `interest` must include each direction in which the descriptor is used.
    #[allow(unsafe_code)]
    pub fn new(fd: OwnedFd, interest: Interest) -> io::Result<Self> {
        // SAFETY: File takes exclusive ownership of the descriptor. It remains
        // private inside this wrapper and cannot be replaced or closed while
        // the AsyncFd is registered; AsyncFd deregisters it before dropping it.
        let inner = unsafe { AsyncFd::register_with_interest(File::from(fd), interest) }?;
        Ok(Self { inner })
    }
}

impl AsyncRead for OwnedAsyncFd {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut guard = ready!(self.inner.poll_read_ready(cx))?;
            match guard.try_io(|fd| loop {
                match fd.get_ref().read(buf.initialize_unfilled()) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => break result,
                }
            }) {
                Ok(Ok(count)) => {
                    buf.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for OwnedAsyncFd {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.inner.poll_write_ready(cx))?;
            match guard.try_io(|fd| loop {
                match fd.get_ref().write(bytes) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => break result,
                }
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::unix::pipe::{Receiver, Sender};
    use tokio::net::UnixStream;
    use tokio::time::timeout;

    #[tokio::test]
    async fn pipe_wakes_after_cancelled_read_and_closes_on_drop() -> io::Result<()> {
        let (reader, writer) = std::io::pipe()?;
        // Tokio's checked pipe constructors set O_NONBLOCK before registration.
        let reader = Receiver::from_owned_fd(reader.into())?;
        let writer = Sender::from_owned_fd(writer.into())?;
        let reader_fd = reader.as_fd().try_clone_to_owned()?;
        let writer_fd = writer.as_fd().try_clone_to_owned()?;
        drop(reader);
        drop(writer);

        let mut reader = OwnedAsyncFd::new(reader_fd, Interest::READABLE)?;
        let mut writer = OwnedAsyncFd::new(writer_fd, Interest::WRITABLE)?;
        let mut bytes = [0; 5];
        assert!(timeout(Duration::from_millis(20), reader.read(&mut bytes))
            .await
            .is_err());
        writer.write_all(b"hello").await?;
        timeout(Duration::from_secs(2), reader.read_exact(&mut bytes)).await??;
        assert_eq!(&bytes, b"hello");
        drop(writer);
        assert_eq!(
            timeout(Duration::from_secs(2), reader.read(&mut bytes)).await??,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn socket_remains_bidirectionally_event_driven() -> io::Result<()> {
        let (socket, peer) = StdUnixStream::pair()?;
        socket.set_nonblocking(true)?;
        peer.set_nonblocking(true)?;
        let mut socket = OwnedAsyncFd::new(socket.into(), Interest::READABLE | Interest::WRITABLE)?;
        let mut peer = UnixStream::from_std(peer)?;
        let mut buf = [0; 4];
        assert!(timeout(Duration::from_millis(20), socket.read(&mut buf))
            .await
            .is_err());
        peer.write_all(b"ping").await?;
        timeout(Duration::from_secs(2), socket.read_exact(&mut buf)).await??;
        assert_eq!(&buf, b"ping");
        socket.write_all(b"pong").await?;
        timeout(Duration::from_secs(2), peer.read_exact(&mut buf)).await??;
        assert_eq!(&buf, b"pong");
        drop(socket);
        assert_eq!(
            timeout(Duration::from_secs(2), peer.read(&mut buf)).await??,
            0
        );
        Ok(())
    }
}
