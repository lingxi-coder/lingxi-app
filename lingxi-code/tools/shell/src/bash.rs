//! `BashTool` — runs a shell command via M2-04 sandbox dispatch + M2-06
//! process polish.
//!
//! See `docs/superpowers/plans/2026-05-24-m4-02-shell.md` for the task list
//! and `docs/superpowers/specs/2026-05-24-m4-tools-implementation-design.md`
//! §4 Flow C and §7 wire identifiers for the locked literals.
//!
//! Image-output handling (claude-code `formatOutput` / `isImageOutput`,
//! `BashTool/utils.ts`): a base64 `data:image/...;base64,...` stdout is
//! returned as an IMAGE content block (riding on `new_messages` per the Rust
//! image contract, mirroring FileRead) rather than truncated as text — see the
//! short-circuit in `call`. PARTIALLY DEFERRED: claude-code
//! `resizeShellImageOutput` (CC-304 image dimension/byte cap) is a follow-up —
//! it needs an image-decode dep tool-shell doesn't pull in. The model still
//! receives the image (the URI payload is already valid base64, emitted as-is);
//! only the optional resize/re-encode is deferred.
//!
//! Timeout/interrupt handling (claude-code `BashTool.tsx` ~602-605 / 720):
//! a timed-out (or interrupted) command is surfaced as a SUCCESSFUL
//! `tool_result` carrying `interrupted: true`, `timed_out: true`, and
//! whatever partial stdout/stderr was captured — NOT a hard error. The
//! `<error>Command was aborted before completion</error>` marker is appended
//! to stderr and `is_error` follows `interrupted`. See `interrupted_result`.

use crate::shared::strip_ansi_count;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{BASH_COMPLETED, BASH_FAILED, BASH_STARTED, BASH_TIMEOUT};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH;
use tool_api::BuiltinToolContext;

// ===== Locked constants =====================================================

/// 2-minute default Bash timeout — the fallback when `BASH_DEFAULT_TIMEOUT_MS`
/// is unset (see [`bash_default_timeout_ms`]).
pub const BASH_DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// 10-minute default maximum Bash timeout — the fallback floor when
/// `BASH_MAX_TIMEOUT_MS` is unset (see [`bash_max_timeout_ms`]).
pub const BASH_MAX_TIMEOUT_MS: u64 = 600_000;
/// Byte-locked timeout error template. `{N}` is substituted at call time.
pub const BASH_TIMEOUT_ERROR_TEMPLATE: &str = "Bash command timed out after {N}ms";

/// claude-code `s0e()` — the effective default Bash timeout. Reads
/// `BASH_DEFAULT_TIMEOUT_MS` via [`resolve_default_timeout_ms`].
#[must_use]
pub fn bash_default_timeout_ms() -> u64 {
    resolve_default_timeout_ms(std::env::var("BASH_DEFAULT_TIMEOUT_MS").ok().as_deref())
}

/// Pure resolver for `s0e()`: `parseInt(raw,10)` used iff `> 0`, else
/// [`BASH_DEFAULT_TIMEOUT_MS`] (120000).
#[must_use]
pub fn resolve_default_timeout_ms(raw: Option<&str>) -> u64 {
    raw.and_then(parse_int_js)
        .filter(|&n| n > 0)
        .map(|n| n as u64)
        .unwrap_or(BASH_DEFAULT_TIMEOUT_MS)
}

/// claude-code `j3n()` — the effective maximum Bash timeout (advisory; shown in
/// the schema `describe` text and the system prompt). Reads `BASH_MAX_TIMEOUT_MS`
/// via [`resolve_max_timeout_ms`].
#[must_use]
pub fn bash_max_timeout_ms() -> u64 {
    resolve_max_timeout_ms(
        std::env::var("BASH_MAX_TIMEOUT_MS").ok().as_deref(),
        bash_default_timeout_ms(),
    )
}

/// Pure resolver for `j3n()`: `parseInt(raw,10)` when `> 0` → `max(n, default)`;
/// otherwise `max(`[`BASH_MAX_TIMEOUT_MS`]` (600000), default)`. The floor
/// against `default` mirrors the binary's `Math.max(_, s0e(env))`.
#[must_use]
pub fn resolve_max_timeout_ms(raw: Option<&str>, default: u64) -> u64 {
    raw.and_then(parse_int_js)
        .filter(|&n| n > 0)
        .map(|n| (n as u64).max(default))
        .unwrap_or_else(|| BASH_MAX_TIMEOUT_MS.max(default))
}
/// Linux/WSL shell path.
pub const BASH_SHELL_LINUX: &str = "/bin/bash";
/// macOS shell path.
pub const BASH_SHELL_MACOS: &str = "/bin/zsh";
/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Bash";

/// Format the locked timeout error string with `{N}` substituted.
#[must_use]
pub fn format_timeout_error(timeout_ms: u64) -> String {
    BASH_TIMEOUT_ERROR_TEMPLATE.replace("{N}", &timeout_ms.to_string())
}

/// Minimum sleep duration (seconds) that triggers the sleep-block — byte-locked
/// to claude-code `G2n=25`. Sleeps shorter than this are permitted (the
/// `if(o<G2n)return null` guard in `t2p`); LingXi previously used 2.
pub const SLEEP_BLOCK_THRESHOLD_SECS: f64 = 25.0;

/// Whether the bash sleep-block is active — the LingXi analogue of claude-code's
/// `sq()=ct("tengu_amber_sentinel", false)` gate. Defaults to **false** (so the
/// block is inert in the default config, exactly like stock claude-code), and is
/// opt-in via a truthy `tengu_amber_sentinel` env override (1/true/yes/on). This
/// mirrors `is_agent_swarms_enabled`'s env-based gate accessor (the GrowthBook
/// gate is a host-runtime signal not threaded into the tool, so LingXi defaults
/// it and exposes an env override for parity/testing).
#[must_use]
pub fn sleep_block_enabled() -> bool {
    std::env::var("tengu_amber_sentinel").ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Format a parsed sleep duration the way JS `${parseFloat(x)}` would: an
/// integral value renders WITHOUT a decimal point (`30`, not `30.0`), a
/// fractional value keeps its fraction (`30.5`).
#[must_use]
fn format_parsefloat(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Detect a standalone or leading `sleep N` (N>=25) pattern that should use the
/// background path / Monitor tool instead of blocking the turn. Faithful port
/// of claude-code `t2p` (`BashTool` `detectBlockedSleepPattern`): splits on the
/// shell list separators, then matches `^sleep\s+(\d+(?:\.\d*)?)\s*$` on the
/// FIRST subcommand only, `parseFloat`s the capture, and returns `null` when it
/// is `< G2n` ([`SLEEP_BLOCK_THRESHOLD_SECS`] = 25). FRACTIONAL durations ARE
/// matched (`sleep 30.5`), unlike the prior integer-only port. The duration is
/// rendered with `parseFloat` semantics. Returns the pattern description
/// (`standalone sleep N` or `sleep N followed by: <rest>`), else `None`.
#[must_use]
pub fn detect_blocked_sleep_pattern(command: &str) -> Option<String> {
    let parts = permission::shell_command::split_command(command);
    let first = parts.first().map(|s| s.trim()).unwrap_or("");
    // `^sleep\s+(\d+(?:\.\d*)?)\s*$` — manual match (no regex dep).
    let rest_after_kw = first.strip_prefix("sleep")?;
    // Require at least one whitespace char after the keyword.
    let after_ws = rest_after_kw.trim_start_matches(|c: char| c.is_ascii_whitespace());
    if after_ws.len() == rest_after_kw.len() {
        return None; // no separating whitespace (e.g. "sleeper")
    }
    // Consume the digit run, then an OPTIONAL `.` + further (optional) digits.
    let bytes = after_ws.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None; // `\d+` needs at least one leading digit
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    let (num_str, tail) = after_ws.split_at(i);
    if !tail
        .trim_end_matches(|c: char| c.is_ascii_whitespace())
        .is_empty()
    {
        return None;
    }
    // `parseFloat`: a trailing `.` (e.g. `30.`) is fine in JS but not for Rust's
    // f64 parser, so strip it first.
    let secs: f64 = num_str.trim_end_matches('.').parse().ok()?;
    if secs < SLEEP_BLOCK_THRESHOLD_SECS {
        return None; // sub-threshold sleeps are fine (rate limiting, pacing)
    }
    let rest = parts
        .iter()
        .skip(1)
        .map(|s| s.trim())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    let num_fmt = format_parsefloat(secs);
    if rest.is_empty() {
        Some(format!("standalone sleep {num_fmt}"))
    } else {
        Some(format!("sleep {num_fmt} followed by: {rest}"))
    }
}

/// `^[-+]?\d+(\.\d+)?$` — claude-code `VF`'s numeric-string predicate (manual,
/// no regex dep). NOTE the fractional part requires ≥1 digit after the `.`
/// (`\.\d+`), unlike the sleep regex's `\.\d*`.
#[must_use]
fn is_vf_numeric_string(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start {
        return false; // need ≥1 integer digit
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return false; // `.` must be followed by ≥1 digit
        }
    }
    i == b.len()
}

/// Resolve the `timeout` input to milliseconds — 1:1 with claude-code's `VF`
/// preprocess (numeric-string coercion) followed by `H5a` (`typeof n==="number"
/// && n>0 ? n : default`). A string matching [`is_vf_numeric_string`] is coerced
/// to a number; the value is used iff it is a finite number > 0, else the
/// default. There is NO upper clamp/rejection (the `max` in the schema's
/// `describe` text is advisory only — claude-code's schema has no `.max()`).
#[must_use]
pub fn resolve_timeout_ms(input: &Value) -> u64 {
    let as_num: Option<f64> = match input.get("timeout") {
        Some(Value::String(s)) => {
            let t = s.trim();
            if is_vf_numeric_string(t) {
                t.parse::<f64>().ok().filter(|n| n.is_finite())
            } else {
                None
            }
        }
        Some(v) => v.as_f64(),
        None => None,
    };
    match as_num {
        Some(n) if n > 0.0 => n as u64,
        _ => bash_default_timeout_ms(),
    }
}

/// Resolve the shell binary to spawn under.
///
/// Mirrors `findSuitableShell()` in `src/utils/Shell.ts`: if
/// `LINGXI_SHELL` is set to a non-empty value that contains `"bash"` or
/// `"zsh"`, return it verbatim (no executable-check — that matches the TS
/// behaviour which only validates that the path exists/is-executable, not
/// that it runs successfully). Fall back to the compile-time OS default when
/// the env var is absent, empty, or names an unsupported shell.
///
/// The return value is either the env-var string (leaked to `'static` so the
/// signature stays `&'static str`) or a compile-time constant.
#[must_use]
pub fn resolve_shell_path() -> &'static str {
    if let Ok(v) = std::env::var("LINGXI_SHELL") {
        if !v.is_empty() && (v.contains("bash") || v.contains("zsh")) {
            return Box::leak(v.into_boxed_str());
        }
    }
    if cfg!(target_os = "macos") {
        BASH_SHELL_MACOS
    } else {
        BASH_SHELL_LINUX
    }
}

// ===== BASH.3 — output-length env override ==================================

/// claude-code `outputLimits.ts` `BASH_MAX_OUTPUT_DEFAULT`.
pub const BASH_MAX_OUTPUT_DEFAULT: usize = 30_000;
/// claude-code `outputLimits.ts` `BASH_MAX_OUTPUT_UPPER_LIMIT`.
pub const BASH_MAX_OUTPUT_UPPER_LIMIT: usize = 150_000;

/// `parseInt(value, 10)` semantics: skip leading ASCII whitespace, an optional
/// sign, then consume leading ASCII digits. Returns `None` (JS `NaN`) when no
/// digit is found. Trailing non-digits are ignored (`"123abc"` ⇒ `123`).
fn parse_int_js(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        i += 1;
    }
    let mut sign: i64 = 1;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        if b[i] == b'-' {
            sign = -1;
        }
        i += 1;
    }
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None;
    }
    // Overflow (astronomically long digit run) ⇒ saturate so the upper-limit
    // cap below still applies; JS would yield a huge float here.
    match s[start..i].parse::<i64>() {
        Ok(v) => Some(sign * v),
        Err(_) => Some(sign * i64::MAX),
    }
}

/// Resolve the effective Bash output cap from a raw `BASH_MAX_OUTPUT_LENGTH`
/// value. 1:1 port of claude-code `envValidation.ts` `validateBoundedIntEnvVar`
/// (driven by `outputLimits.ts` `getMaxOutputLength`): unset/empty/`NaN`/`<= 0`
/// falls back to the default; values above the upper limit are capped.
#[must_use]
pub fn resolve_max_output_length(raw: Option<&str>) -> usize {
    let Some(value) = raw.filter(|v| !v.is_empty()) else {
        return BASH_MAX_OUTPUT_DEFAULT;
    };
    match parse_int_js(value) {
        Some(parsed) if parsed > 0 => {
            if parsed > BASH_MAX_OUTPUT_UPPER_LIMIT as i64 {
                BASH_MAX_OUTPUT_UPPER_LIMIT
            } else {
                parsed as usize
            }
        }
        _ => BASH_MAX_OUTPUT_DEFAULT,
    }
}

/// Read `BASH_MAX_OUTPUT_LENGTH` from the environment and resolve the effective
/// output cap. Mirrors claude-code `getMaxOutputLength()`.
#[must_use]
pub fn bash_max_output_length() -> usize {
    resolve_max_output_length(std::env::var("BASH_MAX_OUTPUT_LENGTH").ok().as_deref())
}

/// Port of claude-code `TFo` (`function TFo(){return
/// st(process.env.LINGXI_BASH_MAINTAIN_PROJECT_WORKING_DIR)}`): when truthy, the
/// shell cwd is ALWAYS reset to the original (workspace) after a command, even
/// for an in-workspace `cd`. `st` is the strict env-truthy allowlist
/// (`1`/`true`/`yes`/`on`) — delegated to the canonical [`traits::env::is_env_truthy`]
/// so it cannot drift. DEFAULT FALSE (unset/empty ⇒ false).
fn tfo_maintain_cwd() -> bool {
    traits::env::is_env_truthy(
        std::env::var("LINGXI_BASH_MAINTAIN_PROJECT_WORKING_DIR")
            .ok()
            .as_deref(),
    )
}

/// Apply claude-code `R0`'s `/private/var` → `/var` and `/private/tmp` → `/tmp`
/// path normalization (the macOS realpath-prefix folding) to a single path.
/// 1:1 with the two `.replace(...)` calls in `R0`:
///   `.replace(/^\/private\/var\//,"/var/").replace(/^\/private\/tmp(\/|$)/,"/tmp$1")`
/// Returns the normalized string form (lossless for our containment compare).
fn normalize_private_prefix(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("/private/var/") {
        return format!("/var/{rest}");
    }
    // `/private/tmp(\/|$)` — the `$1` keeps the trailing `/` (or end-of-string).
    if let Some(rest) = s.strip_prefix("/private/tmp/") {
        return format!("/tmp/{rest}");
    }
    if s == "/private/tmp" {
        return "/tmp".to_string();
    }
    s.into_owned()
}

