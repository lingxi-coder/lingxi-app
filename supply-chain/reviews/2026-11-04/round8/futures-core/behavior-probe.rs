//! Diagnostic probes. Not executed until the disk-space floor permits compilation.
use futures_core::task::__internal::AtomicWaker;
use futures_core::{Stream, TryStream};
use std::marker::PhantomPinned;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Wake, Waker};
use std::time::Duration;

#[derive(Default)]
struct CountWake(AtomicUsize);
impl Wake for CountWake {
    fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
    fn wake_by_ref(self: &Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
}

#[test]
fn sequential_replacement_take_and_wake() {
    let cell = AtomicWaker::new();
    let a = Arc::new(CountWake::default());
    let b = Arc::new(CountWake::default());
    let wa = Waker::from(a.clone());
    let wb = Waker::from(b.clone());
    cell.wake();
    cell.register(&wa);
    cell.register(&wb);
    cell.wake();
    cell.wake();
    assert_eq!(a.0.load(Ordering::SeqCst), 0);
    assert_eq!(b.0.load(Ordering::SeqCst), 1);
    cell.register(&wa);
    let taken = cell.take().expect("registered waker");
    cell.wake();
    assert_eq!(a.0.load(Ordering::SeqCst), 0);
    taken.wake();
    assert_eq!(a.0.load(Ordering::SeqCst), 1);
}

struct BlockingDrop { entered: mpsc::Sender<()>, gate: Arc<(Mutex<bool>, Condvar)> }
impl Wake for BlockingDrop { fn wake(self: Arc<Self>) {} }
impl Drop for BlockingDrop {
    fn drop(&mut self) {
        self.entered.send(()).expect("receiver alive");
        let (lock, cv) = &*self.gate;
        let guard = lock.lock().unwrap();
        let (_guard, timeout) = cv.wait_timeout_while(guard, Duration::from_secs(5), |open| !*open).unwrap();
        assert!(!timeout.timed_out(), "test gate timed out");
    }
}

fn run_contention(take_instead_of_wake: bool) {
    let cell = Arc::new(AtomicWaker::new());
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let old = Waker::from(Arc::new(BlockingDrop { entered: entered_tx, gate: gate.clone() }));
    cell.register(&old);
    drop(old);
    let counter = Arc::new(CountWake::default());
    let new = Waker::from(counter.clone());
    let other = cell.clone();
    let thread = std::thread::spawn(move || other.register(&new));
    entered_rx.recv_timeout(Duration::from_secs(5)).expect("old waker dropped during register");
    if take_instead_of_wake {
        assert!(cell.take().is_none());
    } else {
        cell.wake();
    }
    let (lock, cv) = &*gate;
    *lock.lock().unwrap() = true;
    cv.notify_one();
    thread.join().unwrap();
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    cell.wake();
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);
}

#[test]
fn wake_during_register_is_forwarded_after_unlock() { run_contention(false); }

#[test]
fn contended_take_returns_none_and_register_wakes() { run_contention(true); }

struct PinnedStream { yielded: bool, _pin: PhantomPinned }
impl Stream for PinnedStream {
    type Item = Result<u8, &'static str>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // This test stream is polled once; it only reads its pinned state.
        assert!(!self.yielded);
        Poll::Ready(Some(Ok(7)))
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (1, Some(1)) }
}

#[test]
fn assert_unwind_safe_projects_pinned_stream() {
    let waker = Waker::from(Arc::new(CountWake::default()));
    let mut cx = Context::from_waker(&waker);
    let mut stream = Box::pin(AssertUnwindSafe(PinnedStream { yielded: false, _pin: PhantomPinned }));
    assert_eq!(stream.as_ref().get_ref().size_hint(), (1, Some(1)));
    assert_eq!(stream.as_mut().try_poll_next(&mut cx), Poll::Ready(Some(Ok(7))));
}

fn propagate_ready(value: Poll<u8>) -> Poll<u8> {
    let value = futures_core::ready!(value);
    Poll::Ready(value + 1)
}

#[test]
fn ready_macro_propagates_pending_and_value() {
    assert_eq!(propagate_ready(Poll::Pending), Poll::Pending);
    assert_eq!(propagate_ready(Poll::Ready(3)), Poll::Ready(4));
}

// Diagnostic hypothesis only: no resources or pointers are owned by this
// RawWaker. The clone callback panics before producing a new reference. Its
// no-op drop is thread-safe, and wake callbacks count a synthetic task wake.
static PANIC_WAKES: AtomicUsize = AtomicUsize::new(0);
unsafe fn panic_clone(_: *const ()) -> RawWaker { panic!("diagnostic clone panic") }
unsafe fn panic_wake(_: *const ()) { PANIC_WAKES.fetch_add(1, Ordering::SeqCst); }
unsafe fn panic_drop(_: *const ()) {}
static PANIC_VTABLE: RawWakerVTable = RawWakerVTable::new(panic_clone, panic_wake, panic_wake, panic_drop);

#[test]
fn diagnostic_clone_panic_may_leave_register_stuck() {
    let cell = AtomicWaker::new();
    let bad = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &PANIC_VTABLE)) };
    assert!(catch_unwind(AssertUnwindSafe(|| cell.register(&bad))).is_err());
    let counter = Arc::new(CountWake::default());
    let good = Waker::from(counter.clone());
    cell.register(&good);
    cell.wake();
    // This asserts the *suspected failure*, not a safety or correctness pass.
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);
}
