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

/// Marker prefix for a watchdog abort caused by the machine SUSPENDING.
///
/// The oracle raises a separate error class for this — `StreamSuspendedError`
/// (`code = "StreamSuspended"`, carrying `sleptMs`), whose message reads
/// "Stream watchdog detected system suspend; aborting to retry on a fresh
/// connection" (@228830985). It is deliberately NOT the idle timeout: a stream
/// that stalled because the laptop slept is not a stalled server, and `sir()`
/// tells the user so.
///
/// Follows the same prefix idiom as [`STREAM_IDLE_TIMEOUT_PREFIX`] rather than
/// adding an `LlmError` variant — this is our own marker on our own message,
/// not an inference about a provider's wording.
pub const STREAM_SUSPENDED_PREFIX: &str = "Stream watchdog detected system suspend";

/// How much wall-clock drift past the monotonic timeout counts as a suspend.
///
/// During a real suspend the monotonic clock stops while the wall clock keeps
/// running, so the gap is the sleep duration. A second of slack keeps ordinary
/// scheduling jitter and NTP nudges out of the branch.
const SUSPEND_DRIFT_FLOOR: Duration = Duration::from_secs(1);

/// Classify a fired idle timeout: did the machine sleep through it?
///
/// `wall_elapsed` is measured with the system clock across the same wait the
/// monotonic `timeout` bounded. Monotonic time does not advance while suspended,
/// so a wall-clock excess beyond [`SUSPEND_DRIFT_FLOOR`] is the sleep.
#[must_use]
pub fn watchdog_abort_error(timeout: Duration, wall_elapsed: Duration) -> crate::LlmError {
    let slept = wall_elapsed.saturating_sub(timeout);
    if slept >= SUSPEND_DRIFT_FLOOR {
        return crate::LlmError::StreamInterrupted {
            message: format!(
                "{STREAM_SUSPENDED_PREFIX}; aborting to retry on a fresh connection \
                 (slept {}ms)",
                slept.as_millis()
            ),
        };
    }
    idle_timeout_error(timeout)
}

/// Whether an [`crate::LlmError`] is a watchdog SUSPEND abort.
#[must_use]
pub fn is_stream_suspended(error: &crate::LlmError) -> bool {
    matches!(
        error,
        crate::LlmError::StreamInterrupted { message } if message.starts_with(STREAM_SUSPENDED_PREFIX)
    )
}

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
        .and_then(parse_js_number_millis)
        .unwrap_or(0);
    Duration::from_millis(parsed.max(STREAM_IDLE_TIMEOUT_FLOOR_MS))
}

fn parse_js_number_millis(raw: &str) -> Option<u64> {
    let parsed = raw.trim().parse::<f64>().ok()?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return None;
    }
    if parsed >= u64::MAX as f64 {
        return Some(u64::MAX);
    }
    Some(parsed.trunc() as u64)
}

/// Resolve the effective idle timeout from the process environment:
/// `Some(timeout)` when the watchdog is enabled (the default), `None` when
/// explicitly disabled via `LINGXI_ENABLE_STREAM_WATCHDOG=0` (or the
/// `CLAUDE_` alias).
#[must_use]
pub fn resolve_stream_idle_timeout() -> Option<Duration> {
    let enabled = watchdog_enabled_from_values(
        std::env::var("LINGXI_ENABLE_STREAM_WATCHDOG")
            .ok()
            .as_deref(),
        std::env::var("CLAUDE_ENABLE_STREAM_WATCHDOG")
            .ok()
            .as_deref(),
    );
    if !enabled {
        return None;
    }
    Some(idle_timeout_from_values(
        std::env::var("LINGXI_STREAM_IDLE_TIMEOUT_MS")
            .ok()
            .as_deref(),
        std::env::var("CLAUDE_STREAM_IDLE_TIMEOUT_MS")
            .ok()
            .as_deref(),
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
        assert_eq!(
            idle_timeout_from_values(Some("1e6"), None),
            Duration::from_millis(1_000_000)
        );
        assert_eq!(
            idle_timeout_from_values(Some("2.5e5"), None),
            Duration::from_millis(300_000)
        );
        assert_eq!(
            idle_timeout_from_values(Some("600000.9"), None),
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
        assert!(!is_stream_idle_timeout(
            &crate::LlmError::StreamInterrupted {
                message: "malformed frame".into()
            }
        ));
    }
}
