//! `TeamStatus` footer (claude-code `teams/TeamStatus.tsx`): `{n} teammate[s]`
//! + optional ` · Enter to view`. Hidden when 0; excludes the `team-lead`.

use crate::multiagent::state::WorkerRow;

/// ` · Enter to view` hint (space + U+00B7 + space + text).
const VIEW_HINT: &str = " \u{00B7} Enter to view";

/// The team footer pill. `None` when there are no teammates (excluding the
/// `team-lead`). `with_hint` appends the Enter-to-view affordance.
#[must_use]
pub fn render_team_footer(workers: &[WorkerRow], with_hint: bool) -> Option<String> {
    let n = workers.iter().filter(|w| w.name != "team-lead").count();
    if n == 0 {
        return None;
    }
    let noun = if n == 1 { "teammate" } else { "teammates" };
    let mut s = format!("{n} {noun}");
    if with_hint {
        s.push_str(VIEW_HINT);
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(name: &str) -> WorkerRow {
        WorkerRow {
            agent_id: "a1".into(),
            name: name.into(),
            agent_type: "explorer".into(),
            status: "working".into(),
        }
    }

    #[test]
    fn hidden_when_empty_or_only_lead() {
        assert_eq!(render_team_footer(&[], false), None);
        assert_eq!(render_team_footer(&[w("team-lead")], false), None);
    }

    #[test]
    fn count_and_hint() {
        assert_eq!(
            render_team_footer(&[w("alice")], false).as_deref(),
            Some("1 teammate")
        );
        assert_eq!(
            render_team_footer(&[w("alice"), w("bob")], false).as_deref(),
            Some("2 teammates")
        );
        assert_eq!(
            render_team_footer(&[w("alice")], true).as_deref(),
            Some("1 teammate \u{00B7} Enter to view")
        );
    }

    #[test]
    fn excludes_lead_from_count() {
        assert_eq!(
            render_team_footer(&[w("team-lead"), w("alice")], false).as_deref(),
            Some("1 teammate")
        );
    }
}
