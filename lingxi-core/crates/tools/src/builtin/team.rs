//! Team-management builtin tools (`TeamCreate`, `TeamDelete`).
//!
//! LingXi-internal feature with no upstream `claude-code/src/tools/`
//! counterpart. Both tools operate on the M3-02 team-mem root
//! `~/.claude/team-mem/<team_name>/`. The `config.json` body is a LingXi
//! schema-versioned descriptor; the directory itself becomes the
//! watcher root when `settings.team_memory.enabled == true` (M3-02).
//!
//! Wire identifiers locked in spec §7 lines 501-507 and reproduced byte-for-byte
//! in `parity/fixtures/team_tools.json`.

use std::path::{Path, PathBuf};

use crate::tool_trait::ToolError;

// -- Wire identifier locks (spec §7 lines 501-507) ---------------------------

/// M3-02 lock: the `~/.claude/team-mem/` subdirectory name.
///
/// **Deviation from plan Task 0 step 2:** the plan called for importing
/// `lingxi_memory::memdir::paths::TEAM_MEM_SUBDIR`, but `lingxi-memory`
/// already depends transitively on `lingxi-tools` (via `lingxi-sidequery`),
/// so a direct path-dep would form a cycle. We mirror the M3-02 literal
/// here; the parity fixture asserts the two strings stay in sync.
pub const TEAM_MEM_SUBDIR: &str = "team-mem";

/// Tool name for the `TeamCreate` builtin.
pub const TEAM_CREATE_TOOL_NAME: &str = "TeamCreate";

/// Tool name for the `TeamDelete` builtin.
pub const TEAM_DELETE_TOOL_NAME: &str = "TeamDelete";

/// Default team name (spec §7 line 507 lock).
pub const DEFAULT_TEAM_NAME: &str = "default";

/// Team config filename (spec §7 line 506 lock).
pub const TEAM_CONFIG_FILENAME: &str = "config.json";

/// Maximum team name length (LingXi-internal lock; see plan critical-fidelity note).
pub const MAX_TEAM_NAME_LEN: usize = 64;

/// Allowed-character-class description (user-visible; appears in error strings).
pub const TEAM_NAME_PATTERN_DESC: &str = "[a-zA-Z0-9_-]+";

// -- Tool structs (impl Tool added in Tasks 3 and 5) -------------------------

/// Builtin tool: create a team directory + default `config.json`.
pub struct TeamCreateTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

/// Builtin tool: delete a team directory (with safety opt-in).
pub struct TeamDeleteTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

impl TeamCreateTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

impl TeamDeleteTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

// -- Path helpers ------------------------------------------------------------

/// Resolve the on-disk team directory: `<home>/.claude/team-mem/<team_name>/`.
///
/// The use of [`TEAM_MEM_SUBDIR`] (symbol, not literal) makes the M3-02 lock
/// observable at compile time.
#[must_use]
pub fn resolve_team_dir(home: &Path, team_name: &str) -> PathBuf {
    home.join(".claude").join(TEAM_MEM_SUBDIR).join(team_name)
}

/// Resolve the active HOME directory.
///
/// Reads `$HOME` (the integration / parity tests redirect this env var to a
/// `tempfile::TempDir`). Returns [`ToolError::Internal`] when the env var is
/// missing — admin tools cannot meaningfully operate without a HOME.
pub(crate) fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("Team: HOME directory not available".into()))
}

/// Validate a team name per spec §7 + plan locks.
///
/// Rules:
/// - non-empty
/// - length <= [`MAX_TEAM_NAME_LEN`] bytes
/// - no `/`, `\`, `..`, or `\0`
/// - all chars in `[a-zA-Z0-9_-]`
///
/// Each failure produces a byte-locked error string (see
/// `parity/fixtures/team_tools.json`).
pub(crate) fn validate_team_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() {
        return Err(ToolError::InvalidInput(
            "Team: team_name is empty".into(),
        ));
    }
    if name.len() > MAX_TEAM_NAME_LEN {
        return Err(ToolError::InvalidInput(format!(
            "Team: team_name '{name}' exceeds max length {MAX_TEAM_NAME_LEN}"
        )));
    }
    // Slash / traversal check FIRST so its error string is more specific
    // than the generic "invalid characters" message.
    if name.contains('/') || name.contains('\\') || name == ".." || name.contains("..") {
        return Err(ToolError::InvalidInput(format!(
            "Team: team_name '{name}' contains slashes or path traversal"
        )));
    }
    for ch in name.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
        if !ok {
            return Err(ToolError::InvalidInput(format!(
                "Team: team_name '{name}' contains invalid characters (allowed: {TEAM_NAME_PATTERN_DESC})"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn assert_valid(name: &str) {
        validate_team_name(name).unwrap_or_else(|_| panic!("expected '{name}' valid"));
    }

    fn assert_rejects(name: &str, fragment: &str) {
        let err = validate_team_name(name)
            .unwrap_err_or_panic_with(|| format!("expected '{name}' rejected"));
        let msg = match err {
            ToolError::InvalidInput(s) => s,
            other => panic!("unexpected error variant: {other:?}"),
        };
        assert!(
            msg.contains(fragment),
            "error '{msg}' missing fragment '{fragment}'"
        );
    }

    // Local extension to ToolError to provide an unwrap_err with custom panic.
    trait UnwrapErrOrPanic<T> {
        fn unwrap_err_or_panic_with<F: FnOnce() -> String>(self, msg: F) -> ToolError;
    }
    impl<T: std::fmt::Debug> UnwrapErrOrPanic<T> for Result<T, ToolError> {
        fn unwrap_err_or_panic_with<F: FnOnce() -> String>(self, msg: F) -> ToolError {
            match self {
                Ok(_) => panic!("{}", msg()),
                Err(e) => e,
            }
        }
    }

    #[test]
    fn accepts_simple_lowercase_name() {
        assert_valid("default");
        assert_valid("alpha");
        assert_valid("team1");
    }

    #[test]
    fn accepts_underscore_dash_digits_mixed_case() {
        assert_valid("Alpha_Beta-1");
        assert_valid("A_b-2_C");
        assert_valid("X");
    }

    #[test]
    fn rejects_empty() {
        assert_rejects("", "team_name is empty");
    }

    #[test]
    fn rejects_too_long() {
        let long: String = "a".repeat(MAX_TEAM_NAME_LEN + 1);
        assert_rejects(&long, "exceeds max length 64");
    }

    #[test]
    fn rejects_slash_or_traversal() {
        assert_rejects("a/b", "slashes or path traversal");
        assert_rejects("a\\b", "slashes or path traversal");
        assert_rejects("..", "slashes or path traversal");
        assert_rejects("a/../b", "slashes or path traversal");
    }

    #[test]
    fn rejects_invalid_chars() {
        assert_rejects("hello world", "invalid characters");
        assert_rejects("hello.world", "invalid characters");
        assert_rejects("hello!", "invalid characters");
        assert_rejects("héllo", "invalid characters"); // non-ASCII
        assert_rejects("a\0b", "invalid characters");
    }
}
