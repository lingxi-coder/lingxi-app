//! P1-12 — the background-attach first-frame stall watchdog (2.1.220).
//!
//! A `--bg` worker that never paints its first frame leaves the attacher
//! staring at a blank terminal forever. Claude Code arms a watchdog when an
//! attach session opens, and if no frame arrives inside the stall threshold it
//! restarts the worker as a resume — twice — then gives up and kills it with a
//! diagnostic instead of hanging.
//!
//! # What lives here
//!
//! The DECISION half: thresholds, the tick accounting, the gates, and the
//! give-up budget, all extracted from the 2.1.220 binary. It is deliberately
//! free of I/O so the policy can be tested exhaustively — the oracle's own
//! logic is a `setInterval` closure over mutable state, which is exactly the
//! shape that hides off-by-ones.
//!
//! # What does NOT live here, and why
//!
//! The ACTION half — SIGTERM, respawn-as-resume, SIGKILL — is not wired yet,
//! and the blocker is architectural rather than missing code.
//!
//! The oracle runs this watchdog in one process that BOTH renders the attach
//! stream and owns the worker handle (`b.onStream` for the first-frame signal,
//! `b.kill()` / dispatch for the remedy). `LingXi` splits those: the attach
//! client (`bg_attach::attach_to_socket`) sees the stream but has no respawn
//! authority, and the daemon (`commands::daemon`) owns spawn/kill/respawn but
//! has no view of worker output — its `WorkerRecord` carries pid, sockets and
//! state, never frames.
//!
//! So the missing piece is a first-frame SIGNAL crossing that split. The
//! natural home is the daemon: it already has a durable respawn counter
//! (`jobs/<short>/respawns`), an injectable supervise loop, and the gates below
//! map onto state it already tracks. The worker would stamp "first frame
//! emitted" once, and the supervise loop would run [`StallWatchdog`].
//!
//! That decision changes the roster/worker contract in an area codex is
//! actively developing, and a wrong watchdog SIGKILLs live user sessions — so
//! it is left for an explicit design pass rather than guessed at here.

/// `iIa` — watchdog tick period.
pub const STALL_TICK_MS: u64 = 1_000;
/// `wcf` — delay before the watchdog arms, after which ticking starts.
pub const STALL_ARM_DELAY_MS: u64 = 500;
/// `q9b` — default of the `tengu_bg_attach_stall_ms` flag. `0` disables.
pub const STALL_DEFAULT_MS: u64 = 5_000;
/// `j9b` — floor when the worker was launched WITH argv (a real dispatch:
/// prompt to load, session to resume), so startup is legitimately slower.
pub const STALL_FLOOR_WITH_ARGS_MS: u64 = 12_000;
/// Floor for a bare launch (`gy().length === 0`).
pub const STALL_FLOOR_BARE_MS: u64 = 2_000;
/// `W9b` — how long a respawn waits for the killed worker to actually exit.
pub const RESPAWN_EXIT_WAIT_MS: u64 = 6_000;
/// Respawns tolerated before giving up (`if (attempt >= 2)`).
pub const STALL_RESPAWN_BUDGET: i64 = 2;

/// Banner shown when the worker is being restarted.
pub const NOT_RESPONDING_BANNER: &str = "Session not responding \u{2014} restarting it\u{2026}";
/// Banner shown once the respawn budget is spent.
pub const KEEPS_STALLING_BANNER: &str = "Session keeps stalling at startup.";
/// `kill` reason recorded when the budget is spent.
pub const KEEPS_STALLING_KILL_REASON: &str = "session keeps stalling at startup";

/// The `ESTALLED:` diagnostic written alongside [`KEEPS_STALLING_BANNER`].
#[must_use]
pub fn estalled_notice(short: &str, log_path: &str) -> String {
    format!(
        "ESTALLED: Session {short} keeps stalling at startup \u{2014} check {log_path} for logs."
    )
}

/// `G9b()` — the effective stall threshold in milliseconds.
///
/// `flag_ms` is the `tengu_bg_attach_stall_ms` value; **0 disables the
/// watchdog entirely** and is passed through rather than floored, which is the
/// one case where the floors must NOT apply.
#[must_use]
pub fn stall_threshold_ms(flag_ms: u64, has_launch_args: bool) -> u64 {
    if flag_ms == 0 {
        return 0;
    }
    let floor = if has_launch_args {
        STALL_FLOOR_WITH_ARGS_MS
    } else {
        STALL_FLOOR_BARE_MS
    };
    floor.max(flag_ms)
}

/// `G = W === 0 ? 0 : Math.max(1, Math.ceil((W - wcf) / iIa))` — ticks that
/// must elapse before the watchdog fires.
///
/// The arm delay is subtracted because ticking only starts after it.
#[must_use]
pub fn ticks_before_fire(threshold_ms: u64) -> u32 {
    if threshold_ms == 0 {
        return 0;
    }
    let after_arm = threshold_ms.saturating_sub(STALL_ARM_DELAY_MS);
    let ticks = after_arm.div_ceil(STALL_TICK_MS);
    u32::try_from(ticks).unwrap_or(u32::MAX).max(1)
}

