//! Parity baseline vs Claude Code 2.1.198 (M0 of
//! `docs/superpowers/specs/2026-07-02-cc2.1.198-alignment-plan.md`).
//!
//! Two jobs:
//! 1. Pin the real-binary CLI surface as golden fixtures (captured from the
//!    local `claude` 2.1.198 binary on 2026-07-02).
//! 2. Hold a structured checklist of every 2.1.196–2.1.198 changelog entry.
//!    Each entry must carry an explicit disposition — `Implemented`,
//!    `Mission("M<n>")`, or `Divergence(reason)`. The drift test fails on any
//!    `Unknown`, so a new checklist entry can never be silently ignored.
//!
//! As missions land they flip their entries to `Implemented`; M14 asserts no
//! `Mission` dispositions remain.

const ROOT_HELP: &str = include_str!("../src/parity/fixtures/cc_2_1_198_root_help.txt");
const AGENTS_HELP: &str = include_str!("../src/parity/fixtures/cc_2_1_198_agents_help.txt");
const MCP_HELP: &str = include_str!("../src/parity/fixtures/cc_2_1_198_mcp_help.txt");
const GATEWAY_HELP: &str = include_str!("../src/parity/fixtures/cc_2_1_198_gateway_help.txt");

/// Where a changelog entry stands in LingXi.
// `Implemented` and `Unknown` are unconstructed until missions land / drift
// appears — both are part of the checklist contract, not dead code.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Disposition {
    /// Behavior verified present in lingxi-code.
    Implemented,
    /// Tracked by a mission in the alignment plan; not yet landed.
    Mission(&'static str),
    /// Deliberately not ported; reason recorded.
    Divergence(&'static str),
    /// Not yet triaged — the drift test rejects these.
    Unknown,
}

struct Entry {
    version: &'static str,
    item: &'static str,
    disposition: Disposition,
}

use Disposition::{Divergence, Mission};

/// Every entry of the official changelog for 2.1.196–2.1.198
/// (snapshot in the alignment plan doc, fetched 2026-07-02).
const CHECKLIST: &[Entry] = &[
    // ── 2.1.198 ────────────────────────────────────────────────────────────
    Entry { version: "2.1.198", item: "Claude in Chrome generally available", disposition: Divergence("browser extension + Anthropic cloud service; not portable") },
    // M8 landed: `cli::agents_notify` ports the binary's FleetView pipeline —
    // `$1f` band diff (@222691698: blocked→`agent_needs_input` with `needs`
    // clamped at 120 UTF-16 units, completed→`agent_completed` "finished"/
    // "failed", stopped/self-driving silent, `i2 = "send a prompt to start"`
    // fresh-session sentinel keeps the prev band), `Fhe` band derivation and
    // the `Pon` label — all unit-locked. The agents view (commands/agents.rs
    // `NotificationWatcher`) loads settings hooks standalone and fires
    // `HookEvent::Notification { message, kind }` (= binary `TQ` @219455460
    // payload `{message, title?, notification_type}`, matcher on
    // `notification_type`) on a 1s registry poll; the interactive session
    // writes its own `sessions/<pid>.json` status transitions
    // (`SessionRegistration::update_status` = binary `mvn`: busy/idle/waiting
    // + `waitingFor: "permission prompt"`, statusUpdatedAt on change).
    Entry { version: "2.1.198", item: "Notification hook agent_needs_input/agent_completed for claude agents sessions", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "/dataviz bundled skill", disposition: Divergence("bundled skill content, not core behavior") },
    Entry { version: "2.1.198", item: "Gateway: anthropicAws upstream provider", disposition: Divergence("no enterprise gateway runtime in lingxi; client-side model fallback (tengu_model_fallback_triggered) already ported") },
    Entry { version: "2.1.198", item: "Gateway: model-not-found advances failover chain", disposition: Divergence("no enterprise gateway runtime in lingxi; client-side model fallback (tengu_model_fallback_triggered) already ported") },
    // M8 landed: the binary implements this PROMPT-DRIVEN — `_ff()`
    // (@219583413) appends a `# Background Session` system-prompt section to
    // bg jobs whose shipping paragraph directs the model to commit, push and
    // `gh pr create --draft` on worktree completion (no programmatic git
    // flow exists in the binary; the no-remote case is likewise instruction:
    // "Skip the PR only if … there's no remote to push to (then commit and
    // say where the work is)"). Ported byte-faithfully (lingxi env names +
    // `.lingxi/worktrees/`) as `orchestrator::prompt::bg_session`, gated on
    // `LINGXI_SESSION_KIND=bg` + `LINGXI_JOB_DIR`, spliced after
    // output_style before `# Context management` (binary `cx()` order);
    // byte-locked incl. the isolation-none no-shipping branch. Depth: the
    // `--bg` dispatcher that SETS those envs is still open (the isolation
    // config fallback of `HAo()` lands with it).
    Entry { version: "2.1.198", item: "Background agents auto commit/push/draft PR on worktree completion", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "Explore agent inherits session model capped at opus", disposition: Mission("M10") },
    Entry { version: "2.1.198", item: "Subagents + compaction inherit extended thinking config", disposition: Mission("M10") },
    Entry { version: "2.1.198", item: "Mid-response transient network errors retry with backoff (ECONNRESET etc.)", disposition: Mission("M12") },
    Entry { version: "2.1.198", item: "Sandbox classifier: dedupe repeated same-host requests", disposition: Mission("M11") },
    // M8 landed: REAL lingxi bug found + fixed — `register_self_contained_
    // handlers` registered `LocalBashHandler` with its default
    // `NoopStatusSink`, so a finished background bash task's registry status
    // stayed `Running` forever (the stuck panel). Now a deferred
    // `RegistryStatusSink` (bash terminal status + new `set_exit_code`
    // write-through) is threaded at registration and bound at engine-desktop
    // (5.46f); locked by tasks::registry_test::
    // `finished_background_bash_task_does_not_stay_running`. Resume half:
    // lingxi's task registry is in-process with no disk persistence — a
    // resumed session builds a FRESH registry, so a stale Running row cannot
    // survive resume by construction.
    Entry { version: "2.1.198", item: "Task panels: no stuck Running after finish/resume", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "Teammate API-error reports failed to lead; stuck teammate wake-retries on message", disposition: Mission("M9") },
    Entry { version: "2.1.198", item: "/diff panel refreshes on external branch switch/commit", disposition: Mission("M13") },
    // M6 landed: tui_core::render::markdown_table vertical-format clamp
    // (long labels hard-broken, over-long words hard-wrapped, all lines ≤
    // frame − SAFETY_MARGIN); locked by overflow tests in markdown_table.rs +
    // tui-rata message.rs `wide_markdown_table_never_overflows_narrow_frame`.
    Entry { version: "2.1.198", item: "Markdown tables no longer overflow right border in fullscreen", disposition: Disposition::Implemented },
    // M2 landed: llm_client::aws_auth (ZBd/gIn/t2d port: trust gate + STS
    // probe + QBd=30s cooldown + 3-min timeout) + the V_c/G_c/s_f drive-loop
    // trigger (Ygf=2) in ApiService; settings keys awsAuthRefresh /
    // awsCredentialExport / gcpAuthRefresh in engine SettingsJson.
    Entry { version: "2.1.198", item: "awsAuthRefresh runs automatically on STS expiry (anthropicAws/Mantle)", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "macOS Local Network entitlements for background agent sessions", disposition: Divergence("macOS app packaging/entitlements, not core runtime") },
    Entry { version: "2.1.198", item: "/desktop cwd after entering+exiting a worktree", disposition: Mission("M13") },
    // M7 N/A-with-evidence: the cc fix was in the remote-session WS transport
    // (`useRemoteSession`/`updateReconnectingStatus`, binary strings
    // @206609/221616 regions). lingxi's agents view
    // (apps/cli/commands/agents.rs + tui-rata agents_screen) reads the LOCAL
    // sessions/jobs registry and attaches by respawning `lingxi-cli --resume`
    // — no socket, no timed reconnect loop. The only "Reconnecting" strings
    // in lingxi live in the MCP client (mcp/src/{registry,connection}.rs), a
    // different subsystem.
    Entry { version: "2.1.198", item: "Agents view: no Reconnecting spam every ~52s", disposition: Divergence("no remote-session transport in lingxi's agents view; the ~52s reconnect log loop cannot exist (attach = local respawn, registry = local files)") },
    // M3 landed: `Argv::validate_background_args` rejects --bg/--background ×
    // --print/-p up front in `run_cli` (byte-locked `pof` message @218854391,
    // stderr + exit 1); e2e-locked in apps/cli/tests/cli_argv_errors.rs.
    Entry { version: "2.1.198", item: "--bg + --print/-p rejected up front", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "Workflow progress view keeps earliest agents", disposition: Mission("M9") },
    Entry { version: "2.1.198", item: ".claude/rules conditional rules load via symlinked paths (realpath)", disposition: Mission("M11") },
    // M6 partial: tui_core::render::osc8 ports the binary's OSC 8 emitters
    // (`Bpl` hyperlink bytes, `jx()` support gate, URL wrapping incl. scheme)
    // with byte-locked tests — but tui-rata draws through a ratatui cell
    // Buffer that cannot carry escape sequences, so emission awaits a raw
    // scrollback print path. Stays Mission until wired end-to-end.
    Entry { version: "2.1.198", item: "Cmd+click opens URLs in fullscreen in Warp; double-click selects whole URL", disposition: Mission("M6") },
    Entry { version: "2.1.198", item: "Plan mode auto-allows read-only tools when session starts in plan mode", disposition: Mission("M11") },
    Entry { version: "2.1.198", item: "/branch default fork name from first real prompt, not compaction summary", disposition: Mission("M12") },
    Entry { version: "2.1.198", item: "Focus mode: subagents in activity summary; completed notifications fold to one count", disposition: Mission("M10") },
    Entry { version: "2.1.198", item: "Syntax highlighting upgraded to highlight.js 11", disposition: Divergence("lingxi renders via syntect; visual-equivalence accepted, highlight.js is a JS-runtime dependency") },
    // M6 landed: tui_core::key_hint ports the binary's `Pct()` probe (local
    // macOS, or LC_TERMINAL=iTerm2 / TERM_PROGRAM=Apple_Terminal|iTerm.app
    // forwarded over SSH) + the `nop` modifier table (Opt/Alt, Cmd/Super);
    // wired into the tui-rata footer and locked by key_hint + app.rs tests.
    Entry { version: "2.1.198", item: "opt/cmd hints instead of alt/super for Mac over SSH", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "Retry UX: error reason after 2nd attempt; status page link when overloaded", disposition: Mission("M12") },
    // M7 seam: the standalone agents view (tui-rata `agents_screen`) has no
    // mountable sign-in dialog yet — the OAuth `/connect`/login flow lives in
    // the full TUI runtime. Seam = a `/login` key route in
    // `AgentsScreenState::on_key` once an auth dialog is mountable from the
    // thin view loop.
    Entry { version: "2.1.198", item: "/login opens sign-in dialog from claude agents view", disposition: Mission("M7") },
    Entry { version: "2.1.198", item: "Launcher-agent messages are task direction, never user approval", disposition: Mission("M10") },
    // M4 landed: `/agents` now returns the binary's removed-wizard guidance
    // (`Otf` text, `.lingxi`-branded paths) with the verbatim `(removed) …`
    // description (`commands/core/src/agents.rs`, `core_description`, /help
    // golden re-locked). M4 also wired --agents/--agent/--plugin-dir/
    // --from-pr/--prompt-suggestions/--effort + the `gateway` subcommand
    // surface and locked `ultrareview`'s unsupported exit.
    Entry { version: "2.1.198", item: "Removed /agents wizard", disposition: Disposition::Implemented },
    // ── 2.1.197 ────────────────────────────────────────────────────────────
    Entry { version: "2.1.197", item: "Sonnet 5 default model, native 1M context, promo $2/$10 per Mtok through 2026-08-31", disposition: Mission("M1") },
    // ── 2.1.196 ────────────────────────────────────────────────────────────
    Entry { version: "2.1.196", item: "Org default models (Org default/Role default in /model)", disposition: Mission("M1") },
    Entry { version: "2.1.196", item: "Readable default session names at start", disposition: Mission("M13") },
    // M6 partial: tui_core::render::osc8::file_link ports the binary's `t2()`
    // (file:// OSC 8 target, plain-path display) with byte-locked tests;
    // wiring blocked on the same raw print path as the URL entry above.
    Entry { version: "2.1.196", item: "Clickable file attachments (Cmd/Ctrl-click reveals in Finder)", disposition: Mission("M6") },
    Entry { version: "2.1.196", item: "mcp list/get do not spawn repo-self-approved servers; Pending approval shown", disposition: Mission("M11") },
    // M8 N/A-with-evidence: the cc fix is in the daemon's job-WAKE transcript
    // probe — `s9e` (@206707678) renames an unreadable transcript to
    // `<sid>.orphaned-<ts>-<uuid8>.jsonl` instead of deleting. lingxi has NO
    // wake-that-probes-transcript path (jobs are read-only, M7) and the
    // session crate + resume loader contain zero transcript
    // unlink/remove_file calls — the deletion bug cannot exist here. The
    // set-aside rename ports together with the `--bg` wake path when the job
    // writer lands.
    Entry { version: "2.1.196", item: "Waking a background job never deletes its transcript (set aside instead)", disposition: Divergence("no bg-job wake/transcript-probe path exists in lingxi (jobs read-only) and no code path deletes transcripts; binary set-aside rename (s9e @206707678) ports with the future --bg wake") },
    Entry { version: "2.1.196", item: "Rate-limit warning flicker + over-counted telemetry with parallel requests", disposition: Mission("M12") },
    Entry { version: "2.1.196", item: "No duplicate recap after schema-rejected StructuredOutput retry", disposition: Mission("M9") },
    Entry { version: "2.1.196", item: "PowerShell git diff/grep, egrep/fgrep, quoted | patterns: exit 1 is not failure", disposition: Mission("M13") },
    // M7 seam: this is the IN-APP side panel (task panel inside the running
    // TUI), not the standalone `claude agents` view M7 landed. lingxi's
    // in-app background-task footer/dialog reads `tui::multiagent::
    // PollerFeed`; focus/subagent-type/running-status fixes apply there once
    // the ratatui app grows the panel.
    Entry { version: "2.1.196", item: "Agents side panel: focus, subagent types, running status fixes", disposition: Mission("M7") },
    // M7 landed: `agents::run` gates the interactive view on the bypass
    // request (`Cli::bypass_requested` = binary `nis`), runs the root
    // refusal (`permission::enforce_bypass_safety`, byte-locked message =
    // binary `refuseBypassUnderRoot` @223855350) and mounts the byte-locked
    // `tui::startup_bypass` disclaimer unless
    // `skipDangerousModePermissionPrompt` is already set
    // (`ensureAgentsBypassConsent` parity); the attach respawn forwards
    // `--dangerously-skip-permissions`/`--permission-mode` to the dispatched
    // session (locked by `commands::agents::tests::
    // attach_forwards_bypass_to_dispatched_session`).
    Entry { version: "2.1.196", item: "claude agents --dangerously-skip-permissions shows disclaimer, applies bypass", disposition: Disposition::Implemented },
    Entry { version: "2.1.196", item: "Remote sessions auto-resume after server restart", disposition: Divergence("Anthropic cloud/remote infra; LingXi has no remote-session backend") },
    Entry { version: "2.1.196", item: "/cd moved sessions don't reappear in old dir's resume list (special chars)", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "plugin validate: local '.' plugins included; all error classes reported", disposition: Mission("M13") },
    // M6 partial: tui-rata Esc semantics now match cc 2.1.196/198 (Esc never
    // quits; interrupts a running turn with the "esc to interrupt" hint;
    // Esc-Esc clears composer text with "Esc again to clear"; double-tap Esc
    // at an idle empty prompt reaches the rewind entry point). LingXi has no
    // file-checkpoint/rewind subsystem, so the entry point surfaces the
    // binary's "Nothing to rewind to yet." line instead of the messageSelector
    // menu ("Restore code and conversation" / "Restore conversation" /
    // "Restore code"). Stays Mission until the rewind menu itself exists.
    Entry { version: "2.1.196", item: "Esc Esc at idle prompt opens rewind menu (regression fix)", disposition: Mission("M6") },
    Entry { version: "2.1.196", item: "MCP OAuth: no-scope request must not ask for full scopes_supported catalog", disposition: Mission("M11") },
    Entry { version: "2.1.196", item: "/context shows real token counts on Bedrock", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "/deep-research verifier failures reported as unverified, not all-refuted", disposition: Divergence("bundled skill content, not core behavior") },
    Entry { version: "2.1.196", item: "Plugin dependency pins honored for local-folder git-backed marketplaces", disposition: Mission("M13") },
    // M7 landed: `cli::agents_registry::merged_state` ports the binary's
    // `mGf` (@223855350) — a terminal outcome beats a stale `blocked` tempo,
    // so a row can never flip Done ↔ Needs-input (locked by
    // `merged_state_terminal_beats_blocked_tempo_no_done_needs_input_flip`);
    // tui-rata `agents_screen` bands/labels are byte-locked to `NZo`/`MFc`
    // (@222792125: "Ready for review"/"Needs input"/"Working"/"Completed")
    // and result rows referencing a PR show `PR #N` via the `Hon` token-scan
    // port (`extract_pr_number`, locked by `lines_show_pr_reference_on_row`).
    Entry { version: "2.1.196", item: "Agents view status: no Done/Needs-input flip; Needs attention; PR link", disposition: Disposition::Implemented },
    Entry { version: "2.1.196", item: "Voice dictation: no swallowed spaces / spurious recording on fast typing", disposition: Divergence("platform voice input; LingXi voice stack differs (sherpa)") },
    // M8 partial/deferred: survival requires the daemon-backed bg worker
    // runtime (`--bg` dispatch + `jobs/<short>/state.json` writer) lingxi
    // does not have — bg shells/tasks are in-process today and die with the
    // process. What IS in place: the M7 registry's orphan hygiene
    // (`read_live_sessions` reaps dead-pid records on every read, and the
    // M8 status writer keeps records fresh). Seam: a job-store writer +
    // detached worker process; Windows handoff = platform divergence.
    Entry { version: "2.1.196", item: "Background sessions survive process stop/restart/update (incl. Windows handoff)", disposition: Mission("M8") },
    // M8 deferred with the entry above: auto-resume = the FleetView respawn
    // (`needsRespawn`/`ees`: outcome failure|stopped && terminal && !exec,
    // then `kon`→`vRt` relaunch) over daemon-backed jobs; without the job
    // writer/worker runtime there is nothing to respawn. lingxi's agents
    // view already re-attaches via `lingxi-cli --resume <sid>` on Enter.
    Entry { version: "2.1.196", item: "Workers killed by daemon restart auto-resume when agents view opens", disposition: Mission("M8") },
    Entry { version: "2.1.196", item: "/code-review workflow: five cleanup finders merged into one (-25% tokens)", disposition: Divergence("bundled workflow content, not core behavior") },
    Entry { version: "2.1.196", item: "Per-frame rendering skips no-op subtree walks during streaming", disposition: Disposition::Implemented },
    Entry { version: "2.1.196", item: "Streaming idle watchdog on by default (5 min, env kill-switch)", disposition: Mission("M12") },
    Entry { version: "2.1.196", item: "Remote Control disabled when ANTHROPIC_BASE_URL is non-Anthropic", disposition: Mission("M13") },
    // M7 seam: lingxi's foreground TUI composer has no ←-on-empty entry
    // point yet (the binary's `[PERF:bg-leftarrow-start]` path respawns
    // `claude agents`). What DID land is the attach side: leaving an
    // attached session returns to the agents view instead of the shell
    // (`agents::run_agents_view` remounts with fresh rows after the resumed
    // child exits — the 2.1.198 half of the fix). Remaining seam: a Left-key
    // route in the tui-rata composer that suspends the session and execs
    // `lingxi-cli agents`.
    Entry { version: "2.1.196", item: "Agents view opens with single ← from foreground sessions", disposition: Mission("M7") },
];

/// Valid mission ids from the alignment plan.
const MISSIONS: &[&str] = &[
    "M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M10", "M11", "M12", "M13",
];

#[test]
fn every_changelog_entry_is_triaged() {
    for e in CHECKLIST {
        assert!(
            e.disposition != Disposition::Unknown,
            "untriaged changelog entry ({} — {})",
            e.version,
            e.item
        );
        if let Mission(m) = e.disposition {
            assert!(
                MISSIONS.contains(&m),
                "entry '{}' references unknown mission {m}",
                e.item
            );
        }
        if let Divergence(reason) = e.disposition {
            assert!(
                !reason.is_empty(),
                "divergence for '{}' must state a reason",
                e.item
            );
        }
    }
}

#[test]
fn checklist_is_complete_snapshot() {
    // Locked counts from the official changelog snapshot (2026-07-02):
    // 31 entries in 2.1.198, 1 in 2.1.197, 27 in 2.1.196.
    let count = |v: &str| CHECKLIST.iter().filter(|e| e.version == v).count();
    assert_eq!(count("2.1.198"), 31, "2.1.198 entry count drifted");
    assert_eq!(count("2.1.197"), 1, "2.1.197 entry count drifted");
    assert_eq!(count("2.1.196"), 27, "2.1.196 entry count drifted");
}

// ── Fixture sanity: the golden captures really are the 2.1.198 surface ──────

#[test]
fn root_help_fixture_pins_flags_missions_will_wire() {
    for flag in [
        "--safe-mode",
        "--bare",
        "--agents <json>",
        "--agent <agent>",
        "--plugin-dir <path>",
        "--from-pr",
        "--prompt-suggestions",
        "--effort <level>",
        "--no-session-persistence",
        "--bg, --background",
    ] {
        assert!(
            ROOT_HELP.contains(flag),
            "expected `{flag}` in captured 2.1.198 root help"
        );
    }
}

#[test]
fn gateway_subcommand_surface_is_pinned() {
    assert!(GATEWAY_HELP.contains("Usage: claude gateway [options]"));
    assert!(GATEWAY_HELP.contains("--config <path>"));
    assert!(GATEWAY_HELP.contains("Run the enterprise auth/telemetry gateway"));
}

#[test]
fn agents_and_mcp_help_fixtures_nonempty() {
    assert!(AGENTS_HELP.contains("agents"), "agents help fixture looks wrong");
    assert!(MCP_HELP.contains("mcp"), "mcp help fixture looks wrong");
}
