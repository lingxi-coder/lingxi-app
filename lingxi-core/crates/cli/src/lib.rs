//! Library surface of the `lingxi-cli` crate.
//!
//! Integration tests import the public API instead of shelling out to the
//! binary so they don't need the workspace target dir hot.
//!
//! # One-shot mode
//!
//! ```text
//! $ lingxi-cli "fix the bug in foo.rs"
//! $ lingxi-cli -p "list the files in this repo"
//! $ lingxi-cli --no-stream --json "what's the weather?"
//! ```
//!
//! # Resume mode
//!
//! ```text
//! $ lingxi-cli --resume <uuid>
//! $ lingxi-cli --resume     # interactive picker over 5 most-recent
//! ```
//!
//! # REPL mode (M5-13)
//!
//! Invoking `lingxi-cli` without a positional prompt drops into a
//! line-based REPL:
//!
//! ```text
//! $ lingxi-cli
//! > /version
//! lingxi-cli 0.5.0 (abc1234)
//! > hello, claude
//! Hi! How can I help?
//! > /exit
//! Exiting.
//! ```
//!
//! - **Prompt**: `"> "` printed to stderr (so stdout stays parseable in
//!   `--json` mode).
//! - **EOF (Ctrl+D)**: persists the session and exits 0.
//! - **First Ctrl+C during a turn**: cancels the turn, returns to prompt.
//! - **First Ctrl+C at idle prompt**: arms a flag; second Ctrl+C within
//!   2 seconds exits with code 130.
//! - **`/exit`**: flips the orchestrator's `should_exit` flag; REPL
//!   detects it after the dispatcher returns and exits 0.
//!
//! # Exit codes
//!
//! See [`exit_codes`].
//!
//! # Plan reference
//!
//! `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md` (one-shot),
//! `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` (REPL).

#![forbid(unsafe_code)]

pub mod argv;
pub mod cwd;
pub mod exit_codes;
pub mod init;
pub mod logging;
pub mod mode;
pub mod output;
pub mod output_adapter;
pub mod repl;
pub mod repl_loop;
pub mod run;
pub mod sigint;

use crate::argv::Argv;
use clap::error::ErrorKind;
use std::ffi::OsString;
use std::sync::Arc;

/// Top-level entrypoint. Returns the process exit code.
pub async fn run_cli(args: Vec<OsString>) -> i32 {
    let parsed = match Argv::from_iter(args) {
        Ok(a) => a,
        Err(e) => {
            // clap prints its own help/usage; we just return the locked
            // code. Help/version are not errors.
            e.print().ok();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => exit_codes::SUCCESS,
                _ => exit_codes::ARGV_ERROR,
            };
        }
    };

    logging::init(parsed.debug);
    tracing::debug!(?parsed, "argv parsed");

    if let Err(e) = cwd::apply_cwd(parsed.cwd.as_deref()) {
        eprintln!("lingxi-cli: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // Pick the sink first so we can install it on the orchestrator at
    // construction time. For `--json` the session id used in `turn_start`
    // is minted afresh (synchronous mint via `SessionId::new`); for
    // plain mode the id is unused.
    let sink: Arc<dyn output::OutputSink> = if parsed.json {
        Arc::new(output::JsonSink::new(lingxi_protocol::SessionId::new()))
    } else {
        Arc::new(output::PlainSink::new())
    };
    let adapter: Arc<dyn lingxi_traits::OutputStream> =
        Arc::new(output_adapter::SinkAdapter::new(sink.clone()));

    // For `Mode::Print` we still need the runtime; for `Mode::StdioRepl`
    // and `Mode::Tui` we also build it once so `mode::dispatch` can pass
    // the orchestrator's session id into the TUI. Building the runtime
    // is cheap (no API calls until `run_turn`).
    let runtime = match init::build_runtime(&parsed, adapter).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lingxi-cli: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // --resume still routes through run::run_resume (v0.6.0 behaviour
    // unchanged). M7 wires resume into the TUI.
    if parsed.resume.is_some() {
        return run::run_resume(&parsed, &runtime, sink.as_ref()).await;
    }

    let chosen = mode::decide_mode(&parsed);
    mode::dispatch(chosen, &parsed, &runtime, sink).await
}
