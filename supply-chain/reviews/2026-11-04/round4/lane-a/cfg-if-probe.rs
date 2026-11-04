//! Representative external macro-expansion checks for cfg-if 1.0.4.
#![allow(unexpected_cfgs)]
extern crate cfg_if as renamed_cfg;

renamed_cfg::cfg_if! {
    if #[cfg(all(flag_a, not(flag_c)))] {
        const SELECTED: u8 = 0;
        fn second_item() -> u8 { 10 }
    } else if #[cfg(any(flag_b, all(flag_b, flag_c)))] {
        const SELECTED: u8 = 1;
        fn second_item() -> u8 { 11 }
    } else if #[cfg(flag_c)] {
        const SELECTED: u8 = 2;
        fn second_item() -> u8 { 12 }
    } else {
        const SELECTED: u8 = 3;
        fn second_item() -> u8 { 13 }
    }
}

renamed_cfg::cfg_if! {
    if #[cfg(flag_a)] {
        renamed_cfg::cfg_if! {
            if #[cfg(flag_b)] { const NESTED: u8 = 1; }
            else { const NESTED: u8 = 2; }
        }
    } else {
        const NESTED: u8 = 3;
    }
}

// An absent branch must not resolve its intentionally nonexistent type.
renamed_cfg::cfg_if! {
    if #[cfg(any())] {
        fn gated(_: TypeThatMustNotResolve) -> bool { false }
    } else {
        fn gated() -> bool { true }
    }
}

renamed_cfg::cfg_if! {
    if #[cfg(flag_a)] { const OPTIONAL: u8 = 42; }
}

trait Value { fn value(&self) -> u8; }
struct Item;
impl Value for Item {
    renamed_cfg::cfg_if! {
        if #[cfg(flag_c)] { fn value(&self) -> u8 { 1 } }
        else { fn value(&self) -> u8 { 0 } }
    }
}

fn statements() -> u8 {
    let mut n = 0;
    renamed_cfg::cfg_if! {
        if #[cfg(flag_b)] { n += 1; n += 2; }
        else { n += 4; n += 8; }
    }
    n
}

fn main() {
    let a = cfg!(flag_a);
    let b = cfg!(flag_b);
    let c = cfg!(flag_c);
    let expected = if a && !c { 0 } else if b { 1 } else if c { 2 } else { 3 };
    assert_eq!(SELECTED, expected);
    assert_eq!(second_item(), 10 + expected);
    assert_eq!(NESTED, if a { if b { 1 } else { 2 } } else { 3 });
    assert!(gated());
    #[cfg(flag_a)]
    assert_eq!(OPTIONAL, 42);
    assert_eq!(Item.value(), u8::from(c));
    assert_eq!(statements(), if b { 3 } else { 12 });
    println!("a={a} b={b} c={c}: selected={SELECTED} OK");
}
