//! Independent opaque-debug privacy and macro-scope probe.
#![allow(dead_code)]
extern crate opaque_debug as renamed;

struct Guard;
impl core::fmt::Debug for Guard {
    fn fmt(&self, _: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        panic!("the field formatter must not be called")
    }
}

struct Secret { value: Guard }
renamed::implement!(Secret);
struct Wrapper<T> { value: T }
renamed::implement!(Wrapper<T>);
struct ConstMarker<const N: u64>;

mod custom_macro {
    macro_rules! write {
        ($formatter:expr, $format:expr $(, $args:expr)*) => {{
            ::core::fmt::Write::write_str($formatter, "shadowed output")
        }};
    }
    pub struct Secret;
    renamed::implement!(Secret);
}

fn main() {
    let value = Secret { value: Guard };
    assert_eq!(format!("{value:?}"), "Secret { ... }");
    assert_eq!(format!("{value:#?}"), "Secret { ... }");
    let wrapped = Wrapper { value: Guard };
    let generic = format!("{wrapped:?}");
    assert!(generic.starts_with("Wrapper<") && generic.ends_with("> { ... }"));
    let marker = Wrapper { value: ConstMarker::<31337> };
    let shown_type = format!("{marker:?}");
    assert!(shown_type.contains("ConstMarker<31337>"));
    assert_eq!(format!("{:?}", custom_macro::Secret), "shadowed output");
    println!("field formatter not called; ordinary and alternate output opaque");
    println!("generic type output: {shown_type}");
    println!("caller's write! macro changes generated output");
}
