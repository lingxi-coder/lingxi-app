use futures_sink::Sink;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
fn waker() -> Waker { Waker::from(Arc::new(Noop)) }

#[derive(Default)]
struct State { mode: u8, calls: Vec<String> }
fn poll(state: &Rc<RefCell<State>>, method: &str) -> Poll<Result<(), &'static str>> {
    let mut state = state.borrow_mut();
    state.calls.push(method.to_owned());
    match state.mode { 1 => Poll::Pending, 2 => Poll::Ready(Err("failure")), _ => Poll::Ready(Ok(())) }
}
fn send(state: &Rc<RefCell<State>>, item: u32) -> Result<(), &'static str> {
    let mut state = state.borrow_mut();
    state.calls.push(format!("send:{item}"));
    if state.mode == 2 { Err("failure") } else { Ok(()) }
}

struct TestSink { state: Rc<RefCell<State>> }
impl Sink<u32> for TestSink {
    type Error = &'static str;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> { poll(&self.state, "ready") }
    fn start_send(self: Pin<&mut Self>, item: u32) -> Result<(), Self::Error> { send(&self.state, item) }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> { poll(&self.state, "flush") }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> { poll(&self.state, "close") }
}

struct PinnedSink { state: Rc<RefCell<State>>, _pin: PhantomPinned }
impl Sink<u32> for PinnedSink {
    type Error = &'static str;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> { poll(&self.state, "ready") }
    fn start_send(self: Pin<&mut Self>, item: u32) -> Result<(), Self::Error> { send(&self.state, item) }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> { poll(&self.state, "flush") }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> { poll(&self.state, "close") }
}

#[test]
fn reference_and_box_forward_all_operations() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let state = Rc::new(RefCell::new(State::default()));
    let mut sink = TestSink { state: state.clone() };
    let mut reference = &mut sink;
    assert_eq!(Pin::new(&mut reference).poll_ready(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(Pin::new(&mut reference).start_send(3), Ok(()));
    assert_eq!(Pin::new(&mut reference).poll_flush(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(Pin::new(&mut reference).poll_close(&mut cx), Poll::Ready(Ok(())));
    let mut boxed = Box::new(TestSink { state: state.clone() });
    assert_eq!(Pin::new(&mut boxed).poll_ready(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(Pin::new(&mut boxed).start_send(8), Ok(()));
    assert_eq!(Pin::new(&mut boxed).poll_flush(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(Pin::new(&mut boxed).poll_close(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(state.borrow().calls, ["ready", "send:3", "flush", "close", "ready", "send:8", "flush", "close"]);
}

#[test]
fn pinned_pointer_preserves_nonunpin_target_and_results() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let state = Rc::new(RefCell::new(State::default()));
    let mut sink = Box::pin(PinnedSink { state: state.clone(), _pin: PhantomPinned });
    let address = sink.as_ref().get_ref() as *const PinnedSink;
    state.borrow_mut().mode = 1;
    assert_eq!(Pin::new(&mut sink).poll_ready(&mut cx), Poll::Pending);
    assert_eq!(Pin::new(&mut sink).poll_close(&mut cx), Poll::Pending);
    state.borrow_mut().mode = 2;
    assert_eq!(Pin::new(&mut sink).poll_ready(&mut cx), Poll::Ready(Err("failure")));
    assert_eq!(Pin::new(&mut sink).poll_flush(&mut cx), Poll::Ready(Err("failure")));
    state.borrow_mut().mode = 0;
    assert_eq!(Pin::new(&mut sink).poll_ready(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(Pin::new(&mut sink).start_send(73), Ok(()));
    assert_eq!(Pin::new(&mut sink).poll_flush(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(Pin::new(&mut sink).poll_close(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(address, sink.as_ref().get_ref() as *const PinnedSink);
    assert_eq!(state.borrow().calls, ["ready", "close", "ready", "flush", "ready", "send:73", "flush", "close"]);
}

#[test]
fn in_memory_sinks_preserve_order_and_close_does_not_seal() {
    let w = waker(); let mut cx = Context::from_waker(&w);
    let mut v = Vec::new();
    assert!(matches!(Pin::new(&mut v).poll_ready(&mut cx), Poll::Ready(Ok(()))));
    assert!(Pin::new(&mut v).start_send(1).is_ok());
    assert!(matches!(Pin::new(&mut v).poll_ready(&mut cx), Poll::Ready(Ok(()))));
    assert!(Pin::new(&mut v).start_send(2).is_ok());
    assert!(matches!(Pin::new(&mut v).poll_flush(&mut cx), Poll::Ready(Ok(()))));
    assert!(matches!(Pin::new(&mut v).poll_close(&mut cx), Poll::Ready(Ok(()))));
    assert!(matches!(Pin::new(&mut v).poll_ready(&mut cx), Poll::Ready(Ok(()))));
    assert!(Pin::new(&mut v).start_send(3).is_ok());
    assert_eq!(v, [1, 2, 3]);
    let mut d = VecDeque::new();
    assert!(matches!(Pin::new(&mut d).poll_ready(&mut cx), Poll::Ready(Ok(()))));
    assert!(Pin::new(&mut d).start_send(1).is_ok());
    assert!(matches!(Pin::new(&mut d).poll_ready(&mut cx), Poll::Ready(Ok(()))));
    assert!(Pin::new(&mut d).start_send(2).is_ok());
    assert!(matches!(Pin::new(&mut d).poll_flush(&mut cx), Poll::Ready(Ok(()))));
    assert!(matches!(Pin::new(&mut d).poll_close(&mut cx), Poll::Ready(Ok(()))));
    assert!(matches!(Pin::new(&mut d).poll_ready(&mut cx), Poll::Ready(Ok(()))));
    assert!(Pin::new(&mut d).start_send(3).is_ok());
    assert_eq!(d.into_iter().collect::<Vec<_>>(), [1, 2, 3]);
}

struct Immovable { value: usize, _pin: PhantomPinned }
#[test]
fn pinned_collections_with_nonunpin_elements_grow() {
    let mut v = Box::pin(Vec::<Immovable>::new());
    let v_address = v.as_ref().get_ref() as *const Vec<Immovable>;
    let initial_capacity = v.as_ref().get_ref().capacity();
    let w = waker(); let mut cx = Context::from_waker(&w);
    for value in 0..64 {
        assert!(matches!(v.as_mut().poll_ready(&mut cx), Poll::Ready(Ok(()))));
        assert!(v.as_mut().start_send(Immovable { value, _pin: PhantomPinned }).is_ok());
    }
    assert_eq!(v_address, v.as_ref().get_ref() as *const Vec<Immovable>);
    assert!(v.as_ref().get_ref().capacity() > initial_capacity);
    assert_eq!(v.as_ref().get_ref().iter().map(|x| x.value).collect::<Vec<_>>(), (0..64).collect::<Vec<_>>());

    let mut d = Box::pin(VecDeque::<Immovable>::new());
    let d_address = d.as_ref().get_ref() as *const VecDeque<Immovable>;
    let initial_capacity = d.as_ref().get_ref().capacity();
    for value in 0..64 {
        assert!(matches!(d.as_mut().poll_ready(&mut cx), Poll::Ready(Ok(()))));
        assert!(d.as_mut().start_send(Immovable { value, _pin: PhantomPinned }).is_ok());
    }
    assert_eq!(d_address, d.as_ref().get_ref() as *const VecDeque<Immovable>);
    assert!(d.as_ref().get_ref().capacity() > initial_capacity);
    assert_eq!(d.as_ref().get_ref().iter().map(|x| x.value).collect::<Vec<_>>(), (0..64).collect::<Vec<_>>());
}
