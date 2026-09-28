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
//! $ lingxi-cli --resume <uuid>   # load that session
//! $ lingxi-cli --resume          # TTY: iocraft Resume screen (M7-12)
//! $ lingxi-cli --resume --no-tui # stdio picker over 5 most-recent (M5-08)
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
// Documentation debt, not a decision that docs do not matter: this crate had
// 143 undocumented public item(s) when `missing_docs` was measured across the
// workspace (2026-09-16). The lint stays `warn` at the workspace level so a NEW
// crate still inherits the requirement; this allow is scoped here so the debt
// is visible per crate and can be repaid one crate at a time by deleting this
// line.
#![allow(missing_docs)]
// Dead code kept visible, not swept: this crate had 9 item(s) rustc could
// reach from nothing when the workspace was measured (2026-09-16). The lint
// stays `warn` at the workspace level so a NEW crate still inherits it; this
// allow is scoped here so the count is per crate and repayable by deleting this
// line. This is the category where "named, computed, never wired" hides — some
// of these read like features that were built and never connected. Each wants a
// decision (delete, or wire), not a blanket deletion.
// ⚠️ The count above is ONE macOS, lib-target measurement. It is not a list of
// deletable items — see docs/HANDOFF-dead-code-adjudication-2026-09-17.md,
// which records two near-misses where it said "dead" about live code.
#![allow(dead_code)]

pub mod agents_notify;
pub mod agents_registry;
pub mod argv;
pub mod ax_screen_reader;
pub mod background_dispatch;
pub mod background_launch;
pub mod bg_attach;
pub mod bg_attach_stall;
pub mod bg_reply_queue;
pub mod bg_session_forker;
mod bypass_env;
pub mod commands;
pub mod control_plane;
pub mod cwd;
pub mod daemon_lock;
pub mod daemon_roster;
pub mod exit_codes;
pub mod idle_notify;
pub mod init;
pub mod logging;
mod loop_wakeup;
pub mod mode;
mod model_selection;
pub mod output;
pub mod output_adapter;
mod permission_mode_preference;
pub mod permission_prompt_notify;
pub(crate) mod process_wrapper;
pub mod queued_commands;
pub mod repl;
pub mod repl_loop;
pub mod resume_truncation;
pub mod run;
pub mod session_cost;
pub mod sigint;
mod startup_resources;
mod startup_trace;
pub mod stream_json;
pub mod stream_json_input;
pub mod structured_output;
pub mod teammate_worker;

use crate::argv::Argv;

#[cfg(test)]
#[path = "lib/tests/startup_notice_tests.rs"]
mod startup_notice_tests;

#[cfg(test)]
#[path = "lib/tests/config_startup_tests.rs"]
mod config_startup_tests;

#[cfg(test)]
#[path = "lib/tests/session_id_store_tests.rs"]
mod session_id_store_tests;

#[cfg(test)]
#[path = "lib/tests/cli_mode_settings_tests.rs"]
mod cli_mode_settings_tests;

#[cfg(test)]
#[path = "lib/tests/help_layout_tests.rs"]
mod help_layout_tests;

mod shell_handoff;

mod command_line_output;
mod entrypoint;
mod permission_settings;
mod startup;

pub use entrypoint::run_cli;
pub(crate) use permission_settings::auto_mode_cycle_available;
pub(crate) use permission_settings::resolve_permission_mode;

#[cfg(test)]
use command_line_output::normalise_preamble;
#[cfg(test)]
use command_line_output::reorder_help_sections;
#[cfg(test)]
use entrypoint::session_id_exists_in_store;
#[cfg(test)]
use permission_settings::read_cli_mode_settings;
#[cfg(test)]
use startup::command_initializes_user_id;
#[cfg(test)]
use startup::command_runs_config_startup;

#[cfg(test)]
use startup::ghosting_terminal_notice;
#[cfg(test)]
use startup::startup_deprecation_notice;
