//! `goal_checkin` — the goal check-in interstitial, NEW in Claude Code 2.1.238
//! (`CC_VER=2.1.220 oracle.sh count 'Goal check-in'` → **0**).
//!
//! When a goal (`/goal`, the session Stop hook that keeps looping until a
//! condition is met) is active and BACKGROUND WORK is still running, the oracle
//! does not evaluate the goal at all — it removes the goal's Stop hook for that
//! turn and defers. Deferral is not free: once the goal has been deferred for
//! longer than the check-in interval, the model is told, at turn end, that the
//! goal is still there and what is holding it up.
//!
//! Oracle anatomy (offsets into `~/.local/share/claude/versions/2.1.238`):
//!
//! * interval `Zil()` @**292038365**:
//!   ```js
//!   function Zil(){if(!it("tengu_saffron_wren",!0))return 0;
//!     return(V.CLAUDE_CODE_GOAL_CHECKIN_MINUTES??cUv)*60000}
//!   ```
//!   with `cUv = 30` @292041909. The GrowthBook default is **TRUE**, so unlike
//!   most of this family the feature is LIVE in a stock install.
//! * text builder `Szf(condition, deferredMs, tasks)` @**292038578** — see
//!   [`build_checkin_body`].
//! * deferral state machine `Tzf` @**292039748** + `wzf` @**292040130** — see
//!   [`GoalDeferralState::advance`].
//! * the deferring-task filter `_qf` @**292181184**:
//!   ```js
//!   function _qf(e){return Object.values(e).filter((t)=>!rR(t)&&
//!     !(t.type==="local_agent"&&t.agentType==="main-session")&&(xXo(t)||IXo(t)))}
//!   ```
//!   `xXo` = a non-terminal agent-ish task (`j2b = {local_agent, remote_agent,
//!   in_process_teammate, local_workflow}`), `IXo` = a non-terminal `local_bash`,
//!   `rR` = an observer agent.
//! * turn-end call site @**292174788**:
//!   ```js
//!   if(L&&M){let U=_qf(i.taskRegistry.all());
//!     if(U.length>0){ y=…the goal's Stop hook…;
//!       if(y){ i.sessionHooksRegistry.remove(zt(),"Stop",y);   // ⇐ NO evaluation this turn
//!         let W=wzf(L,U,Date.now()); _=W.nextGoal; Z=W.checkinText;
//!         if(Z!==void 0){let W=kn({content:Z,isMeta:!0});S.push(W),h.push(W),yield W}}}
//!     else if(L.deferredSince!==void 0){…clear the deferral fields…}}
//!   ```
//!
//! The check-in body is NOT `<system-reminder>`-wrapped in the oracle — it is a
//! plain `isMeta` user message (`kn({content:checkinText,isMeta:!0})`), unlike
//! the attachment family, which goes through `Zy`/`NT`. The port keeps that.
//!
//! # Divergence (reason)
//!
//! * `bzf(e)` (@292041909 `gsa(pjo(e))`) homoglyph/invisible-character
//!   sanitizes the goal condition and each task label before `Ma`-escaping. The
//!   port has no confusables table, so only the `Ma` half
//!   ([`super::sanitize::escape_reminder_html`]) is applied. The escape that
//!   matters for envelope integrity is present; the anti-spoofing pass is not.
//! * `Tzf`'s new-run detection keys on `Math.min(...tasks.map(t=>t.startTime)) >
//!   lastDeferralPassAt`. The port's Stop-hook `background_tasks` projection
//!   drops `startTime`, so [`GoalDeferralState::advance`] uses the equivalent
//!   predicate over task IDENTITY: every currently-deferring task is one that
//!   was not deferring at the previous pass. Same meaning ("this is a brand-new
//!   batch of background work"), computed from what the port carries.

/// `cUv = 30` @292041909 — the default check-in interval, in minutes.
pub const DEFAULT_CHECKIN_MINUTES: i64 = 30;

