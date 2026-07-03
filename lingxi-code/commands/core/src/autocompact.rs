//! `/autocompact` — report (and explain) the auto-compact window size.
//!
//! Ported from the real 2.1.198 binary. Upstream ships **two** `autocompact`
//! command objects: an interactive `type:"local-jsx"` picker
//! (`"Set how full the context gets before auto-summarizing"`) and a headless
//! `type:"local",supportsNonInteractive:!0,thinClientDispatch:"post-text"`
//! variant (`"Configure the auto-compact window size"`). This handler ports the
//! **headless** variant — the one that produces plain text — since LingXi's
//! command registry is the non-interactive / bridge / headless dispatch path.
//!
//! ## Upstream behaviour (probed from the binary)
//!
//! The headless `call` is `N$m = async (e,t) => e.trim() ? EQt(n,t) : M$m(...)`:
//!
//! * **No argument** → `M$m` renders a status block: the current window (`auto`
//!   / `N tokens (from <ENV>)` / `N tokens (from settings)`), an optional
//!   "currently disabled" line, and two fixed explanation lines (plus an
//!   "Overriding auto…" line when the window is pinned).
//! * **An argument** → `EQt` first short-circuits when the env override is
//!   active (`"<ENV> is set and takes precedence. Unset it to change this
//!   setting."`), otherwise parses `auto`/`reset`/`unset`/`default` or a
//!   `100k–1M` token spec (`odo`) and **persists it to `userSettings`**.
//!
//! ## LingXi divergence (why this is read-only)
//!
//! LingXi's auto-compact window is resolved by
//! `compaction::thresholds::effective_context_window_size`, whose only override
//! knob is the `LINGXI_AUTO_COMPACT_WINDOW` env var (a positive integer). There
//! is **no** `userSettings.autoCompactWindow` store and **no**
//! `OrchestratorHandle` getter/setter for it (checked `traits/src/orchestrator.rs`).
//! So this handler is a faithful **read-only status/echo**: it reports the
//! current resolution and, when an argument is supplied while the env override
//! is active, surfaces the byte-exact env-precedence note explaining why the
//! value can't be changed here. Every emitted literal is byte-exact from the
//! 2.1.198 binary, with the env var rebranded `CLAUDE_CODE_AUTO_COMPACT_WINDOW`
//! → `LINGXI_AUTO_COMPACT_WINDOW` (the var LingXi actually honours), matching
//! the established rebrand of product/env nouns in `core_description`.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;

/// The env var LingXi honours for the auto-compact window override
/// (`compaction::thresholds::effective_context_window_size`). Rebrand of the
/// binary's `CLAUDE_CODE_AUTO_COMPACT_WINDOW`.
const WINDOW_ENV_VAR: &str = "LINGXI_AUTO_COMPACT_WINDOW";

/// Byte-exact from the binary's `EQt` env-precedence short-circuit (env noun
/// rebranded). Returned when a set-argument is supplied but the env override
/// pins the window.
const ENV_PRECEDENCE_NOTE: &str =
    "LINGXI_AUTO_COMPACT_WINDOW is set and takes precedence. Unset it to change this setting.";

/// Byte-exact `M$m` explanation line #1.
const EXPLAIN_THRESHOLD: &str = "Auto-compact summarizes the conversation when context usage approaches this limit. The actual threshold is the minimum of this setting and your model's maximum context window.";

/// Byte-exact `M$m` explanation line #2.
const EXPLAIN_AUTO_RECOMMENDED: &str = "The auto setting picks a window tuned for your model and is strongly recommended for the best cost and performance.";

/// Byte-exact `M$m` trailing line, emitted only when the window is pinned
/// (source is env / settings) rather than `auto`.
const EXPLAIN_OVERRIDE_WARNING: &str =
    "Overriding auto may result in high token usage, especially when resuming long sessions.";

/// `/autocompact` handler (headless, read-only). See module docs.
#[derive(Debug, Default, Clone)]
pub struct AutocompactHandler;

impl AutocompactHandler {
    /// Construct a new `AutocompactHandler`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The current window override from `LINGXI_AUTO_COMPACT_WINDOW`, or `None`
    /// when the var is unset / not a positive integer — mirroring
    /// `compaction::thresholds`' `parse_positive_u64` gate (0 / non-numeric are
    /// ignored, leaving the model default = `auto`).
    fn env_window() -> Option<u64> {
        std::env::var(WINDOW_ENV_VAR)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
    }

    /// Render the no-argument status block (`M$m`). `window` is the env override
    /// (`Some`) or `None` (model default = `auto`).
    fn status_block(window: Option<u64>) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(4);
        match window {
            Some(n) => lines.push(format!(
                "Auto-compact window: {} tokens (from {WINDOW_ENV_VAR})",
                format_tokens_compact(n)
            )),
            None => lines.push("Auto-compact window: auto".to_string()),
        }
        lines.push(EXPLAIN_THRESHOLD.to_string());
        lines.push(EXPLAIN_AUTO_RECOMMENDED.to_string());
        if window.is_some() {
            lines.push(EXPLAIN_OVERRIDE_WARNING.to_string());
        }
        lines.join("\n")
    }
}

