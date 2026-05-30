//! Binary entrypoint for `lingxi-cli`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md`.

#![forbid(unsafe_code)]

use lingxi_cli::run_cli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let code = run_cli(std::env::args_os().collect()).await;
    std::process::exit(code);
}