/// `dUv = 120` @292041971 — the per-task line character cap (`w5(line, dUv)`).
pub const TASK_LINE_CHAR_CAP: usize = 120;

/// The `background_tasks[].type` labels that DEFER a goal evaluation — the port
/// side of `_qf`'s `xXo(t) || IXo(t)`.
///
/// These are the labels `crate::stop_hook_snapshot::build_background_tasks`
/// emits (claude-code `O1o`), i.e. already mapped from the wire task types:
/// `local_agent`→`subagent`, `local_workflow`→`workflow`, `local_bash`→`shell`,
/// `in_process_teammate`→`teammate`, `remote_agent`→`cloud session`.
/// The snapshot's `monitor` label means `monitor_mcp` / `monitor_ws`, which
/// 2.1.263 `n3t` / `r3t` excludes. A `local_bash` monitor is projected as
/// `shell` and already participates. `MCP task` and `dream` are excluded too.
pub const DEFERRING_TASK_LABELS: &[&str] =
    &["subagent", "workflow", "shell", "teammate", "cloud session"];

/// `w5(e, t)` @281369905 — truncate to `t` UTF-16 code units and append the
/// dropped-character count:
///
/// ```js
/// function w5(e,t){if(e.length<=t)return e;let r=wo(e,t);
///   return `${r}… [+${e.length-r.length} chars]`}
/// ```
///
/// `wo` slices at a code-unit boundary (never inside a surrogate pair); the Rust
/// analogue walks `char_indices` and stops before the first char that would
/// cross the cap.
#[must_use]
pub fn truncate_with_char_count(s: &str, cap: usize) -> String {
    let total = s.encode_utf16().count();
    if total <= cap {
        return s.to_string();
    }
    let mut end = 0usize;
    let mut units = 0usize;
    for (idx, ch) in s.char_indices() {
        if units + ch.len_utf16() > cap {
            end = idx;
            break;
        }
        units += ch.len_utf16();
        end = idx + ch.len_utf8();
    }
    format!("{}\u{2026} [+{} chars]", &s[..end], total - units)
}

/// One background task that is currently deferring the goal — the port's stand-in
/// for `_qf`'s output elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferringTask {
    /// `s.id`.
    pub id: String,
    /// `efr[s.type]` — the already-mapped type label.
    pub label: String,
    /// `s.type==="local_bash"&&!isMonitor ? s.command : s.description`.
    pub detail: String,
}

