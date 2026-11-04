use futures_io::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite};
use std::cell::RefCell;
use std::io::{self, IoSlice, IoSliceMut, SeekFrom};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
fn waker() -> Waker { Waker::from(Arc::new(Noop)) }

#[derive(Default)]
struct Reader { seen: Vec<usize>, mode: u8 }
impl AsyncRead for Reader {
    fn poll_read(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.seen.push(buf.len());
        match self.mode {
            1 => Poll::Pending,
            2 => Poll::Ready(Err(io::ErrorKind::PermissionDenied.into())),
            _ => { if !buf.is_empty() { buf[0] = 42; } Poll::Ready(Ok(buf.len())) }
        }
    }
}

#[derive(Default)]
struct Writer { seen: Vec<Vec<u8>>, mode: u8 }
impl AsyncWrite for Writer {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.seen.push(buf.to_vec());
        match self.mode {
            1 => Poll::Pending,
            2 => Poll::Ready(Err(io::ErrorKind::PermissionDenied.into())),
            _ => Poll::Ready(Ok(buf.len()))
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { Poll::Ready(Ok(())) }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { Poll::Ready(Ok(())) }
}

#[test]
fn default_vectored_read_selects_one_buffer_and_propagates() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let mut reader = Reader::default();
    let mut empty = []; let mut first = [0; 3]; let mut second = [0; 2];
    let mut bufs = [IoSliceMut::new(&mut empty), IoSliceMut::new(&mut first), IoSliceMut::new(&mut second)];
    assert!(matches!(Pin::new(&mut reader).poll_read_vectored(&mut cx, &mut bufs), Poll::Ready(Ok(3))));
    assert_eq!(reader.seen, [3]);
    assert_eq!(first, [42, 0, 0]); assert_eq!(second, [0, 0]);
    let mut empty = []; let mut bufs = [IoSliceMut::new(&mut empty)];
    assert!(matches!(Pin::new(&mut reader).poll_read_vectored(&mut cx, &mut bufs), Poll::Ready(Ok(0))));
    assert!(matches!(Pin::new(&mut reader).poll_read_vectored(&mut cx, &mut []), Poll::Ready(Ok(0))));
    assert_eq!(reader.seen, [3, 0, 0]);
    reader.mode = 1;
    let mut data = [0; 2]; let mut bufs = [IoSliceMut::new(&mut data)];
    assert!(matches!(Pin::new(&mut reader).poll_read_vectored(&mut cx, &mut bufs), Poll::Pending));
    reader.mode = 2;
    assert!(matches!(Pin::new(&mut reader).poll_read_vectored(&mut cx, &mut bufs), Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::PermissionDenied));
    assert_eq!(reader.seen, [3, 0, 0, 2, 2]);
}

#[test]
fn default_vectored_write_selects_one_buffer_and_propagates() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let mut writer = Writer::default();
    let bufs = [IoSlice::new(b""), IoSlice::new(b"abc"), IoSlice::new(b"xy")];
    assert!(matches!(Pin::new(&mut writer).poll_write_vectored(&mut cx, &bufs), Poll::Ready(Ok(3))));
    assert!(matches!(Pin::new(&mut writer).poll_write_vectored(&mut cx, &[]), Poll::Ready(Ok(0))));
    writer.mode = 1;
    assert!(matches!(Pin::new(&mut writer).poll_write_vectored(&mut cx, &bufs), Poll::Pending));
    writer.mode = 2;
    assert!(matches!(Pin::new(&mut writer).poll_write_vectored(&mut cx, &bufs), Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::PermissionDenied));
    assert_eq!(writer.seen, [b"abc".to_vec(), vec![], b"abc".to_vec(), b"abc".to_vec()]);
}

