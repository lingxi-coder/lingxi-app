extern crate version_check;
use version_check::{Channel,Date,Version};
fn main() {
    assert!(Version::parse("1.2.3.4").is_none());
    assert!(Date::parse("1-2-3-4-5").is_none());
    println!("out-of-calendar date accepted: {}", Date::parse("2024-02-31").is_some());
    println!("unrecognized channel string classified stable: {}", Channel::parse("notaversion").map(|channel| channel.is_stable()).unwrap_or(false));
    let compiler=std::env::args().nth(1).expect("fake compiler path");
    std::env::set_var("RUSTC", compiler);
    println!("nonzero fake compiler still reported version: {:?}",Version::read().map(|v| v.to_mmp()));
    assert_eq!(Version::read().map(|v| v.to_mmp()), Some((1,99,7)));
}