/// `Szf(e, t, r).body` @292038578 — the check-in text.
///
/// ```js
/// function Szf(e,t,r){let n=Math.max(1,Math.round(t/60000)),o=Ma(bzf(e));
///  if(r.length===0)return{…,body:`Goal check-in: \xAB${o}\xBB is still active. Its evaluation was deferred for ${n} min while background work ran, and that work is no longer running (it finished or was stopped without reporting back). Continue toward the goal.`};
///  let i=r.map((s)=>{…return Ma(w5(`- ${s.id} \xB7 ${l} \xB7 ${bzf(c)}`,dUv))});
///  return{…,body:`Goal check-in: \xAB${o}\xBB is still active, and evaluation has been deferred for ${n} min because background work is still running:\n${i.join("\n")}\nCheck on their progress (e.g. read their output). If they are progressing, say so briefly and keep waiting; if they are stuck or no longer needed, fix or stop them and continue toward the goal.`}}
/// ```
///
/// `«` / `»` are U+00AB / U+00BB; the minute count is
/// `max(1, round(deferred_ms / 60000))`, so a sub-minute deferral still reads
/// "1 min".
#[must_use]
pub fn build_checkin_body(condition: &str, deferred_ms: i64, tasks: &[DeferringTask]) -> String {
    // `Math.max(1, Math.round(t/60000))`. Integer round-half-up: for a
    // non-negative `t`, `(t + 30_000) / 60_000` is exactly `Math.round(t/60000)`
    // (JS rounds `.5` toward +Infinity), with no float cast.
    let minutes = deferred_ms.max(0).saturating_add(30_000) / 60_000;
    let minutes = minutes.max(1);
    let goal = super::sanitize::escape_reminder_html(condition);
    if tasks.is_empty() {
        return format!(
            "Goal check-in: \u{ab}{goal}\u{bb} is still active. Its evaluation was deferred for \
{minutes} min while background work ran, and that work is no longer running (it finished or was \
stopped without reporting back). Continue toward the goal."
        );
    }
    let lines = tasks
        .iter()
        .map(|task| {
            super::sanitize::escape_reminder_html(&truncate_with_char_count(
                &format!("- {} \u{b7} {} \u{b7} {}", task.id, task.label, task.detail),
                TASK_LINE_CHAR_CAP,
            ))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Goal check-in: \u{ab}{goal}\u{bb} is still active, and evaluation has been deferred for \
{minutes} min because background work is still running:\n{lines}\nCheck on their progress (e.g. \
read their output). If they are progressing, say so briefly and keep waiting; if they are stuck \
or no longer needed, fix or stop them and continue toward the goal."
    )
}

/// `Zil()` @292038365 — the check-in interval in milliseconds, or `0` (disabled).
///
/// `CLAUDE_CODE_GOAL_CHECKIN_MINUTES` overrides [`DEFAULT_CHECKIN_MINUTES`];
/// setting it to `0` disables the feature, exactly like the oracle's
/// `if(n===0)return{nextGoal:e,checkinText:void 0}`. There is no GrowthBook in
/// the port and `tengu_saffron_wren` defaults to TRUE upstream, so the feature
/// is ON by default here too.
#[must_use]
pub fn checkin_interval_ms() -> i64 {
    let minutes = std::env::var("CLAUDE_CODE_GOAL_CHECKIN_MINUTES")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|m| *m >= 0)
        .unwrap_or(DEFAULT_CHECKIN_MINUTES);
    minutes.saturating_mul(60_000)
}

/// Claude Code 2.1.241 backoff: first check-in after `base`, then `2x base`,
/// then every `4x base` thereafter.
#[must_use]
pub fn next_checkin_interval_ms(base_interval_ms: i64, checkin_count: u32) -> i64 {
    let multiplier = match checkin_count {
        0 => 1,
        1 => 2,
        _ => 4,
    };
    base_interval_ms.saturating_mul(multiplier)
}

/// The deferral bookkeeping the oracle keeps on `activeGoal`
/// (`deferredSince` / `checkinCount` / `lastDeferralPassAt`).
///
/// Held session-scoped by the orchestrator rather than on
/// `lingxi_core::session::ActiveGoalState`: it is pure turn-local timing state, it
/// must never reach the persisted goal shape (the oracle explicitly STRIPS the
/// three fields before yielding the goal when nothing is deferring —
/// `let{deferredSince:Z,checkinCount:W,lastDeferralPassAt:te,...ee}=L;
/// yield{type:"active_goal",value:ee}` @292174788), and keeping it out avoids
/// touching the compaction/JSONL goal wire shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalDeferralState {
    /// `deferredSince` — when the current deferral stretch began (epoch ms).
    /// `None` ⇒ nothing is deferred.
    pub deferred_since: Option<i64>,
    /// `checkinCount` — how many check-ins this stretch has produced.
    pub checkin_count: u32,
    /// `lastDeferralPassAt` — when the last deferral pass ran (epoch ms).
    pub last_deferral_pass_at: Option<i64>,
    /// The task ids that were deferring at the last pass — the port's stand-in
    /// for `Math.min(...startTimes)` (see the module divergence note).
    pub last_deferring_ids: Vec<String>,
}

