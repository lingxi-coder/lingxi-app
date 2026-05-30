//! claude-code path-pattern resolution. Ports
//! `resolvePathPatternForSandbox` from
//! `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
//!
//! Three CC-specific conventions:
//! - `//path` → absolute from filesystem root (strip one leading `/`).
//! - `/path`  → relative to the settings file directory.
//! - `~/path`, `./path`, bare `path` → passthrough; the sandbox-runtime layer
//!   handles tilde expansion / cwd relativization later.

use std::path::{Path, PathBuf};

/// Resolve `pattern` according to claude-code's three permission-rule path
/// prefix conventions.
///
/// `settings_dir` is the directory the settings file with this rule lives in.
/// For `~/.claude/settings.json` that's `~/.claude`. For
/// `<project>/.claude/settings.json` that's `<project>/.claude`. For ad-hoc /
/// in-memory settings, callers may pass any path; only `/path` patterns are
/// affected.
#[must_use]
pub fn resolve_path_pattern_for_sandbox(pattern: &str, settings_dir: &Path) -> String {
    // `//path` → strip ONE leading slash. `//etc` → `/etc`.
    if let Some(stripped) = pattern.strip_prefix("//") {
        // The remaining string is already absolute-from-root (claude-code uses
        // this to escape the `/path = settings-relative` convention).
        return format!("/{stripped}");
    }

    // `/path` → relative to settings file directory.
    if let Some(stripped) = pattern.strip_prefix('/') {
        // Skip empty strip (the only way is the input was a single `/`, which
        // we treat as "the settings dir itself").
        let mut out: PathBuf = settings_dir.to_path_buf();
        if !stripped.is_empty() {
            out.push(stripped);
        }
        return out.to_string_lossy().into_owned();
    }

    // Everything else passes through unchanged — sandbox-runtime
    // (or the Rust equivalent) will handle `~/`, `./`, and bare paths.
    pattern.to_string()
}
