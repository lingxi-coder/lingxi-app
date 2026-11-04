use futures_task::{noop_waker, noop_waker_ref, waker, waker_ref, ArcWake, FutureObj, LocalFutureObj, WakerRef};
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::task::{Context, Poll, Waker};

struct Notify {
    wakes: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    panic_once: AtomicBool,
}
impl ArcWake for Notify {
    fn wake_by_ref(this: &Arc<Self>) {
        if this.panic_once.swap(false, Ordering::SeqCst) { panic!("expected wake panic"); }
        this.wakes.fetch_add(1, Ordering::SeqCst);
    }
}
impl Drop for Notify {
    fn drop(&mut self) { self.drops.fetch_add(1, Ordering::SeqCst); }
}
fn notify(panic_once: bool) -> (Arc<Notify>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    (Arc::new(Notify { wakes: wakes.clone(), drops: drops.clone(), panic_once: AtomicBool::new(panic_once) }), wakes, drops)
}

#[test]
fn owned_waker_counts_and_consuming_wake() {
    let (node, wakes, drops) = notify(false);
    let owned = waker(node.clone());
    assert_eq!(Arc::strong_count(&node), 2);
    let other = owned.clone();
    assert_eq!(Arc::strong_count(&node), 3);
    owned.wake_by_ref();
    other.wake();
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert_eq!(Arc::strong_count(&node), 2);
    drop(owned);
    drop(node);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn borrowed_waker_clone_survives_owner_and_can_wake_on_another_thread() {
    let (node, wakes, drops) = notify(false);
    let cloned = {
        let borrowed = waker_ref(&node);
        assert_eq!(Arc::strong_count(&node), 1);
        std::thread::scope(|scope| { scope.spawn(|| borrowed.wake_by_ref()).join().unwrap(); });
        let cloned: Waker = (*borrowed).clone();
        assert_eq!(Arc::strong_count(&node), 2);
        cloned
    };
    assert_eq!(Arc::strong_count(&node), 2);
    drop(node);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    std::thread::spawn(move || cloned.wake()).join().unwrap();
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn borrowed_from_owned_waker_does_not_drop_owner() {
    let (node, wakes, drops) = notify(false);
    let owned = waker(node.clone());
    let cloned = {
        let borrowed = WakerRef::new(&owned);
        assert_eq!(Arc::strong_count(&node), 2);
        borrowed.wake_by_ref();
        (*borrowed).clone()
    };
    assert_eq!(Arc::strong_count(&node), 3);
    drop(owned);
    drop(node);
    cloned.wake();
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn borrowed_wake_panic_keeps_original_count() {
    let (node, wakes, drops) = notify(true);
    let borrowed = waker_ref(&node);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| borrowed.wake_by_ref()));
    assert!(r.is_err());
    assert_eq!(Arc::strong_count(&node), 1);
    borrowed.wake_by_ref();
    let cloned = (*borrowed).clone();
    drop(borrowed);
    drop(node);
    cloned.wake();
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

struct PinnedFuture {
    address: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    polls: usize,
    _pin: PhantomPinned,
}
impl Future for PinnedFuture {
    type Output = usize;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<usize> {
        let address = (&*self as *const Self) as usize;
        // We do not move the pinned object or its pinned fields.
        let this = unsafe { self.get_unchecked_mut() };
        let old = this.address.swap(address, Ordering::SeqCst);
        if old != 0 { assert_eq!(old, address); }
        this.polls += 1;
        if this.polls == 1 { Poll::Pending } else { Poll::Ready(this.polls) }
    }
}
impl Drop for PinnedFuture {
    fn drop(&mut self) { self.drops.fetch_add(1, Ordering::SeqCst); }
}
fn pinned() -> (Pin<Box<PinnedFuture>>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let address = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    (Box::pin(PinnedFuture { address: address.clone(), drops: drops.clone(), polls: 0, _pin: PhantomPinned }), address, drops)
}
#[test]
fn future_obj_pinned_heap_stays_put_and_drops_once() {
    let (future, address, drops) = pinned();
    let mut obj = FutureObj::new(future);
    let mut cx = Context::from_waker(noop_waker_ref());
    assert_eq!(Pin::new(&mut obj).poll(&mut cx), Poll::Pending);
    let mut moved = vec![obj].pop().unwrap();
    assert_eq!(Pin::new(&mut moved).poll(&mut cx), Poll::Ready(2));
    assert_ne!(address.load(Ordering::SeqCst), 0);
    drop(moved);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn local_borrowed_pinned_future_is_not_dropped_by_wrapper() {
    let (mut future, _, drops) = pinned();
    {
        let mut obj = LocalFutureObj::new(future.as_mut());
        let w = noop_waker();
        let mut cx = Context::from_waker(&w);
        assert_eq!(Pin::new(&mut obj).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut obj).poll(&mut cx), Poll::Ready(2));
    }
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(future);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn send_future_obj_can_be_polled_on_worker() {
    let (future, _, drops) = pinned();
    let obj = FutureObj::new(future);
    std::thread::spawn(move || {
        let mut obj = obj;
        let mut cx = Context::from_waker(noop_waker_ref());
        assert_eq!(Pin::new(&mut obj).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut obj).poll(&mut cx), Poll::Ready(2));
    }).join().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn noop_waker_static_reference_is_cross_thread() {
    let waker = std::thread::spawn(noop_waker_ref).join().unwrap();
    waker.wake_by_ref();
    waker.clone().wake();
    noop_waker().wake();
}
