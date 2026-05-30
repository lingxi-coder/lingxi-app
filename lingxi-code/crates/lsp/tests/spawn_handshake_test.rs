//! Integration test: spawn the mock LSP binary through `PosixLspTransport`
//! and drive a full initialize -> shutdown cycle.
//!
//! This is the end-to-end exercise of the ENOENT guard + connection setup +
//! request round-trip.

use lingxi_test_harness::mocks::mock_lsp_server::mock_lsp_server_path;
use lingxi_traits::LspServerConfig;
use std::collections::HashMap;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn mock_config() -> LspServerConfig {
    let bin = mock_lsp_server_path();
    let mut ext = HashMap::new();
    ext.insert(".rs".into(), "rust".into());
    LspServerConfig {
        name: "mock".into(),
        command: bin.to_string_lossy().into_owned(),
        args: vec![],
        env: HashMap::new(),
        trigger_languages: vec!["rust".into()],
        root_dir_markers: vec![],
        initialization_options: None,
        extension_to_language: ext,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn posix_spawn_and_initialize_round_trip() {
    use lingxi_platform_posix::lsp::PosixLspTransport;
    use lingxi_traits::LspTransport;

    // The mock binary must be present. Cargo does not cross-build artifacts
    // for integration tests of another package automatically — the test
    // runner is expected to invoke `cargo build -p lingxi-test-harness
    // --bin mock_lsp_server` first. We skip gracefully if missing.
    let candidate = mock_lsp_server_path();
    if !candidate.exists() {
        eprintln!(
            "skipping spawn_handshake_test: mock_lsp_server not built at {}",
            candidate.display()
        );
        return;
    }

    let transport = PosixLspTransport::new();
    let conn = transport
        .start_server(&mock_config())
        .await
        .expect("spawn succeeded");

    let caps = transport
        .initialize(&conn, "file:///tmp")
        .await
        .expect("initialize ok");
    assert!(caps.hover, "mock advertises hover");
    assert!(caps.definition, "mock advertises definition");

    transport
        .shutdown(conn.connection_id)
        .await
        .expect("shutdown ok");
}

/// Verifies the ENOENT-guard: spawning a non-existent binary surfaces a
/// clear `LspError::Transport` instead of panicking. This works on every
/// platform where we have `PosixLspTransport`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_spawn_enoent_is_clean_error() {
    use lingxi_platform_posix::lsp::PosixLspTransport;
    use lingxi_traits::LspTransport;
    let mut config = mock_config();
    config.command = "/nonexistent/path/to/lsp-server".into();
    let transport = PosixLspTransport::new();
    let err = transport
        .start_server(&config)
        .await
        .expect_err("must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("spawn")
            || msg.contains("not found")
            || msg.contains("No such file")
            || msg.contains("os error"),
        "ENOENT surfaces as Transport error, got: {msg}"
    );
}