/// Port of claude-code `R0(e, t)` (`caseFold:false`): "is path `e` contained in
/// directory `t`?" — the predicate `kF` applies over the allowed-dir set.
///
/// `R0` realpaths both via `Ds`, applies the `/private/var`+`/private/tmp`
/// normalization to BOTH, then computes `posix.relative(t, e)`:
///   - `""`            (same path)            ⇒ contained          → true
///   - contains `..`   (`poe`)                ⇒ outside            → false
///   - absolute        (no common base)       ⇒ outside            → false
///   - else (relative, non-`..`)              ⇒ inside             → true
///
/// `cwd` arrives already canonicalized (the readback `canon`); we canonicalize
/// `dir` to mirror `Ds`'s realpath, then fold the `/private/*` prefixes on both
/// before the prefix-containment check. caseFold is FALSE on the `kF` path
/// (case-sensitive) so we compare bytes directly.
fn is_within_allowed(cwd: &std::path::Path, dir: &std::path::Path) -> bool {
    // `Ds` realpaths; `cwd` is already `canon`. Canonicalize `dir`; if that
    // fails (dir gone), fall back to the raw `dir` so the compare still runs
    // (claude-code's `Ds` would throw and `R0`'s callers treat a throw as
    // not-contained, but a missing workspace is already handled upstream by the
    // deleted-cwd recovery — this fallback only affects an exotic race).
    let dir_canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let e = normalize_private_prefix(cwd);
    let t = normalize_private_prefix(&dir_canon);
    if e == t {
        // `posix.relative(t, e)` == "" ⇒ same dir ⇒ contained.
        return true;
    }
    // `posix.relative(t, e)` for `e` strictly inside `t` is a forward relative
    // path with no leading `..` and is not absolute. We model that with a
    // path-component prefix check: `e` starts with `t` AND the boundary is a
    // path separator (so `/tmp/foobar` is NOT "inside" `/tmp/foo`).
    let t_with_sep = if t.ends_with('/') {
        t.clone()
    } else {
        format!("{t}/")
    };
    e.starts_with(&t_with_sep)
}

/// Truncate Bash output the way claude-code `BashTool/utils.ts` `formatOutput`
/// does (`:156-158`): keep the first `max` chars, then append
/// `\n\n... [N lines truncated] ...` where `N` is the number of `\n` characters
/// in the truncated tail (`countCharInString(content, '\n', max)`) plus one.
///
/// Returns `(out, did_truncate)`. When `content` fits within `max`, it is
/// returned verbatim with `did_truncate == false`. Char-based slicing keeps
/// multibyte UTF-8 codepoints intact; newlines are ASCII so the tail line count
/// is identical to the TS UTF-16 `indexOf` walk.
#[must_use]
fn truncate_bash_output(content: String, max: usize) -> (String, bool) {
    if content.chars().count() <= max {
        return (content, false);
    }
    let head: String = content.chars().take(max).collect();
    // `remainingLines = countCharInString(content, '\n', max) + 1`: count the
    // newlines in everything after the kept head (the truncated tail).
    let remaining_lines = content.chars().skip(max).filter(|&c| c == '\n').count() + 1;
    let truncated = format!("{head}\n\n... [{remaining_lines} lines truncated] ...");
    (truncated, true)
}

/// The interrupt/abort marker appended to stderr (`BashTool.tsx:602-604`).
const ABORT_MARKER: &str = "<error>Command was aborted before completion</error>";

/// Build the MODEL-facing `tool_result` content string exactly as claude-code's
/// Bash result mapper: `content: [c, u, d].filter(Boolean).join("\n")`
/// (v2.1.185 binary offset 202593080). The model sees this plain-text string —
/// NOT a JSON dump of the structured `data` object (that object still flows to
/// the TUI / `PostToolUse` hook). Parts:
/// - `c` = stdout via `replace(/^(\s*\n)+/,"").trimEnd()` ([`normalize_stdout`]),
/// - `u` = `stderr.trim()`, plus (when `interrupted`) a `\n` separator (`rYa`,
///   only when stderr was non-empty) and the [`ABORT_MARKER`],
/// - `d` = an optional background-run note,
///
/// then the non-empty parts are joined by `\n`.
fn bash_model_content(
    stdout: &str,
    stderr: &str,
    interrupted: bool,
    background_note: Option<&str>,
) -> String {
    let c = crate::shared::normalize_stdout(stdout);
    let mut u = stderr.trim().to_string();
    if interrupted {
        if !stderr.is_empty() {
            u.push('\n');
        }
        u.push_str(ABORT_MARKER);
    }
    let mut parts: Vec<String> = Vec::new();
    if !c.is_empty() {
        parts.push(c);
    }
    if !u.is_empty() {
        parts.push(u);
    }
    if let Some(d) = background_note {
        if !d.is_empty() {
            parts.push(d.to_string());
        }
    }
    parts.join("\n")
}

/// Build the BashTool result `data` — claude-code 2.1.191 `BashTool` outputSchema
/// (pure metadata; the model-facing render rides on `ToolCallResult.model_content`,
/// NOT inside `data`). Field order mirrors the binary's `return{data:{…}}`
/// construction (`preserve_order` is on): `stdout, stderr, interrupted, isImage,
/// returnCodeInterpretation?, noOutputExpected, backgroundTaskId?`. The two
/// `?`-fields are OPTIONAL in the schema and are OMITTED when absent — the binary
/// sets them to `p?.message` / `undefined`, which `JSON.stringify` drops. The
/// telemetry-only fields LingXi used to carry here (`exit_code`, `is_error`,
/// `timed_out`, `truncated`) are NOT part of the result data — they live in the
/// `tengu`/`BASH_COMPLETED` analytics payload only.
fn bash_result_data(
    stdout: &str,
    stderr: &str,
    interrupted: bool,
    is_image: bool,
    return_code_interpretation: Option<&str>,
    no_output_expected: bool,
    background_task_id: Option<&str>,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert(
        "stdout".into(),
        serde_json::Value::String(stdout.to_string()),
    );
    m.insert(
        "stderr".into(),
        serde_json::Value::String(stderr.to_string()),
    );
    m.insert("interrupted".into(), serde_json::Value::Bool(interrupted));
    m.insert("isImage".into(), serde_json::Value::Bool(is_image));
    if let Some(rci) = return_code_interpretation {
        m.insert(
            "returnCodeInterpretation".into(),
            serde_json::Value::String(rci.to_string()),
        );
    }
    m.insert(
        "noOutputExpected".into(),
        serde_json::Value::Bool(no_output_expected),
    );
    if let Some(bid) = background_task_id {
        m.insert(
            "backgroundTaskId".into(),
            serde_json::Value::String(bid.to_string()),
        );
    }
    serde_json::Value::Object(m)
}

/// Build the SUCCESSFUL `tool_result` for a timed-out / interrupted Bash run,
/// mirroring claude-code's interrupted shape (`BashTool.tsx` ~602-605 / 720):
/// `interrupted: true`, `timed_out: true`, partial stdout normalized + truncated
/// through the same pipeline as the success arm, and the
/// `<error>Command was aborted before completion</error>` marker appended to
/// stderr (`BashTool.tsx:602-604`). `is_error` follows `interrupted` (TS
/// `is_error: interrupted`) and is therefore `true`.
/// Human-readable duration — claude-code's `qs()` in its default (no-options)
/// form: a sub-minute value is `"<floor(seconds)>s"`; otherwise the largest
/// units down, `"Xd Yh Zm"` / `"Yh Zm Ws"` / `"Zm Ws"` / `"Ws"`, with the
/// seconds field rounded and 60→carry normalization (`60s→+1m`, `60m→+1h`,
/// `24h→+1d`). Used for the timed-out-command annotation (94 call sites in the
/// binary; ported for the one the Bash tool needs).
#[must_use]
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 60_000 {
        return format!("{}s", ms / 1000);
    }
    let mut days = ms / 86_400_000;
    let mut hours = (ms % 86_400_000) / 3_600_000;
    let mut mins = (ms % 3_600_000) / 60_000;
    let mut secs = ((ms % 60_000) as f64 / 1000.0).round() as u64;
    if secs == 60 {
        secs = 0;
        mins += 1;
    }
    if mins == 60 {
        mins = 0;
        hours += 1;
    }
    if hours == 24 {
        hours = 0;
        days += 1;
    }
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m {secs}s")
    } else if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

/// Build the model-facing result for a killed command. `timeout_ms = Some(ms)`
/// marks a *timed-out* command (exit `143`/`R9c`): claude's shell prepends
/// `Command timed out after <qs(ms)>` to stderr (`uqh`, space-joined) before the
/// tool layer appends the abort marker; `None` is a plain interrupt.
fn build_interrupted_result(
    stdout_partial: &str,
    stderr_partial: &str,
    cmd_str: &str,
    timeout_ms: Option<u64>,
) -> ToolCallResult {
    let (stdout_clean, _ansi_out) = strip_ansi_count(stdout_partial);
    let (stderr_clean, _ansi_err) = strip_ansi_count(stderr_partial);
    // Timeout annotation (claude `n(\`Command timed out after ${qs(#d)}\`)` →
    // `r.stderr = uqh(msg, prev)` = `${msg} ${prev}` when prev is non-empty).
    let stderr_clean = match timeout_ms {
        Some(ms) => {
            let annotation = format!("Command timed out after {}", format_duration_ms(ms));
            if stderr_clean.is_empty() {
                annotation
            } else {
                format!("{annotation} {stderr_clean}")
            }
        }
        None => stderr_clean,
    };
    let normalized =
        crate::shared::strip_empty_lines(&crate::shared::normalize_stdout(&stdout_clean));
    // `truncated` is telemetry-only, not part of the result data — discard it.
    let (stdout_final, _truncated_out) = truncate_bash_output(normalized, bash_max_output_length());

    // claude-code appends the abort marker to stderr, preceded by EOL when
    // stderr is non-empty (`BashTool.tsx:602-604`).
    let mut stderr_final = stderr_clean.trim_end().to_string();
    if !stderr_final.is_empty() {
        stderr_final.push('\n');
    }
    stderr_final.push_str(ABORT_MARKER);

    // Model-facing render: `[c, u(+abort marker)].join("\n")` (the binary appends
    // the abort marker inside the result mapper, so build it from the pre-marker
    // stderr with `interrupted = true`).
    let model_content = bash_model_content(&stdout_final, &stderr_clean, true, None);

    ToolCallResult {
        // A killed/interrupted command: `interrupted: true`. No exit code or
        // `returnCodeInterpretation` on kill (the binary's `p?.message` is absent).
        data: bash_result_data(
            &stdout_final,
            &stderr_final,
            true,
            false,
            None,
            crate::silent::is_silent_bash_command(cmd_str),
            None,
        ),
        model_content: Some(model_content),
        new_messages: vec![],
        context_modifier: None,
        is_error: false,
        mcp_meta: None,
    }
}

fn invalidate_written_read_state(ctx: &BuiltinToolContext, cwd: &std::path::Path, command: &str) {
    let paths = crate::command_semantics::parsed_written_paths(command);
    if paths.is_empty() {
        return;
    }
    let Ok(mut state) = ctx.read_file_state.lock() else {
        return;
    };
    for raw in paths {
        let path = std::path::Path::new(&raw);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        state.remove(&absolute);
    }
}

// ===== Image-output handling (claude-code `BashTool/utils.ts`) ==============

/// True when `content` is a base64 image data URI. 1:1 port of claude-code
/// `isImageOutput` (`BashTool/utils.ts:49-50`):
/// `/^data:image\/[a-z0-9.+_-]+;base64,/i`. Callers pass the trimmed,
/// model-facing stdout (matching TS, which tests `stripEmptyLines(stdout)`).
#[must_use]
fn is_image_output(content: &str) -> bool {
    // ASCII, case-insensitive — mirror the `i` flag. Manual scan avoids a regex
    // dep tool-shell doesn't pull in.
    let prefix = b"data:image/";
    let b = content.as_bytes();
    if b.len() < prefix.len() {
        return false;
    }
    if !b[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return false;
    }
    // `[a-z0-9.+_-]+` (the image subtype) — at least one char.
    let mut i = prefix.len();
    let start = i;
    while i < b.len() {
        let c = b[i];
        // The TS class is case-insensitive via the `i` flag, so accept A-Z too.
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'_' | b'-') {
            i += 1;
        } else {
            break;
        }
    }
    if i == start {
        return false; // empty subtype
    }
    // Followed by the literal `;base64,`.
    const SEP: &[u8] = b";base64,";
    b.len() >= i + SEP.len() && &b[i..i + SEP.len()] == SEP
}

/// Parse a `data:<media_type>;base64,<payload>` URI into `(media_type, payload)`.
/// 1:1 port of claude-code `parseDataUri` (`BashTool/utils.ts:53-65`):
/// `/^data:([^;]+);base64,(.+)$/`. Input is trimmed before matching. Returns
/// `None` when it doesn't match (so callers fall through to text handling).
/// The returned payload is already valid base64 of the image bytes, so it can
/// be handed straight to [`protocol::ImageSource::Base64`] with no re-encode.
#[must_use]
fn parse_data_uri(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let rest = s.strip_prefix("data:")?;
    // media_type = `[^;]+` up to the first `;`.
    let semi = rest.find(';')?;
    if semi == 0 {
        return None; // empty media type
    }
    let media_type = &rest[..semi];
    // Must be exactly `;base64,` after the media type, then a non-empty payload
    // (`(.+)$`).
    let payload = rest[semi..].strip_prefix(";base64,")?;
    if payload.is_empty() {
        return None;
    }
    Some((media_type.to_string(), payload.to_string()))
}

// ===== BASH.1 — extended-glob disable prefix (SECURITY) =====================

/// Return the shell command that disables extended-glob expansion for the
/// given shell, or `None` for an unknown shell. 1:1 port of claude-code
/// `bashProvider.ts` `getDisableExtglobCommand`.
///
/// Extended globs (bash `extglob`, zsh `EXTENDED_GLOB`) can be exploited via
/// malicious filenames that expand *after* our security validation, so this
/// prefix is prepended to the user command before it is spawned.
#[must_use]
pub fn disable_extglob_command(shell_path: &str) -> Option<String> {
    // When LINGXI_SHELL_PREFIX is set, the wrapper may run a different
    // shell than `shell_path`, so emit commands for BOTH shells. Redirect
    // stdout+stderr because zsh's `command_not_found_handler` writes to stdout.
    if std::env::var("LINGXI_SHELL_PREFIX").is_ok_and(|v| !v.is_empty()) {
        return Some(
            "{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true".into(),
        );
    }
    if shell_path.contains("bash") {
        Some("shopt -u extglob 2>/dev/null || true".into())
    } else if shell_path.contains("zsh") {
        Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true".into())
    } else {
        // Unknown shell — we don't know the right command.
        None
    }
}

// ===== BASH.2 — Windows null-redirect rewrite ===============================

