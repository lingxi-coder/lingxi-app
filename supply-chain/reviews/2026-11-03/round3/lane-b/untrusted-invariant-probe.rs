use std::panic::{catch_unwind, AssertUnwindSafe};
use untrusted::{Input, Reader};

#[test]
fn replacing_the_public_reader_can_invalidate_read_partial_range() {
    let original = Input::from(b"abc");
    let mut reader = Reader::new(original);
    reader.skip(2).unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| {
        reader.read_partial(|cursor| {
            *cursor = Reader::new(original);
            Ok::<(), ()>(())
        })
    }));
    assert!(result.is_err(), "expected invalid internal range to panic");
}

#[test]
fn replacing_the_public_reader_can_defeat_the_outer_full_input_check() {
    let original = Input::from(b"unconsumed data");
    let result = original.read_all((), |cursor| {
        *cursor = Reader::new(Input::from(b""));
        Ok::<(), ()>(())
    });
    assert_eq!(result, Ok(()));
}
