//! Binary entrypoint for `lingxi-cli`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md`.

#![forbid(unsafe_code)]

use cli::run_cli;

fn main() {
    // `run_cli` is intentionally broad: it owns argv dispatch plus the full
    // startup pipeline, so its async state machine is much deeper than a
    // default Tokio worker stack can accommodate on macOS. Keep the process
    // entrypoint synchronous and construct that future on a dedicated,
    // explicitly sized stack. This changes no dispatch or exit-code behavior;
    // it only removes a platform-dependent stack limit from every CLI mode.
    let args = std::env::args_os().collect();
    let worker = std::thread::Builder::new()
        .name("lingxi-cli-runtime".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build CLI runtime");
            runtime.block_on(run_cli(args))
        })
        .expect("spawn CLI runtime thread");
    let code = worker.join().expect("CLI runtime thread panicked");
    std::process::exit(code);
}
