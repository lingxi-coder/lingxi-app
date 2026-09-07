//! build.rs — capture the short git SHA at compile time for the `/version`
//! slash-command handler in `src/builtin/version.rs`.
//!
//! Mirrors `lingxi-cli/build.rs`. The slash-command crate has its own
//! build.rs because `option_env!("LINGXI_GIT_SHA_SHORT")` reads the env
//! at the *consumer's* compile time (here, the commands crate), not the
//! binary's; without a local build.rs the env var would always be unset
//! when the version handler was compiled and the SHA would render as
//! `"unknown"`.
//!
//! Falls back to "unknown" when:
//! - the workspace isn't inside a git repo (e.g. tarball install)
//! - the `git` command isn't on `$PATH`
//! - any other failure

#[path = "../../build-support/git_metadata.rs"]
mod git_metadata;

fn main() {
    git_metadata::emit();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../build-support/git_metadata.rs");
}
