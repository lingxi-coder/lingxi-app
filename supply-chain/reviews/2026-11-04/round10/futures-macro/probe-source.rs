use futures_core::Stream;
use futures_util::{future::{self, FutureExt}, join, select, select_biased, stream_select, try_join};
use std::{cell::Cell, future::Future, marker::PhantomPinned, panic::{self, AssertUnwindSafe}, pin::Pin, rc::Rc, task::{Context, Poll, Waker}};

// Once one legal stream terminates, it is entitled to panic on any repoll.
// Shared state forces a terminated-then-item order regardless of shuffle.
#[derive(Default)]
struct PairState { first: Cell<Option<usize>>, returned_item: Cell<bool>, repolled: Cell<bool> }
struct Coupled { id: usize, state: Rc<PairState> }
impl Stream for Coupled {
    type Item = u8;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<u8>> {
        let me = self.get_mut();
        match me.state.first.get() {
            None => { me.state.first.set(Some(me.id)); println!("input {} -> Ready(None)", me.id); Poll::Ready(None) }
            Some(id) if id == me.id => { me.state.repolled.set(true); println!("input {} repolled after Ready(None)", me.id); panic!("legal terminated stream rejects repoll") }
            Some(_) if !me.state.returned_item.replace(true) => { println!("input {} -> Ready(Some(7))", me.id); Poll::Ready(Some(7)) }
            Some(_) => Poll::Pending,
        }
    }
}

// Each input has a correct exact size hint; the sum cannot fit in usize.
struct Count { remaining: usize }
impl Stream for Count {
    type Item = u8;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<u8>> {
        if self.remaining == 0 { Poll::Ready(None) } else { self.remaining -= 1; Poll::Ready(Some(1)) }
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.remaining, Some(self.remaining)) }
}

struct PinnedFuture { address: Rc<Cell<usize>>, polls: Rc<Cell<usize>>, _pin: PhantomPinned }
impl Future for PinnedFuture {
    type Output = u8;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
        let me = self.as_ref().get_ref();
        let here = me as *const Self as usize;
        if me.address.get() == 0 { me.address.set(here); } else { assert_eq!(me.address.get(), here, "pinned future moved"); }
        me.polls.set(me.polls.get() + 1);
        if me.polls.get() == 1 { cx.waker().wake_by_ref(); Poll::Pending } else { Poll::Ready(9) }
    }
}
impl Drop for PinnedFuture {
    fn drop(&mut self) { if self.address.get() != 0 { assert_eq!(self.address.get(), self as *const Self as usize, "pinned future moved before drop"); } }
}
struct PendingDrop(Rc<Cell<usize>>);
impl Future for PendingDrop {
    type Output = Result<u8, &'static str>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> { Poll::Pending }
}
impl Drop for PendingDrop { fn drop(&mut self) { self.0.set(self.0.get() + 1); } }

fn main() {
    panic::set_hook(Box::new(|info| println!("caught-panic: {info}")));
    let mut cx = Context::from_waker(Waker::noop());
    let address = Rc::new(Cell::new(0));
    let polls = Rc::new(Cell::new(0));
    let pinned = PinnedFuture { address: address.clone(), polls: polls.clone(), _pin: PhantomPinned };
    let mut joined = Box::pin(async { join!(pinned, future::ready(5_u8)) });
    assert!(matches!(joined.as_mut().poll(&mut cx), Poll::Pending));
    assert_eq!(joined.as_mut().poll(&mut cx), Poll::Ready((9, 5)));
    assert_eq!(polls.get(), 2);
    println!("join: !Unpin input polled twice in place; outputs (9, 5)");

    let dropped = Rc::new(Cell::new(0));
    let pending = PendingDrop(dropped.clone());
    let mut joined = Box::pin(async { try_join!(pending, future::ready(Err::<u8, _>("stop"))) });
    assert_eq!(joined.as_mut().poll(&mut cx), Poll::Ready(Err("stop")));
    drop(joined);
    assert_eq!(dropped.get(), 1);
    println!("try_join: error returned and pending input dropped once");

    let mut biased = Box::pin(async { select_biased! { a = future::ready(1).fuse() => a, b = future::ready(2).fuse() => b } });
    assert_eq!(biased.as_mut().poll(&mut cx), Poll::Ready(1));
    let mut random = Box::pin(async { select! { a = future::ready(1).fuse() => a, b = future::ready(2).fuse() => b } });
    assert!(matches!(random.as_mut().poll(&mut cx), Poll::Ready(1 | 2)));
    let mut defaulted = Box::pin(async { select_biased! { _x = future::pending::<u8>().fuse() => 1, default => 5 } });
    assert_eq!(defaulted.as_mut().poll(&mut cx), Poll::Ready(5));
    let terminated = future::Fuse::<future::Ready<u8>>::terminated();
    let mut complete = Box::pin(async { select_biased! { _x = terminated => 1, complete => 6 } });
    assert_eq!(complete.as_mut().poll(&mut cx), Poll::Ready(6));
    println!("select: biased first, random one-of-two, pending default, terminated complete");

    let state = Rc::new(PairState::default());
    let mut combined = stream_select!(Coupled { id: 0, state: state.clone() }, Coupled { id: 1, state: state.clone() });
    assert_eq!(Pin::new(&mut combined).poll_next(&mut cx), Poll::Ready(Some(7)));
    assert!(panic::catch_unwind(AssertUnwindSafe(|| Pin::new(&mut combined).poll_next(&mut cx))).is_err());
    assert!(state.repolled.get());
    println!("OBSERVED: stream_select repolled a terminated input and triggered its legal panic");

    let combined = stream_select!(Count { remaining: usize::MAX }, Count { remaining: 1 });
    match panic::catch_unwind(|| combined.size_hint()) {
        Err(_) => println!("OBSERVED: stream_select size_hint addition panics with overflow checks enabled"),
        Ok(hint) => { println!("OBSERVED: stream_select size_hint without overflow checks = {hint:?}"); assert_eq!(hint, (0, Some(0))); }
    }
    println!("expected mathematically valid bounds: ({}, None)", usize::MAX);
}
