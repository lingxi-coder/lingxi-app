use stable_deref_trait::{CloneStableDeref, StableDeref};
use std::{borrow::Cow, cell::RefCell, ffi::{CString, OsString}, path::PathBuf, sync::{Arc, Mutex, RwLock}};

fn check_move<T: StableDeref>(value: T) {
    let before = std::ops::Deref::deref(&value) as *const T::Target;
    let moved = std::hint::black_box(value);
    let after = std::ops::Deref::deref(&moved) as *const T::Target;
    assert!(std::ptr::eq(before, after));
}

fn check_clone<T: CloneStableDeref>(value: T) {
    let other = value.clone();
    let left = std::ops::Deref::deref(&value) as *const T::Target;
    let right = std::ops::Deref::deref(&other) as *const T::Target;
    assert!(std::ptr::eq(left, right));
    check_move(other);
}

#[test]
fn owned_borrowed_and_shared_targets_survive_moves_and_clones() {
    check_move(Box::new(10u32));
    check_move(vec![1u8, 2, 3]);
    check_move(String::from("hello"));
    check_move(CString::new("hello").unwrap());
    check_move(OsString::from("hello"));
    check_move(PathBuf::from("/tmp/example"));
    check_move(Cow::<'_, str>::Borrowed("hello"));
    check_move(Cow::<'_, str>::Owned(String::from("hello")));
    check_move(Cow::<'_, [u8]>::Owned(vec![1,2,3]));
    check_clone(std::rc::Rc::new(10u32));
    check_clone(Arc::new(10u32));
    let value = 7;
    check_clone(&value);
    let mut value = 8;
    check_move(&mut value);
}

#[test]
fn borrow_and_lock_guards_keep_targets_stable_when_moved() {
    let cell = RefCell::new(1);
    check_move(cell.borrow());
    check_move(cell.borrow_mut());
    let mutex = Mutex::new(1);
    check_move(mutex.lock().unwrap());
    let rwlock = RwLock::new(1);
    check_move(rwlock.read().unwrap());
    check_move(rwlock.write().unwrap());
}
