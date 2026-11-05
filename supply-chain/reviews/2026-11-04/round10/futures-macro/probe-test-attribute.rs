extern crate self as futures_test;
pub mod __private {
    use std::{future::Future, pin::pin, task::{Context, Poll, Waker}};
    pub fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = pin!(f);
        match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("probe future unexpectedly pending"),
        }
    }
}
#[futures_macro::test_internal]
async fn executes_async_body() -> Result<(), &'static str> { assert_eq!(2 + 2, 4); Ok(()) }
