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
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    println!("cargo:rerun-if-changed=build.rs");
}
