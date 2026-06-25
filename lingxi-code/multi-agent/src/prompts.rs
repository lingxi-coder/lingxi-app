//! Prompt templates for the dual-LLM multi-agent execution strategy.
//!
//! The four phase prompts (implementer / reviewer / reviser / arbiter) are
//! authored verbatim under `multi-agent/prompts/*.md` per the design doc
//! (§Prompt 模板) and embedded at compile time via [`include_str!`] so the
//! crate carries them with no runtime file dependency.

/// Prompt given to each independent candidate implementation agent.
pub const IMPLEMENTER: &str = include_str!("../prompts/implementer.md");

/// Prompt given to a candidate when reviewing the competing implementation.
///
/// The reviewer is document-only and MUST NOT edit files; that constraint is
/// enforced at the host level, not by this prompt alone.
pub const REVIEWER: &str = include_str!("../prompts/reviewer.md");

/// Prompt given to a candidate when revising its own branch from a review.
pub const REVISER: &str = include_str!("../prompts/reviser.md");

/// Prompt given to the arbiter when selecting a winner (or rejecting both).
pub const ARBITER: &str = include_str!("../prompts/arbiter.md");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_prompts_are_non_empty() {
        for p in [IMPLEMENTER, REVIEWER, REVISER, ARBITER] {
            assert!(!p.trim().is_empty());
        }
    }

    #[test]
    fn prompts_carry_their_phase_intent() {
        assert!(IMPLEMENTER.contains("independent implementation agent"));
        assert!(REVIEWER.contains("Do not edit files."));
        assert!(REVISER.contains("Only modify your assigned worktree."));
        assert!(ARBITER.contains("You are the arbiter."));
        assert!(ARBITER.contains("reject both"));
    }
}
