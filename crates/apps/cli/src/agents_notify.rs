//! Background-agent band-transition notifications (M8 cc2.1.198).
//!
//! Ports the real 2.1.198 binary's FleetView notification pipeline:
//!
//! * `$1f` (@222691698) — diff the previous band map against the current job
//!   rows; a transition INTO `blocked` raises an `agent_needs_input`
//!   notification, a transition INTO `completed` raises `agent_completed`
//!   (unless the outcome is `stopped` or the job is self-driving/loopish).
//!   A `blocked` row whose `needs` is the fresh-session sentinel
//!   `"send a prompt to start"` KEEPS its previous band (`i` in `$1f`) so a
//!   just-dispatched empty session never fires "needs your input".
//! * `Fhe` (@209972919 region) — derive the band from the persisted job
//!   state. The FleetView call site passes NO live peer status
//!   (`Fhe(lt.state)` @222750113), so the `busy`/`waiting` arms are inert
//!   here: terminal → `completed`, `tempo === "blocked"` → `blocked`, else
//!   `active`.
//! * Row filter (@222750113): only daemon-backed, non-exec jobs are watched
//!   (`lt.state.backend==="daemon" && !ISe(lt.state)`, where `ISe` =
//!   `template==="exec" && respawnFlags.length===0` @209973132).
//! * `Pon` (@222792125 region) — the job display label: explicit `name`
//!   (control chars stripped, whitespace collapsed), else the first ≤3 words
//!   of the intent clamped to 25 display columns, else the
//!   "current session"/"new session"/template fallbacks.
//! * `mf` (@215961557) — UTF-16 truncation with `…`, used to clamp the
//!   `needs` text at `B1f = 120` units.
//!
//! Everything here is pure; the firing side lives in
//! `commands/agents.rs` (the lingxi FleetView equivalent), which feeds each
//! notification to the user's `Notification` hook with the byte-faithful
//! `notification_type` (binary `TQ` @219455460: `{...base,
//! hook_event_name:"Notification", message, title, notification_type}`).

use crate::agents_registry::{job_is_loopish, job_is_terminal, terminal_outcome, JobState};
use std::collections::HashMap;

/// `i2` (@ the `$1f` module) — the `needs` sentinel a freshly-dispatched
/// background session carries before its first prompt. A `blocked` row with
/// this sentinel keeps its previous band and never notifies.
pub const NEEDS_SEND_PROMPT_SENTINEL: &str = "send a prompt to start";

/// `B1f = 120` — UTF-16 length the `needs` text is clamped to in the
/// `agent_needs_input` message.
pub const NEEDS_TRUNCATE_UNITS: usize = 120;

/// The three `$1f` bands (`Fhe` outputs). Distinct from the four VIEW bands
/// (`review`/`blocked`/`working`/`done`) — notifications diff on this
/// coarser lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBand {
    /// Job is live and not blocked.
    Active,
    /// Job needs the user (tempo `blocked`).
    Blocked,
    /// Job reached a terminal outcome.
    Completed,
}

impl AgentBand {
    /// Stable string form (used only for tests/debugging; the binary keys the
    /// prev-map on the raw band strings).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
        }
    }
}

/// `Fhe(state)` with no live peer status (the FleetView notification call
/// site): terminal-and-not-loopish-success → `Completed`; `tempo ===
/// "blocked"` → `Blocked`; else `Active`.
#[must_use]
pub fn derive_band(job: &JobState) -> AgentBand {
    if job_is_terminal(job)
        && !(terminal_outcome(&job.state) == Some("success") && job_is_loopish(job))
    {
        return AgentBand::Completed;
    }
    if job.tempo.as_deref() == Some("blocked") {
        return AgentBand::Blocked;
    }
    AgentBand::Active
}

