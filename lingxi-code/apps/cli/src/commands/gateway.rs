//! `lingxi-cli gateway` — surface parity for `claude gateway` (cc 2.1.198 M4).
//!
//! claude-code 2.1.198 registers `gateway [options]` ("Run the enterprise
//! auth/telemetry gateway") with a REQUIRED `--config <path>` ("Path to
//! gateway YAML config"). Verified live against the real binary:
//!
//! * `claude gateway --help`  → the fixture text
//!   (`test-harness/src/parity/fixtures/cc_2_1_198_gateway_help.txt`), exit 0;
//! * `claude gateway`         → stderr `error: required option '--config
//!   <path>' not specified`, exit 1 (commander `.requiredOption`);
//! * `claude gateway --config /missing.yaml` → stderr `claude gateway:
//!   ENOENT: no such file or directory, open '/missing.yaml'`, exit 1;
//! * `claude gateway --config empty.yaml` → stderr `claude gateway: [ …zod
//!   config-schema errors… ]`, exit 1 (the gateway runtime rejects the config
//!   before serving).
//!
//! lingxi-cli does NOT ship the enterprise gateway runtime (an
//! Anthropic-operated auth/telemetry proxy), so this family byte-matches the
//! CHEAP surface — help text (verbatim fixture), the missing-required-option
//! error, and the ENOENT line for a nonexistent config — then reports the
//! runtime as unavailable (exit `NOT_IMPLEMENTED`) instead of serving. It
//! never parses/validates the YAML (that would fake the zod error surface).
//!
//! clap's derive cannot render commander's exact help block, so `-h/--help`
//! is a manual flag here (help auto-flag disabled) that prints the locked
//! text below — kept byte-identical to the captured fixture (including the
//! `claude gateway` usage line, which names the upstream binary on purpose:
//! the fixture is the oracle).

use clap::Args;
use std::path::PathBuf;

/// The locked `claude gateway --help` text — byte-identical to
/// `cc_2_1_198_gateway_help.txt` (captured from the real 2.1.198 binary;
/// `parity_claude_2_1_198.rs` pins the same bytes from the fixture side).
pub const GATEWAY_HELP: &str = "Usage: claude gateway [options]\n\nRun the enterprise auth/telemetry gateway\n\nOptions:\n  --config <path>  Path to gateway YAML config\n  -h, --help       Display help for command\n";

/// `gateway` args. `--config` is REQUIRED by commander but declared optional
/// here so the missing-option error can be emitted byte-exactly (clap's own
/// missing-required rendering differs); `-h/--help` is manual for the same
/// reason (fixture-verbatim help).
#[derive(Debug, Clone, Args)]
#[command(disable_help_flag = true)]
pub struct Cli {
    /// Path to gateway YAML config
    #[arg(long = "config", value_name = "path")]
    pub config: Option<PathBuf>,

    /// Display help for command
    #[arg(short = 'h', long = "help")]
    pub help: bool,
}

/// Run the `gateway` family (see the module doc for the binary-verified
/// surface being matched).
pub async fn run(cli: &Cli) -> i32 {
    if cli.help {
        // Byte-locked fixture text, stdout, exit 0 (commander help path).
        print!("{GATEWAY_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    let Some(config) = cli.config.as_deref() else {
        // commander `.requiredOption` — verified live: exit 1.
        eprintln!("error: required option '--config <path>' not specified");
        return crate::exit_codes::ARGV_ERROR;
    };
    if !config.exists() {
        // The binary reads the config before serving — verified live:
        // `claude gateway: ENOENT: no such file or directory, open '<path>'`.
        eprintln!(
            "lingxi-cli gateway: ENOENT: no such file or directory, open '{}'",
            config.display()
        );
        return crate::exit_codes::RUNTIME_ERROR;
    }
    // A readable config would start the enterprise gateway — not part of
    // lingxi-cli. Clear unsupported notice on stderr + non-zero exit; never
    // serve, never fabricate the config-schema validation.
    eprintln!(
        "lingxi-cli gateway: the enterprise auth/telemetry gateway is not available in lingxi-cli."
    );
    crate::exit_codes::NOT_IMPLEMENTED
}
