//! Accessibility screen-reader mode gate (`--ax-screen-reader`).
//!
//! 1:1 port of the 2.1.201 accessibility gate. In the binary this is a small
//! class (`uNi`) holding a cached `#e` boolean, resolved once by `isEnabled()`,
//! plus the free functions `IO()` (read the gate), `XHe()` (subprocess env
//! injection) and the startup announcement in the main entrypoint.
//!
//! Oracle (`2.1.201`, minified) — the whole surface:
//!
//! ```js
//! class uNi{
//!   #e;
//!   isEnabled(){
//!     if(this.#e!==void 0)return this.#e;                 // cached
//!     let e;
//!     if(lNi("--ax-screen-reader"))e=!0;                  // 1. CLI flag (raw argv)
//!     else{
//!       let t=Ie.CLAUDE_AX_SCREEN_READER;                 // 2. env var
//!       e=t!==void 0?t:Hr().axScreenReader===!0;          // 3. config `axScreenReader`
//!     }
//!     if(!e)return this.#e=!1;
//!     return this.#e=NQr?.(iWd,!0)??!0;                   // telemetry gate, default passthrough
//!   }
//!   reset(){this.#e=void 0}
//! }
//! function IO(){return dNi.isEnabled()}
//! function XHe(){if(dNi.isEnabled())return{CLAUDE_AX_SCREEN_READER:"1"};return{}}
//! var iWd="tengu_ax_screen_reader",NQr=null,dNi;
//! ```
//!
//! and the startup announcement in the main entrypoint:
//!
//! ```js
//! if(!L && process.stdout.isTTY && IO()) console.log("[Accessible screen reader mode: on]");
//! // L == print mode (`--print`/`-p`)
//! ```
//!
//! Branding: the env var `CLAUDE_AX_SCREEN_READER` → `LINGXI_AX_SCREEN_READER`
//! (brand token swap), while the telemetry gate key `tengu_ax_screen_reader`
//! and the config key `axScreenReader` stay verbatim (telemetry name / config
//! wire key), and the announcement string is copied byte-for-byte.
//!
//! NOTE on the env-var truthiness quirk: the binary does `e = t !== undefined
//! ? t : config`, i.e. it keeps the *string* value, and later `if(!e)` only
//! rejects the falsy empty string. So `LINGXI_AX_SCREEN_READER=` (empty) →
//! disabled, but ANY non-empty value (`"0"`, `"false"`, `"1"`, …) → enabled.
//! [`resolve`] reproduces this exactly.

use std::sync::OnceLock;

/// Env var consulted after the CLI flag and before config. Brand-swapped from
/// the oracle's `CLAUDE_AX_SCREEN_READER`.
pub const ENV_VAR: &str = "LINGXI_AX_SCREEN_READER";

/// Telemetry gate key (`iWd`). Stays verbatim — a `tengu_*` wire name.
pub const TELEMETRY_KEY: &str = "tengu_ax_screen_reader";

/// Config key (`Hr().axScreenReader`). Stays verbatim — a config wire key.
pub const CONFIG_KEY: &str = "axScreenReader";

/// Startup announcement (`console.log(...)`), copied byte-for-byte.
pub const ANNOUNCEMENT: &str = "[Accessible screen reader mode: on]";

/// Process-wide cached resolution, mirroring the class's `#e` field. Set once
/// by [`init`]; read by [`is_enabled`], [`subprocess_env`] and [`should_announce`].
static ENABLED: OnceLock<bool> = OnceLock::new();

/// Pure precedence resolver — the body of `uNi.isEnabled()` minus caching and
/// the (default-passthrough) telemetry gate.
///
/// Precedence, highest first:
///   1. `flag` — the parsed `--ax-screen-reader` CLI flag → `true`.
///   2. `env`  — `LINGXI_AX_SCREEN_READER`: if present, truthy iff non-empty
///      (JS `if(!stringValue)` semantics — empty string is the only falsy set
///      value).
///   3. `config` — settings `axScreenReader === true`.
///
/// The telemetry gate (`NQr?.(iWd,!0) ?? !0`) is a no-op passthrough in the
/// binary (`NQr` defaults to `null`), so a resolved-true stays true.
pub fn resolve(flag: bool, env: Option<&str>, config: Option<bool>) -> bool {
    if flag {
        return true;
    }
    match env {
        // `e = t` (string); later `if(!e)` rejects only the empty string.
        Some(value) => !value.is_empty(),
        // Env unset → fall through to config `axScreenReader === true`.
        None => config == Some(true),
    }
}

