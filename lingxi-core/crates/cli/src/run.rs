//! One-shot conversation: feed the prompt, run the orchestrator, print
//! results, exit. Also hosts the resume entrypoint wired in Task 8.

use crate::argv::Argv;
use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use lingxi_session::jsonl::loader::LoaderError;
use lingxi_traits::{SlashCommandDispatcher, SlashDispatchResult};

/// Drive a one-shot conversation: either a `/slash-command` or a normal
/// prompt that runs through the orchestrator turn loop.
pub async fn run_oneshot(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let prompt = argv.prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        eprintln!("lingxi-cli: empty prompt");
        return exit_codes::ARGV_ERROR;
    }

    // Slash branch — bypasses the API entirely.
    if prompt.starts_with('/') {
        return run_slash_command(&prompt, runtime, sink).await;
    }

    // Non-slash branch — drive the orchestrator turn loop. Without a real
    // ANTHROPIC_API_KEY this returns 401; we surface the error verbatim.
    sink.turn_start().await;
    match runtime.orchestrator.run_turn(&prompt).await {
        Ok(_outcome) => exit_codes::SUCCESS,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Dispatch a `/command [args]` line through the registry.
pub async fn run_slash_command(input: &str, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    match runtime.dispatcher.dispatch(input).await {
        SlashDispatchResult::Handled { display } => {
            sink.command_output("", &display).await;
            exit_codes::SUCCESS
        }
        SlashDispatchResult::Unknown { name: _, display } => {
            sink.command_output("", &display).await;
            exit_codes::RUNTIME_ERROR
        }
        SlashDispatchResult::NotASlashCommand => {
            // Defensive: only reached when caller violated the slash-prefix
            // contract.
            sink.error("runtime", "not a slash command (internal error)")
                .await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Resume an existing session, optionally running an additional turn on top.
///
/// M5-12 baseline: requires a concrete UUID via `--resume <ID>`. Interactive
/// picker (over the 5 most-recent sessions in the current cwd) is a known
/// surface deferred until M5-13 wires the REPL — passing `--resume` without
/// a value reports "interactive picker not yet wired" and exits with
/// [`exit_codes::NOT_IMPLEMENTED`].
pub async fn run_resume(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let arg = argv.resume.as_deref().unwrap_or("");
    if arg.is_empty() {
        eprintln!("lingxi-cli: interactive resume picker not yet wired (M5-13)");
        return exit_codes::NOT_IMPLEMENTED;
    }
    let session_id = match resolve_session_id(arg) {
        Ok(id) => id,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // M5-08's `with_resume` constructor reads a JSONL session file from
    // disk and rebuilds session state. Wiring that constructor through
    // the CLI requires a `FileSystem` arg (M5-08's loader signature) and
    // a `claude_home` path resolution; for M5-12 baseline we surface the
    // resolved id, run a follow-up turn if a prompt is supplied, and
    // defer the full file-system-backed replay to M5-13.
    sink.text(&format!("Resumed session {session_id}\n")).await;

    if let Some(p) = &argv.prompt {
        if !p.trim().is_empty() {
            return run_oneshot(argv, runtime, sink).await;
        }
    }
    eprintln!("lingxi-cli: resumed; REPL not yet wired (M5-13)");
    exit_codes::NOT_IMPLEMENTED
}

/// Resolve the `--resume <ID>` argument into a concrete UUID.
fn resolve_session_id(arg: &str) -> Result<uuid::Uuid, LoaderError> {
    uuid::Uuid::parse_str(arg).map_err(|_| LoaderError::SessionNotFound {
        arg: arg.to_string(),
    })
}