impl GoalDeferralState {
    /// `wzf(goal, tasks, now)` @292040130, with `Tzf` @292039748 inlined.
    ///
    /// ```js
    /// function Tzf(e,t,r,n){if(e.deferredSince===void 0)return{deferredSince:r,checkinCount:0,isNewRun:!0};
    ///   let o=Math.min(...t.map((s)=>s.startTime)),
    ///       i=e.lastDeferralPassAt!==void 0&&Number.isFinite(o)&&o>e.lastDeferralPassAt&&r-e.lastDeferralPassAt>n;
    ///   return i?{deferredSince:r,checkinCount:0,isNewRun:i}
    ///           :{deferredSince:e.deferredSince,checkinCount:e.checkinCount??0,isNewRun:i}}
    /// function wzf(e,t,r){let n=Zil();if(n===0)return{nextGoal:e,checkinText:void 0};
    ///   let o=Tzf(e,t,r,n),i=r-o.deferredSince;
    ///   if(i<n)return{nextGoal:{...e,deferredSince:o.deferredSince,checkinCount:o.checkinCount,lastDeferralPassAt:r},checkinText:void 0};
    ///   let s=o.checkinCount+1,a=Szf(e.condition,i,t).body;
    ///   return{nextGoal:{...e,deferredSince:r,checkinCount:s,lastDeferralPassAt:r},checkinText:a}}
    /// ```
    ///
    /// Mutates `self` in place and returns the check-in body when one fires.
    /// `interval_ms == 0` (feature disabled) leaves the state untouched.
    pub fn advance(
        &mut self,
        condition: &str,
        tasks: &[DeferringTask],
        now_ms: i64,
        interval_ms: i64,
    ) -> Option<String> {
        if interval_ms == 0 {
            return None;
        }
        // --- Tzf ---
        let deferred_since = match self.deferred_since {
            None => {
                // First pass of a stretch: start the clock, no check-in yet.
                self.deferred_since = Some(now_ms);
                self.checkin_count = 0;
                now_ms
            }
            Some(since) => {
                let all_tasks_are_new = tasks
                    .iter()
                    .all(|t| !self.last_deferring_ids.contains(&t.id));
                let is_new_run = self.last_deferral_pass_at.is_some_and(|last| {
                    all_tasks_are_new && now_ms.saturating_sub(last) > interval_ms
                });
                if is_new_run {
                    self.deferred_since = Some(now_ms);
                    self.checkin_count = 0;
                    now_ms
                } else {
                    since
                }
            }
        };
        // --- wzf ---
        self.last_deferral_pass_at = Some(now_ms);
        self.last_deferring_ids = tasks.iter().map(|t| t.id.clone()).collect();
        let deferred_for = now_ms.saturating_sub(deferred_since);
        // 2.1.263 `aJn`: new-run detection uses the base interval; backoff
        // uses the count AFTER `iJn` has reset it for a new batch.
        if deferred_for < next_checkin_interval_ms(interval_ms, self.checkin_count) {
            return None;
        }
        self.checkin_count = self.checkin_count.saturating_add(1);
        self.deferred_since = Some(now_ms);
        Some(build_checkin_body(condition, deferred_for, tasks))
    }

