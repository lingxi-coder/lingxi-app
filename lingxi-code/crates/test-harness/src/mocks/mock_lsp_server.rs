//! Public path to the `mock_lsp_server` binary so tests can spawn it.
//!
//! At build time cargo emits the binary at
//! `$CARGO_TARGET_DIR/debug/mock_lsp_server` (or the equivalent for the
//! current profile/triple). We expose [`mock_lsp_server_path`] which
//! returns that path, panicking if the binary has not been built (which
//! `cargo test -p lingxi-lsp --test spawn_handshake_test` ensures via the
//! `mock_lsp_server` `[[bin]]` target on `lingxi-test-harness`).

use std::path::PathBuf;

/// Return the path to the compiled `mock_lsp_server` binary.
///
/// # Panics
/// Panics when the binary was not built. To ensure it's built, the test
/// harness invokes `cargo build -p lingxi-test-harness --bin mock_lsp_server`
/// before spawning.
#[must_use]
pub fn mock_lsp_server_path() -> PathBuf {
    // env!("CARGO_BIN_EXE_mock_lsp_server") is set by cargo for integration
    // tests within the same package, but we live in a different package
    // here — so fall back to walking from CARGO_MANIFEST_DIR.
    let target_dir = if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        PathBuf::from(dir)
    } else {
        // <workspace>/target by default; we are at
        // <workspace>/crates/test-harness, so walk up two levels
        // and append target.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .ancestors()
            .nth(2)
            .map(|p| p.join("target"))
            .expect("workspace target dir")
    };
    let exe_name = if cfg!(windows) {
        "mock_lsp_server.exe"
    } else {
        "mock_lsp_server"
    };
    let candidate_debug = target_dir.join("debug").join(exe_name);
    if candidate_debug.exists() {
        return candidate_debug;
    }
    let candidate_release = target_dir.join("release").join(exe_name);
    if candidate_release.exists() {
        return candidate_release;
    }
    panic!(
        "mock_lsp_server binary not found at {} or {}. Run `cargo build -p lingxi-test-harness --bin mock_lsp_server` first.",
        candidate_debug.display(),
        candidate_release.display()
    );
}
