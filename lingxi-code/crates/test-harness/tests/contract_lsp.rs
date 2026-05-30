//! Drive the platform [`LspTransport`] impls through the canonical contract.

use lingxi_test_harness::contracts::lsp::lsp_transport_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_lsp_passes_contract() {
    let t = lingxi_platform_posix::PosixLspTransport::new();
    lsp_transport_contract_tests(&t).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_lsp_passes_contract() {
    let t = lingxi_platform_windows::WindowsLspTransport::new();
    lsp_transport_contract_tests(&t).await;
}
