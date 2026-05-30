//! build.rs — capture the short git SHA at compile time for `/version`.
//!
//! Falls back to "unknown" when:
//! - the workspace isn't inside a git repo (e.g. tarball install)
//! - the `git` command isn't on `$PATH`
//! - any other failure
//!
//! The captured value is read by `lingxi-commands/src/builtin/version.rs`
//! via `option_env!("LINGXI_GIT_SHA_SHORT")`.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=LINGXI_GIT_SHA_SHORT={sha}");
    // Re-run if HEAD moves.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    println!("cargo:rerun-if-changed=build.rs");
}