/// One watched job row — the `$1f` input shape
/// (`{id, band, label, needs, outcome, selfDriving}` @222750113).
#[derive(Debug, Clone)]
pub struct NotifyRow {
    /// Job short id (the prev-map key).
    pub id: String,
    /// Current band (`Fhe(state)`).
    pub band: AgentBand,
    /// Display label (`Pon(state, isCurrent)`).
    pub label: String,
    /// What a blocked job needs (`state.needs`).
    pub needs: Option<String>,
    /// Terminal outcome (`$re(state.state)`): `success`/`failure`/`stopped`.
    pub outcome: Option<&'static str>,
    /// `lDe(state)` — loopish/self-driving jobs never notify completion.
    pub self_driving: bool,
}

/// One notification to deliver — feeds the `Notification` hook (and, in the
/// binary, the OS notifier `QQ`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentNotification {
    /// Human-readable message (byte-faithful `$1f` templates).
    pub message: String,
    /// `"agent_needs_input"` or `"agent_completed"` — the wire
    /// `notification_type` (also the hook matcher query).
    pub notification_type: &'static str,
}

/// Build the watched rows from the job store — the FleetView filter + map
/// (@222750113): `backend === "daemon"`, not an exec one-shot (`ISe`).
/// `current_job_id` is the job id of THIS process's own session, if any
/// (`lt.id===t` feeds `Pon`'s "current session" fallback; the standalone
/// agents view has none).
#[must_use]
pub fn notify_rows(jobs: &[(String, JobState)], current_job_id: Option<&str>) -> Vec<NotifyRow> {
    jobs.iter()
        .filter(|(_, job)| {
            job.backend.as_deref() == Some("daemon")
                && !(job.template.as_deref() == Some("exec") && job.respawn_flags.is_empty())
        })
        .map(|(short, job)| NotifyRow {
            id: short.clone(),
            band: derive_band(job),
            label: job_label(job, current_job_id == Some(short.as_str())),
            needs: job.needs.clone(),
            outcome: terminal_outcome(&job.state),
            self_driving: job_is_loopish(job),
        })
        .collect()
}

/// `$1f(e, t)` — diff `prev` (band by job id) against `rows`; return the next
/// band map and the notifications the transitions raise.
#[must_use]
pub fn detect_transitions(
    prev: &HashMap<String, AgentBand>,
    rows: &[NotifyRow],
) -> (HashMap<String, AgentBand>, Vec<AgentNotification>) {
    let mut next = HashMap::new();
    let mut notifications = Vec::new();
    for row in rows {
        let seen = prev.get(&row.id).copied();
        // `i` — a blocked row still waiting for its FIRST prompt keeps the
        // previous band (when one exists) and never notifies.
        let fresh_blocked = row.band == AgentBand::Blocked
            && row.needs.as_deref() == Some(NEEDS_SEND_PROMPT_SENTINEL);
        next.insert(
            row.id.clone(),
            match seen {
                Some(s) if fresh_blocked => s,
                _ => row.band,
            },
        );
        if seen.is_none() || seen == Some(row.band) || fresh_blocked {
            continue;
        }
        match row.band {
            AgentBand::Blocked => notifications.push(AgentNotification {
                message: match row.needs.as_deref() {
                    Some(needs) => format!(
                        "{} needs your input: {}",
                        row.label,
                        truncate_utf16(needs, NEEDS_TRUNCATE_UNITS)
                    ),
                    None => format!("{} needs your input", row.label),
                },
                notification_type: "agent_needs_input",
            }),
            AgentBand::Completed => {
                if row.outcome != Some("stopped") && !row.self_driving {
                    notifications.push(AgentNotification {
                        message: format!(
                            "{} {}",
                            row.label,
                            if row.outcome == Some("failure") {
                                "failed"
                            } else {
                                "finished"
                            }
                        ),
                        notification_type: "agent_completed",
                    });
                }
            }
            AgentBand::Active => {}
        }
    }
    (next, notifications)
}

