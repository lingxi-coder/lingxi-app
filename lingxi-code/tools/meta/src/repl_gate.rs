//! REPL-mode gating registry (port of `REPLTool/constants.ts:23-46`).
//!
//! When REPL mode is enabled, the 8 primitive tools listed in
//! [`REPL_ONLY_TOOLS`] are hidden from Claude's direct wire tool list and are
//! reachable only via the REPL. This module is a *gating-data* port: it
//! encodes the env gate and the tool-name set. There is no Rust REPL crate, so
//! the JS REPL VM execution context (running primitives inside a sandbox) does
//! not exist here — this module only provides the gating predicates.
//!
//! **Wiring hook (out of scope for this batch):** the application of this gate
//! — filtering the wire tool list — is a conversation-layer concern. A later
//! batch should, when [`is_repl_mode_enabled`] returns true, drop the tools
//! whose registry name is in [`REPL_ONLY_TOOLS`] from `build_wire_tools` (the
//! orchestrator's `conversation.rs`). The names here are the live Rust registry
//! keys (`Read`/`Write`/`Edit`/`Glob`/`Grep`/`Bash`/`NotebookEdit`/`Agent`),
//! so a membership check against a tool's registry name is sufficient.
//!
//! Divergence from TS: the TS `REPL_ONLY_TOOLS` is a `Set` built from the tool
//! `*_TOOL_NAME` constants; we use string literals matching those same wire
//! names rather than importing the per-tool name constants from other crates
//! (scope-locked, and the names are fixture-locked wire identifiers anyway).

use traits::env::is_env_truthy;

/// Tools that are only accessible via REPL when REPL mode is enabled.
///
/// When REPL mode is on, these tools are hidden from Claude's direct use,
/// forcing Claude to use the REPL for batch operations. Mirrors the TS
/// `REPL_ONLY_TOOLS` set (`REPLTool/constants.ts:37-46`). Each entry is the
/// live Rust builtin registry key.
pub const REPL_ONLY_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Glob",
    "Grep",
    "Bash",
    "NotebookEdit",
    "Agent",
];

/// Whether REPL mode is enabled, porting `isReplModeEnabled`
/// (`REPLTool/constants.ts:23-30`).
///
/// REPL mode is default-on for ants in the interactive CLI (opt out with
/// `CLAUDE_CODE_REPL=0`). The legacy `CLAUDE_REPL_MODE=1` also forces it on.
///
/// SDK entrypoints are NOT defaulted on — SDK consumers script direct tool
/// calls (Bash, Read, etc.) and REPL mode hides those tools.
///
/// Logic (1:1 with TS):
/// - `CLAUDE_CODE_REPL` defined-falsy (`0`/`false`/`no`/`off`) → `false`
/// - else `CLAUDE_REPL_MODE` truthy (`1`/`true`/`yes`/`on`) → `true`
/// - else `USER_TYPE == "ant" && CLAUDE_CODE_ENTRYPOINT == "cli"`
#[must_use]
pub fn is_repl_mode_enabled() -> bool {
    if is_env_defined_falsy(std::env::var("CLAUDE_CODE_REPL").ok().as_deref()) {
        return false;
    }
    if is_env_truthy(std::env::var("CLAUDE_REPL_MODE").ok().as_deref()) {
        return true;
    }
    std::env::var("USER_TYPE").ok().as_deref() == Some("ant")
        && std::env::var("CLAUDE_CODE_ENTRYPOINT").ok().as_deref() == Some("cli")
}

/// Port of `isEnvDefinedFalsy` (`utils/envUtils.ts:39-47`): a defined,
/// non-empty value that normalizes (lowercase + trim) to one of
/// `0`/`false`/`no`/`off`. An undefined or empty value is NOT falsy (the TS
/// returns `false` for `undefined` and for `''`).
fn is_env_defined_falsy(env_var: Option<&str>) -> bool {
    match env_var {
        None => false,
        Some(v) => {
            if v.is_empty() {
                return false;
            }
            matches!(v.to_lowercase().trim(), "0" | "false" | "no" | "off")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Env tests mutate process-global state; serialize them so concurrent
    /// test threads don't observe each other's vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Clears every env var the gate reads, so each case starts from a known
    /// baseline regardless of the ambient environment.
    fn clear_env() {
        std::env::remove_var("CLAUDE_CODE_REPL");
        std::env::remove_var("CLAUDE_REPL_MODE");
        std::env::remove_var("USER_TYPE");
        std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");
    }

    #[test]
    fn repl_only_tools_has_expected_eight_names() {
        assert_eq!(
            REPL_ONLY_TOOLS,
            &["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit", "Agent"]
        );
        assert_eq!(REPL_ONLY_TOOLS.len(), 8);
    }

    #[test]
    fn claude_code_repl_falsy_forces_off() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        // Even with ant+cli (which would otherwise enable), the falsy opt-out wins.
        std::env::set_var("USER_TYPE", "ant");
        std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli");
        std::env::set_var("CLAUDE_REPL_MODE", "1");
        std::env::set_var("CLAUDE_CODE_REPL", "0");
        assert!(!is_repl_mode_enabled());
        clear_env();
    }

    #[test]
    fn claude_repl_mode_truthy_forces_on() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("CLAUDE_REPL_MODE", "1");
        assert!(is_repl_mode_enabled());
        clear_env();
    }

    #[test]
    fn ant_cli_default_enables() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("USER_TYPE", "ant");
        std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli");
        assert!(is_repl_mode_enabled());
        clear_env();
    }

    #[test]
    fn non_ant_or_non_cli_default_disabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_env();
        // No env at all → off.
        assert!(!is_repl_mode_enabled());
        // ant but not cli → off.
        std::env::set_var("USER_TYPE", "ant");
        std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "sdk-ts");
        assert!(!is_repl_mode_enabled());
        clear_env();
        // cli but not ant → off.
        std::env::set_var("USER_TYPE", "external");
        std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli");
        assert!(!is_repl_mode_enabled());
        clear_env();
    }

    #[test]
    fn env_truthy_variants() {
        assert!(is_env_truthy(Some("1")));
        assert!(is_env_truthy(Some("true")));
        assert!(is_env_truthy(Some("YES")));
        assert!(is_env_truthy(Some(" On ")));
        assert!(!is_env_truthy(Some("0")));
        assert!(!is_env_truthy(Some("")));
        assert!(!is_env_truthy(None));
    }

    #[test]
    fn env_defined_falsy_variants() {
        assert!(is_env_defined_falsy(Some("0")));
        assert!(is_env_defined_falsy(Some("false")));
        assert!(is_env_defined_falsy(Some("NO")));
        assert!(is_env_defined_falsy(Some(" off ")));
        assert!(!is_env_defined_falsy(Some("1")));
        // Undefined and empty are NOT falsy (matches TS).
        assert!(!is_env_defined_falsy(None));
        assert!(!is_env_defined_falsy(Some("")));
    }
}
