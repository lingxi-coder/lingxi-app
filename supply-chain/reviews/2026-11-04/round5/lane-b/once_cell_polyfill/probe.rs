use once_cell_polyfill::sync::OnceLock;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc, Barrier};
use std::thread;

fn main() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<OnceLock<u64>>();

    let mut cell = OnceLock::new();
    assert!(cell.get().is_none(), "NEW_EMPTY");
    assert!(cell.set(10).is_ok(), "INITIAL_SET");
    assert_eq!(cell.get(), Some(&10), "GET_VALUE");
    assert_eq!(cell.set(20), Err(20), "REPEATED_SET");
    assert_eq!(cell.get_mut(), Some(&mut 10), "GET_MUT");
    *cell.get_mut().unwrap() = 11;
    let clone = cell.clone();
    assert_eq!(clone, cell, "CLONE_EQUAL");
    assert_eq!(cell.take(), Some(11), "TAKE_VALUE");
    assert!(cell.get().is_none(), "TAKE_CLEARS");
    assert_eq!(clone.into_inner(), Some(11), "CLONE_INDEPENDENT");
    assert_eq!(OnceLock::from(23).into_inner(), Some(23), "FROM_VALUE");
    assert_eq!(OnceLock::<u64>::default().into_inner(), None, "DEFAULT_EMPTY");

    let retry = OnceLock::new();
    assert!(catch_unwind(AssertUnwindSafe(|| {
        retry.get_or_init(|| -> u64 { panic!("EXPECTED_INITIALIZER_PANIC") });
    })).is_err(), "PANIC_PROPAGATES");
    assert_eq!(retry.get(), None, "PANIC_LEAVES_EMPTY");
    assert_eq!(*retry.get_or_init(|| 31), 31, "RETRY_AFTER_PANIC");

    let value = Arc::new(OnceLock::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(Barrier::new(16));
    let workers: Vec<_> = (0..16).map(|_| {
        let (value, calls, ready) = (value.clone(), calls.clone(), ready.clone());
        thread::spawn(move || {
            ready.wait();
            *value.get_or_init(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                42
            })
        })
    }).collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), 42, "CONCURRENT_VALUE");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "CONCURRENT_ONCE");
    println!("all OnceLock wrapper probes passed");
}
