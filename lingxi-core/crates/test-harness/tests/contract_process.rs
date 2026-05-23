//! Drive the platform [`ProcessRunner`] impls through the canonical contract.

use lingxi_test_harness::contracts::process::process_runner_contract_tests;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn posix_process_passes_contract() {
    let proc = lingxi_platform_posix::PosixProcess::new();
    let sandbox = lingxi_platform_posix::PosixSandbox::new();
    process_runner_contract_tests(&proc, &sandbox).await;
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_process_passes_contract() {
    // Windows has no `/bin/sh`. The suite short-circuits on platforms where
    // /bin/sh is absent and falls back to the `is_available()` smoke test —
    // that's the agreed-upon contract today (a Windows-specific subcontract
    // using `cmd.exe` lands in M2.next).
    let proc = lingxi_platform_windows::WindowsProcess::new();
    let sandbox = lingxi_platform_windows::WindowsSandbox::new();
    process_runner_contract_tests(&proc, &sandbox).await;
}
