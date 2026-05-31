//! `teamMemSaved` — team-memory-saved system-line segment.
//!
//! Literal lock (claude-code `teamMemSaved.ts` `teamMemSavedPart`): returns
//! `Some("{count} team {memory|memories}")`; `None` when count is 0. A pure
//! segment helper consumed by a memory-saved system line (no TUI consumer yet
//! — UI-first).

/// Build the team-memory-saved segment. `None` when `team_count == 0`.
#[must_use]
pub fn team_mem_saved_segment(team_count: u64) -> Option<String> {
    if team_count == 0 {
        return None;
    }
    let noun = if team_count == 1 {
        "memory"
    } else {
        "memories"
    };
    Some(format!("{team_count} team {noun}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_none() {
        assert_eq!(team_mem_saved_segment(0), None);
    }

    #[test]
    fn singular_and_plural() {
        assert_eq!(team_mem_saved_segment(1).as_deref(), Some("1 team memory"));
        assert_eq!(
            team_mem_saved_segment(5).as_deref(),
            Some("5 team memories")
        );
    }
}
