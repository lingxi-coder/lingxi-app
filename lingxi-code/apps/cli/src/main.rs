//! Binary entrypoint for `lingxi-cli`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md`.

#![forbid(unsafe_code)]

use cli::run_cli;

fn main() {
    #[cfg(any(unix, windows))]
    if engine_desktop::shell_supervisor::is_supervisor_invocation() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("shell supervisor runtime");
        let result = runtime.block_on(engine_desktop::shell_supervisor::run_supervisor(
            engine_desktop::supervisor_exit_sink,
        ));
        if let Err(error) = result {
            eprintln!("shell supervisor: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(any(unix, windows))]
    if let Ok(executable) = std::env::current_exe() {
        engine_desktop::shell_supervisor::enable_supervisor(executable);
    }
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