#[test]
fn slice_and_vec_use_memory_stdio_and_close_only_flushes() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let mut data: &[u8] = b"abcd";
    assert!(matches!(Pin::new(&mut data).poll_fill_buf(&mut cx), Poll::Ready(Ok(b"abcd"))));
    Pin::new(&mut data).consume(1);
    let mut out = [0; 2];
    assert!(matches!(Pin::new(&mut data).poll_read(&mut cx, &mut out), Poll::Ready(Ok(2))));
    assert_eq!(&out, b"bc");
    assert!(matches!(Pin::new(&mut data).poll_fill_buf(&mut cx), Poll::Ready(Ok(b"d"))));
    Pin::new(&mut data).consume(1);
    assert!(matches!(Pin::new(&mut data).poll_read(&mut cx, &mut out), Poll::Ready(Ok(0))));

    let mut output = Vec::new();
    assert!(matches!(Pin::new(&mut output).poll_write(&mut cx, b"a"), Poll::Ready(Ok(1))));
    let bufs = [IoSlice::new(b"bc"), IoSlice::new(b"de")];
    assert!(matches!(Pin::new(&mut output).poll_write_vectored(&mut cx, &bufs), Poll::Ready(Ok(4))));
    assert!(matches!(Pin::new(&mut output).poll_flush(&mut cx), Poll::Ready(Ok(()))));
    assert!(matches!(Pin::new(&mut output).poll_close(&mut cx), Poll::Ready(Ok(()))));
    assert!(matches!(Pin::new(&mut output).poll_write(&mut cx, b"f"), Poll::Ready(Ok(1))));
    assert_eq!(output, b"abcdef");
}

struct Pinned { calls: Rc<RefCell<Vec<&'static str>>>, _pin: PhantomPinned }
impl Pinned { fn record(&self, call: &'static str) { self.calls.borrow_mut().push(call); } }
impl AsyncRead for Pinned {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut [u8]) -> Poll<io::Result<usize>> { self.record("read"); Poll::Pending }
    fn poll_read_vectored(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut [IoSliceMut<'_>]) -> Poll<io::Result<usize>> { self.record("readv"); Poll::Pending }
}
impl AsyncWrite for Pinned {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> { self.record("write"); Poll::Pending }
    fn poll_write_vectored(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[IoSlice<'_>]) -> Poll<io::Result<usize>> { self.record("writev"); Poll::Pending }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { self.record("flush"); Poll::Pending }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { self.record("close"); Poll::Pending }
}
impl AsyncSeek for Pinned {
    fn poll_seek(self: Pin<&mut Self>, _: &mut Context<'_>, pos: SeekFrom) -> Poll<io::Result<u64>> { assert_eq!(pos, SeekFrom::Start(7)); self.record("seek"); Poll::Pending }
}
impl AsyncBufRead for Pinned {
    fn poll_fill_buf(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<&[u8]>> { self.record("fill"); Poll::Ready(Ok(b"abc")) }
    fn consume(self: Pin<&mut Self>, amount: usize) { assert_eq!(amount, 2); self.record("consume"); }
}

#[test]
fn pinned_pointer_forwards_to_non_unpin_target() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut p = Box::pin(Pinned { calls: calls.clone(), _pin: PhantomPinned });
    assert!(matches!(Pin::new(&mut p).poll_read(&mut cx, &mut []), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_read_vectored(&mut cx, &mut []), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_write(&mut cx, &[]), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_write_vectored(&mut cx, &[]), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_flush(&mut cx), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_close(&mut cx), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_seek(&mut cx, SeekFrom::Start(7)), Poll::Pending));
    assert!(matches!(Pin::new(&mut p).poll_fill_buf(&mut cx), Poll::Ready(Ok(b"abc"))));
    Pin::new(&mut p).consume(2);
    assert_eq!(&*calls.borrow(), &["read", "readv", "write", "writev", "flush", "close", "seek", "fill", "consume"]);
}

#[test]
fn boxed_and_mutable_reference_forward() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let mut reader = Reader::default();
    let mut reference = &mut reader;
    assert!(matches!(Pin::new(&mut reference).poll_read(&mut cx, &mut [0; 2]), Poll::Ready(Ok(2))));
    assert_eq!(reader.seen, [2]);
    let mut writer = Box::new(Writer::default());
    assert!(matches!(Pin::new(&mut writer).poll_write(&mut cx, b"hi"), Poll::Ready(Ok(2))));
    assert_eq!(writer.seen, [b"hi".to_vec()]);
}
