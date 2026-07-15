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
