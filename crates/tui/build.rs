//! Capture host identity for the TUI's direct `/version` command path.
#[path = "../build-support/git_metadata.rs"]
mod git_metadata;

fn main() {
    git_metadata::emit();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build-support/git_metadata.rs");
}
