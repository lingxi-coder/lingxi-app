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
//! # REPL mode
//!
//! ```text
//! $ lingxi-cli           # → REPL (M5-13)
//! ```
//!
//! # Exit codes
//!
//! See [`exit_codes`].
//!
//! # Plan reference
//!
//! `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md`.

#![forbid(unsafe_code)]

pub mod argv;
pub mod cwd;
pub mod exit_codes;
pub mod init;
pub mod logging;
pub mod output;
pub mod output_adapter;
pub mod repl;
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

    if parsed.is_repl_mode() {
        return repl::run_repl(&parsed).await;
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

    let runtime = match init::build_runtime(&parsed, adapter).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lingxi-cli: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };

    if parsed.resume.is_some() {
        run::run_resume(&parsed, &runtime, sink.as_ref()).await
    } else {
        run::run_oneshot(&parsed, &runtime, sink.as_ref()).await
    }
}