#[inline]
fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Rewrite Windows CMD-style `>nul` redirects to POSIX `/dev/null`. 1:1 port of
/// claude-code `shellQuoting.ts` `rewriteWindowsNullRedirect`, implementing the
/// regex `/(\d?&?>+\s*)[Nn][Uu][Ll](?=\s|$|[|&;)\n])/g` → `$1/dev/null`
/// (no `regex` crate dep; ASCII whitespace approximates JS `\s`).
///
/// Matches `>nul`, `> NUL`, `2>nul`, `&>nul`, `>>nul` (case-insensitive); does
/// NOT match `>null`, `>nullable`, `>nul.txt`, or `cat nul.txt`.
#[must_use]
pub fn rewrite_windows_null_redirect(command: &str) -> String {
    let b = command.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut p = 0;
    while p < n {
        if let Some(group1_end) = match_null_redirect(b, p) {
            // group1 = b[p..group1_end] (the `\d?&?>+\s*` prefix), then `nul`.
            out.extend_from_slice(&b[p..group1_end]);
            out.extend_from_slice(b"/dev/null");
            p = group1_end + 3; // skip the matched `nul`
        } else {
            out.push(b[p]);
            p += 1;
        }
    }
    // Replacements only ever touch ASCII spans, so the bytes remain valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| command.to_owned())
}

/// Try to match `(\d?&?>+\s*)nul(?=\s|$|[|&;)\n])` at byte offset `p`. On
/// success returns the byte offset where `nul` begins (i.e. the end of
/// group 1); the caller knows the literal `nul` is exactly 3 bytes.
fn match_null_redirect(b: &[u8], p: usize) -> Option<usize> {
    let n = b.len();
    let mut q = p;
    // \d? — optional single digit
    if q < n && b[q].is_ascii_digit() {
        q += 1;
    }
    // &? — optional single ampersand
    if q < n && b[q] == b'&' {
        q += 1;
    }
    // >+ — one or more redirects (required)
    let gt_start = q;
    while q < n && b[q] == b'>' {
        q += 1;
    }
    if q == gt_start {
        return None;
    }
    // \s* — optional whitespace
    while q < n && is_ascii_ws(b[q]) {
        q += 1;
    }
    let nul_start = q;
    // nul (case-insensitive), exactly 3 chars
    if nul_start + 3 > n {
        return None;
    }
    if !(b[nul_start].eq_ignore_ascii_case(&b'n')
        && b[nul_start + 1].eq_ignore_ascii_case(&b'u')
        && b[nul_start + 2].eq_ignore_ascii_case(&b'l'))
    {
        return None;
    }
    let after = nul_start + 3;
    // Lookahead: \s | end-of-string | [|&;)\n]
    let boundary = after == n
        || is_ascii_ws(b[after])
        || matches!(b[after], b'|' | b'&' | b';' | b')' | b'\n');
    if !boundary {
        return None;
    }
    Some(nul_start)
}

/// Compute the per-task output file path used when `run_in_background=true`.
///
/// Mirrors `platform_posix::process::task_output_path` (which we
/// cannot depend on from this crate without forming a Cargo cycle —
/// lingxi-tools → lingxi-platform-posix → lingxi-lsp → lingxi-tools).
#[must_use]
pub fn task_output_path(task_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("lingxi-task-output")
        .join(format!("{task_id}.out"))
}

fn ephemeral_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mix = (nanos as u64) ^ u64::from(std::process::id());
    format!("{prefix}-{mix:016x}")
}

