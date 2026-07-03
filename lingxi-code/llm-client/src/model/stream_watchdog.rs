//! Streaming idle watchdog (cc 2.1.196 default-on).
//!
//! Ports the 2.1.198 binary's stream-idle watchdog resolution:
//!
//! ```js
//! // query loop @219636542:
//! jo = Oe.CLAUDE_ENABLE_STREAM_WATCHDOG ?? !0          // default ON
//! ss = fzr()                                           // event-idle timeout
//! // fzr @208691984:
//! function fzr(){ return Math.max(Number(process.env.CLAUDE_STREAM_IDLE_TIMEOUT_MS)||0, 300000) }
//! ```
//!
//! LingXi accepts BOTH the `LINGXI_*` and `CLAUDE_*` spellings of the two env
//! knobs (LINGXI takes precedence), per the branding-alias convention:
//!
//! - `LINGXI_ENABLE_STREAM_WATCHDOG` / `CLAUDE_ENABLE_STREAM_WATCHDOG` —
//!   `0`/`false` (any case) disables; absent or anything else keeps the
//!   default ON.
//! - `LINGXI_STREAM_IDLE_TIMEOUT_MS` / `CLAUDE_STREAM_IDLE_TIMEOUT_MS` —
//!   raises the idle timeout; values below the 5-minute floor are clamped up
//!   (binary `Math.max(n||0, 300000)` — the floor also swallows unparseable
//!   or non-positive values).
//!
//! When the watchdog fires, the stream is aborted with
//! [`crate::LlmError::StreamInterrupted`] whose message starts with
//! [`STREAM_IDLE_TIMEOUT_PREFIX`] — callers (the orchestrator turn loop)
//! detect that prefix via [`is_stream_idle_timeout`] to drive the 2.1.198
//! watchdog-retry rules (retry once after a thinking-only yield; finalize the
//! partial response otherwise).

use std::time::Duration;

/// The 5-minute idle floor: binary `fzr()`'s `300000` ms.
pub const STREAM_IDLE_TIMEOUT_FLOOR_MS: u64 = 300_000;

/// Marker prefix for watchdog-aborted streams. The binary's synthesized
/// errors use the same prefix (`"Stream idle timeout - partial response
/// received"` / `"Stream idle timeout - no chunks received"` @219649648).
pub const STREAM_IDLE_TIMEOUT_PREFIX: &str = "Stream idle timeout";

/// Whether an [`crate::LlmError`] is a watchdog idle-timeout abort.
#[must_use]
pub fn is_stream_idle_timeout(error: &crate::LlmError) -> bool {
    matches!(
        error,
        crate::LlmError::StreamInterrupted { message } if message.starts_with(STREAM_IDLE_TIMEOUT_PREFIX)
    )
}

/// Build the watchdog abort error for a given configured timeout.
#[must_use]
pub fn idle_timeout_error(timeout: Duration) -> crate::LlmError {
    crate::LlmError::StreamInterrupted {
        message: format!(
            "{STREAM_IDLE_TIMEOUT_PREFIX} - no stream event within {}ms",
            timeout.as_millis()
        ),
    }
}

/// `Oe.CLAUDE_ENABLE_STREAM_WATCHDOG ?? !0` — enabled unless the env value is
/// explicitly falsy (`0` / `false`, case-insensitive). `lingxi` wins over
/// `claude` when both are set.
#[must_use]
pub fn watchdog_enabled_from_values(lingxi: Option<&str>, claude: Option<&str>) -> bool {
    let raw = lingxi.or(claude);
    match raw {
        Some(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false"))
        }
        None => true,
    }
}

/// Binary `fzr()`: `Math.max(Number(env)||0, 300000)`. Unparseable / missing
/// / non-positive values fall to the floor. `lingxi` wins over `claude`.
#[must_use]
pub fn idle_timeout_from_values(lingxi: Option<&str>, claude: Option<&str>) -> Duration {
    let parsed: u64 = lingxi
        .or(claude)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Duration::from_millis(parsed.max(STREAM_IDLE_TIMEOUT_FLOOR_MS))
}

/// Resolve the effective idle timeout from the process environment:
/// `Some(timeout)` when the watchdog is enabled (the default), `None` when
/// explicitly disabled via `LINGXI_ENABLE_STREAM_WATCHDOG=0` (or the
/// `CLAUDE_` alias).
#[must_use]
pub fn resolve_stream_idle_timeout() -> Option<Duration> {
    let enabled = watchdog_enabled_from_values(
        std::env::var("LINGXI_ENABLE_STREAM_WATCHDOG").ok().as_deref(),
        std::env::var("CLAUDE_ENABLE_STREAM_WATCHDOG").ok().as_deref(),
    );
    if !enabled {
        return None;
    }
    Some(idle_timeout_from_values(
        std::env::var("LINGXI_STREAM_IDLE_TIMEOUT_MS").ok().as_deref(),
        std::env::var("CLAUDE_STREAM_IDLE_TIMEOUT_MS").ok().as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_with_five_minute_floor() {
        // Binary: `jo = Oe.CLAUDE_ENABLE_STREAM_WATCHDOG ?? !0` (default ON),
        // `fzr()` floors at 300000ms.
        assert!(watchdog_enabled_from_values(None, None));
        assert_eq!(
            idle_timeout_from_values(None, None),
            Duration::from_millis(300_000)
        );
    }

    #[test]
    fn zero_or_false_disables_via_either_spelling() {
        assert!(!watchdog_enabled_from_values(Some("0"), None));
        assert!(!watchdog_enabled_from_values(None, Some("0")));
        assert!(!watchdog_enabled_from_values(Some("false"), None));
        assert!(!watchdog_enabled_from_values(None, Some("FALSE")));
        // Any other value (including "1"/"true"/junk) keeps it on.
        assert!(watchdog_enabled_from_values(Some("1"), None));
        assert!(watchdog_enabled_from_values(Some("yes"), None));
    }

    #[test]
    fn lingxi_spelling_wins_over_claude() {
        // LINGXI=1 overrides CLAUDE=0.
        assert!(watchdog_enabled_from_values(Some("1"), Some("0")));
        // LINGXI=0 overrides CLAUDE=1.
        assert!(!watchdog_enabled_from_values(Some("0"), Some("1")));
        assert_eq!(
            idle_timeout_from_values(Some("600000"), Some("900000")),
            Duration::from_millis(600_000)
        );
    }

    #[test]
    fn timeout_below_floor_clamps_up_and_junk_falls_to_floor() {
        // Binary `Math.max(Number(v)||0, 300000)`.
        assert_eq!(
            idle_timeout_from_values(Some("1000"), None),
            Duration::from_millis(300_000)
        );
        assert_eq!(
            idle_timeout_from_values(Some("junk"), None),
            Duration::from_millis(300_000)
        );
        assert_eq!(
            idle_timeout_from_values(Some("600000"), None),
            Duration::from_millis(600_000)
        );
    }

    #[test]
    fn idle_timeout_error_is_detectable() {
        let err = idle_timeout_error(Duration::from_millis(300_000));
        assert!(is_stream_idle_timeout(&err));
        assert!(!is_stream_idle_timeout(&crate::LlmError::Transport {
            message: "connection reset".into()
        }));
        assert!(!is_stream_idle_timeout(&crate::LlmError::StreamInterrupted {
            message: "malformed frame".into()
        }));
    }
}
