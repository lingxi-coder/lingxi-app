//! Binary entrypoint for `lingxi-cli`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md`.

#![forbid(unsafe_code)]

use cli::run_cli;
use std::ffi::OsString;
use std::time::Duration;

fn is_mcp_serve(args: &[OsString]) -> bool {
    matches!(
        cli::argv::Argv::from_iter(args.iter().cloned())
            .ok()
            .and_then(|args| args.command),
        Some(cli::commands::Commands::Mcp(cli::commands::mcp::Cli {
            command: Some(cli::commands::mcp::Sub::Serve(_)),
        }))
    )
}

// Tokio's shutdown_timeout returns no completion status. Drop on a separate
// thread instead, so reaching the deadline cannot be mistaken for verified
// task shutdown. The process will shortly exit if teardown remains unknown;
// this does not prove external processes or other side effects were cleaned up.
fn shutdown_mcp_runtime(runtime: tokio::runtime::Runtime, timeout: Duration) -> Result<(), String> {
    let (runtime_tx, runtime_rx) = std::sync::mpsc::sync_channel::<tokio::runtime::Runtime>(1);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    if let Err(error) = std::thread::Builder::new()
        .name("lingxi-mcp-runtime-shutdown".into())
        .spawn(move || {
            if let Ok(runtime) = runtime_rx.recv() {
                drop(runtime);
                let _ = done_tx.send(());
            }
        })
    {
        // Dropping the runtime here could block forever. This entrypoint exits
        // immediately after reporting failure, and must not claim cleanup.
        std::mem::forget(runtime);
        return Err(format!("failed to start runtime shutdown: {error}"));
    }
    if let Err(error) = runtime_tx.send(runtime) {
        std::mem::forget(error.0);
        return Err("runtime shutdown thread stopped before receiving the runtime".into());
    }
    match done_rx.recv_timeout(timeout) {
        Ok(()) => Ok(()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err("runtime tasks did not stop before the deadline".into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("runtime shutdown thread stopped without a completion receipt".into())
        }
    }
}

fn run_mcp_runtime<F: std::future::Future<Output = i32>>(
    runtime: tokio::runtime::Runtime,
    future: F,
    timeout: Duration,
) -> i32 {
    run_mcp_runtime_with_scope(
        runtime,
        future,
        timeout,
        || {
            #[cfg(unix)]
            {
                cli::begin_mcp_process_io()
            }
            #[cfg(not(unix))]
            {
                Ok(())
            }
        },
        || {
            #[cfg(unix)]
            {
                cli::stop_and_restore_mcp_process_io(Duration::from_secs(2))
            }
            #[cfg(not(unix))]
            {
                Ok(())
            }
        },
    )
}

fn run_mcp_runtime_with_scope<F, B, C>(
    runtime: tokio::runtime::Runtime,
    future: F,
    timeout: Duration,
    begin: B,
    cleanup: C,
) -> i32
where
    F: std::future::Future<Output = i32>,
    B: FnOnce() -> std::io::Result<()>,
    C: FnOnce() -> std::io::Result<()>,
{
    let begin_error = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(begin)) {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some("MCP stdio scope initialization panicked".into()),
    };
    if let Some(error) = begin_error {
        // A rejected scope may belong to another caller. Do not stop it.
        // Transfer this runtime before diagnostics, which may have a closed
        // stderr; the MCP future has not been polled.
        let shutdown = shutdown_mcp_runtime(runtime, timeout);
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "Failed to begin MCP stdio scope: {error}"
        );
        if let Err(error) = shutdown {
            let _ = writeln!(
                std::io::stderr(),
                "MCP runtime teardown is unverified: {error}"
            );
        }
        return cli::exit_codes::RUNTIME_ERROR;
    }
    run_mcp_runtime_with_cleanup(runtime, future, timeout, cleanup)
}

fn run_mcp_runtime_with_cleanup<F, C>(
    runtime: tokio::runtime::Runtime,
    future: F,
    timeout: Duration,
    cleanup: C,
) -> i32
where
    F: std::future::Future<Output = i32>,
    C: FnOnce() -> std::io::Result<()>,
{
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(future)));
    // Transfer the runtime before writing diagnostics. A closed stderr must
    // not cause another panic while an unbounded runtime drop is still local.
    let shutdown = shutdown_mcp_runtime(runtime, timeout);
    let stdio_cleanup = cleanup();
    use std::io::Write as _;
    let code = match result {
        Ok(code) => code,
        Err(_) => {
            let _ = writeln!(std::io::stderr(), "MCP server panicked");
            cli::exit_codes::RUNTIME_ERROR
        }
    };
    let mut code = code;
    if let Err(error) = shutdown {
        let _ = writeln!(
            std::io::stderr(),
            "MCP runtime teardown is unverified: {error}"
        );
        code = if code == 0 {
            cli::exit_codes::RUNTIME_ERROR
        } else {
            code
        };
    }
    if let Err(error) = stdio_cleanup {
        let _ = writeln!(
            std::io::stderr(),
            "MCP stdio flag restoration is unverified: {error}"
        );
        if code == 0 {
            code = cli::exit_codes::RUNTIME_ERROR;
        }
    }
    code
}