/// Conditions under which the watchdog must stay its hand.
///
/// Every one of these means the worker's silence is EXPECTED, so firing would
/// kill something that is behaving correctly.
//
// Four independent booleans mirroring the oracle's four-clause guard
// (`!isKilling && !isRetiring && !isBooting && launch.mode!=="exec"`).
// Collapsing them into a set would obscure which condition suppressed a fire,
// which is the first thing anyone debugging a spurious respawn wants to know.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StallGates {
    /// A kill is already in flight.
    pub is_killing: bool,
    /// The worker is being retired.
    pub is_retiring: bool,
    /// The worker is still booting.
    pub is_booting: bool,
    /// `launch.mode === "exec"` — a one-shot exec paints no TUI frame at all,
    /// so a frame-based watchdog would fire on every healthy run.
    pub is_exec_launch: bool,
}

impl StallGates {
    /// May the watchdog act?
    #[must_use]
    pub fn allow_fire(self) -> bool {
        !self.is_killing && !self.is_retiring && !self.is_booting && !self.is_exec_launch
    }
}

/// What the watchdog decided on this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallDecision {
    /// Keep waiting.
    Wait,
    /// Restart the worker as a resume; emit `tengu_bg_attach_stall_respawn`.
    Respawn,
    /// Budget spent — emit `tengu_bg_attach_stall_gave_up` and SIGKILL.
    GiveUp,
}

/// The tick accounting behind the first-frame watchdog.
#[derive(Debug, Clone)]
pub struct StallWatchdog {
    ticks_needed: u32,
    elapsed_ticks: u32,
}

impl StallWatchdog {
    /// Arm a watchdog for `threshold_ms`. A threshold of 0 yields a watchdog
    /// that never fires.
    #[must_use]
    pub fn new(threshold_ms: u64) -> Self {
        Self {
            ticks_needed: ticks_before_fire(threshold_ms),
            elapsed_ticks: 0,
        }
    }

