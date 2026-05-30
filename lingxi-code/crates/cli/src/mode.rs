//! Three-way mode dispatch added in M6-01. Routes argv to one of:
//! - `Mode::Print(prompt)`: existing `run::run_oneshot` (v0.6.0, unchanged)
//! - `Mode::Tui`: new `lingxi_tui::run_tui_session` (v0.7.0 default)
//! - `Mode::StdioRepl`: existing `repl::run_repl` (v0.6.0, kept as `--no-tui`
//!   and non-TTY fallback)
//!
//! TTY detection uses [`std::io::IsTerminal`] (stable since Rust 1.70). If
//! stdin OR stdout isn't a terminal, we fall back to `StdioRepl` (CI, pipes,
//! redirected I/O). The `--no-tui` flag forces `StdioRepl` regardless of
//! TTY state.

use crate::argv::Argv;
use std::io::IsTerminal;

/// The resolved execution mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// One-shot print mode: prompt is set, exit after first `end_turn`.
    /// Carries the prompt string so the caller doesn't re-clone argv.
    Print(String),
    /// Fullscreen iocraft TUI (default when no prompt + TTY + no `--no-tui`).
    Tui,
    /// Line-based stdio REPL from v0.6.0 (`--no-tui` or non-TTY).
    StdioRepl,
}

/// Pick a mode from argv + the current process's TTY state.
///
/// Decision tree:
/// 1. If `argv.prompt` has a non-empty value → `Print(prompt)`.
/// 2. Else if `argv.no_tui` is set OR stdin/stdout isn't a TTY → `StdioRepl`.
/// 3. Else → `Tui`.
#[must_use]
pub fn decide_mode(argv: &Argv) -> Mode {
    decide_mode_with(argv, is_full_tty())
}

/// Test seam: `is_tty` argument decouples the decision from the real
/// process TTY state.
#[must_use]
pub fn decide_mode_with(argv: &Argv, is_tty: bool) -> Mode {
    if let Some(p) = argv.prompt.as_deref() {
        if !p.trim().is_empty() {
            return Mode::Print(p.to_string());
        }
    }
    if argv.no_tui || !is_tty {
        return Mode::StdioRepl;
    }
    Mode::Tui
}

/// True iff BOTH stdin and stdout are terminals. (M7-12) Exposed
/// `pub(crate)` so `run::run_resume` can make the same TTY decision the mode
/// dispatcher uses — one source of truth for "are we interactive".
pub(crate) fn is_full_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Execute the chosen mode. Returns the process exit code.
///
/// `Mode::Print` and `Mode::StdioRepl` call into the existing v0.6.0
/// code paths unchanged. `Mode::Tui` calls into the new `lingxi-tui`
/// entry point.
pub async fn dispatch(
    mode: Mode,
    argv: &Argv,
    runtime: &Runtime,
    sink: Arc<dyn OutputSink>,
) -> i32 {
    match mode {
        Mode::Print(_) => {
            // run_oneshot reads the prompt directly from argv.prompt;
            // the captured-prompt copy in Mode::Print(prompt) exists for
            // test introspection only.
            crate::run::run_oneshot(argv, runtime, sink.as_ref()).await
        }
        Mode::StdioRepl => {
            // v0.6.0 REPL path. `repl::run_repl` rebuilds the runtime
            // internally (it owns its own sink/orchestrator construction
            // for the streaming-stdout case). Pass argv through and
            // ignore the `runtime` arg in this arm.
            let _ = runtime; // intentionally unused in this arm
            let _ = sink; // intentionally unused (repl mints its own)
            crate::repl::run_repl(argv).await
        }
        Mode::Tui => {
            // M6-03 path: rebuild the runtime with `BridgeOutputStream` as
            // the orchestrator's output, then pass the bridge_rx into
            // `run_tui_session` so streaming events route into AppState.
            //
            // The `runtime` arg here was built with the standard
            // sink-adapter output (for one-shot / NDJSON modes); we
            // discard it and construct a TUI-specific build. The original
            // sink is therefore unused in this arm.
            let _ = runtime; // intentionally unused — we mint a TUI build
            let _ = sink; // intentionally unused
            let tui_build = match crate::init::build_runtime_for_tui(argv).await {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("lingxi-cli: tui init failed: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            // (M7-13 review) Coerce the concrete orchestrator to the
            // `OrchestratorHandle` trait object so it can drive both the session
            // id read and the Settings open pump inside the TUI mount.
            let orchestrator: Arc<dyn OrchestratorHandle> = tui_build.runtime.orchestrator.clone();
            let session_id = orchestrator.current_session_id().await;
            let bridge = lingxi_tui::session::TuiBridge {
                rx: tui_build.bridge_rx,
            };
            // Status snapshot — model from argv (if set), cwd from current
            // dir, cost placeholder. Full status wiring lands in M6-06.
            let mut status = lingxi_tui::state::StatusSnapshot::default();
            if let Some(m) = &argv.model {
                status.model.clone_from(m);
            }
            if let Ok(cwd) = std::env::current_dir() {
                status.cwd = cwd;
            }
            let tui_runtime = lingxi_tui::session::Runtime::with_bridge(session_id, bridge, status)
                .with_orchestrator(orchestrator);
            let cancel = CancellationToken::new();
            match lingxi_tui::run_tui_session(tui_runtime, cancel).await {
                Ok(()) => exit_codes::SUCCESS,
                Err(e) => {
                    eprintln!("lingxi-cli: tui session failed: {e}");
                    exit_codes::RUNTIME_ERROR
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(prompt: Option<&str>, no_tui: bool) -> Argv {
        Argv {
            prompt: prompt.map(String::from),
            print: false,
            resume: None,
            model: None,
            cwd: None,
            no_stream: false,
            json: false,
            debug: false,
            no_tui,
        }
    }

    #[test]
    fn prompt_routes_to_print() {
        let a = argv(Some("fix it"), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Print("fix it".into()));
    }

    #[test]
    fn empty_prompt_does_not_route_to_print() {
        let a = argv(Some("   "), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_prompt_tty_routes_to_tui() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_tui_flag_forces_stdio_repl() {
        let a = argv(None, true);
        assert_eq!(decide_mode_with(&a, true), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_routes_to_stdio_repl() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, false), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_with_prompt_still_prints() {
        // -p with piped input should still print one-shot (matches v0.6.0).
        let a = argv(Some("hi"), false);
        assert_eq!(decide_mode_with(&a, false), Mode::Print("hi".into()));
    }
}