fn main() {
    #[cfg(any(unix, windows))]
    if harness_runtime::desktop::shell_supervisor::is_supervisor_invocation() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("shell supervisor runtime");
        let result = runtime.block_on(harness_runtime::desktop::shell_supervisor::run_supervisor(
            harness_runtime::desktop::supervisor_exit_sink,
        ));
        if let Err(error) = result {
            eprintln!("shell supervisor: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(any(unix, windows))]
    if let Ok(executable) = std::env::current_exe() {
        harness_runtime::desktop::shell_supervisor::enable_supervisor(executable);
    }
    // `run_cli` is intentionally broad: it owns argv dispatch plus the full
    // startup pipeline, so its async state machine is much deeper than a
    // default Tokio worker stack can accommodate on macOS. Keep the process
    // entrypoint synchronous and construct that future on a dedicated,
    // explicitly sized stack, removing a platform-dependent stack limit from
    // every CLI mode. MCP additionally reports unverified runtime teardown.
    let args: Vec<_> = std::env::args_os().collect();
    let worker = std::thread::Builder::new()
        .name("lingxi-cli-runtime".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let bounded_shutdown = is_mcp_serve(&args);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(harness_runtime::RUNTIME_THREAD_STACK_SIZE)
                .build()
                .expect("build CLI runtime");
            if bounded_shutdown {
                run_mcp_runtime(runtime, run_cli(args), Duration::from_secs(5))
            } else {
                runtime.block_on(run_cli(args))
            }
        })
        .expect("spawn CLI runtime thread");
    let code = worker.join().expect("CLI runtime thread panicked");
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_panicking_blocked_runtime() -> i32 {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        run_mcp_runtime(
            runtime,
            async {
                let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
                drop(tokio::task::spawn_blocking(move || {
                    let _ = entered_tx.send(());
                    loop {
                        std::thread::park();
                    }
                }));
                entered_rx.await.unwrap();
                panic!("controlled MCP root future panic");
            },
            Duration::from_millis(100),
        )
    }

    fn run_isolated_test(name: &str, env_name: &str, marker: &str) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(env_name, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let output = child.wait_with_output().unwrap();
                assert!(status.success(), "{output:?}");
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains(marker),
                    "fixture did not execute: {output:?}"
                );
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("bounded runtime shutdown child did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn only_the_parsed_mcp_serve_command_uses_bounded_shutdown() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(is_mcp_serve(&args(&["lingxi-cli", "mcp", "serve"])));
        assert!(is_mcp_serve(&args(&[
            "lingxi-cli",
            "--verbose",
            "mcp",
            "serve"
        ])));
        for values in [
            &["lingxi-cli", "mcp", "list"][..],
            &["lingxi-cli", "mcp"][..],
            &["lingxi-cli", "--help"][..],
            &["lingxi-cli", "mcp", "serve", "--unknown-flag"][..],
            &["lingxi-cli", "mcp serve"][..],
        ] {
            assert!(!is_mcp_serve(&args(values)), "{values:?}");
        }
    }

    #[test]
    fn runtime_shutdown_reports_completion_for_an_idle_runtime() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        shutdown_mcp_runtime(runtime, Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn mcp_runtime_preserves_the_exit_code_when_shutdown_completes() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            run_mcp_runtime(runtime, async { 42 }, Duration::from_secs(5)),
            42
        );
    }

    #[test]
    fn mcp_stdio_restore_failure_is_not_reported_as_success() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            run_mcp_runtime_with_cleanup(runtime, async { 0 }, Duration::from_secs(5), || {
                Err(std::io::Error::other("controlled restore failure"))
            }),
            cli::exit_codes::RUNTIME_ERROR
        );
    }

    #[test]
    fn mcp_stdio_scope_is_started_before_poll_and_restored_after_shutdown() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let code = run_mcp_runtime_with_scope(
            runtime,
            async {
                events.borrow_mut().push("future");
                0
            },
            Duration::from_secs(5),
            || {
                events.borrow_mut().push("begin");
                Ok(())
            },
            || {
                events.borrow_mut().push("restore");
                Ok(())
            },
        );
        assert_eq!(code, 0);
        assert_eq!(*events.borrow(), ["begin", "future", "restore"]);
    }

    #[test]
    fn rejected_mcp_stdio_scope_neither_polls_nor_stops_another_scope() {
        for panic_in_begin in [false, true] {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            let polled = std::cell::Cell::new(false);
            let cleanup_called = std::cell::Cell::new(false);
            let code = run_mcp_runtime_with_scope(
                runtime,
                async {
                    polled.set(true);
                    0
                },
                Duration::from_secs(5),
                || {
                    assert!(!panic_in_begin, "controlled begin panic");
                    Err(std::io::Error::other("scope already in use"))
                },
                || {
                    cleanup_called.set(true);
                    Ok(())
                },
            );
            assert_eq!(code, cli::exit_codes::RUNTIME_ERROR);
            assert!(!polled.get());
            assert!(!cleanup_called.get());
        }
    }

    #[test]
    fn mcp_stdio_restore_runs_after_runtime_shutdown_times_out() {
        const CHILD: &str = "LINGXI_TEST_MCP_RESTORE_AFTER_TIMEOUT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let called = std::cell::Cell::new(false);
            let code = run_mcp_runtime_with_cleanup(
                runtime,
                async {
                    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
                    drop(tokio::task::spawn_blocking(move || {
                        let _ = entered_tx.send(());
                        loop {
                            std::thread::park();
                        }
                    }));
                    entered_rx.await.unwrap();
                    0
                },
                Duration::from_millis(100),
                || {
                    called.set(true);
                    Ok(())
                },
            );
            assert_eq!(code, cli::exit_codes::RUNTIME_ERROR);
            assert!(
                called.get(),
                "stdio restoration must be attempted on timeout"
            );
            println!("MCP_RESTORE_AFTER_TIMEOUT");
            return;
        }
        run_isolated_test(
            "tests::mcp_stdio_restore_runs_after_runtime_shutdown_times_out",
            CHILD,
            "MCP_RESTORE_AFTER_TIMEOUT",
        );
    }

    #[test]
    fn runtime_shutdown_reports_a_blocked_worker_without_hanging_the_process() {
        const CHILD: &str = "LINGXI_TEST_MCP_RUNTIME_SHUTDOWN_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
            drop(runtime.spawn_blocking(move || {
                entered_tx.send(()).unwrap();
                loop {
                    std::thread::park();
                }
            }));
            entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let error = shutdown_mcp_runtime(runtime, Duration::from_millis(100)).unwrap_err();
            assert!(
                error.contains("did not stop before the deadline"),
                "{error}"
            );
            println!("MCP_RUNTIME_SHUTDOWN_UNVERIFIED");
            return;
        }

        run_isolated_test(
            "tests::runtime_shutdown_reports_a_blocked_worker_without_hanging_the_process",
            CHILD,
            "MCP_RUNTIME_SHUTDOWN_UNVERIFIED",
        );
    }

    #[test]
    fn mcp_panic_also_has_bounded_unverified_shutdown() {
        const CHILD: &str = "LINGXI_TEST_MCP_RUNTIME_PANIC_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let code = run_panicking_blocked_runtime();
            assert_eq!(code, cli::exit_codes::RUNTIME_ERROR);
            println!("MCP_RUNTIME_PANIC_BOUNDED");
            return;
        }
        run_isolated_test(
            "tests::mcp_panic_also_has_bounded_unverified_shutdown",
            CHILD,
            "MCP_RUNTIME_PANIC_BOUNDED",
        );
    }

    #[test]
    fn mcp_panic_with_closed_stderr_still_has_bounded_shutdown() {
        use std::io::{BufRead as _, Write as _};

        const CHILD: &str = "LINGXI_TEST_MCP_CLOSED_STDERR_CHILD";
        if std::env::var_os(CHILD).is_some() {
            println!("MCP_STDERR_CLOSE_READY");
            std::io::stdout().flush().unwrap();
            let mut gate = String::new();
            std::io::stdin().read_line(&mut gate).unwrap();
            assert_eq!(gate, "proceed\n");
            assert_eq!(
                run_panicking_blocked_runtime(),
                cli::exit_codes::RUNTIME_ERROR
            );
            println!("MCP_CLOSED_STDERR_BOUNDED");
            return;
        }

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::mcp_panic_with_closed_stderr_still_has_bounded_shutdown",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (output_tx, output_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if output_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let ready = loop {
            match output_rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(Ok(line)) if line.contains("MCP_STDERR_CLOSE_READY") => break true,
                Ok(Ok(_)) => {}
                _ => break false,
            }
        };
        if !ready {
            let _ = child.kill();
            let _ = child.wait();
            panic!("closed-stderr child did not become ready");
        }
        drop(child.stderr.take());
        child.stdin.take().unwrap().write_all(b"proceed\n").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("closed-stderr runtime shutdown child did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "closed-stderr child failed: {status}");
        let mut observed = false;
        while let Ok(line) = output_rx.recv_timeout(Duration::from_secs(1)) {
            if line.unwrap().contains("MCP_CLOSED_STDERR_BOUNDED") {
                observed = true;
            }
        }
        assert!(observed, "closed-stderr child did not finish its fixture");
    }
}