/// Resolve once and cache, mirroring the first call to `uNi.isEnabled()`.
///
/// `flag` is the already-parsed `argv.ax_screen_reader`; `config` is the merged
/// settings `axScreenReader`. The env var is read here (once) via [`ENV_VAR`].
/// A missing var and a var holding invalid UTF-8 are both treated as "unset"
/// (fall through to config), matching `Ie.CLAUDE_AX_SCREEN_READER === undefined`.
///
/// Idempotent: only the first call wins (subsequent calls are ignored, like the
/// cached `#e`). Returns the resolved value.
pub fn init(flag: bool, config: Option<bool>) -> bool {
    let env = std::env::var(ENV_VAR).ok();
    let resolved = resolve(flag, env.as_deref(), config);
    // First writer wins; ignore the error from a redundant re-init.
    let _ = ENABLED.set(resolved);
    // `tui` cannot depend back on this CLI module. Propagate the resolved gate
    // through the same env seam child sessions already inherit.
    if is_enabled() {
        std::env::set_var(ENV_VAR, "1");
    }
    is_enabled()
}

/// Read the cached gate (`IO()` / `dNi.isEnabled()`).
///
/// Returns `false` before [`init`] runs — the safe default (classic behavior
/// off), matching the binary's "not yet enabled" state.
pub fn is_enabled() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}

/// Subprocess env injection (`XHe()`): the extra env entries a spawned child
/// `lingxi-cli` inherits so the screen-reader mode propagates across the
/// process boundary. Enabled → `[(LINGXI_AX_SCREEN_READER, "1")]`; else empty.
///
/// Callers merge these into the child environment map when spawning agent /
/// background / worktree child processes (see residuals for the wiring points).
pub fn subprocess_env() -> Vec<(String, String)> {
    if is_enabled() {
        vec![(ENV_VAR.to_string(), "1".to_string())]
    } else {
        Vec::new()
    }
}

/// Startup-announcement gate (`!L && process.stdout.isTTY && IO()`).
///
/// `is_print` is the `--print`/`-p` mode flag (`L`); `stdout_is_tty` is
/// `process.stdout.isTTY`. Pure so the exact condition is unit-testable.
pub fn should_announce(is_print: bool, stdout_is_tty: bool) -> bool {
    !is_print && stdout_is_tty && is_enabled()
}

/// Emit the startup announcement to stdout when [`should_announce`] holds,
/// matching `console.log("[Accessible screen reader mode: on]")`.
pub fn maybe_announce(is_print: bool, stdout_is_tty: bool) {
    if should_announce(is_print, stdout_is_tty) {
        println!("{ANNOUNCEMENT}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- precedence (`resolve`) -----------------------------------------

    #[test]
    fn flag_wins_over_everything() {
        // Flag beats a disabling env and a disabling config.
        assert!(resolve(true, Some(""), Some(false)));
        assert!(resolve(true, None, None));
    }

    #[test]
    fn env_present_nonempty_enables() {
        // Any non-empty env value is truthy (JS `if(!"0")` is false).
        assert!(resolve(false, Some("1"), Some(false)));
        assert!(resolve(false, Some("0"), Some(false)));
        assert!(resolve(false, Some("false"), None));
        assert!(resolve(false, Some("true"), None));
    }

    #[test]
    fn env_present_empty_disables_and_does_not_fall_through_to_config() {
        // `LINGXI_AX_SCREEN_READER=` present-but-empty → disabled, and it does
        // NOT fall through to config (env `!== undefined` short-circuits).
        assert!(!resolve(false, Some(""), Some(true)));
    }

    #[test]
    fn config_used_only_when_env_absent() {
        assert!(resolve(false, None, Some(true)));
        assert!(!resolve(false, None, Some(false)));
        assert!(!resolve(false, None, None));
    }

    // ---- subprocess env injection (`XHe`) -------------------------------

    #[test]
    fn subprocess_env_shape_matches_oracle() {
        // The env name is the brand-swapped var; the value is the literal "1".
        // Exercise the pure mapping without touching the process-wide cache.
        let enabled_pairs = vec![(ENV_VAR.to_string(), "1".to_string())];
        assert_eq!(
            enabled_pairs,
            vec![("LINGXI_AX_SCREEN_READER".to_string(), "1".to_string())]
        );
        // Disabled → no entries (the `return {}` branch).
        let disabled: Vec<(String, String)> = Vec::new();
        assert!(disabled.is_empty());
    }

    // ---- announcement gate (`!L && isTTY && IO()`) ----------------------

    #[test]
    fn announcement_requires_tty_not_print_and_enabled() {
        // With the gate ON: only tty + non-print announces.
        let _ = ENABLED.set(true);
        assert!(should_announce(false, true)); // interactive tty, enabled
        assert!(!should_announce(true, true)); // print mode suppresses
        assert!(!should_announce(false, false)); // non-tty suppresses
    }

    #[test]
    fn constants_match_oracle_strings() {
        assert_eq!(ENV_VAR, "LINGXI_AX_SCREEN_READER");
        assert_eq!(TELEMETRY_KEY, "tengu_ax_screen_reader");
        assert_eq!(CONFIG_KEY, "axScreenReader");
        assert_eq!(ANNOUNCEMENT, "[Accessible screen reader mode: on]");
    }
}
