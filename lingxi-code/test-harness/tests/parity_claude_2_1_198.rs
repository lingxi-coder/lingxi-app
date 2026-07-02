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
    Entry { version: "2.1.198", item: "Notification hook agent_needs_input/agent_completed for claude agents sessions", disposition: Mission("M8") },
    Entry { version: "2.1.198", item: "/dataviz bundled skill", disposition: Divergence("bundled skill content, not core behavior") },
    Entry { version: "2.1.198", item: "Gateway: anthropicAws upstream provider", disposition: Divergence("no enterprise gateway runtime in lingxi; client-side model fallback (tengu_model_fallback_triggered) already ported") },
    Entry { version: "2.1.198", item: "Gateway: model-not-found advances failover chain", disposition: Divergence("no enterprise gateway runtime in lingxi; client-side model fallback (tengu_model_fallback_triggered) already ported") },
    Entry { version: "2.1.198", item: "Background agents auto commit/push/draft PR on worktree completion", disposition: Mission("M8") },
    Entry { version: "2.1.198", item: "Explore agent inherits session model capped at opus", disposition: Mission("M10") },
    Entry { version: "2.1.198", item: "Subagents + compaction inherit extended thinking config", disposition: Mission("M10") },
    Entry { version: "2.1.198", item: "Mid-response transient network errors retry with backoff (ECONNRESET etc.)", disposition: Mission("M12") },
    Entry { version: "2.1.198", item: "Sandbox classifier: dedupe repeated same-host requests", disposition: Mission("M11") },
    Entry { version: "2.1.198", item: "Task panels: no stuck Running after finish/resume", disposition: Mission("M8") },
    Entry { version: "2.1.198", item: "Teammate API-error reports failed to lead; stuck teammate wake-retries on message", disposition: Mission("M9") },
    Entry { version: "2.1.198", item: "/diff panel refreshes on external branch switch/commit", disposition: Mission("M13") },
    Entry { version: "2.1.198", item: "Markdown tables no longer overflow right border in fullscreen", disposition: Mission("M6") },
    // M2 landed: llm_client::aws_auth (ZBd/gIn/t2d port: trust gate + STS
    // probe + QBd=30s cooldown + 3-min timeout) + the V_c/G_c/s_f drive-loop
    // trigger (Ygf=2) in ApiService; settings keys awsAuthRefresh /
    // awsCredentialExport / gcpAuthRefresh in engine SettingsJson.
    Entry { version: "2.1.198", item: "awsAuthRefresh runs automatically on STS expiry (anthropicAws/Mantle)", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "macOS Local Network entitlements for background agent sessions", disposition: Divergence("macOS app packaging/entitlements, not core runtime") },
    Entry { version: "2.1.198", item: "/desktop cwd after entering+exiting a worktree", disposition: Mission("M13") },
    Entry { version: "2.1.198", item: "Agents view: no Reconnecting spam every ~52s", disposition: Mission("M7") },
    // M3 landed: `Argv::validate_background_args` rejects --bg/--background ×
    // --print/-p up front in `run_cli` (byte-locked `pof` message @218854391,
    // stderr + exit 1); e2e-locked in apps/cli/tests/cli_argv_errors.rs.
    Entry { version: "2.1.198", item: "--bg + --print/-p rejected up front", disposition: Disposition::Implemented },
    Entry { version: "2.1.198", item: "Workflow progress view keeps earliest agents", disposition: Mission("M9") },
    Entry { version: "2.1.198", item: ".claude/rules conditional rules load via symlinked paths (realpath)", disposition: Mission("M11") },
    Entry { version: "2.1.198", item: "Cmd+click opens URLs in fullscreen in Warp; double-click selects whole URL", disposition: Mission("M6") },
    Entry { version: "2.1.198", item: "Plan mode auto-allows read-only tools when session starts in plan mode", disposition: Mission("M11") },
    Entry { version: "2.1.198", item: "/branch default fork name from first real prompt, not compaction summary", disposition: Mission("M12") },
    Entry { version: "2.1.198", item: "Focus mode: subagents in activity summary; completed notifications fold to one count", disposition: Mission("M10") },
    Entry { version: "2.1.198", item: "Syntax highlighting upgraded to highlight.js 11", disposition: Mission("M6") },
    Entry { version: "2.1.198", item: "opt/cmd hints instead of alt/super for Mac over SSH", disposition: Mission("M6") },
    Entry { version: "2.1.198", item: "Retry UX: error reason after 2nd attempt; status page link when overloaded", disposition: Mission("M12") },
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
    Entry { version: "2.1.196", item: "Clickable file attachments (Cmd/Ctrl-click reveals in Finder)", disposition: Mission("M6") },
    Entry { version: "2.1.196", item: "mcp list/get do not spawn repo-self-approved servers; Pending approval shown", disposition: Mission("M11") },
    Entry { version: "2.1.196", item: "Waking a background job never deletes its transcript (set aside instead)", disposition: Mission("M8") },
    Entry { version: "2.1.196", item: "Rate-limit warning flicker + over-counted telemetry with parallel requests", disposition: Mission("M12") },
    Entry { version: "2.1.196", item: "No duplicate recap after schema-rejected StructuredOutput retry", disposition: Mission("M9") },
    Entry { version: "2.1.196", item: "PowerShell git diff/grep, egrep/fgrep, quoted | patterns: exit 1 is not failure", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "Agents side panel: focus, subagent types, running status fixes", disposition: Mission("M7") },
    Entry { version: "2.1.196", item: "claude agents --dangerously-skip-permissions shows disclaimer, applies bypass", disposition: Mission("M7") },
    Entry { version: "2.1.196", item: "Remote sessions auto-resume after server restart", disposition: Divergence("Anthropic cloud/remote infra; LingXi has no remote-session backend") },
    Entry { version: "2.1.196", item: "/cd moved sessions don't reappear in old dir's resume list (special chars)", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "plugin validate: local '.' plugins included; all error classes reported", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "Esc Esc at idle prompt opens rewind menu (regression fix)", disposition: Mission("M6") },
    Entry { version: "2.1.196", item: "MCP OAuth: no-scope request must not ask for full scopes_supported catalog", disposition: Mission("M11") },
    Entry { version: "2.1.196", item: "/context shows real token counts on Bedrock", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "/deep-research verifier failures reported as unverified, not all-refuted", disposition: Divergence("bundled skill content, not core behavior") },
    Entry { version: "2.1.196", item: "Plugin dependency pins honored for local-folder git-backed marketplaces", disposition: Mission("M13") },
    Entry { version: "2.1.196", item: "Agents view status: no Done/Needs-input flip; Needs attention; PR link", disposition: Mission("M7") },
    Entry { version: "2.1.196", item: "Voice dictation: no swallowed spaces / spurious recording on fast typing", disposition: Divergence("platform voice input; LingXi voice stack differs (sherpa)") },
    Entry { version: "2.1.196", item: "Background sessions survive process stop/restart/update (incl. Windows handoff)", disposition: Mission("M8") },
    Entry { version: "2.1.196", item: "Workers killed by daemon restart auto-resume when agents view opens", disposition: Mission("M8") },
    Entry { version: "2.1.196", item: "/code-review workflow: five cleanup finders merged into one (-25% tokens)", disposition: Divergence("bundled workflow content, not core behavior") },
    Entry { version: "2.1.196", item: "Per-frame rendering skips no-op subtree walks during streaming", disposition: Disposition::Implemented },
    Entry { version: "2.1.196", item: "Streaming idle watchdog on by default (5 min, env kill-switch)", disposition: Mission("M12") },
    Entry { version: "2.1.196", item: "Remote Control disabled when ANTHROPIC_BASE_URL is non-Anthropic", disposition: Mission("M13") },
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