/// Mirrors the binary's `gl` token formatter (`Intl.NumberFormat` compact
/// notation, lower-cased, with a trailing `.0` stripped): `200000` → `200k`,
/// `1000000` → `1m`, `1500000` → `1.5m`. For the round values a user sets via
/// the env var this matches upstream exactly; it may differ by a rounding digit
/// for non-round inputs (upstream uses full Intl 3-significant-figure rounding),
/// which is unobservable for the integer window values LingXi accepts.
fn format_tokens_compact(n: u64) -> String {
    fn trim(v: f64) -> String {
        // One decimal place, `.0` stripped (matches `gl`'s `.replace(".0","")`).
        let s = format!("{v:.1}");
        s.strip_suffix(".0").map_or(s.clone(), str::to_string)
    }
    if n >= 1_000_000 {
        format!("{}m", trim(n as f64 / 1_000_000.0))
    } else if n >= 1_000 {
        format!("{}k", trim(n as f64 / 1_000.0))
    } else {
        n.to_string()
    }
}

#[async_trait]
impl BuiltinCommandHandler for AutocompactHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let window = Self::env_window();
        let arg = args.raw_args.trim();

        let display = if arg.is_empty() {
            // No argument → status block (`M$m`).
            Self::status_block(window)
        } else if window.is_some() {
            // Set-argument while the env override is active → byte-exact
            // env-precedence note (`EQt` short-circuit).
            ENV_PRECEDENCE_NOTE.to_string()
        } else {
            // Set-argument, no env override: LingXi has no writable settings
            // store for the window (the only knob is the env var), so this is
            // read-only — report the current (model-default) resolution.
            Self::status_block(None)
        };

        CommandResult::Done {
            display: Some(display),
        }
    }

    fn name(&self) -> &str {
        "autocompact"
    }

    fn description(&self) -> &str {
        "Configure the auto-compact window size"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the env-mutating tests in this module so parallel test threads
    /// can't race the shared `LINGXI_AUTO_COMPACT_WINDOW` process env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "autocompact".to_string(),
            raw_args: raw.to_string(),
            positional_args: raw.split_whitespace().map(str::to_string).collect(),
        }
    }

    #[test]
    fn name_and_description() {
        let h = AutocompactHandler::new();
        assert_eq!(h.name(), "autocompact");
        assert_eq!(h.description(), "Configure the auto-compact window size");
    }

    #[test]
    fn compact_formatter_matches_gl_for_round_values() {
        assert_eq!(format_tokens_compact(200_000), "200k");
        assert_eq!(format_tokens_compact(500_000), "500k");
        assert_eq!(format_tokens_compact(1_000_000), "1m");
        assert_eq!(format_tokens_compact(1_500_000), "1.5m");
        assert_eq!(format_tokens_compact(50_000), "50k");
    }

    #[tokio::test]
    async fn no_env_no_arg_reports_auto_window() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var(WINDOW_ENV_VAR);
        let h = AutocompactHandler::new();
        match h.handle(&args("")).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Auto-compact window: auto\n"));
                assert!(s.contains("The actual threshold is the minimum of this setting"));
                assert!(s.contains("strongly recommended for the best cost and performance."));
                // The "Overriding auto…" line only appears when pinned.
                assert!(!s.contains("Overriding auto may result in high token usage"));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn env_set_no_arg_reports_env_sourced_window() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var(WINDOW_ENV_VAR, "200000");
        let h = AutocompactHandler::new();
        match h.handle(&args("")).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with(
                    "Auto-compact window: 200k tokens (from LINGXI_AUTO_COMPACT_WINDOW)\n"
                ));
                assert!(s.ends_with(
                    "Overriding auto may result in high token usage, especially when resuming long sessions."
                ));
            }
            other => panic!("expected Done, got {other:?}"),
        }
        std::env::remove_var(WINDOW_ENV_VAR);
    }

    #[tokio::test]
    async fn env_set_with_arg_returns_precedence_note() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var(WINDOW_ENV_VAR, "500000");
        let h = AutocompactHandler::new();
        match h.handle(&args("200k")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(
                    s,
                    "LINGXI_AUTO_COMPACT_WINDOW is set and takes precedence. Unset it to change this setting."
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
        std::env::remove_var(WINDOW_ENV_VAR);
    }

    #[tokio::test]
    async fn no_env_with_arg_is_read_only_status() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var(WINDOW_ENV_VAR);
        let h = AutocompactHandler::new();
        match h.handle(&args("200k")).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Auto-compact window: auto\n"));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zero_or_garbage_env_falls_back_to_auto() {
        let _g = ENV_LOCK.lock().unwrap();
        for bad in ["0", "abc", ""] {
            std::env::set_var(WINDOW_ENV_VAR, bad);
            let h = AutocompactHandler::new();
            match h.handle(&args("")).await {
                CommandResult::Done { display: Some(s) } => {
                    assert!(s.starts_with("Auto-compact window: auto\n"), "bad={bad}: {s}");
                }
                other => panic!("expected Done, got {other:?}"),
            }
        }
        std::env::remove_var(WINDOW_ENV_VAR);
    }
}