    /// Whether this watchdog can ever fire (`threshold_ms > 0`).
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.ticks_needed > 0
    }

    /// Advance one tick.
    ///
    /// `since_last_tick_ms` is real elapsed time, not the nominal period: a tick
    /// gap over `3 × STALL_TICK_MS` means the machine SLEPT, and the counter is
    /// reset rather than credited. Without that, closing a laptop for a minute
    /// would restart every attached session on wake.
    ///
    /// `respawns` is the worker's current `attachStallRespawns`.
    pub fn tick(
        &mut self,
        since_last_tick_ms: u64,
        gates: StallGates,
        respawns: i64,
    ) -> StallDecision {
        if since_last_tick_ms > STALL_TICK_MS * 3 {
            self.elapsed_ticks = 0;
        }
        self.elapsed_ticks = self.elapsed_ticks.saturating_add(1);

        if !self.is_armed() || self.elapsed_ticks < self.ticks_needed || !gates.allow_fire() {
            return StallDecision::Wait;
        }
        if respawns >= STALL_RESPAWN_BUDGET {
            StallDecision::GiveUp
        } else {
            StallDecision::Respawn
        }
    }

    /// The first frame arrived — disarm.
    pub fn saw_frame(&mut self) {
        self.ticks_needed = 0;
        self.elapsed_ticks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_floors_depend_on_whether_the_worker_got_argv() {
        // A dispatched worker has real startup work, so it gets the 12s floor.
        assert_eq!(stall_threshold_ms(STALL_DEFAULT_MS, true), 12_000);
        // A bare launch only gets 2s of grace, so the default flag wins.
        assert_eq!(stall_threshold_ms(STALL_DEFAULT_MS, false), 5_000);
        // A flag above both floors is used as-is.
        assert_eq!(stall_threshold_ms(30_000, true), 30_000);
        // A flag below the floor is raised to it.
        assert_eq!(stall_threshold_ms(100, false), 2_000);
    }

    #[test]
    fn a_zero_flag_disables_the_watchdog_and_is_not_floored() {
        // The one case the floors must not touch: 0 means OFF, and flooring it
        // to 2s would turn a kill-switch into a 2-second hair trigger.
        assert_eq!(stall_threshold_ms(0, false), 0);
        assert_eq!(stall_threshold_ms(0, true), 0);
        assert_eq!(ticks_before_fire(0), 0);
        assert!(!StallWatchdog::new(0).is_armed());
    }

    #[test]
    fn tick_count_subtracts_the_arm_delay_and_rounds_up() {
        // ceil((5000-500)/1000) = 5
        assert_eq!(ticks_before_fire(5_000), 5);
        // ceil((12000-500)/1000) = 12
        assert_eq!(ticks_before_fire(12_000), 12);
        // Never zero for an enabled watchdog, however small the threshold.
        assert_eq!(ticks_before_fire(1), 1);
        assert_eq!(ticks_before_fire(STALL_ARM_DELAY_MS), 1);
    }

    fn open() -> StallGates {
        StallGates::default()
    }

    #[test]
    fn fires_only_once_the_full_tick_budget_has_elapsed() {
        let mut w = StallWatchdog::new(5_000); // 5 ticks
        for i in 1..5 {
            assert_eq!(
                w.tick(STALL_TICK_MS, open(), 0),
                StallDecision::Wait,
                "tick {i} must not fire"
            );
        }
        assert_eq!(w.tick(STALL_TICK_MS, open(), 0), StallDecision::Respawn);
    }

    #[test]
    fn a_sleep_gap_resets_the_counter_instead_of_crediting_it() {
        let mut w = StallWatchdog::new(5_000);
        for _ in 0..4 {
            assert_eq!(w.tick(STALL_TICK_MS, open(), 0), StallDecision::Wait);
        }
        // Laptop closed. Without the reset this tick would fire and restart a
        // perfectly healthy session on wake.
        assert_eq!(
            w.tick(STALL_TICK_MS * 3 + 1, open(), 0),
            StallDecision::Wait
        );
        // The budget restarts from scratch — but note the oracle does
        // `if (gap) P = 0; P++;`, so the gap tick itself already counts as 1.
        // Three more waits reach 4, and the next one fires at 5.
        for _ in 0..3 {
            assert_eq!(w.tick(STALL_TICK_MS, open(), 0), StallDecision::Wait);
        }
        assert_eq!(w.tick(STALL_TICK_MS, open(), 0), StallDecision::Respawn);
    }

    #[test]
    fn a_gap_of_exactly_three_ticks_does_not_reset() {
        // The oracle's predicate is `> iIa*3`, not `>=`.
        let mut w = StallWatchdog::new(5_000);
        for _ in 0..4 {
            w.tick(STALL_TICK_MS, open(), 0);
        }
        assert_eq!(w.tick(STALL_TICK_MS * 3, open(), 0), StallDecision::Respawn);
    }

    #[test]
    fn the_budget_is_two_respawns_then_give_up() {
        let fire = |respawns| {
            let mut w = StallWatchdog::new(5_000);
            let mut out = StallDecision::Wait;
            for _ in 0..5 {
                out = w.tick(STALL_TICK_MS, open(), respawns);
            }
            out
        };
        assert_eq!(fire(0), StallDecision::Respawn);
        assert_eq!(fire(1), StallDecision::Respawn);
        assert_eq!(fire(2), StallDecision::GiveUp);
        assert_eq!(fire(9), StallDecision::GiveUp);
    }

    #[test]
    fn every_gate_suppresses_firing() {
        // Each of these means the silence is EXPECTED; firing would kill a
        // worker that is behaving correctly.
        for gates in [
            StallGates {
                is_killing: true,
                ..Default::default()
            },
            StallGates {
                is_retiring: true,
                ..Default::default()
            },
            StallGates {
                is_booting: true,
                ..Default::default()
            },
            StallGates {
                is_exec_launch: true,
                ..Default::default()
            },
        ] {
            let mut w = StallWatchdog::new(5_000);
            for _ in 0..8 {
                assert_eq!(
                    w.tick(STALL_TICK_MS, gates, 0),
                    StallDecision::Wait,
                    "{gates:?} must suppress"
                );
            }
            assert!(!gates.allow_fire());
        }
        assert!(open().allow_fire());
    }

    #[test]
    fn the_first_frame_disarms_it_permanently() {
        let mut w = StallWatchdog::new(5_000);
        w.tick(STALL_TICK_MS, open(), 0);
        w.saw_frame();
        assert!(!w.is_armed());
        for _ in 0..20 {
            assert_eq!(w.tick(STALL_TICK_MS, open(), 0), StallDecision::Wait);
        }
    }

    #[test]
    fn the_user_facing_strings_are_byte_exact() {
        assert_eq!(
            NOT_RESPONDING_BANNER,
            "Session not responding \u{2014} restarting it\u{2026}"
        );
        assert_eq!(KEEPS_STALLING_BANNER, "Session keeps stalling at startup.");
        assert_eq!(
            KEEPS_STALLING_KILL_REASON,
            "session keeps stalling at startup"
        );
        assert_eq!(
            estalled_notice("cafe0001", "/tmp/jobs/cafe0001"),
            "ESTALLED: Session cafe0001 keeps stalling at startup \u{2014} check /tmp/jobs/cafe0001 for logs."
        );
    }

    #[test]
    fn constants_match_the_binary() {
        assert_eq!(STALL_TICK_MS, 1_000); // iIa
        assert_eq!(STALL_ARM_DELAY_MS, 500); // wcf
        assert_eq!(STALL_DEFAULT_MS, 5_000); // q9b
        assert_eq!(STALL_FLOOR_WITH_ARGS_MS, 12_000); // j9b
        assert_eq!(RESPAWN_EXIT_WAIT_MS, 6_000); // W9b
        assert_eq!(STALL_RESPAWN_BUDGET, 2);
    }
}
