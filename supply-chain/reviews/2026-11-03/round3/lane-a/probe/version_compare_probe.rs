use version_compare::{compare, Cmp, Manifest, Version};
fn main() {
    let pairs = [
        ("1.0.0-1", "1.0.0-alpha"),
        ("1.2.1", "1.alpha.1"),
        ("1.alpha.1", "1.3.1"),
        ("1.2.1", "1.3.1"),
        ("1.0.0+build.1", "1.0.0"),
    ];
    for (a, b) in pairs {
        println!("compare({a:?}, {b:?}) = {:?}", compare(a, b));
    }
    assert_eq!(compare("1.0.0-1", "1.0.0-alpha"), Ok(Cmp::Eq));
    assert_eq!(compare("1.2.1", "1.alpha.1"), Ok(Cmp::Eq));
    assert_eq!(compare("1.alpha.1", "1.3.1"), Ok(Cmp::Eq));
    assert_eq!(compare("1.2.1", "1.3.1"), Ok(Cmp::Lt));
    println!("upper-bound witness: compare(\"2\", \"1\") = {:?}; equals Ge = {}", compare("2", "1"), compare("2", "1") == Ok(Cmp::Ge));
    assert_eq!(compare("2", "1"), Ok(Cmp::Gt));
    assert!(version_compare::compare_to("2", "1", Cmp::Ge).unwrap());
    let mut gnu=Manifest::default(); gnu.gnu_ordering=true;
    let lhs=Version::from("1.2").unwrap();
    let rhs=Version::from_manifest("1.02", &gnu).unwrap();
    println!("mixed-manifest: lhs == rhs: {}; rhs == lhs: {}", lhs==rhs, rhs==lhs);
    assert!(lhs == rhs && rhs != lhs);
}
