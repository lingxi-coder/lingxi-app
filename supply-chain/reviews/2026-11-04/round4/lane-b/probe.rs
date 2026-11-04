use std::fmt::{self, Write};

struct PanicOnDebug;
impl fmt::Debug for PanicOnDebug {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("a field's Debug implementation must not be called")
    }
}

#[allow(dead_code)]
struct Plain {
    secret: PanicOnDebug,
}
opaque_debug::implement!(Plain);

#[allow(dead_code)]
struct Generic<T> {
    secret: T,
}
opaque_debug::implement!(Generic<T>);

struct Marker<const N: u32>;

struct Reject;
impl fmt::Write for Reject {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

mod shadowed {
    // This caller-defined macro deliberately shadows std's write! at the
    // expansion site. It shows a boundary of the exported macro's hygiene.
    macro_rules! write {
        ($dst:expr, $fmt:expr $(, $args:expr)*) => {{
            ::core::fmt::Write::write_str($dst, "caller write! override")
        }};
    }

    pub struct Example;
    opaque_debug::implement!(Example);
}

fn main() {
    let plain = Plain { secret: PanicOnDebug };
    let generic = Generic { secret: PanicOnDebug };
    assert_eq!(format!("{plain:?}"), "Plain { ... }");
    let formatted = format!("{generic:?}");
    assert!(formatted.contains("PanicOnDebug"));
    assert!(formatted.ends_with("> { ... }"));
    let const_type = format!("{:?}", Generic { secret: Marker::<42> });
    assert!(const_type.contains("Marker<42>"));
    assert!(write!(&mut Reject, "{plain:?}").is_err());
    assert!(write!(&mut Reject, "{generic:?}").is_err());
    assert_eq!(format!("{:?}", shadowed::Example), "caller write! override");
    println!("plain={plain:?}");
    println!("generic={formatted}");
    println!("const_type={const_type}");
    println!("failing writer propagated; caller write! shadowing reproduced");
}
