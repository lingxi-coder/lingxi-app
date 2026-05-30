//! Minimal CLI demo that drives the engine's reducer with the
//! `posix-minimal` platform.
//!
//! Reads one line at a time from stdin, feeds it to
//! [`lingxi_core::reduce`] as an [`Event::UserMessage`], then walks the
//! returned [`Effect`] vector and prints a human-readable trace. The CLI
//! deliberately does NOT make a live API call — Plan 17 wires the real
//! run loop (drive `SendApiRequest` through `HttpTransport`, fold the
//! resulting `ApiStream*` events back into the reducer).
//!
//! Run with `cargo run -p lingxi-cli-demo --bin lingxi-demo`. Exit with
//! Ctrl-D.

#![allow(missing_docs)]
#![forbid(unsafe_code)]

use clap::Parser;
use lingxi_api_client::AnthropicProvider;
use lingxi_core::{reduce, ConversationState, Event, SessionState};
use lingxi_platform_posix_minimal::{PosixFileSystem, PosixHttp, PosixRuntime};
use lingxi_protocol::{Effect, MessageId, RequestId, SessionId};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

/// CLI arguments understood by `lingxi-demo`.
#[derive(Parser)]
#[command(version, about = "LingXi Core demo CLI")]
struct Args {
    /// Anthropic API key (also reads `ANTHROPIC_API_KEY` env).
    #[arg(long)]
    api_key: Option<String>,

    /// Model to use.
    #[arg(long, default_value = "claude-opus-4-6")]
    model: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let api_key = args
        .api_key
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!("API key required (--api-key or ANTHROPIC_API_KEY)"))?;

    // Construct the platform façade. We keep the handles around even though
    // the M1.22 demo doesn't dispatch effects through them yet — Plan 17's
    // run loop is the consumer.
    let _fs = Arc::new(PosixFileSystem::new(std::env::current_dir()?));
    let _http = Arc::new(PosixHttp::new());
    let _rt = Arc::new(PosixRuntime::new());
    let _provider = AnthropicProvider::new(api_key, None);

    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::new(), args.model.clone()),
    };

    println!("LingXi demo — type a message, Ctrl-D to exit.");
    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let event = Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: line,
        };
        let (next, effects) = reduce(state, event);
        state = next;

        // For M1.22 we just demonstrate the effect flow; real API call
        // wiring (drive Effect::SendApiRequest through http transport,
        // then feed events back into the reducer) lands when the run-loop
        // scaffold is fleshed out.
        for e in &effects {
            match e {
                Effect::SendApiRequest { .. } => println!("[would call API]"),
                Effect::RenderStreamDelta { text } => {
                    print!("{text}");
                    stdout.lock().flush().ok();
                }
                Effect::RenderError { error } => eprintln!("ERROR: {error}"),
                Effect::Terminate { reason } => {
                    println!("[terminate: {reason}]");
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    Ok(())
}