/// `mf(e, t)` (@215961557) — clamp `e` to `t` UTF-16 code units, appending
/// `…`. JS operates on UTF-16: `if (e.length <= t) return e; let n = t - 1;
/// if (isHighSurrogate(e.charCodeAt(n-1))) n--; return e.slice(0, n) + "…"`.
#[must_use]
pub fn truncate_utf16(s: &str, max_units: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() <= max_units {
        return s.to_string();
    }
    let mut n = max_units.saturating_sub(1);
    // Never split a surrogate pair: dropping the low surrogate would leave a
    // lone high surrogate at the cut.
    if n >= 1 && (0xD800..=0xDBFF).contains(&units[n - 1]) {
        n -= 1;
    }
    let mut out = String::from_utf16_lossy(&units[..n]);
    out.push('\u{2026}');
    out
}

/// `PZo` — strip C0/C1 control chars, collapse whitespace runs to one space,
/// trim (the same clean the registry's `sanitize_name` applies, but keeping
/// an empty result as empty rather than `None`).
fn clean(raw: &str) -> String {
    let stripped: String = raw
        .chars()
        .filter(|c| {
            !matches!(*c,
                '\u{0}'..='\u{8}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
        })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Display width of `s` (binary `an` = `Bun.stringWidth(e,
/// {ambiguousIsNarrow: true})`).
fn width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}

/// `Pon(e, t)` — the job display label:
///
/// 1. explicit `name` → cleaned name;
/// 2. else the first words of `displayIntent ?? intent`: >3 words → first 3
///    + `…`; the result clamped to 25 display columns (char-accumulating
///    clamp at ≤24 columns + `…`);
/// 3. empty intent → `"current session"` (this process's own job),
///    `"new session"` (a working `bg`/`claude` template), else the cleaned
///    template name.
///
/// Depth note: the binary additionally passes the intent through its secret
/// REDACTION pass (`Mc`) and segments by GRAPHEME (`Intl.Segmenter`) for the
/// column clamp; lingxi has no shared redaction seam here and clamps by
/// `char`, which only diverges on multi-codepoint grapheme clusters inside
/// >25-column labels.
#[must_use]
pub fn job_label(job: &JobState, is_current: bool) -> String {
    if let Some(name) = job.name.as_deref() {
        let cleaned = clean(name);
        if !cleaned.is_empty() || !name.is_empty() {
            return cleaned;
        }
    }
    const MAX_COLS: usize = 25;
    let intent = job
        .display_intent
        .as_deref()
        .or(job.intent.as_deref())
        .unwrap_or("");
    let cleaned_intent = clean(intent);
    let words: Vec<&str> = cleaned_intent
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        if is_current {
            return "current session".to_string();
        }
        // `Iie.name` — the FleetView default template's agentType is
        // `"claude"` (binary `YXt` @217235865).
        if matches!(job.template.as_deref(), Some("bg") | Some("claude")) && job.state == "working"
        {
            return "new session".to_string();
        }
        return clean(job.template.as_deref().unwrap_or(""));
    }
    let joined = if words.len() > 3 {
        format!("{}\u{2026}", words[..3].join(" "))
    } else {
        words.join(" ")
    };
    if width(&joined) <= MAX_COLS {
        return joined;
    }
    let mut out = String::new();
    let mut cols = 0usize;
    for ch in joined.chars() {
        let w = {
            use unicode_width::UnicodeWidthChar;
            ch.width().unwrap_or(0)
        };
        if cols + w > MAX_COLS - 1 {
            break;
        }
        out.push(ch);
        cols += w;
    }
    format!("{out}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(state: &str, tempo: Option<&str>) -> JobState {
        JobState {
            state: state.to_string(),
            tempo: tempo.map(str::to_string),
            backend: Some("daemon".to_string()),
            ..JobState::default()
        }
    }

    fn row(id: &str, band: AgentBand) -> NotifyRow {
        NotifyRow {
            id: id.to_string(),
            band,
            label: format!("job {id}"),
            needs: None,
            outcome: match band {
                AgentBand::Completed => Some("success"),
                _ => None,
            },
            self_driving: false,
        }
    }

    #[test]
    fn derive_band_matches_fhe_without_peer_status() {
        // Fhe(state) — no live status arg at the FleetView call site.
        assert_eq!(
            derive_band(&job("working", Some("active"))),
            AgentBand::Active
        );
        assert_eq!(
            derive_band(&job("working", Some("blocked"))),
            AgentBand::Blocked
        );
        assert_eq!(
            derive_band(&job("done", Some("idle"))),
            AgentBand::Completed
        );
        assert_eq!(derive_band(&job("failed", None)), AgentBand::Completed);
        // Terminal state with a still-active tempo is not terminal (Xg).
        assert_eq!(derive_band(&job("done", Some("active"))), AgentBand::Active);
        // Loopish success stays live (lDe).
        let mut loopy = job("done", Some("idle"));
        loopy.routine = Some(serde_json::json!("nightly"));
        assert_eq!(derive_band(&loopy), AgentBand::Active);
    }

    #[test]
    fn first_observation_never_notifies() {
        // $1f: `s === void 0` → record the band, no notification (a view
        // opening onto an ALREADY-blocked job stays quiet).
        let (next, notes) = detect_transitions(&HashMap::new(), &[row("a", AgentBand::Blocked)]);
        assert!(notes.is_empty());
        assert_eq!(next["a"], AgentBand::Blocked);
    }

    #[test]
    fn active_to_blocked_fires_agent_needs_input_with_needs_text() {
        let prev = HashMap::from([("a".to_string(), AgentBand::Active)]);
        let mut r = row("a", AgentBand::Blocked);
        r.needs = Some("pick a migration strategy".to_string());
        let (_, notes) = detect_transitions(&prev, &[r]);
        assert_eq!(
            notes,
            vec![AgentNotification {
                message: "job a needs your input: pick a migration strategy".to_string(),
                notification_type: "agent_needs_input",
            }]
        );

        // Without needs text the message has no suffix.
        let (_, notes) = detect_transitions(&prev, &[row("a", AgentBand::Blocked)]);
        assert_eq!(notes[0].message, "job a needs your input");
    }

    #[test]
    fn needs_text_clamped_to_120_utf16_units() {
        let prev = HashMap::from([("a".to_string(), AgentBand::Active)]);
        let mut r = row("a", AgentBand::Blocked);
        r.needs = Some("x".repeat(200));
        let (_, notes) = detect_transitions(&prev, &[r]);
        let suffix = notes[0]
            .message
            .strip_prefix("job a needs your input: ")
            .unwrap();
        // mf: 119 units + "…" = 120 total.
        assert_eq!(suffix.encode_utf16().count(), 120);
        assert!(suffix.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_utf16_never_splits_surrogate_pairs() {
        // "😀" = 2 UTF-16 units; cutting at an odd boundary must back off.
        let s = "😀😀😀"; // 6 units
        let t = truncate_utf16(s, 4); // n = 3 → unit 2 is a high surrogate → n = 2
        assert_eq!(t, "😀\u{2026}");
        assert_eq!(truncate_utf16("abc", 120), "abc");
    }

    #[test]
    fn completed_transition_fires_finished_or_failed() {
        let prev = HashMap::from([
            ("ok".to_string(), AgentBand::Active),
            ("bad".to_string(), AgentBand::Active),
        ]);
        let mut bad = row("bad", AgentBand::Completed);
        bad.outcome = Some("failure");
        let (_, notes) = detect_transitions(&prev, &[row("ok", AgentBand::Completed), bad]);
        let msgs: Vec<&str> = notes.iter().map(|n| n.message.as_str()).collect();
        assert_eq!(msgs, ["job ok finished", "job bad failed"]);
        assert!(notes
            .iter()
            .all(|n| n.notification_type == "agent_completed"));
    }

    #[test]
    fn stopped_and_self_driving_completions_stay_silent() {
        let prev = HashMap::from([
            ("s".to_string(), AgentBand::Active),
            ("loop".to_string(), AgentBand::Active),
        ]);
        let mut stopped = row("s", AgentBand::Completed);
        stopped.outcome = Some("stopped");
        let mut loopy = row("loop", AgentBand::Completed);
        loopy.self_driving = true;
        let (_, notes) = detect_transitions(&prev, &[stopped, loopy]);
        assert!(notes.is_empty());
    }

    #[test]
    fn fresh_session_sentinel_keeps_previous_band_and_never_notifies() {
        // `i` in $1f: blocked + needs === "send a prompt to start" → keep
        // prev band (when present) and skip.
        let prev = HashMap::from([("a".to_string(), AgentBand::Active)]);
        let mut r = row("a", AgentBand::Blocked);
        r.needs = Some(NEEDS_SEND_PROMPT_SENTINEL.to_string());
        let (next, notes) = detect_transitions(&prev, &[r.clone()]);
        assert!(notes.is_empty());
        assert_eq!(next["a"], AgentBand::Active); // prev band survives

        // With NO previous entry the row's own band is recorded.
        let (next, notes) = detect_transitions(&HashMap::new(), &[r]);
        assert!(notes.is_empty());
        assert_eq!(next["a"], AgentBand::Blocked);
    }

    #[test]
    fn unchanged_band_is_silent() {
        let prev = HashMap::from([("a".to_string(), AgentBand::Blocked)]);
        let (_, notes) = detect_transitions(&prev, &[row("a", AgentBand::Blocked)]);
        assert!(notes.is_empty());
    }

    #[test]
    fn notify_rows_watch_daemon_backed_non_exec_jobs_only() {
        let daemon = job("working", Some("active"));
        let mut exec = job("working", Some("active"));
        exec.template = Some("exec".to_string());
        let mut local = job("working", Some("active"));
        local.backend = None;
        // An exec job WITH respawn flags is watched (ISe requires both).
        let mut exec_respawn = job("working", Some("active"));
        exec_respawn.template = Some("exec".to_string());
        exec_respawn.respawn_flags = vec!["--model".to_string()];
        let rows = notify_rows(
            &[
                ("d".to_string(), daemon),
                ("e".to_string(), exec),
                ("l".to_string(), local),
                ("r".to_string(), exec_respawn),
            ],
            None,
        );
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["d", "r"]);
    }

    #[test]
    fn job_label_prefers_name_then_intent_words_then_fallbacks() {
        let mut j = job("working", Some("active"));
        j.name = Some("  fix\u{7} the   bug ".to_string());
        assert_eq!(job_label(&j, false), "fix the bug");

        let mut j = job("working", Some("active"));
        j.intent = Some("audit the sandbox network proxy end to end".to_string());
        // >3 words → first 3 + …
        assert_eq!(job_label(&j, false), "audit the sandbox\u{2026}");

        let mut j = job("working", Some("active"));
        j.intent = Some("short task".to_string());
        assert_eq!(job_label(&j, false), "short task");

        // Empty intent fallbacks.
        let j = job("working", Some("active"));
        assert_eq!(job_label(&j, true), "current session");
        let mut j = job("working", Some("active"));
        j.template = Some("bg".to_string());
        assert_eq!(job_label(&j, false), "new session");
        let mut j = job("blocked", Some("blocked"));
        j.template = Some("reviewer".to_string());
        assert_eq!(job_label(&j, false), "reviewer");
    }

    #[test]
    fn job_label_clamps_to_25_columns() {
        let mut j = job("working", Some("active"));
        j.intent = Some("supercalifragilisticexpialidocious refactor".to_string());
        let label = job_label(&j, false);
        assert!(label.ends_with('\u{2026}'));
        use unicode_width::UnicodeWidthStr;
        assert!(label.width() <= 25, "label too wide: {label:?}");
    }
}
