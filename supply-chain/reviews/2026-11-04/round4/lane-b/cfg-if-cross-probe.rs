#![allow(unexpected_cfgs, unused_macros)]
extern crate cfg_if as renamed;

// The crate must use $crate recursion rather than this caller-local macro.
macro_rules! cfg_if { ($($anything:tt)*) => { compile_error!("caller shadow invoked"); } }

// A false branch contains multiple tokens: neither may escape its cfg gate.
renamed::cfg_if! {
    if #[cfg(any())] {
        type SelectedType = u8;
        compile_error!("false branch escaped its gate");
    } else {
        type SelectedType = u32;
    }
}

// An empty matched branch still suppresses every later branch.
renamed::cfg_if! {
    if #[cfg(flag_a)] {
    } else if #[cfg(flag_b)] {
        const AFTER_EMPTY: u8 = 7;
    } else {
        const AFTER_EMPTY: u8 = 9;
    }
}
#[cfg(flag_a)]
const AFTER_EMPTY: u8 = 5;

// Overlapping clauses have the first-match behavior, not last-match behavior.
renamed::cfg_if! {
    if #[cfg(flag_a)] { fn choose() -> u8 { 1 } }
    else if #[cfg(flag_a)] { fn choose() -> u8 { 2 } }
    else { fn choose() -> u8 { 3 } }
}

fn main() {
    let a = cfg!(flag_a);
    let b = cfg!(flag_b);
    assert_eq!(std::mem::size_of::<SelectedType>(), 4);
    assert_eq!(AFTER_EMPTY, if a { 5 } else if b { 7 } else { 9 });
    assert_eq!(choose(), if a { 1 } else { 3 });
    println!("a={a}, b={b}: first match, empty branch, gating, hygiene OK");
}
