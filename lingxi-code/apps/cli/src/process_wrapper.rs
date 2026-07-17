//! Shared process-wrapper support for CLI self-spawns.
//!
//! Claude Code honors `CLAUDE_CODE_PROCESS_WRAPPER` for child/self launches.
//! LingXi also accepts the branded `LINGXI_CODE_PROCESS_WRAPPER`, with LingXi
//! taking precedence. The wrapper is prepended to the command argv; callers then
//! spawn `argv[0]` with the remaining args.

/// Resolve the active process wrapper from the environment.
///
/// The value is intentionally split on shell whitespace. This keeps the helper
/// dependency-free; wrapper paths containing spaces should be exposed through a
/// small shim script.
#[must_use]
pub(crate) fn process_wrapper_tokens_from_env() -> Option<Vec<String>> {
    let raw = std::env::var("LINGXI_CODE_PROCESS_WRAPPER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CLAUDE_CODE_PROCESS_WRAPPER")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })?;
    let tokens: Vec<String> = raw.split_whitespace().map(ToString::to_string).collect();
    (!tokens.is_empty()).then_some(tokens)
}

/// Prepend the configured process wrapper, if any, to an argv vector.
#[must_use]
pub(crate) fn wrap_argv(argv: Vec<String>) -> Vec<String> {
    if let Some(mut wrapper) = process_wrapper_tokens_from_env() {
        wrapper.extend(argv);
        wrapper
    } else {
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    // The process-wrapper env vars are process-global; serialize the tests that
    // mutate them so they don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard<'a> {
        _lock: MutexGuard<'a, ()>,
    }

    impl EnvGuard<'_> {
        fn new() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::remove_var("LINGXI_CODE_PROCESS_WRAPPER");
            std::env::remove_var("CLAUDE_CODE_PROCESS_WRAPPER");
            Self { _lock: guard }
        }
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            std::env::remove_var("LINGXI_CODE_PROCESS_WRAPPER");
            std::env::remove_var("CLAUDE_CODE_PROCESS_WRAPPER");
        }
    }

    #[test]
    fn no_wrapper_leaves_argv_untouched() {
        let _g = EnvGuard::new();
        let argv = vec!["lingxi-cli".to_string(), "--resume".to_string()];
        assert_eq!(wrap_argv(argv.clone()), argv);
    }

    #[test]
    fn lingxi_wrapper_is_prepended() {
        let _g = EnvGuard::new();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "sandbox --net none");
        let argv = vec!["lingxi-cli".to_string(), "--resume".to_string()];
        assert_eq!(
            wrap_argv(argv),
            vec![
                "sandbox".to_string(),
                "--net".to_string(),
                "none".to_string(),
                "lingxi-cli".to_string(),
                "--resume".to_string(),
            ]
        );
    }

    #[test]
    fn lingxi_takes_precedence_over_claude() {
        let _g = EnvGuard::new();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "lingxi-wrap");
        std::env::set_var("CLAUDE_CODE_PROCESS_WRAPPER", "claude-wrap");
        assert_eq!(
            wrap_argv(vec!["exe".to_string()]),
            vec!["lingxi-wrap".to_string(), "exe".to_string()]
        );
    }

    #[test]
    fn claude_wrapper_used_when_lingxi_absent() {
        let _g = EnvGuard::new();
        std::env::set_var("CLAUDE_CODE_PROCESS_WRAPPER", "claude-wrap --flag");
        assert_eq!(
            wrap_argv(vec!["exe".to_string()]),
            vec![
                "claude-wrap".to_string(),
                "--flag".to_string(),
                "exe".to_string()
            ]
        );
    }

    #[test]
    fn blank_wrapper_is_ignored() {
        let _g = EnvGuard::new();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "   ");
        assert_eq!(process_wrapper_tokens_from_env(), None);
        let argv = vec!["exe".to_string()];
        assert_eq!(wrap_argv(argv.clone()), argv);
    }
}
