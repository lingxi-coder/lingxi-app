//! build.rs — capture the short git SHA at compile time for `/version`.
//!
//! Falls back to "unknown" when:
//! - the workspace isn't inside a git repo (e.g. tarball install)
//! - the `git` command isn't on `$PATH`
//! - any other failure
//!
//! The captured value is read by `lingxi-commands/src/builtin/version.rs`
//! via `option_env!("LINGXI_GIT_SHA_SHORT")`.

#[path = "../../build-support/git_metadata.rs"]
mod git_metadata;

fn main() {
    git_metadata::emit();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../build-support/git_metadata.rs");
}