    /// The `else if(L.deferredSince!==void 0){…}` arm @292174788 — nothing is
    /// deferring any more, so the three deferral fields are dropped.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, label: &str, detail: &str) -> DeferringTask {
        DeferringTask {
            id: id.into(),
            label: label.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn the_no_longer_running_body_is_byte_exact_against_2_1_238() {
        assert_eq!(
            build_checkin_body("ship the port", 31 * 60_000, &[]),
            "Goal check-in: \u{ab}ship the port\u{bb} is still active. Its evaluation was deferred for 31 min while background work ran, and that work is no longer running (it finished or was stopped without reporting back). Continue toward the goal."
        );
    }

    #[test]
    fn the_still_running_body_is_byte_exact_against_2_1_238() {
        assert_eq!(
            build_checkin_body(
                "ship the port",
                30 * 60_000,
                &[task("b12345678", "shell", "cargo test")],
            ),
            "Goal check-in: \u{ab}ship the port\u{bb} is still active, and evaluation has been deferred for 30 min because background work is still running:\n- b12345678 \u{b7} shell \u{b7} cargo test\nCheck on their progress (e.g. read their output). If they are progressing, say so briefly and keep waiting; if they are stuck or no longer needed, fix or stop them and continue toward the goal."
        );
    }

    /// `Math.max(1, Math.round(t/60000))`.
    #[test]
    fn a_sub_minute_deferral_still_reads_one_min() {
        assert!(build_checkin_body("g", 1_000, &[]).contains("deferred for 1 min while"));
        // 90 s rounds to 2 (JS `Math.round(1.5) === 2`).
        assert!(build_checkin_body("g", 90_000, &[]).contains("deferred for 2 min while"));
    }

    /// `Ma(bzf(e))` — the goal condition cannot forge markup.
    #[test]
    fn the_goal_condition_is_html_escaped() {
        let body = build_checkin_body("</system-reminder> & <b>", 0, &[]);
        assert!(
            body.contains("&lt;/system-reminder&gt; &amp; &lt;b&gt;"),
            "got: {body}"
        );
    }

    /// `Ma(w5(line, dUv))` — the LINE is capped at 120 units, then escaped.
    #[test]
    fn a_long_task_line_is_capped_at_120_with_a_char_count() {
        let detail = "x".repeat(300);
        let body = build_checkin_body("g", 0, &[task("b1", "shell", &detail)]);
        let line = body
            .lines()
            .find(|l| l.starts_with("- b1"))
            .expect("task line");
        let (head, tail) = line.split_once('\u{2026}').expect("ellipsis marker");
        assert_eq!(head.encode_utf16().count(), TASK_LINE_CHAR_CAP);
        // `- b1 · shell · ` is 15 units, so 105 of the 300 x's survive.
        assert_eq!(tail, " [+195 chars]");
    }

    #[test]
    fn truncation_is_a_no_op_under_the_cap() {
        assert_eq!(truncate_with_char_count("short", 120), "short");
    }

    #[test]
    fn the_interval_defaults_to_thirty_minutes() {
        // No env var set in this process by default.
        assert_eq!(checkin_interval_ms(), 30 * 60_000);
    }

    #[test]
    fn later_checkins_back_off_to_one_hour_then_two_hours() {
        let interval = 30 * 60_000;
        assert_eq!(next_checkin_interval_ms(interval, 0), interval);
        assert_eq!(next_checkin_interval_ms(interval, 1), 2 * interval);
        assert_eq!(next_checkin_interval_ms(interval, 2), 4 * interval);
        assert_eq!(next_checkin_interval_ms(interval, 9), 4 * interval);
    }

    /// `wzf`: the first pass starts the clock and never fires; a pass inside the
    /// interval never fires; later check-ins back off 30m → 60m → 120m(cap),
    /// restarting the clock each time (`deferredSince:r`).
    #[test]
    fn checkins_back_off_after_each_fire() {
        let interval = 30 * 60_000;
        let tasks = vec![task("b1", "shell", "sleep 900")];
        let mut state = GoalDeferralState::default();

        assert!(state.advance("g", &tasks, 0, interval).is_none());
        assert_eq!(state.deferred_since, Some(0));
        assert!(state.advance("g", &tasks, interval - 1, interval).is_none());
        assert_eq!(state.checkin_count, 0);

        let text = state
            .advance("g", &tasks, interval, interval)
            .expect("check-in at the interval");
        assert!(text.contains("deferred for 30 min because"), "got: {text}");
        assert_eq!(state.checkin_count, 1);
        assert_eq!(state.deferred_since, Some(interval));

        // The clock restarted with a 60m backoff.
        assert!(state
            .advance("g", &tasks, 2 * interval - 1, interval)
            .is_none());
        let second = state
            .advance("g", &tasks, 3 * interval, interval)
            .expect("second check-in at one hour");
        assert!(
            second.contains("deferred for 60 min because"),
            "got: {second}"
        );
        assert_eq!(state.checkin_count, 2);

        // The cap is 120m for every later check-in.
        assert!(state
            .advance("g", &tasks, 7 * interval - 1, interval)
            .is_none());
        let third = state
            .advance("g", &tasks, 7 * interval, interval)
            .expect("third check-in at two hours");
        assert!(
            third.contains("deferred for 120 min because"),
            "got: {third}"
        );
        assert_eq!(state.checkin_count, 3);
    }

    /// `Tzf`'s `isNewRun`: a brand-new batch of background work more than one
    /// interval after the last pass resets the stretch, so the next check-in is
    /// a full interval away rather than immediate.
    #[test]
    fn a_brand_new_batch_after_a_long_gap_resets_the_stretch() {
        let interval = 30 * 60_000;
        let mut state = GoalDeferralState::default();
        let first = vec![task("b1", "shell", "one")];
        assert!(state.advance("g", &first, 0, interval).is_none());

        // Long gap, and every deferring task is new ⇒ new run.
        let second = vec![task("b2", "shell", "two")];
        assert!(
            state
                .advance("g", &second, 3 * interval, interval)
                .is_none(),
            "a new run restarts the clock instead of firing immediately"
        );
        assert_eq!(state.deferred_since, Some(3 * interval));
        assert_eq!(state.checkin_count, 0);
    }

    #[test]
    fn a_new_batch_resets_backoff_after_the_base_interval() {
        let interval = 30 * 60_000;
        let mut state = GoalDeferralState::default();
        let first = vec![task("b1", "shell", "one")];
        assert!(state.advance("g", &first, 0, interval).is_none());
        assert!(state.advance("g", &first, interval, interval).is_some());
        assert_eq!(state.checkin_count, 1);

        // 45 minutes after the last pass is greater than the base interval,
        // but less than the old batch's 60-minute backed-off interval.
        let second = vec![task("b2", "shell", "two")];
        let restart = interval + interval * 3 / 2;
        assert!(state.advance("g", &second, restart, interval).is_none());
        assert_eq!(state.deferred_since, Some(restart));
        assert_eq!(state.checkin_count, 0);
        assert!(state
            .advance("g", &second, restart + interval - 1, interval)
            .is_none());
        assert!(state
            .advance("g", &second, restart + interval, interval)
            .is_some());
    }

    /// The SAME task still running across a long gap is NOT a new run — that is
    /// exactly the case the check-in exists for.
    #[test]
    fn the_same_task_across_a_long_gap_is_not_a_new_run() {
        let interval = 30 * 60_000;
        let mut state = GoalDeferralState::default();
        let tasks = vec![task("b1", "shell", "one")];
        assert!(state.advance("g", &tasks, 0, interval).is_none());
        let text = state.advance("g", &tasks, 3 * interval, interval);
        assert!(
            text.is_some(),
            "the long-running task must trigger a check-in"
        );
    }

    #[test]
    fn a_zero_interval_disables_the_feature_entirely() {
        let mut state = GoalDeferralState::default();
        let tasks = vec![task("b1", "shell", "one")];
        assert!(state.advance("g", &tasks, 10_000_000, 0).is_none());
        assert_eq!(state, GoalDeferralState::default(), "no state is touched");
    }

    #[test]
    fn clear_drops_every_deferral_field() {
        let mut state = GoalDeferralState {
            deferred_since: Some(1),
            checkin_count: 3,
            last_deferral_pass_at: Some(2),
            last_deferring_ids: vec!["b1".into()],
        };
        state.clear();
        assert_eq!(state, GoalDeferralState::default());
    }
}