fn cmd_hash(s: &str) -> String {
    // Lightweight FNV-1a; cryptographic strength not required — used solely
    // for `_PROTO_command_hash` routing in telemetry.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

async fn emit_failed(
    bus: &telemetry::AnalyticsBus,
    request_id: &str,
    error_kind: &str,
    started_at: SystemTime,
) {
    let elapsed_ms = SystemTime::now()
        .duration_since(started_at)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut meta: LogEventMetadata = HashMap::new();
    meta.insert(
        "request_id".into(),
        AnalyticsValue::String(request_id.into()),
    );
    meta.insert(
        "error_kind".into(),
        AnalyticsValue::String(error_kind.into()),
    );
    meta.insert("duration_ms".into(), AnalyticsValue::Int(elapsed_ms as i64));
    bus.log_event(BASH_FAILED, meta).await;
}

// ===== BASH.7 — model-gated prompt selector (`Dh` predicate) ===============
//
// claude-code builds the Bash tool's model-facing prompt as
// `getSimplePrompt(model) = Dh(model) ? qUp(/*CONCISE*/) : TXa(/*VERBOSE*/)`.
// `Dh` (binary offset ~195159752) is the "simple system prompt" gate that
// selects the SHORT (current-gen) vs LONG (classic) variant. It is the shared
// `tool_api::dh_simple_system_prompt` (single source of truth in
// `tool-api/src/model_prompt_gate.rs`), consulted identically by the WebSearch
// and file/task tools — the `UWu`/`dfe`/`FWu` parity notes live there. The
// Bash `prompt()` method calls it directly at its model gate.

// ===== Tool type ============================================================

/// `BashTool` — spawn a shell command through the configured `ProcessRunner`,
/// optionally wrapping it via the M2-04 sandbox decision matrix.
#[derive(Clone)]
pub struct BashTool {
    ctx: BuiltinToolContext,
    /// Persistent shell working directory (claude-code `STATE.cwd`).
    ///
    /// A foreground `cd` updates this via a `pwd -P` readback after the
    /// command runs, so subsequent Bash calls inherit the new directory
    /// (the tool registry holds one long-lived `Arc<BashTool>` per session,
    /// so this `Arc<Mutex<..>>` field persists across calls — BASH.4 Design B).
    ///
    /// Design-B local-to-`BashTool` divergence: claude-code keeps this
    /// session-global (`STATE.cwd`) and shares it with the permission gate; we
    /// keep it `BashTool`-local until a permission-gate live-cwd consumer is
    /// wired. Observable behavior is identical today since nothing else
    /// consumes a shared session-cwd yet.
    shell_cwd: std::sync::Arc<std::sync::Mutex<std::path::PathBuf>>,
    /// Optional `CwdChanged` hook firer (default `None` => strict no-op).
    ///
    /// Closes the seam claude-code's `onCwdChangedForHooks(cwd, newCwd)`
    /// (`Shell.ts:409`) fills: when the `pwd -P` readback below moves the cwd,
    /// fire the `CwdChanged` hook best-effort. The firer can NOT ride the
    /// construction `BuiltinToolContext` (that struct is built by a full literal
    /// in the FORBIDDEN `engine-mobile` code — a new field would break it), so
    /// it is injected via [`with_cwd_changed_firer`](BashTool::with_cwd_changed_firer),
    /// leaving the `BashTool::new` signature unchanged. The desktop composition
    /// root wires it over the shared `Arc<HookExecutorImpl>`; every other caller
    /// (mobile, tests) leaves it `None` and the fire is a no-op.
    cwd_changed_firer: hooks::OptionalCwdChangedFirer,
}

impl BashTool {
    /// Construct a fresh tool bound to the given builtin context.
    ///
    /// The `CwdChanged` firer defaults to `None` (no fire). Inject one via
    /// [`with_cwd_changed_firer`](BashTool::with_cwd_changed_firer) — the
    /// signature here is deliberately UNCHANGED so the `engine-mobile` literal
    /// and every other `BashTool::new(ctx)` caller keep compiling untouched.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        let shell_cwd = std::sync::Arc::new(std::sync::Mutex::new(ctx.workspace.clone()));
        Self {
            ctx,
            shell_cwd,
            cwd_changed_firer: None,
        }
    }

    /// Inject the `CwdChanged` hook firer (builder; default is `None`).
    ///
    /// The desktop composition root calls this over the SAME
    /// `Arc<HookExecutorImpl>` the orchestrator fires its other hooks through,
    /// so a `cd` inside a Bash call fires the `CwdChanged` hook
    /// (`onCwdChangedForHooks`, `Shell.ts:409`). Optional + additive: callers
    /// that skip it (mobile, tests) keep the strict no-op.
    #[must_use]
    pub fn with_cwd_changed_firer(
        mut self,
        firer: std::sync::Arc<dyn hooks::CwdChangedFirer>,
    ) -> Self {
        self.cwd_changed_firer = Some(firer);
        self
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "command":           { "type": "string", "description": "The command to execute" },
            // claude-code `BashTool.tsx:229` names this param `timeout`
            // (milliseconds). A model sending `timeout` must be honored.
            // claude-code: `VF(E.number().optional()).describe(...)` — NO `.max()`
            // (the "(max …)" is advisory describe text only). The max is the
            // dynamic `Wdt()`=`j3n()` value, env-overridable via BASH_MAX_TIMEOUT_MS.
            "timeout":           {
                "type": "number",
                "description": format!("Optional timeout in milliseconds (max {})", bash_max_timeout_ms())
            },
            "run_in_background": { "type": "boolean", "description": "Set to true to run this command in the background." },
            "description":       { "type": "string", "description": "Clear, concise description of what this command does in active voice. Never use words like \"complex\" or \"risk\" in the description - just describe what it does.\n\nFor simple commands (git, npm, standard CLI tools), keep it brief (5-10 words):\n- ls → \"List files in current directory\"\n- git status → \"Show working tree status\"\n- npm install → \"Install package dependencies\"\n\nFor commands that are harder to parse at a glance (piped commands, obscure flags, etc.), add enough context to clarify what it does:\n- find . -name \"*.tmp\" -exec rm {} \\; → \"Find and delete all .tmp files recursively\"\n- git reset --hard origin/main → \"Discard all local changes and match remote main\"\n- curl -s url | jq '.data[]' → \"Fetch JSON from URL and extract data array elements\"" },
            // BASH.5: 1:1 with claude-code `BashTool.tsx` schema —
            // `dangerouslyDisableSandbox: z.boolean().optional().describe(...)`.
            "dangerouslyDisableSandbox": {
                "type": "boolean",
                "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."
            }
        },
        "required": ["command"],
        // R-MINOR: claude-code's Bash input schema is `E.strictObject(...)`
        // (additionalProperties:false) — unknown keys are rejected.
        "additionalProperties": false
    })
});

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("execute shell commands")
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        MAX_TOOL_OUTPUT_LENGTH
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // claude-code `BashTool.tsx:434-436`: `isConcurrencySafe` delegates to
        // `isReadOnly`.
        self.is_read_only(input)
    }

    fn is_read_only(&self, input: &Value) -> bool {
        // claude-code `BashTool.tsx:437-441`: derive from
        // `checkReadOnlyConstraints(input, commandHasAnyCd(input.command))`,
        // read-only iff `result.behavior === 'allow'`.
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return false;
        };
        let compound_has_cd = crate::read_only::command_has_any_cd(command);
        crate::read_only::check_read_only(command, compound_has_cd).is_read_only()
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-02 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _opts: &DescriptionOptions) -> String {
        // claude-code `BashTool.tsx:426-429`: `return description || 'Run shell
        // command'`. A present-but-empty `description` falls through to the
        // default (JS `||` is falsy on `''`); the command is NOT used.
        match input.get("description").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => "Run shell command".into(),
        }
    }

    async fn prompt(&self, opts: &PromptOptions) -> String {
        // BASH.6/BASH.7 — 1:1 with claude-code
        // `getSimplePrompt(model) = Dh(model) ? qUp(/*CONCISE*/) : TXa(/*VERBOSE*/)`.
        // The `Dh(model)` "simple system prompt" gate (binary @~195159752)
        // selects the SHORT current-gen variant vs the LONG classic one; the
        // session/subagent model is threaded via `PromptOptions::model`, and
        // `None` mirrors the binary's `Dh(undefined)` → LONG. BOTH variants are
        // driven by the live sandbox runtime config on this context (the SHORT
        // `qUp` calls the SAME `yXa()`/`sandbox_section`).
        // (/sandbox) Use the effective config so the prompt reflects the live
        // toggle (the frozen config until `/sandbox` flips the shared cell).
        let sandbox_runtime = self.ctx.effective_sandbox_runtime();
        if tool_api::dh_simple_system_prompt(opts.model.as_deref()) {
            crate::prompt::simple_prompt_concise(&sandbox_runtime)
        } else {
            crate::prompt::simple_prompt(&sandbox_runtime)
        }
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let cmd = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("missing `command`".into()))?;
        if cmd.is_empty() {
            return Err(ValidationError("`command` must not be empty".into()));
        }
        // claude-code's bash `timeout` schema is `VF(E.number().optional())` with
        // NO `.max()` — the resolver `H5a` accepts any number > 0, so an
        // over-`max` value is honored (the schema `describe` text's "(max
        // 600000)" is advisory only). The prior >max rejection is removed (#4).
        //
        // claude-code `validateInput`: block bare `sleep N` (N>=25) when NOT run
        // in the background — BUT only when `sq()=ct("tengu_amber_sentinel",
        // false)` is enabled (default false → inert, like stock claude-code).
        // The message is byte-identical to the binary; `errorCode: 10` has no
        // Rust surface and is dropped.
        let run_bg = input
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if sleep_block_enabled() && !run_bg {
            if let Some(pattern) = detect_blocked_sleep_pattern(cmd) {
                return Err(ValidationError(format!(
                    "Blocked: {pattern}. To wait for a condition, use Monitor with an until-loop (e.g. `until <check>; do sleep 2; done`). To wait for a command you started, use run_in_background: true. Do not chain shorter sleeps to work around this block."
                )));
            }
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        use sandbox::decision::{should_use_sandbox, SandboxDecision};
        use traits::sandbox::ProcessCommand as SbxCommand;

        // PHASE-2: tool-abort `CancellationToken` threaded in by the streaming
        // executor (a child of `tool_abort`). When the turn is discarded
        // (streaming fallback) or the user interrupts, the executor fires it; the
        // foreground run below races against it and, on cancel, drops the `run`
        // future — `kill_on_drop` SIGKILLs the subprocess — returning `Aborted`
        // so the executor substitutes the synthetic abort block. `None` for every
        // non-streaming caller, in which case the run is awaited directly (no
        // behavior change).
        let cancel = ctx.cancel.clone();

        let cmd_str = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing command".into()))?
            .to_string();
        // claude-code `BashTool.tsx` sends the timeout as `timeout` (ms). Resolve
        // it with the faithful `VF` (numeric-string coercion) + `H5a` (use iff a
        // finite number > 0, else default) logic — no upper clamp/rejection (#4/#5).
        let timeout_ms = resolve_timeout_ms(&input);
        let run_bg = input
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // BASH.5: optional `dangerouslyDisableSandbox` override.
        let dangerously_disable_sandbox = input
            .get("dangerouslyDisableSandbox")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if cfg!(target_os = "windows") {
            return Err(ToolError::InvalidInput(
                "Bash is not supported on Windows; use PowerShellTool".into(),
            ));
        }

        let started_at = SystemTime::now();
        let request_id = ephemeral_id("bash");
        let hash = cmd_hash(&cmd_str);

        // ===== Start telemetry =====
        let mut meta_start: LogEventMetadata = HashMap::new();
        meta_start.insert(
            "request_id".into(),
            AnalyticsValue::String(request_id.clone()),
        );
        meta_start.insert("_PROTO_command_hash".into(), AnalyticsValue::String(hash));
        meta_start.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
        meta_start.insert("run_in_background".into(), AnalyticsValue::Bool(run_bg));
        self.ctx.bus.log_event(BASH_STARTED, meta_start).await;

        // ===== Sandbox decision =====
        // 1:1 with claude-code `shouldUseSandbox.ts`. The `dangerouslyDisableSandbox`
        // override and the empty-command / excluded-command short-circuits all live
        // INSIDE `should_use_sandbox` now; we just feed it the inputs.
        // `unsandboxed_allowed` is `SandboxManager.areUnsandboxedCommandsAllowed()`,
        // mapped to the canonical `are_unsandboxed_commands_allowed()` accessor (the
        // same mapping used by the BASH.6 prompt section).
        // (/sandbox) Resolve the effective sandbox config ONCE for this command:
        // the frozen `sandbox_runtime` with `enabled` overridden by the live
        // `/sandbox` toggle cell when wired. This is THE read that makes the
        // toggle affect the wrap decision (`should_use_sandbox` keys off
        // `cfg.enabled`); the excluded-commands / allow-unsandboxed values are
        // unchanged, so they stay consistent with the frozen config.
        let sandbox_runtime = self.ctx.effective_sandbox_runtime();
        let decision = should_use_sandbox(
            &cmd_str,
            self.ctx.sandbox_available,
            dangerously_disable_sandbox,
            sandbox_runtime.are_unsandboxed_commands_allowed(),
            &sandbox_runtime,
            self.ctx.workspace.clone(),
        );

        let shell = resolve_shell_path().to_string();

        // BASH.2: defensively rewrite Windows CMD-style `2>nul` redirects to
        // POSIX `/dev/null` before the command is spawned (claude-code
        // `bashProvider.ts` calls `rewriteWindowsNullRedirect(command)` to
        // produce the `normalizedCommand` that is then eval'd).
        //
        // BASH.1 (SECURITY): prepend the extglob-disable prefix so extended
        // globs cannot expand malicious filenames after security validation
        // (claude-code `bashProvider.ts` pushes `getDisableExtglobCommand`
        // before the user command in the `&&`-joined `commandParts`). The
        // prefix is injected INTO the command that becomes `inner_cmd` so it
        // runs in the SAME shell that expands the user's globs — inside the
        // sandbox wrap for the sandbox path, or directly in the login shell for
        // the no-sandbox path — mirroring the TS order `disableExtglob && <cmd>`.
        let normalized_cmd = rewrite_windows_null_redirect(&cmd_str);
        let spawn_cmd = match disable_extglob_command(&shell) {
            Some(prefix) => format!("{prefix} && {normalized_cmd}"),
            None => normalized_cmd,
        };

        let inner_cmd = match decision {
            SandboxDecision::NoSandbox => spawn_cmd.clone(),
            SandboxDecision::Sandbox { policy: _ } => {
                // Wrap through the injected async `SandboxRunner`. The default
                // `LegacyWrapRunner` forwards straight to the sync
                // `wrap_with_sandbox` (ignoring `bin_shell`/`cwd`), so this is
                // byte-identical to the previous direct call; a live runner uses
                // the shell + workspace cwd to scope the sandbox.
                match self
                    .ctx
                    .sandbox_runner
                    .wrap(
                        &spawn_cmd,
                        &sandbox_runtime,
                        self.ctx.platform,
                        Some(&shell),
                        Some(self.ctx.workspace.as_path()),
                    )
                    .await
                {
                    Ok(wrapped) => wrapped,
                    Err(sandbox::wrap::SandboxWrapError::Unsupported(s)) => {
                        emit_failed(&self.ctx.bus, &request_id, "sandbox_refused", started_at)
                            .await;
                        return Err(ToolError::InvalidInput(s));
                    }
                    Err(sandbox::wrap::SandboxWrapError::SbplWrite(s)) => {
                        emit_failed(
                            &self.ctx.bus,
                            &request_id,
                            "sandbox_wrap_failed",
                            started_at,
                        )
                        .await;
                        return Err(ToolError::Io(s));
                    }
                }
            }
        };

        // ===== Background path (UNCHANGED — TS `!result.backgroundTaskId`) =====
        // Background tasks never mutate the shared cwd, so they skip the
        // `pwd -P` readback entirely and keep spawning under the workspace.
        if run_bg {
            let pcmd = SbxCommand {
                command: shell,
                // BASH.4: login-shell init (see the foreground site) — `-l`
                // after `-c`, matching `bashProvider.ts:201-205` with the
                // snapshot path deferred.
                args: vec!["-c".into(), "-l".into(), inner_cmd],
                cwd: Some(self.ctx.workspace.clone()),
                env: HashMap::new(),
                timeout: Some(Duration::from_millis(timeout_ms)),
                stdin: None,
            };
            let sandboxed = self.ctx.sandbox.bypass_with_audit(pcmd, "bash_tool_call");
            return match self.ctx.process.spawn_background(&sandboxed).await {
                Ok(handle) => {
                    let out_path = task_output_path(&handle.task_id).display().to_string();
                    // Model-facing background note (`d` in the binary's mapper):
                    // `Command running in background with ID: … Output is being
                    // written to: … use Read on that file path.` (offset 183106320).
                    let note = format!(
                        "Command running in background with ID: {}. Output is being written to: {}. You will be notified when it completes. To check interim output, use Read on that file path.",
                        handle.task_id, out_path
                    );
                    let model_content = bash_model_content("", "", false, Some(&note));
                    Ok(ToolCallResult {
                        // Backgrounded launch: the binary's result data is the
                        // main shape with empty stdout/stderr + `backgroundTaskId`
                        // (the ID + output path ride in the model note, NOT data).
                        data: bash_result_data(
                            "",
                            "",
                            false,
                            false,
                            None,
                            crate::silent::is_silent_bash_command(&cmd_str),
                            Some(&handle.task_id),
                        ),
                        model_content: Some(model_content),
                        new_messages: vec![],
                        context_modifier: None,
                        is_error: false,
                        mcp_meta: None,
                    })
                }
                Err(e) => {
                    emit_failed(&self.ctx.bus, &request_id, "spawn_failed", started_at).await;
                    Err(ToolError::Io(format!("{e}")))
                }
            };
        }

        // ===== Foreground: BASH.4 persistent cwd =====
        // A subagent isolated in a worktree (`isolation:"worktree"`) or given an
        // explicit `cwd` runs EACH command in that directory, reset per call
        // (claude-code's "Agent threads always have their cwd reset between bash
        // calls"), WITHOUT touching the shared persistent shell cwd (which the
        // main loop owns). The main loop (`ctx.cwd` None) reads the live
        // persistent shell cwd as before.
        let agent_cwd = ctx.cwd.clone();
        let cwd = match &agent_cwd {
            Some(c) => c.clone(),
            None => self.shell_cwd.lock().unwrap().clone(),
        };
        // Deleted-cwd recovery (Shell.ts:220-238): if the live cwd no longer
        // exists on disk (e.g. a prior command deleted its own dir), fall back
        // to the tool workspace (TS `getOriginalCwd`); if that is also gone,
        // fail with the byte-locked message.
        let cwd = if std::fs::canonicalize(&cwd).is_ok() {
            cwd
        } else {
            let workspace = self.ctx.workspace.clone();
            if std::fs::canonicalize(&workspace).is_ok() {
                // Only the main loop persists the recovered cwd to the shared
                // shell; a subagent's cwd is per-call.
                if agent_cwd.is_none() {
                    self.shell_cwd.lock().unwrap().clone_from(&workspace);
                }
                workspace
            } else {
                return Err(ToolError::Internal(format!(
                    "Working directory \"{}\" no longer exists. Please restart Claude from an existing directory.",
                    cwd.display()
                )));
            }
        };

        // Internal cwd-tracking temp file (not model-facing): the shell writes
        // its physical cwd here via `pwd -P` once the user command succeeds.
        let cwd_file = std::env::temp_dir().join(format!("claude-{request_id}-cwd"));
        let q = format!(
            "'{}'",
            cwd_file.display().to_string().replace('\'', "'\\''")
        );
        // Append the readback after the (possibly sandbox-wrapped) command
        // (bashProvider.ts:185-187). `&&` skips the readback when the user
        // command fails (cwd left unchanged — correct); `>|` overrides
        // noclobber and is valid in both bash and zsh.
        let fg_cmd = format!("{inner_cmd} && pwd -P >| {q}");

        let pcmd = SbxCommand {
            command: shell,
            // BASH.4: login-shell init so the shell is "initialized from the
            // user's profile" (the prompt's claim). 1:1 with `bashProvider.ts`
            // `getSpawnArgs` (`:201-205`): `['-c', ...(skipLoginShell ? [] :
            // ['-l']), cmd]`. TS skips `-l` only when a shell SNAPSHOT exists;
            // the snapshot mechanism is deferred here, so `lastSnapshotFilePath`
            // is always undefined ⇒ `skipLoginShell == false` ⇒ `-l` always
            // added (TS's no-snapshot login-shell fallback, `:88-93`).
            args: vec!["-c".into(), "-l".into(), fg_cmd],
            cwd: Some(cwd.clone()),
            env: HashMap::new(),
            timeout: Some(Duration::from_millis(timeout_ms)),
            stdin: None,
        };
        let sandboxed = self.ctx.sandbox.bypass_with_audit(pcmd, "bash_tool_call");

        // ===== Foreground spawn =====
        // PHASE-2: race the run against the sibling cancel token. On cancel the
        // `run` future is dropped — the posix `ProcessRunner` set
        // `kill_on_drop(true)`, so the child is SIGKILLed — and we return
        // `Aborted`; the streaming executor substitutes the synthetic
        // sibling-cancel result. With no token, await the run directly.
        let run_fut = self.ctx.process.run(&sandboxed);
        let run_result = match &cancel {
            Some(token) => {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        return Err(ToolError::Aborted);
                    }
                    res = run_fut => res,
                }
            }
            None => run_fut.await,
        };
        // The wrapped command has finished: tear down any per-command sandbox
        // state (e.g. bwrap mount points). No-op for the default
        // `LegacyWrapRunner`; only the live runner has anything to clean up.
        self.ctx.sandbox_runner.cleanup_after_command().await;
        // claude-code surfaces a timed-out/interrupted command as a SUCCESSFUL
        // result with `interrupted: true` plus whatever partial output it
        // produced (`BashTool.tsx` ~602-605 / 720), NOT a hard error. Both
        // timeout arms below build such a result via `build_interrupted_result`.
        // `format_timeout_error` / `BASH_TIMEOUT_ERROR_TEMPLATE` are retained
        // (still referenced by the locked-constant test) but no longer returned.
        match run_result {
            Ok(out) if out.timed_out => {
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert(
                    "request_id".into(),
                    AnalyticsValue::String(request_id.clone()),
                );
                meta.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
                self.ctx.bus.log_event(BASH_TIMEOUT, meta).await;
                // The streaming/stub runner may carry partial stdout/stderr
                // captured before the kill — surface it (claude-code attaches
                // whatever the accumulator held).
                Ok(build_interrupted_result(
                    &out.stdout,
                    &out.stderr,
                    &cmd_str,
                    Some(timeout_ms),
                ))
            }
            Ok(out) => {
                // BASH.4 cwd readback (Shell.ts:395-419). Subagents must NOT
                // mutate the shared cwd — TS `preventCwdChanges = !isMainThread`.
                // The main thread has no `agent_id`; a subagent call carries one.
                let prevent_cwd_changes = ctx.agent_id.is_some();
                // claude-code `J2n` (the cwd-reset gate): when the readback shows
                // the shell navigated OUTSIDE the allowed dirs — or the
                // `LINGXI_BASH_MAINTAIN_PROJECT_WORKING_DIR` env forces it — chdir
                // the shell back to the original (workspace) cwd and warn. We do
                // not chdir a real shell (the cwd is reasserted by the next call's
                // `cwd:` spawn arg), so "reset" here = keep `shell_cwd` at the
                // workspace instead of advancing it to the new dir, plus append
                // the `Y2n` warning to stderr below. `Some(workspace)` => a reset
                // warning must be appended; `None` => no reset.
                let mut cwd_reset_warning: Option<std::path::PathBuf> = None;
                if !prevent_cwd_changes {
                    if let Ok(contents) = std::fs::read_to_string(&cwd_file) {
                        let trimmed = contents.trim();
                        if !trimmed.is_empty() {
                            let new_cwd = std::path::PathBuf::from(trimmed);
                            if new_cwd != cwd {
                                if let Ok(canon) = std::fs::canonicalize(&new_cwd) {
                                    // claude-code `J2n` decision (the bash result
                                    // path's `if(l){if(J2n(Fr(t)))h=Y2n("")}`):
                                    //   J2n: `t=Pt()` (cwd after the command =
                                    //   `canon`), `n=Ar()` (original = workspace),
                                    //   reset when `TFo() || (t!==n && !kF(t))`.
                                    // We compare `canon` to the WORKSPACE (not the
                                    // prior `cwd`), so this is correct across
                                    // multiple calls: `is_within_allowed` returns
                                    // true when `canon == workspace` (subsuming
                                    // J2n's `t!==n` equality guard — a `cd` back to
                                    // the workspace is NOT a reset) and true when
                                    // `canon` is inside it (`!kF` false). `kF`'s
                                    // allowed set is the workspace (TS `b$` also
                                    // folds in `additionalWorkingDirectories` — see
                                    // the documented residual; the common
                                    // no-additional-dirs case is faithful).
                                    let force = tfo_maintain_cwd();
                                    let should_reset =
                                        force || !is_within_allowed(&canon, &self.ctx.workspace);
                                    if should_reset {
                                        // RESET (J2n fires): chdir back to original.
                                        // We do NOT advance `shell_cwd` to `canon`;
                                        // reset it to the workspace (TS `x_(n)`).
                                        // Do NOT fire `CwdChanged` — the cwd did not
                                        // really move from the model's view.
                                        // A subagent's cwd is per-call (no shared
                                        // persistent shell) — never mutate it.
                                        if agent_cwd.is_none() {
                                            (*self.shell_cwd.lock().unwrap())
                                                .clone_from(&self.ctx.workspace);
                                        }
                                        // `Y2n` appends `\nShell cwd was reset to
                                        // {Pt()}` where `Pt()` is the cwd AFTER the
                                        // chdir-back == the workspace.
                                        cwd_reset_warning = Some(self.ctx.workspace.clone());
                                        // `j("tengu_bash_tool_reset_to_original_dir")`
                                        // fires ONLY on the non-TFo branch
                                        // (`if(!r)`). It is NOT a registered
                                        // tengu/ALL_EVENT_NAMES const — emit it as
                                        // an inline tracing event so the registry
                                        // stays at 347.
                                        if !force {
                                            tracing::info!(
                                                event = "tengu_bash_tool_reset_to_original_dir"
                                            );
                                        }
                                    } else {
                                        // UPDATE (J2n does not fire): today's
                                        // behavior exactly. Update the persistent
                                        // cwd, then drop the guard BEFORE the
                                        // best-effort hook fire (no mutex held
                                        // across an `.await`). `clone_from` keeps
                                        // `canon` owned for the fire below.
                                        // A subagent's cwd is per-call — do not
                                        // advance the shared persistent shell.
                                        if agent_cwd.is_none() {
                                            (*self.shell_cwd.lock().unwrap()).clone_from(&canon);
                                        }
                                        // BASH.4 `onCwdChangedForHooks(cwd, newCwd)`
                                        // (Shell.ts:409): fire the `CwdChanged` hook
                                        // when the cwd actually moved. We are already
                                        // inside `new_cwd != cwd`, and the firer
                                        // re-asserts `old != new` (claude-code's
                                        // `oldCwd !== newCwd` guard,
                                        // fileChangedWatcher.ts:137). Best-effort:
                                        // a no-op when no firer is registered
                                        // (mobile / plain `new`) or when the hook is
                                        // absent — never breaks the cwd update.
                                        if let Some(firer) = &self.cwd_changed_firer {
                                            firer
                                                .fire(hooks::CwdChangedFire {
                                                    old: cwd.clone(),
                                                    new: canon,
                                                })
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Always clean up the tracking file (Shell.ts:415-419).
                let _ = std::fs::remove_file(&cwd_file);

                let (stdout_clean, ansi_dropped_out) = strip_ansi_count(&out.stdout);
                let (stderr_clean, ansi_dropped_err) = strip_ansi_count(&out.stderr);
                // claude-code `Y2n` (the cwd-reset warning): when `J2n` fired, the
                // bash result path sets `d=Y2n("")` == `"\nShell cwd was reset to
                // {cwd-after-chdir}"` (Y2n's arg is the EMPTY string; `"".trim()`
                // == "" so the warning is exactly a leading-newline sentence).
                //
                // BINARY DIVERGENCE (documented residual): claude-code's BASH tool
                // result `stderr` field is JUST `d` — the command's stderr folds
                // into stdout via the shell (`stderr_length:0` in its
                // `tengu_bash_tool_command_executed`), so there is never any
                // pre-existing stderr to combine with. LingXi surfaces the
                // command's stderr in the result `stderr` field (a PRE-EXISTING
                // design difference, NOT introduced by this finding), so we append
                // the warning to the already-cleaned `stderr_clean` — the only
                // stderr consumer downstream (the image short-circuit and both text
                // result blocks all read this binding). In claude-code's only real
                // case (empty command stderr) this is byte-identical: an empty
                // `stderr_clean` trims to "" giving exactly `\nShell cwd was reset
                // to {workspace}` == `Y2n("")`.
                let stderr_clean = match &cwd_reset_warning {
                    Some(reset_to) => format!(
                        "{}\nShell cwd was reset to {}",
                        stderr_clean.trim(),
                        reset_to.display()
                    ),
                    None => stderr_clean,
                };
                // Model-facing stdout normalization (claude-code): strip leading
                // whitespace-only lines + trimEnd, then drop outer empty lines.
                let normalized = crate::shared::strip_empty_lines(
                    &crate::shared::normalize_stdout(&stdout_clean),
                );

                // Image-output short-circuit (claude-code `BashTool/utils.ts`
                // `formatOutput`:138-144 + `BashTool.tsx`:785-802): when the
                // model-facing stdout is a base64 `data:image/…;base64,…` URI
                // (matplotlib/screenshot helpers), return it as an IMAGE rather
                // than truncating it as text. Detection runs on `normalized`
                // (= TS `stripEmptyLines(stdout)`), BEFORE `truncate_bash_output`
                // — truncated base64 would decode to a corrupt image. The image
                // rides on `new_messages` via the Rust image contract (mirrors
                // FileRead `read.rs:718-759`); `data` carries `isImage: true` +
                // the `model_content` placeholder. NOTE: claude-code
                // `resizeShellImageOutput` (CC-304 dimension/size cap) is a
                // follow-up — it needs an image-decode dep tool-shell doesn't
                // have. We emit the URI payload as-is (it is already valid
                // base64), so the model still receives the image — the
                // parity-critical behavior. Only the optional re-encode/resize
                // is deferred.
                if is_image_output(&normalized) {
                    if let Some((media_type, payload)) = parse_data_uri(&normalized) {
                        let mut meta: LogEventMetadata = HashMap::new();
                        meta.insert("request_id".into(), AnalyticsValue::String(request_id));
                        meta.insert(
                            "exit_code".into(),
                            AnalyticsValue::Int(i64::from(out.exit_code)),
                        );
                        meta.insert(
                            "output_bytes".into(),
                            AnalyticsValue::Int(payload.len() as i64),
                        );
                        let elapsed_ms = SystemTime::now()
                            .duration_since(started_at)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        meta.insert("duration_ms".into(), AnalyticsValue::Int(elapsed_ms as i64));
                        meta.insert(
                            "ansi_chars_stripped".into(),
                            AnalyticsValue::Int((ansi_dropped_out + ansi_dropped_err) as i64),
                        );
                        meta.insert("truncated".into(), AnalyticsValue::Bool(false));
                        self.ctx.bus.log_event(BASH_COMPLETED, meta).await;

                        // The data-URI payload is ALREADY valid base64 of the
                        // image bytes — emit it directly, no decode/re-encode.
                        let source = protocol::ImageSource::Base64 {
                            media_type: media_type.clone(),
                            data: payload,
                        };
                        // Pure image: no leading text block (TS attaches none),
                        // matching FileRead's empty-text case (`read.rs:729`).
                        let msg = protocol::ConversationMessage::user_with_images(
                            protocol::MessageId::new(),
                            String::new(),
                            vec![source],
                        );
                        let interp = crate::command_semantics::interpret_command_result(
                            &cmd_str,
                            out.exit_code,
                        );
                        return Ok(ToolCallResult {
                            // Image output: the binary's result data is the main
                            // shape with `isImage: true`. `isImage` flags that
                            // `stdout` CONTAINS the image (the data-URI) — the
                            // result mapper builds the image block FROM `stdout`
                            // (`mapToolResultToToolResultBlockParam`). So `stdout`
                            // carries the (untruncated) URI; the model also gets
                            // the image as a separate content block via `new_messages`.
                            data: bash_result_data(
                                &normalized,
                                &stderr_clean,
                                false,
                                true,
                                interp.message.as_deref(),
                                crate::silent::is_silent_bash_command(&cmd_str),
                                None,
                            ),
                            model_content: Some(
                                "[Image content provided in the following message.]".to_string(),
                            ),
                            new_messages: vec![msg],
                            context_modifier: None,
                            is_error: false,
                            mcp_meta: None,
                        });
                    }
                }

                // BASH.3: honor the `BASH_MAX_OUTPUT_LENGTH` env override
                // (claude-code `outputLimits.ts` `getMaxOutputLength`); falls
                // back to the 30_000-char default when unset/invalid. The
                // truncation message matches claude-code `formatOutput`
                // (`BashTool/utils.ts:156-158`): `... [N lines truncated] ...`.
                let (stdout_final, truncated_out) =
                    truncate_bash_output(normalized, bash_max_output_length());
                // Exit-code reinterpretation (claude-code interpretCommandResult):
                // e.g. `grep` no-match (exit 1) is NOT an error.
                let interp =
                    crate::command_semantics::interpret_command_result(&cmd_str, out.exit_code);

                let elapsed_ms = SystemTime::now()
                    .duration_since(started_at)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert("request_id".into(), AnalyticsValue::String(request_id));
                meta.insert(
                    "exit_code".into(),
                    AnalyticsValue::Int(i64::from(out.exit_code)),
                );
                meta.insert(
                    "output_bytes".into(),
                    AnalyticsValue::Int((stdout_final.len() + stderr_clean.len()) as i64),
                );
                meta.insert("duration_ms".into(), AnalyticsValue::Int(elapsed_ms as i64));
                meta.insert(
                    "ansi_chars_stripped".into(),
                    AnalyticsValue::Int((ansi_dropped_out + ansi_dropped_err) as i64),
                );
                meta.insert("truncated".into(), AnalyticsValue::Bool(truncated_out));
                self.ctx.bus.log_event(BASH_COMPLETED, meta).await;
                invalidate_written_read_state(&self.ctx, &cwd, &cmd_str);

                // Model sees the plain-text `[stdout, stderr].join("\n")` render
                // (`content` in the binary's tool_result mapper), NOT the JSON
                // object — which stays for the TUI / PostToolUse hook.
                let model_content = bash_model_content(&stdout_final, &stderr_clean, false, None);
                Ok(ToolCallResult {
                    // claude-code 2.1.191 `BashTool` outputSchema (pure metadata).
                    // A completed text command: `interrupted: false`, `isImage:
                    // false`; `returnCodeInterpretation` only when the exit code
                    // carries a semantic meaning (`interp.message`, else omitted).
                    data: bash_result_data(
                        &stdout_final,
                        &stderr_clean,
                        false,
                        false,
                        interp.message.as_deref(),
                        crate::silent::is_silent_bash_command(&cmd_str),
                        None,
                    ),
                    model_content: Some(model_content),
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
                    mcp_meta: None,
                })
            }
            Err(traits::process::ProcessError::Timeout) => {
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert(
                    "request_id".into(),
                    AnalyticsValue::String(request_id.clone()),
                );
                meta.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
                self.ctx.bus.log_event(BASH_TIMEOUT, meta).await;
                // RESIDUAL: the non-streaming posix runner uses
                // `timeout(..).wait_with_output()` and drops captured bytes on
                // kill, so no partial output is available on this arm — emit the
                // correct interrupted shape with empty partial. (The
                // `Ok(timed_out)` arm above DOES carry partial bytes.)
                Ok(build_interrupted_result("", "", &cmd_str, Some(timeout_ms)))
            }
            Err(e) => {
                emit_failed(&self.ctx.bus, &request_id, "spawn_failed", started_at).await;
                Err(ToolError::Io(format!("{e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    fn use_ctx() -> ToolUseContext {
        tool_api::test_support::fresh_ctx()
    }

    #[test]
    fn format_duration_ms_matches_qs() {
        // Sub-minute → floored seconds.
        assert_eq!(format_duration_ms(0), "0s");
        assert_eq!(format_duration_ms(200), "0s");
        assert_eq!(format_duration_ms(5_000), "5s");
        assert_eq!(format_duration_ms(59_999), "59s");
        // Minute+ → largest units down, seconds rounded, 60→carry.
        assert_eq!(format_duration_ms(60_000), "1m 0s");
        assert_eq!(format_duration_ms(120_000), "2m 0s");
        assert_eq!(format_duration_ms(119_600), "2m 0s"); // 1m 59.6s → round → 2m 0s
        assert_eq!(format_duration_ms(3_600_000), "1h 0m 0s");
        assert_eq!(format_duration_ms(3_661_000), "1h 1m 1s");
        assert_eq!(format_duration_ms(90_061_000), "1d 1h 1m"); // days form drops seconds
    }

    #[test]
    fn timeout_result_prepends_duration_annotation_to_stderr() {
        let r = build_interrupted_result("out", "boom", "sleep 200", Some(120_000));
        let mc = r.model_content.unwrap();
        // claude `uqh`: annotation, space, partial stderr — then the abort marker.
        assert!(
            mc.contains("Command timed out after 2m 0s boom"),
            "annotation missing/misplaced: {mc}"
        );
        assert!(
            mc.contains("<error>Command was aborted before completion</error>"),
            "abort marker missing: {mc}"
        );
        // A plain interrupt (no timeout) carries NO duration annotation.
        let plain = build_interrupted_result("out", "boom", "cmd", None);
        assert!(!plain.model_content.unwrap().contains("Command timed out after"));
    }

    #[test]
    fn locked_constants_unchanged() {
        assert_eq!(BASH_DEFAULT_TIMEOUT_MS, 120_000);
        assert_eq!(BASH_MAX_TIMEOUT_MS, 600_000);
        assert_eq!(
            BASH_TIMEOUT_ERROR_TEMPLATE,
            "Bash command timed out after {N}ms"
        );
        assert_eq!(BASH_SHELL_LINUX, "/bin/bash");
        assert_eq!(BASH_SHELL_MACOS, "/bin/zsh");
        assert_eq!(TOOL_NAME, "Bash");
    }

    #[test]
    fn model_content_matches_binary_join() {
        // [c, u, d].filter(Boolean).join("\n"): stdout + stderr joined by newline.
        assert_eq!(
            bash_model_content("hello\n", "warn: x\n", false, None),
            "hello\nwarn: x"
        );
        // stdout only (stderr empty ⇒ dropped, no trailing newline).
        assert_eq!(bash_model_content("ok\n", "", false, None), "ok");
        // stderr only (empty stdout ⇒ dropped).
        assert_eq!(bash_model_content("", "boom", false, None), "boom");
        // Leading blank lines stripped + trimEnd (normalize_stdout / the `c` rule).
        assert_eq!(
            bash_model_content("\n\n  data  \n", "", false, None),
            "  data"
        );
        // Interrupt appends the abort marker after a newline (rYa) when stderr present.
        assert_eq!(
            bash_model_content("partial\n", "err", true, None),
            format!("partial\nerr\n{ABORT_MARKER}")
        );
        // Interrupt with empty stderr ⇒ marker only (no leading newline).
        assert_eq!(
            bash_model_content("", "", true, None),
            ABORT_MARKER.to_string()
        );
        // Background note is the `d` part.
        assert_eq!(
            bash_model_content(
                "",
                "",
                false,
                Some("Command running in background with ID: 7. Output is being written to: /p.")
            ),
            "Command running in background with ID: 7. Output is being written to: /p."
        );
    }

    // ===== Finding #8 — `R0` containment port (`is_within_allowed`) ==========

    #[test]
    fn normalize_private_prefix_folds_var_and_tmp() {
        use std::path::Path;
        // `/private/var/...` → `/var/...`
        assert_eq!(
            normalize_private_prefix(Path::new("/private/var/folders/ab/x")),
            "/var/folders/ab/x"
        );
        // `/private/tmp/...` → `/tmp/...` (the `$1` keeps the trailing segment)
        assert_eq!(
            normalize_private_prefix(Path::new("/private/tmp/foo")),
            "/tmp/foo"
        );
        // `/private/tmp` (end-of-string) → `/tmp`
        assert_eq!(normalize_private_prefix(Path::new("/private/tmp")), "/tmp");
        // Non-matching prefixes pass through unchanged (no `/private/varX` fold).
        assert_eq!(
            normalize_private_prefix(Path::new("/private/variable")),
            "/private/variable"
        );
        assert_eq!(normalize_private_prefix(Path::new("/usr/bin")), "/usr/bin");
    }

    #[test]
    fn is_within_allowed_same_dir_is_contained() {
        // `R0`: `posix.relative(t, e) == ""` ⇒ same dir ⇒ contained.
        let dir = std::env::temp_dir();
        let canon = std::fs::canonicalize(&dir).unwrap();
        assert!(is_within_allowed(&canon, &canon));
    }

    #[test]
    fn is_within_allowed_subdir_is_contained() {
        // A real subdir of a real (canonicalized) workspace is contained — the
        // `/private/var` realpath fold on macOS must not break the prefix check.
        let ws = tempfile::TempDir::new().unwrap();
        let ws_canon = std::fs::canonicalize(ws.path()).unwrap();
        let sub = ws_canon.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let sub_canon = std::fs::canonicalize(&sub).unwrap();
        assert!(
            is_within_allowed(&sub_canon, &ws_canon),
            "{} should be within {}",
            sub_canon.display(),
            ws_canon.display()
        );
    }

    #[test]
    fn is_within_allowed_sibling_is_not_contained() {
        // `R0`: `posix.relative(t, e)` is `..`-leading (or absolute) ⇒ outside.
        // Two independent temp dirs are siblings, never nested.
        let a = tempfile::TempDir::new().unwrap();
        let b = tempfile::TempDir::new().unwrap();
        let a_canon = std::fs::canonicalize(a.path()).unwrap();
        let b_canon = std::fs::canonicalize(b.path()).unwrap();
        assert!(
            !is_within_allowed(&b_canon, &a_canon),
            "{} must NOT be within {}",
            b_canon.display(),
            a_canon.display()
        );
    }

    #[test]
    fn is_within_allowed_prefix_string_not_path_boundary() {
        // `/var/foobar` is NOT inside `/var/foo` — the boundary must be a path
        // separator, mirroring `posix.relative` (`relative('/var/foo','/var/foobar')`
        // == `../foobar`, `..`-leading ⇒ not contained). Build real dirs whose
        // canonical names share a string prefix but are siblings.
        let root = tempfile::TempDir::new().unwrap();
        let root_canon = std::fs::canonicalize(root.path()).unwrap();
        let foo = root_canon.join("foo");
        let foobar = root_canon.join("foobar");
        std::fs::create_dir(&foo).unwrap();
        std::fs::create_dir(&foobar).unwrap();
        assert!(
            !is_within_allowed(&foobar, &foo),
            "{} must NOT be 'within' {} (string-prefix, not path-prefix)",
            foobar.display(),
            foo.display()
        );
    }

    #[test]
    fn format_timeout_error_substitutes_n() {
        assert_eq!(
            format_timeout_error(200),
            "Bash command timed out after 200ms"
        );
        assert_eq!(
            format_timeout_error(120_000),
            "Bash command timed out after 120000ms"
        );
    }

    #[test]
    fn resolve_shell_path_matches_host_os() {
        // Ensure LINGXI_SHELL is unset so we hit the OS-fallback branch.
        std::env::remove_var("LINGXI_SHELL");
        if cfg!(target_os = "macos") {
            assert_eq!(resolve_shell_path(), "/bin/zsh");
        } else {
            assert_eq!(resolve_shell_path(), "/bin/bash");
        }
    }

    /// LINGXI_SHELL override: a bash path is honoured.
    #[test]
    fn resolve_shell_path_honours_claude_code_shell_bash() {
        std::env::set_var("LINGXI_SHELL", "/opt/homebrew/bin/bash");
        let result = resolve_shell_path();
        std::env::remove_var("LINGXI_SHELL");
        assert_eq!(result, "/opt/homebrew/bin/bash");
    }

    /// LINGXI_SHELL override: a zsh path is honoured.
    #[test]
    fn resolve_shell_path_honours_claude_code_shell_zsh() {
        std::env::set_var("LINGXI_SHELL", "/usr/local/bin/zsh");
        let result = resolve_shell_path();
        std::env::remove_var("LINGXI_SHELL");
        assert_eq!(result, "/usr/local/bin/zsh");
    }

    /// LINGXI_SHELL set to an unsupported shell (neither bash nor zsh)
    /// falls back to the OS default — matches TS fallback path.
    #[test]
    fn resolve_shell_path_rejects_unsupported_shell() {
        std::env::set_var("LINGXI_SHELL", "/bin/sh");
        std::env::remove_var("LINGXI_SHELL"); // first clear; now test with fish
        std::env::set_var("LINGXI_SHELL", "/usr/bin/fish");
        let result = resolve_shell_path();
        std::env::remove_var("LINGXI_SHELL");
        // fish contains neither "bash" nor "zsh", so OS default is used.
        if cfg!(target_os = "macos") {
            assert_eq!(result, "/bin/zsh");
        } else {
            assert_eq!(result, "/bin/bash");
        }
    }

    /// Empty LINGXI_SHELL falls back to OS default.
    #[test]
    fn resolve_shell_path_ignores_empty_claude_code_shell() {
        std::env::set_var("LINGXI_SHELL", "");
        let result = resolve_shell_path();
        std::env::remove_var("LINGXI_SHELL");
        if cfg!(target_os = "macos") {
            assert_eq!(result, "/bin/zsh");
        } else {
            assert_eq!(result, "/bin/bash");
        }
    }

    #[test]
    fn task_output_path_embeds_task_id() {
        let p = task_output_path("abc123");
        let s = p.display().to_string();
        assert!(s.contains("abc123"), "got {s}");
        assert!(
            std::path::Path::new(&s)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("out")),
            "got {s}",
        );
    }

    #[tokio::test]
    async fn foreground_zero_exit_returns_stdout_and_is_error_false() {
        let out = ProcessOutput {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "echo hello"}), use_ctx(), fresh_tx())
            .await
            .expect("call should succeed");
        // claude-code normalizes model-facing stdout (trimEnd + stripEmptyLines),
        // so the trailing newline is dropped.
        assert_eq!(res.data["stdout"], "hello");
        assert_eq!(res.data["interrupted"], false);
        assert_eq!(res.data["isImage"], false);
        // `exit_code`/`is_error`/`timed_out` are telemetry-only — NOT result data.
        assert!(res.data.get("exit_code").is_none());
        assert!(res.data.get("is_error").is_none());
        assert!(res.data.get("timed_out").is_none());
        // exit 0 → no semantic interpretation → field omitted (binary `p?.message`).
        assert!(res.data.get("returnCodeInterpretation").is_none());
    }

    /// PHASE-2: when `ctx.cancel` is a token that is already fired, the
    /// foreground run's `tokio::select!` takes the (biased) cancel arm and the
    /// call returns `ToolError::Aborted` — the seam the streaming executor uses
    /// to substitute the synthetic sibling-cancel.
    ///
    /// Determinism: the stub `ProcessRunner::run` returns Ready immediately
    /// (no yield), so BOTH select arms are Ready. We pre-cancel the token AND
    /// use `biased;` with the cancel arm FIRST, so the cancel branch is chosen
    /// deterministically — mirroring real cancellation where the executor fires
    /// the token before/while the subprocess is in flight.
    #[tokio::test]
    async fn foreground_returns_aborted_when_cancel_token_fired() {
        let out = ProcessOutput {
            stdout: "should-not-be-seen\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel(); // pre-fire so `cancelled()` is Ready
        let mut ctx = use_ctx();
        ctx.cancel = Some(token);
        let err = tool
            .call(json!({"command": "sleep 100"}), ctx, fresh_tx())
            .await
            .expect_err("a fired cancel token must abort the run");
        assert!(
            matches!(err, ToolError::Aborted),
            "expected ToolError::Aborted, got {err:?}"
        );
    }

    /// Control: with NO cancel token (`ctx.cancel == None`) the run is awaited
    /// directly and completes normally — proving the select wrap is inert for
    /// every non-streaming caller (byte-for-byte unchanged behavior).
    #[tokio::test]
    async fn foreground_no_cancel_token_runs_normally() {
        let out = ProcessOutput {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "echo hello"}), use_ctx(), fresh_tx())
            .await
            .expect("no token → normal completion");
        assert_eq!(res.data["stdout"], "hello");
        assert_eq!(res.data["interrupted"], false);
    }

    #[tokio::test]
    async fn foreground_nonzero_exit_is_ok_data_not_err() {
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: "boom\n".into(),
            exit_code: 7,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "exit 7"}), use_ctx(), fresh_tx())
            .await
            .expect("non-zero exit is data, not Err");
        // A non-zero exit is a SUCCESSFUL result (Ok data), not a hard error.
        // claude-code surfaces no `exit_code`/`is_error` in result data — the
        // failure rides via stderr, the model render, and the non-zero default
        // `returnCodeInterpretation` ("Command failed with exit code N").
        assert!(res.data["stderr"].as_str().unwrap().contains("boom"));
        assert_eq!(res.data["interrupted"], false);
        assert_eq!(
            res.data["returnCodeInterpretation"],
            "Command failed with exit code 7"
        );
        assert!(res.data.get("exit_code").is_none());
        assert!(res.data.get("is_error").is_none());
    }

    #[tokio::test]
    async fn foreground_grep_no_match_carries_return_code_interpretation() {
        // `grep` exit 1 = "no matches": a NON-error carrying a semantic
        // `returnCodeInterpretation` (claude-code `interpretCommandResult`). The
        // field is present (string); there is still no `is_error` in `data`.
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(
                json!({"command": "grep needle file"}),
                use_ctx(),
                fresh_tx(),
            )
            .await
            .expect("grep no-match is Ok data");
        assert_eq!(res.data["returnCodeInterpretation"], "No matches found");
        assert_eq!(res.data["interrupted"], false);
        assert!(res.data.get("is_error").is_none());
        assert!(res.data.get("exit_code").is_none());
    }

    #[tokio::test]
    async fn foreground_timed_out_returns_ok_interrupted_with_partial() {
        // claude-code parity: a timeout is a SUCCESSFUL result carrying the
        // partial output + `interrupted: true`, NOT a hard error.
        let out = ProcessOutput {
            stdout: "partial-out\n".into(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(
                json!({"command": "do-work", "timeout": 200}),
                use_ctx(),
                fresh_tx(),
            )
            .await
            .expect("timeout should be Ok(interrupted)");
        assert_eq!(res.data["interrupted"], true);
        assert_eq!(res.data["stdout"], "partial-out");
        assert!(res.data.get("timed_out").is_none());
        assert!(res.data.get("is_error").is_none());
        assert!(
            res.data["stderr"]
                .as_str()
                .unwrap()
                .contains("<error>Command was aborted before completion</error>"),
            "expected abort marker in stderr, got: {:?}",
            res.data["stderr"]
        );
    }

    #[tokio::test]
    async fn foreground_process_timeout_err_returns_ok_interrupted_empty_partial() {
        // The non-streaming runner reports a kill as `Err(ProcessError::Timeout)`
        // and drops captured bytes — still an Ok(interrupted) shape, empty stdout.
        struct TimeoutStub;
        #[async_trait]
        impl traits::process::ProcessRunner for TimeoutStub {
            async fn run(
                &self,
                _: &traits::sandbox::SandboxedCommand,
            ) -> Result<ProcessOutput, traits::process::ProcessError> {
                Err(traits::process::ProcessError::Timeout)
            }
            async fn spawn_background(
                &self,
                _: &traits::sandbox::SandboxedCommand,
            ) -> Result<traits::process::ProcessHandle, traits::process::ProcessError> {
                unreachable!()
            }
            async fn kill(
                &self,
                _: &traits::process::ProcessHandle,
            ) -> Result<(), traits::process::ProcessError> {
                Ok(())
            }
            fn is_available(&self) -> bool {
                true
            }
        }
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.process = std::sync::Arc::new(TimeoutStub);
        let tool = BashTool::new(ctx);
        let res = tool
            .call(
                json!({"command": "do-work", "timeout": 200}),
                use_ctx(),
                fresh_tx(),
            )
            .await
            .expect("process-timeout should be Ok(interrupted)");
        assert_eq!(res.data["interrupted"], true);
        assert_eq!(res.data["stdout"], "");
        assert!(res.data.get("timed_out").is_none());
        assert!(res.data.get("is_error").is_none());
    }

    /// Serializes every test that mutates the process-global
    /// `tengu_amber_sentinel` gate env (the sleep-block opt-in). A tokio mutex
    /// keeps the guard `Send` across the `.await` in these async tests.
    static SLEEP_GATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn bash_tool_noop() -> BashTool {
        BashTool::new(shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }))
    }

    #[tokio::test]
    async fn validate_input_blocks_standalone_sleep_when_gate_on() {
        let _g = SLEEP_GATE_LOCK.lock().await;
        let tool = bash_tool_noop();
        // Gate ON + duration >= 25 (G2n) → blocked with the byte-exact message.
        std::env::set_var("tengu_amber_sentinel", "1");
        let result = tool
            .validate_input(&json!({"command": "sleep 30"}), &use_ctx())
            .await;
        std::env::remove_var("tengu_amber_sentinel");
        let err = result.expect_err("sleep 30 must be blocked when the gate is on");
        assert_eq!(
            err.0,
            "Blocked: standalone sleep 30. To wait for a condition, use Monitor with an until-loop (e.g. `until <check>; do sleep 2; done`). To wait for a command you started, use run_in_background: true. Do not chain shorter sleeps to work around this block."
        );
    }

    #[tokio::test]
    async fn validate_input_blocks_sleep_with_followup_when_gate_on() {
        let _g = SLEEP_GATE_LOCK.lock().await;
        let tool = bash_tool_noop();
        std::env::set_var("tengu_amber_sentinel", "1");
        let result = tool
            .validate_input(&json!({"command": "sleep 30 && echo done"}), &use_ctx())
            .await;
        std::env::remove_var("tengu_amber_sentinel");
        let err = result.expect_err("sleep 30 && ... must be blocked when the gate is on");
        assert!(
            err.0
                .starts_with("Blocked: sleep 30 followed by: echo done. To wait"),
            "got: {}",
            err.0
        );
    }

    #[tokio::test]
    async fn validate_input_sleep_block_is_off_by_default() {
        let _g = SLEEP_GATE_LOCK.lock().await;
        let tool = bash_tool_noop();
        // No gate env set (default) → even a long sleep is allowed (1:1 with
        // stock claude-code, whose `sq()` defaults false).
        std::env::remove_var("tengu_amber_sentinel");
        tool.validate_input(&json!({"command": "sleep 30"}), &use_ctx())
            .await
            .expect("sleep 30 must be ALLOWED by default (gate off)");
    }

    #[tokio::test]
    async fn validate_input_allows_sub_threshold_and_float_even_when_gate_on() {
        let _g = SLEEP_GATE_LOCK.lock().await;
        let tool = bash_tool_noop();
        std::env::set_var("tengu_amber_sentinel", "1");
        // < 25 (incl. fractional, and non-sleep commands) are never blocked.
        let mut results = Vec::new();
        for ok in [
            "sleep 24",
            "sleep 24.9",
            "sleep 0.5",
            "echo hi",
            "sleeper foo",
        ] {
            results.push((
                ok,
                tool.validate_input(&json!({ "command": ok }), &use_ctx())
                    .await,
            ));
        }
        std::env::remove_var("tengu_amber_sentinel");
        for (ok, r) in results {
            r.unwrap_or_else(|e| panic!("{ok:?} should be allowed, got: {}", e.0));
        }
    }

    #[tokio::test]
    async fn validate_input_allows_sleep_when_backgrounded_even_with_gate_on() {
        let _g = SLEEP_GATE_LOCK.lock().await;
        let tool = bash_tool_noop();
        std::env::set_var("tengu_amber_sentinel", "1");
        let result = tool
            .validate_input(
                &json!({"command": "sleep 30", "run_in_background": true}),
                &use_ctx(),
            )
            .await;
        std::env::remove_var("tengu_amber_sentinel");
        result.expect("sleep 30 backgrounded must be allowed even with the gate on");
    }

    #[test]
    fn detect_blocked_sleep_pattern_threshold_and_fractional() {
        // Below the 25s threshold → not blocked.
        assert_eq!(detect_blocked_sleep_pattern("sleep 24"), None);
        assert_eq!(detect_blocked_sleep_pattern("sleep 24.9"), None);
        assert_eq!(detect_blocked_sleep_pattern("sleep 0.5"), None);
        // At/above threshold → blocked; integral renders WITHOUT a decimal.
        assert_eq!(
            detect_blocked_sleep_pattern("sleep 25"),
            Some("standalone sleep 25".to_string())
        );
        assert_eq!(
            detect_blocked_sleep_pattern("sleep 30"),
            Some("standalone sleep 30".to_string())
        );
        // Fractional duration is matched and kept (parseFloat semantics).
        assert_eq!(
            detect_blocked_sleep_pattern("sleep 30.5"),
            Some("standalone sleep 30.5".to_string())
        );
        // Trailing dot (`30.`) parses as 30 → "30".
        assert_eq!(
            detect_blocked_sleep_pattern("sleep 30."),
            Some("standalone sleep 30".to_string())
        );
        // Leading sleep with a follow-up command.
        assert_eq!(
            detect_blocked_sleep_pattern("sleep 40 && echo done"),
            Some("sleep 40 followed by: echo done".to_string())
        );
        // Non-matches.
        assert_eq!(detect_blocked_sleep_pattern("sleeper foo"), None);
        assert_eq!(detect_blocked_sleep_pattern("echo hi"), None);
    }

    #[test]
    fn timeout_env_resolvers_s0e_j3n() {
        // s0e(): unset / invalid / non-positive → 120000; positive → that value.
        assert_eq!(resolve_default_timeout_ms(None), 120_000);
        assert_eq!(resolve_default_timeout_ms(Some("")), 120_000);
        assert_eq!(resolve_default_timeout_ms(Some("abc")), 120_000);
        assert_eq!(resolve_default_timeout_ms(Some("0")), 120_000);
        assert_eq!(resolve_default_timeout_ms(Some("-5")), 120_000);
        assert_eq!(resolve_default_timeout_ms(Some("300000")), 300_000);
        assert_eq!(resolve_default_timeout_ms(Some("90000")), 90_000);
        // j3n(): unset → max(600000, default); positive → max(n, default).
        assert_eq!(resolve_max_timeout_ms(None, 120_000), 600_000);
        // A custom default above 600000 raises the floor (Math.max(_, s0e)).
        assert_eq!(resolve_max_timeout_ms(None, 900_000), 900_000);
        assert_eq!(resolve_max_timeout_ms(Some("1200000"), 120_000), 1_200_000);
        // A max BELOW the default is floored up to the default.
        assert_eq!(resolve_max_timeout_ms(Some("50000"), 120_000), 120_000);
        assert_eq!(resolve_max_timeout_ms(Some("bad"), 120_000), 600_000);
    }

    #[test]
    fn resolve_timeout_ms_vf_h5a_semantics() {
        // Plain number > 0 honored, even over the advisory 600000 "max" (#4).
        assert_eq!(resolve_timeout_ms(&json!({"timeout": 200})), 200);
        assert_eq!(resolve_timeout_ms(&json!({"timeout": 700_000})), 700_000);
        // Numeric STRING coerced (VF): "30000" → 30000 (#5).
        assert_eq!(resolve_timeout_ms(&json!({"timeout": "30000"})), 30_000);
        assert_eq!(resolve_timeout_ms(&json!({"timeout": " 5000 "})), 5_000);
        // 0 / negative / non-numeric string / absent → default (H5a).
        assert_eq!(
            resolve_timeout_ms(&json!({"timeout": 0})),
            BASH_DEFAULT_TIMEOUT_MS
        );
        assert_eq!(
            resolve_timeout_ms(&json!({"timeout": -5})),
            BASH_DEFAULT_TIMEOUT_MS
        );
        assert_eq!(
            resolve_timeout_ms(&json!({"timeout": "abc"})),
            BASH_DEFAULT_TIMEOUT_MS
        );
        assert_eq!(resolve_timeout_ms(&json!({})), BASH_DEFAULT_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn validate_input_no_longer_rejects_over_max_timeout() {
        // #4: an over-600000 timeout is accepted (no schema/runtime max).
        let tool = bash_tool_noop();
        tool.validate_input(
            &json!({"command": "echo hi", "timeout": 900_000}),
            &use_ctx(),
        )
        .await
        .expect("over-max timeout must be accepted (no max rejection)");
    }

    #[tokio::test]
    async fn foreground_strips_ansi_from_stdout() {
        let out = ProcessOutput {
            stdout: "\x1b[31mred\x1b[0m\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "printf-red"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        // ANSI stripped → "red\n", then normalized (trimEnd) → "red".
        assert_eq!(res.data["stdout"], "red");
    }

    #[tokio::test]
    async fn foreground_normalizes_leading_and_trailing_blank_lines() {
        let out = ProcessOutput {
            stdout: "\n\n\nhello\nworld\n\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "cat thing"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Leading blank lines stripped, trailing whitespace trimmed, inner kept.
        assert_eq!(res.data["stdout"], "hello\nworld");
    }

    #[tokio::test]
    async fn foreground_sets_no_output_expected_for_silent_command() {
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "mkdir foo"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["noOutputExpected"], true);
    }

    #[tokio::test]
    async fn foreground_no_output_expected_false_for_non_silent_command() {
        let out = ProcessOutput {
            stdout: "a\nb\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "ls -la"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["noOutputExpected"], false);
    }

    #[test]
    fn input_schema_uses_timeout_not_timeout_ms() {
        // claude-code `BashTool.tsx:229` names the param `timeout` (ms).
        let tool = BashTool::new(tool_api::test_support::shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }));
        let props = &tool.input_schema()["properties"];
        assert!(
            props.get("timeout").is_some(),
            "schema must expose `timeout`"
        );
        assert!(
            props.get("timeout_ms").is_none(),
            "schema must NOT expose the old `timeout_ms`"
        );
        assert_eq!(
            props["timeout"]["description"],
            "Optional timeout in milliseconds (max 600000)"
        );
    }

    /// Binary gap #1/#2: timeout type must be `number` (not `integer`), no `minimum`.
    /// Binary @203768858: `timeout:sB(A.number().optional())` — Zod `.number()` → `{"type":"number"}`.
    #[test]
    fn bash_timeout_schema_type_number_no_minimum() {
        let tool = BashTool::new(tool_api::test_support::shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }));
        let timeout = &tool.input_schema()["properties"]["timeout"];
        assert_eq!(
            timeout["type"], "number",
            "timeout type must be number not integer"
        );
        assert!(
            timeout.get("minimum").is_none(),
            "timeout must have no minimum constraint"
        );
    }

    #[tokio::test]
    async fn foreground_success_sets_interrupted_false() {
        // claude-code `BashTool.tsx:283` outputSchema field `interrupted`: a
        // command that completes (does not time out) is not interrupted.
        let out = ProcessOutput {
            stdout: "done\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "echo done"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["interrupted"], false);
    }

    #[tokio::test]
    async fn foreground_truncates_at_30k_chars_with_lines_truncated_suffix() {
        // Head of 30_000 `a`s (no newlines) then a 5-newline tail: the kept head
        // is the first 30_000 chars; the truncated tail holds 5 `\n`, so the TS
        // `formatOutput` count is `5 + 1 = 6` lines truncated
        // (`BashTool/utils.ts:156-158`).
        let head = "a".repeat(30_000);
        let tail = format!("{}tail", "\n".repeat(5));
        let out = ProcessOutput {
            stdout: format!("{head}{tail}"),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "yes"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        let s = res.data["stdout"].as_str().unwrap();
        // New TS-form truncation message with a correct N (6). (`truncated` is
        // telemetry-only — the truncation is observable in the stdout suffix.)
        assert!(
            s.ends_with("... [6 lines truncated] ..."),
            "expected TS lines-truncated suffix with N=6, got tail: {:?}",
            &s[s.len().saturating_sub(40)..]
        );
        // Head preserved verbatim, joined by the literal `\n\n` separator.
        assert!(
            s.starts_with(&format!("{head}\n\n... [")),
            "head must be the kept prefix followed by the separator",
        );
        // The old generic suffix is gone.
        assert!(!s.contains("[Output truncated due to length]"));
    }

    // ----- Background path: bespoke stub that returns a fake ProcessHandle. -----

    use std::sync::Arc;
    use traits::process::{ProcessError, ProcessHandle, ProcessRunner};
    use traits::sandbox::SandboxedCommand;

    struct BgStub;
    #[async_trait]
    impl ProcessRunner for BgStub {
        async fn run(&self, _: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            unreachable!()
        }
        async fn spawn_background(
            &self,
            _: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Ok(ProcessHandle {
                task_id: "task-abc123".into(),
                pid: 4242,
            })
        }
        async fn kill(&self, _: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn background_returns_background_task_id() {
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.process = Arc::new(BgStub);
        let tool = BashTool::new(ctx);
        let res = tool
            .call(
                json!({"command": "sleep 5", "run_in_background": true}),
                use_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        // claude-code backgrounded result: the main shape with `backgroundTaskId`
        // + empty stdout/stderr. No `pid`/`task_output_path` in `data` — the ID
        // and output path ride in the model note (`model_content`).
        assert_eq!(res.data["backgroundTaskId"], "task-abc123");
        assert_eq!(res.data["stdout"], "");
        assert_eq!(res.data["interrupted"], false);
        assert!(res.data.get("pid").is_none());
        assert!(res.data.get("task_output_path").is_none());
        let note = res.model_content.as_deref().expect("background model note");
        assert!(
            note.contains("task-abc123") && note.contains("Command running in background"),
            "note must carry the task id + path, got: {note}",
        );
    }

    // ----- BASH.1 / BASH.2 / BASH.3 / BASH.5 ports -------------------------

    #[test]
    fn disable_extglob_command_byte_locked_per_shell() {
        // The LINGXI_SHELL_PREFIX branch overrides the shell-specific form;
        // only assert the per-shell strings when that env var is unset.
        if !std::env::var("LINGXI_SHELL_PREFIX").is_ok_and(|v| !v.is_empty()) {
            assert_eq!(
                disable_extglob_command("/bin/bash").as_deref(),
                Some("shopt -u extglob 2>/dev/null || true")
            );
            assert_eq!(
                disable_extglob_command("/bin/zsh").as_deref(),
                Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true")
            );
            assert_eq!(disable_extglob_command("/usr/bin/fish"), None);
        }
    }

    #[test]
    fn null_redirect_rewrite_matches_ts_regex() {
        // Rewritten cases (case-insensitive `nul`, optional fd/`&`/whitespace).
        assert_eq!(rewrite_windows_null_redirect("ls 2>nul"), "ls 2>/dev/null");
        assert_eq!(rewrite_windows_null_redirect("ls >nul"), "ls >/dev/null");
        assert_eq!(rewrite_windows_null_redirect("ls > NUL"), "ls > /dev/null");
        assert_eq!(rewrite_windows_null_redirect("ls &>nul"), "ls &>/dev/null");
        assert_eq!(rewrite_windows_null_redirect("ls >>nul"), "ls >>/dev/null");
        assert_eq!(
            rewrite_windows_null_redirect("a 2>nul | b"),
            "a 2>/dev/null | b"
        );
        assert_eq!(
            rewrite_windows_null_redirect("(x 2>nul)"),
            "(x 2>/dev/null)"
        );
        // Non-matching cases (must pass through unchanged).
        assert_eq!(rewrite_windows_null_redirect("ls >null"), "ls >null");
        assert_eq!(
            rewrite_windows_null_redirect("ls >nullable"),
            "ls >nullable"
        );
        assert_eq!(rewrite_windows_null_redirect("ls >nul.txt"), "ls >nul.txt");
        assert_eq!(rewrite_windows_null_redirect("cat nul.txt"), "cat nul.txt");
        // UTF-8 passthrough around a rewritten redirect.
        assert_eq!(
            rewrite_windows_null_redirect("echo café 2>nul"),
            "echo café 2>/dev/null"
        );
    }

    #[test]
    fn resolve_max_output_length_honors_env_value() {
        assert_eq!(resolve_max_output_length(None), 30_000); // unset → default
        assert_eq!(resolve_max_output_length(Some("")), 30_000); // empty → default
        assert_eq!(resolve_max_output_length(Some("100")), 100); // valid
        assert_eq!(resolve_max_output_length(Some("123abc")), 123); // parseInt prefix
        assert_eq!(resolve_max_output_length(Some("abc")), 30_000); // NaN → default
        assert_eq!(resolve_max_output_length(Some("0")), 30_000); // 0 → default
        assert_eq!(resolve_max_output_length(Some("-5")), 30_000); // negative → default
        assert_eq!(resolve_max_output_length(Some("999999")), 150_000); // capped
        assert_eq!(resolve_max_output_length(Some("150000")), 150_000); // at limit
                                                                        // The env-reading wrapper falls back to the default when unset.
        if std::env::var_os("BASH_MAX_OUTPUT_LENGTH").is_none() {
            assert_eq!(bash_max_output_length(), 30_000);
        }
    }

    // Capturing runner that records the argv handed to the process runner so we
    // can assert what actually gets spawned.
    struct CapturingRunner {
        out: ProcessOutput,
        last_args: Arc<std::sync::Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl ProcessRunner for CapturingRunner {
        async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            self.last_args.lock().unwrap().clone_from(&cmd.inner().args);
            Ok(self.out.clone())
        }
        async fn spawn_background(
            &self,
            _: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            unreachable!()
        }
        async fn kill(&self, _: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    fn ok_output() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[tokio::test]
    async fn foreground_spawn_prepends_extglob_disable_prefix() {
        let last = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut ctx = shell_test_ctx(ok_output());
        ctx.process = Arc::new(CapturingRunner {
            out: ok_output(),
            last_args: last.clone(),
        });
        let tool = BashTool::new(ctx);
        tool.call(json!({"command": "echo hi"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        let args = last.lock().unwrap().clone();
        // Spawn shape: `-c -l <command>`.
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], "-l");
        let spawned = &args[2];
        let expected_prefix = disable_extglob_command(resolve_shell_path())
            .expect("known host shell has an extglob-disable prefix");
        assert!(
            spawned.starts_with(&format!("{expected_prefix} && ")),
            "extglob disable must lead the spawned command, got: {spawned}"
        );
        // The user command and the BASH.4 cwd readback survive after the prefix.
        assert!(spawned.contains("echo hi"), "got: {spawned}");
        assert!(spawned.contains("pwd -P >|"), "got: {spawned}");
    }

    #[tokio::test]
    async fn dangerously_disable_sandbox_bypasses_when_unsandboxed_allowed() {
        let last = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut ctx = shell_test_ctx(ok_output());
        // Force the default decision to Sandbox: available sandbox + no excluded
        // commands yields `Sandbox { .. }`.
        ctx.sandbox_available = true;
        ctx.sandbox_runtime.excluded_commands = vec![];
        ctx.sandbox_runtime.allow_unsandboxed_commands = true;
        ctx.process = Arc::new(CapturingRunner {
            out: ok_output(),
            last_args: last.clone(),
        });
        let tool = BashTool::new(ctx);

        // Baseline (no flag): the command is sandbox-wrapped.
        tool.call(json!({"command": "echo hi"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        let wrapped = last.lock().unwrap()[2].clone();
        assert!(
            wrapped.contains("sandbox-exec") || wrapped.contains("bwrap"),
            "baseline should be sandbox-wrapped, got: {wrapped}"
        );

        // With the flag AND a policy that allows unsandboxed commands, the
        // sandbox is bypassed — no wrapper appears in the spawned command.
        tool.call(
            json!({"command": "echo hi", "dangerouslyDisableSandbox": true}),
            use_ctx(),
            fresh_tx(),
        )
        .await
        .expect("ok");
        let bypassed = last.lock().unwrap()[2].clone();
        assert!(
            !bypassed.contains("sandbox-exec") && !bypassed.contains("bwrap"),
            "dangerouslyDisableSandbox should bypass the sandbox, got: {bypassed}"
        );
    }

    /// A `SandboxRunner` that records every `wrap`/`cleanup_after_command` call
    /// and returns a sentinel-prefixed wrapped command, so a test can prove the
    /// Bash tool routes through `ctx.sandbox_runner` (not the sync free fn) with
    /// the right args and invokes cleanup after the command finishes.
    struct WrapCall {
        command: String,
        bin_shell: Option<String>,
        cwd: Option<std::path::PathBuf>,
    }

    #[derive(Default)]
    struct RecordingSandboxRunner {
        wrap_calls: std::sync::Mutex<Vec<WrapCall>>,
        cleanups: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl tool_api::SandboxRunner for RecordingSandboxRunner {
        async fn wrap(
            &self,
            command: &str,
            _cfg: &sandbox::runtime_config::SandboxRuntimeConfig,
            _platform: sandbox::runtime_config::Platform,
            bin_shell: Option<&str>,
            cwd: Option<&std::path::Path>,
        ) -> Result<String, sandbox::wrap::SandboxWrapError> {
            self.wrap_calls.lock().unwrap().push(WrapCall {
                command: command.to_string(),
                bin_shell: bin_shell.map(ToString::to_string),
                cwd: cwd.map(std::path::Path::to_path_buf),
            });
            Ok(format!("WRAPPED::{command}"))
        }

        async fn cleanup_after_command(&self) {
            self.cleanups
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn sandbox_branch_routes_through_injected_runner_and_cleans_up() {
        let last = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let runner = Arc::new(RecordingSandboxRunner::default());
        let mut ctx = shell_test_ctx(ok_output());
        // Force the Sandbox branch: available sandbox + no excluded commands.
        ctx.sandbox_available = true;
        ctx.sandbox_runtime.excluded_commands = vec![];
        ctx.workspace = std::path::PathBuf::from("/tmp");
        ctx.sandbox_runner = runner.clone();
        ctx.process = Arc::new(CapturingRunner {
            out: ok_output(),
            last_args: last.clone(),
        });
        let tool = BashTool::new(ctx);
        tool.call(json!({"command": "echo hi"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");

        // The injected runner was called exactly once, with the resolved shell
        // and the workspace cwd.
        let calls = runner.wrap_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "wrap should be called once");
        let call = &calls[0];
        assert!(
            call.command.contains("echo hi"),
            "runner received the spawn command, got: {}",
            call.command
        );
        assert_eq!(call.bin_shell.as_deref(), Some(resolve_shell_path()));
        assert_eq!(call.cwd.as_deref(), Some(std::path::Path::new("/tmp")));

        // The runner's wrapped output (sentinel) is what actually got spawned.
        let spawned = last.lock().unwrap()[2].clone();
        assert!(
            spawned.contains("WRAPPED::"),
            "the runner's wrapped command must be spawned, got: {spawned}"
        );

        // cleanup_after_command ran after the command finished.
        assert_eq!(
            runner.cleanups.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "cleanup_after_command must be invoked once"
        );
    }

    #[tokio::test]
    async fn dangerously_disable_sandbox_ignored_when_policy_disallows() {
        let last = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut ctx = shell_test_ctx(ok_output());
        ctx.sandbox_available = true;
        ctx.sandbox_runtime.excluded_commands = vec![];
        // `allow_unsandboxed_commands = false` ⇒ `areUnsandboxedCommandsAllowed()`
        // is false, so the flag must be ignored and the command stays sandboxed.
        ctx.sandbox_runtime.allow_unsandboxed_commands = false;
        ctx.process = Arc::new(CapturingRunner {
            out: ok_output(),
            last_args: last.clone(),
        });
        let tool = BashTool::new(ctx);
        tool.call(
            json!({"command": "echo hi", "dangerouslyDisableSandbox": true}),
            use_ctx(),
            fresh_tx(),
        )
        .await
        .expect("ok");
        let spawned = last.lock().unwrap()[2].clone();
        assert!(
            spawned.contains("sandbox-exec") || spawned.contains("bwrap"),
            "flag must be ignored when policy disallows unsandboxed cmds, got: {spawned}"
        );
    }

    // ===== Image-output handling (claude-code `BashTool/utils.ts`) ==========

    /// A minimal valid 1x1 transparent PNG, base64-encoded (the payload portion
    /// of a `data:image/png;base64,…` URI).
    const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    #[test]
    fn is_image_output_mirrors_ts_regex() {
        // Matches `^data:image/[a-z0-9.+_-]+;base64,` (case-insensitive).
        assert!(is_image_output("data:image/png;base64,AAAA"));
        assert!(is_image_output("data:image/jpeg;base64,AAAA"));
        assert!(is_image_output("data:image/svg+xml;base64,AAAA"));
        assert!(is_image_output("data:image/x-icon;base64,AAAA"));
        // `i` flag: uppercase scheme/subtype still matches.
        assert!(is_image_output("DATA:IMAGE/PNG;base64,AAAA"));
        // Non-matches.
        assert!(!is_image_output("hello world"));
        assert!(!is_image_output("data:text/plain;base64,AAAA")); // not image/*
        assert!(!is_image_output("data:image/;base64,AAAA")); // empty subtype
        assert!(!is_image_output("data:image/png;,AAAA")); // missing base64 token
        assert!(!is_image_output("data:image/png")); // no `;base64,`
        assert!(!is_image_output(" data:image/png;base64,AAAA")); // leading space (TS `^`)
    }

    #[test]
    fn parse_data_uri_extracts_media_type_and_payload() {
        let (mt, data) = parse_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
        assert_eq!(mt, "image/png");
        assert_eq!(data, "iVBORw0KGgo=");
        // Trimmed before matching.
        let (mt, data) = parse_data_uri("  data:image/jpeg;base64,QQ==  ").unwrap();
        assert_eq!(mt, "image/jpeg");
        assert_eq!(data, "QQ==");
        // Malformed: missing `;base64,` → None.
        assert!(parse_data_uri("data:image/png;base64").is_none());
        assert!(parse_data_uri("data:image/png,QQ==").is_none());
        // Empty payload → None (TS `(.+)$`).
        assert!(parse_data_uri("data:image/png;base64,").is_none());
        // Not a data URI → None.
        assert!(parse_data_uri("just text").is_none());
    }

    #[tokio::test]
    async fn image_stdout_emits_image_message_not_text() {
        // A command whose stdout is a valid `data:image/png;base64,…` URI is
        // returned as an IMAGE: the payload rides on `new_messages` via
        // `ImageSource::Base64` and `data.isImage == true`. The model sees the
        // image block (not the URI text); the URI itself stays in `data.stdout`
        // (binary: `isImage` flags that stdout contains the image).
        let uri = format!("data:image/png;base64,{TINY_PNG_B64}");
        let out = ProcessOutput {
            stdout: format!("{uri}\n"),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "python plot.py"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");

        // `data` is the main result shape with `isImage: true`; no LingXi-only
        // `type`/`media_type`/`truncated` keys, and the model placeholder rides
        // on `model_content`, not inside `data`.
        assert_eq!(res.data["isImage"], true);
        assert_eq!(res.data["interrupted"], false);
        assert_eq!(
            res.model_content.as_deref(),
            Some("[Image content provided in the following message.]")
        );
        assert!(res.data.get("type").is_none());
        assert!(res.data.get("media_type").is_none());
        assert!(res.data.get("truncated").is_none());
        // `isImage` flags that `stdout` CONTAINS the image: the (untruncated)
        // data-URI rides in `stdout`, and the result mapper derives the image
        // block from it. (The model also receives the image via `new_messages`.)
        assert_eq!(res.data["stdout"], uri);

        // Exactly one follow-up message carrying the image as base64.
        assert_eq!(res.new_messages.len(), 1);
        match &res.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => {
                // Pure image: no leading text block, exactly one Image block.
                assert_eq!(content.len(), 1, "expected only the image block");
                match &content[0] {
                    protocol::ContentBlock::Image {
                        source: protocol::ImageSource::Base64 { media_type, data },
                    } => {
                        assert_eq!(media_type, "image/png");
                        assert_eq!(
                            data, TINY_PNG_B64,
                            "payload must be the URI's base64 verbatim"
                        );
                    }
                    other => panic!("expected Image/Base64 block, got {other:?}"),
                }
            }
            other => panic!("expected a User message with the image, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn normal_text_command_is_not_an_image() {
        // A normal text command: `isImage == false`, stdout flows as text, and
        // no image is attached to `new_messages`.
        let out = ProcessOutput {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "echo hello"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["isImage"], false);
        assert_eq!(res.data["stdout"], "hello");
        assert!(
            res.new_messages.is_empty(),
            "no image message for plain text"
        );
    }

    #[tokio::test]
    async fn malformed_image_uri_is_treated_as_text() {
        // Image-like but malformed (no `;base64,`): NOT an image — flows as
        // normal text and is NOT attached as an image message.
        let bad = "data:image/png;not-base64-here";
        let out = ProcessOutput {
            stdout: format!("{bad}\n"),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "cat weird.txt"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["isImage"], false);
        assert_eq!(res.data["stdout"], bad);
        assert!(res.new_messages.is_empty());
    }

    // ===== BASH.7 — `Dh(model)` prompt gate ================================
    //
    // The `Dh(model)` predicate itself (SHORT/LONG, `UWu`/`dfe` model
    // classification) is unit-tested in `tool-api`'s `model_prompt_gate`; here
    // we lock the Bash tool's observable behavior — that `prompt()` routes the
    // gate to the correct SHORT/LONG variant per `PromptOptions::model`.

    #[tokio::test]
    async fn prompt_default_model_none_returns_long_variant() {
        // Default opts (model=None) ⇒ `Dh(None)` false ⇒ LONG prompt. The
        // byte-locked LONG anchors must still hold (regression guard for the
        // existing parity tests).
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let p = tool.prompt(&PromptOptions::default()).await;
        assert!(
            p.starts_with("Executes a given bash command and returns its output."),
            "LONG prompt opening missing; got:\n{p}"
        );
        assert!(
            p.contains(
                "The working directory persists between commands, but shell state does not."
            ),
            "LONG cwd sentence missing"
        );
        assert!(
            p.contains("# Committing changes with git"),
            "LONG committing-changes header missing"
        );
    }

    #[tokio::test]
    async fn prompt_current_gen_model_returns_short_variant() {
        // `model: Some("claude-opus-4-8")` ⇒ `Dh` true ⇒ SHORT prompt — exactly
        // what claude-code serves opus-4-8. Default test sandbox is disabled, so
        // the sandbox section is absent and the git section is the CONCISE one.
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let opts = PromptOptions {
            model: Some("claude-opus-4-8".into()),
            ..Default::default()
        };
        let p = tool.prompt(&opts).await;

        // Opening — SHORT ("a bash command", NOT "a given bash command").
        assert!(
            p.starts_with("Executes a bash command and returns its output.\n\n"),
            "SHORT opening missing; got:\n{p}"
        );
        assert!(
            !p.contains("Executes a given bash command"),
            "must NOT be the LONG opening"
        );
        // Working-directory bullet (em-dash U+2014, straight apostrophe).
        assert!(
            p.contains("- Working directory persists between calls, but prefer absolute paths \u{2014} `cd` in a compound command can trigger a permission prompt. Shell state (env vars, functions) does not persist; the shell is initialized from the user's profile."),
            "SHORT working-directory bullet missing/incorrect; got:\n{p}"
        );
        // IMPORTANT avoid-list bullet — the SHORT (Dh-true) branch DROPS
        // `find`/`grep` vs the LONG prompt (starts at `cat`; verified vs the
        // v2.1.183 binary + rendered opus-4-8 output).
        assert!(
            p.contains("- IMPORTANT: Avoid using this tool to run `cat`, `head`, `tail`, `sed`, `awk`, or `echo` commands, unless explicitly instructed"),
            "SHORT avoid-list bullet missing/incorrect; got:\n{p}"
        );
        assert!(
            !p.contains("`find`, `grep`"),
            "SHORT avoid-list must NOT include find/grep (those are LONG-only); got:\n{p}"
        );
        // Raw timeout bullet (no `/ N minutes` conversion).
        assert!(
            p.contains("- `timeout` is in milliseconds: default 120000, max 600000."),
            "SHORT timeout bullet missing/incorrect; got:\n{p}"
        );
        // Detached run_in_background bullet (background note enabled by default,
        // no Monitor clause since the amber-sentinel gate is default-false).
        assert!(
            p.contains("- `run_in_background` runs the command detached: it keeps running across turns and re-invokes you when it exits. No `&` needed."),
            "SHORT run_in_background bullet missing; got:\n{p}"
        );
        assert!(
            !p.contains("Foreground `sleep` is blocked"),
            "Monitor clause must be absent by default (amber-sentinel off)"
        );
        // CONCISE `# Git` section — three fixed bullets, NO attribution bullets,
        // NO LONG "Committing changes with git" header.
        assert!(
            p.contains("# Git\n- Interactive flags (`-i`, e.g. `git rebase -i`, `git add -i`) are not supported in this environment.\n- Use the `gh` CLI for GitHub operations (PRs, issues, API).\n- Commit or push only when the user asks. If on the default branch, branch first."),
            "SHORT `# Git` section missing/incorrect; got:\n{p}"
        );
        assert!(
            !p.contains("# Committing changes with git"),
            "SHORT prompt must not carry the LONG git section"
        );
        assert!(
            !p.contains("End git commit messages with:") && !p.contains("End PR bodies with:"),
            "attribution bullets must be omitted (no attribution source, like the LONG prompt)"
        );
        // Sandbox section absent (default test sandbox disabled).
        assert!(
            !p.contains("## Command sandbox"),
            "sandbox section should be absent when sandbox disabled"
        );
    }
}
