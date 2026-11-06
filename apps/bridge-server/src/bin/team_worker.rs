//! Private major-3 Shared Bot worker. No shell guardian or public RPC role.

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(harness_runtime::desktop::team_process::worker_main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    std::process::exit(78);
}
