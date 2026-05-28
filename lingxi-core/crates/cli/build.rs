//! build.rs — Task 11 fills this in: capture short git SHA at compile time
//! via `cargo:rustc-env=LINGXI_GIT_SHA_SHORT=<sha>`. For now the script only
//! emits the rerun-if-changed marker so the crate builds.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
