//! F1-08 — JSON-schema golden snapshots (the frozen wire contract).
//!
//! This is the single most important F1 deliverable: it serializes ONE canonical
//! instance of EVERY `ClientEvent` / `ClientCommand` variant + the `MessageDto`
//! block set + each permission / error DTO into checked-in
//! `client-protocol/snapshots/*.json` goldens, plus a `feed_status.json` golden
//! enumerating the `RenderedMessage` feed-status table (LIVE-FED vs.
//! RESERVED/feed-deferred). The snapshot IS the frozen wire format and the
//! auditable feed-status record (plan F1-08, governing decisions §0.7 / §0.9).
//!
//! ## Framework choice (plan F1-08)
//!
//! The plan says: "`insta` if already a workspace dev-dep, else a hand-rolled
//! `assert_eq!(serde_json::to_string_pretty(&x), include_str!(golden))` (no new
//! prod dep — dev-only)." `insta` is NOT a `[workspace.dependencies]` entry — it
//! is declared per-crate in `tui` / `engine-*` with a literal version, never as
//! a shared workspace dev-dep — so this harness is the sanctioned hand-rolled
//! variant: `serde_json::to_string_pretty` compared against a checked-in golden
//! read from disk. `serde_json` is a DEV-ONLY dep (it must never enter the
//! contract crate itself, §0.4).
//!
//! ## Regenerating goldens
//!
//! Run with `BLESS=1` to (re)write every golden from the current canonical
//! instances, then review the diff before committing:
//!
//! ```text
//! BLESS=1 cargo test -p client-protocol --test snapshot_test
//! ```
//!
//! The "red" at F1-08 is that NO goldens exist yet, so every case fails with a
//! missing-file error; `BLESS=1` generates them, the goldens are reviewed, and a
//! plain run goes green. Any later structural drift (a renamed tag, a retyped
//! field, a dropped variant) flips the matching golden and the test fails — that
//! is the contract-freeze guarantee.

use std::fs;
use std::path::{Path, PathBuf};

use client_protocol::commands::{ClientCommand, ImageRefDto, ListingKindDto, PromptModeDto};
use client_protocol::error::ClientError;
use client_protocol::events::{ClientEvent, CostDto, ErrorKindDto, TurnOutcomeDto};
use client_protocol::listings::{
    AgentDto, AuthStateDto, CheckStatusDto, CoordinatorWorkerDto, DoctorCheckDto, DoctorReportDto,
    DoctorSummaryDto, HookDto, McpServerDto, McpStatusDto, MemoryEntryDto, MemoryTierDto,
    SessionRowDto, SlashCommandDto, StatusSnapshotDto, TaskRowDto, TaskStatusDto,
};
use client_protocol::message::{MessageBlockDto, MessageDto};
use client_protocol::permission::{
    PermissionKindDto, PermissionRequest, PermissionResolved, PermissionResponseDto, WorkerInfoDto,
};
use serde::Serialize;
use serde_json::Value;

/// Directory holding the checked-in goldens.
fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots")
}

/// `true` when the test is invoked in regeneration mode (`BLESS=1`).
fn bless() -> bool {
    matches!(std::env::var("BLESS").as_deref(), Ok("1" | "true"))
}

/// Pretty-print a DTO to the canonical golden string (trailing newline so the
/// file is a well-formed text file and `git diff` is clean).
fn pretty<T: Serialize>(value: &T) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("serialize golden instance");
    s.push('\n');
    s
}

/// Assert one canonical instance matches (or, under `BLESS=1`, (re)writes) its
/// golden. Collects a human-readable failure rather than panicking so a single
/// run reports EVERY drifted golden at once.
fn check_golden<T>(filename: &str, value: &T, failures: &mut Vec<String>)
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let path = snapshots_dir().join(filename);
    let want = pretty(value);

    if bless() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create snapshots dir");
        }
        fs::write(&path, &want).unwrap_or_else(|e| panic!("write golden {filename}: {e}"));
        return;
    }

    let got = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            failures.push(format!(
                "missing golden `{filename}` ({e}); regenerate with `BLESS=1 cargo test -p client-protocol --test snapshot_test`"
            ));
            return;
        }
    };

    if got != want {
        failures.push(format!(
            "golden `{filename}` drifted from the canonical instance — the wire \
             contract changed.\n--- on disk ---\n{got}\n--- canonical ---\n{want}\n\
             If this change is intentional, bump CLIENT_PROTOCOL_VERSION per §0.10 \
             and re-bless with `BLESS=1`."
        ));
    }

    // The golden must also be a faithful, deserializable representation of the
    // value — round-trips through the on-disk JSON byte-stably (decision §0.1).
    let parsed: Value = serde_json::from_str(&got)
        .unwrap_or_else(|e| panic!("golden `{filename}` is not valid JSON: {e}"));
    let back: T = serde_json::from_value(parsed)
        .unwrap_or_else(|e| panic!("golden `{filename}` does not deserialize back: {e}"));
    if &back != value {
        failures.push(format!(
            "golden `{filename}` does not round-trip back to its canonical instance"
        ));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Canonical instances — ONE per variant, with stable field values.
// ─────────────────────────────────────────────────────────────────────────────

/// Every `ClientEvent` variant, paired with its golden filename.
#[allow(clippy::too_many_lines)] // a flat data table: one row per ClientEvent variant
fn event_goldens() -> Vec<(&'static str, ClientEvent)> {
    vec![
        (
            "event/error.json",
            ClientEvent::Error {
                kind: ErrorKindDto::Transport,
                message: "connection reset".to_string(),
            },
        ),
        (
            "event/text_delta.json",
            ClientEvent::TextDelta {
                text: "Hello, world.".to_string(),
            },
        ),
        (
            "event/tool_use_started.json",
            ClientEvent::ToolUseStarted {
                id: "toolu_01".to_string(),
                tool: "Read".to_string(),
                input_json: r#"{"file_path":"/tmp/example.txt"}"#.to_string(),
            },
        ),
        (
            "event/tool_use_result.json",
            ClientEvent::ToolUseResult {
                id: "toolu_01".to_string(),
                tool: "Read".to_string(),
                result_json: r#"{"content":"file body"}"#.to_string(),
                is_error: false,
            },
        ),
        (
            "event/message_complete.json",
            ClientEvent::MessageComplete {
                stop_reason: Some("end_turn".to_string()),
                message: Some(canonical_message()),
            },
        ),
        (
            "event/turn_started.json",
            ClientEvent::TurnStarted { turn_id: Some(1) },
        ),
        (
            "event/turn_ended.json",
            ClientEvent::TurnEnded {
                outcome: TurnOutcomeDto::EndTurn,
                stop_reason: Some("end_turn".to_string()),
                cost: canonical_cost(),
            },
        ),
        (
            "event/cost_update.json",
            ClientEvent::CostUpdate {
                total_usd: 0.0123,
                input_tokens: 1200,
                output_tokens: 340,
                api_calls: 3,
                session_duration_secs: 42,
                formatted: "$0.0123".to_string(),
            },
        ),
        (
            "event/compaction_completed.json",
            ClientEvent::CompactionCompleted {
                messages_before: 50,
                messages_after: 12,
                bytes_saved: 4096,
            },
        ),
        (
            "event/session_started.json",
            ClientEvent::SessionStarted {
                session_id: "11111111-1111-4111-8111-111111111111".to_string(),
            },
        ),
        ("event/session_ended.json", ClientEvent::SessionEnded),
        (
            "event/session_resumed.json",
            ClientEvent::SessionResumed {
                session_id: "22222222-2222-4222-8222-222222222222".to_string(),
                // The restored transcript, OLDEST-FIRST. The canonical golden
                // carries a two-message conversation (a user turn + the
                // assistant block set) so the wire shape pins the lowered
                // `MessageDto` element exactly for the client mappers.
                messages: vec![
                    MessageDto {
                        role: "user".to_string(),
                        blocks: vec![MessageBlockDto::Text {
                            text: "Resume me.".to_string(),
                        }],
                    },
                    canonical_message(),
                ],
            },
        ),
        (
            "event/session_list.json",
            ClientEvent::SessionList {
                sessions: vec![SessionRowDto {
                    uuid: "33333333-3333-4333-8333-333333333333".to_string(),
                    title: "Implement the parser".to_string(),
                    modified_rfc3339: "2026-06-02T12:00:00Z".to_string(),
                    message_count: 17,
                    path: "/home/dev/.claude/sessions/33333333.jsonl".to_string(),
                }],
            },
        ),
        (
            "event/model_list.json",
            ClientEvent::ModelList {
                models: vec!["claude-opus-4-7".to_string(), "claude-sonnet-4-5".to_string()],
                current: "claude-opus-4-7".to_string(),
            },
        ),
        (
            "event/model_changed.json",
            ClientEvent::ModelChanged {
                model: "claude-sonnet-4-5".to_string(),
            },
        ),
        (
            "event/mcp_servers.json",
            ClientEvent::McpServers {
                servers: vec![
                    McpServerDto {
                        name: "filesystem".to_string(),
                        status: McpStatusDto::Connected,
                        transport: "stdio".to_string(),
                    },
                    McpServerDto {
                        name: "github".to_string(),
                        status: McpStatusDto::Error {
                            reason: "handshake timeout".to_string(),
                        },
                        transport: "http".to_string(),
                    },
                ],
            },
        ),
        (
            "event/hooks.json",
            ClientEvent::Hooks {
                hooks: vec![HookDto {
                    name: "format-on-write".to_string(),
                    event: "PostToolUse".to_string(),
                    matcher: Some("Write|Edit".to_string()),
                    timeout_ms: 60_000,
                }],
            },
        ),
        (
            "event/agents.json",
            ClientEvent::Agents {
                agents: vec![AgentDto {
                    name: "reviewer".to_string(),
                    description: "Reviews diffs for correctness".to_string(),
                    tools_allowed: vec!["Read".to_string(), "Grep".to_string()],
                }],
            },
        ),
        (
            "event/slash_command_catalog.json",
            ClientEvent::SlashCommandCatalog {
                commands: vec![SlashCommandDto {
                    name: "model".to_string(),
                    description: "Switch the active model".to_string(),
                    source: "builtin".to_string(),
                }],
            },
        ),
        (
            "event/memory_entries.json",
            ClientEvent::MemoryEntries {
                entries: vec![MemoryEntryDto {
                    path: "/home/dev/project/CLAUDE.md".to_string(),
                    tier: MemoryTierDto::Project,
                    body: "# Project notes".to_string(),
                    age_days: 3,
                    size_bytes: 256,
                }],
            },
        ),
        (
            "event/status_snapshot.json",
            ClientEvent::StatusSnapshot {
                snapshot: canonical_status(),
            },
        ),
        (
            "event/settings_snapshot.json",
            ClientEvent::SettingsSnapshot {
                effective_json: r#"{"model":"claude-opus-4-7"}"#.to_string(),
                provenance_json: r#"{"model":"user-settings"}"#.to_string(),
            },
        ),
        (
            "event/auth_state.json",
            ClientEvent::AuthState {
                state: AuthStateDto::SignedIn {
                    email: "dev@example.com".to_string(),
                    org_id: "org_abc123".to_string(),
                },
            },
        ),
        (
            "event/doctor_report.json",
            ClientEvent::DoctorReport {
                report: canonical_doctor(),
            },
        ),
        (
            "event/task_row.json",
            ClientEvent::TaskRow {
                task: canonical_task_row(),
            },
        ),
        (
            "event/task_output_chunk.json",
            ClientEvent::TaskOutputChunk {
                task_id: "b12345678".to_string(),
                content: "build output line\n".to_string(),
                total_lines: 128,
                truncated: false,
            },
        ),
        (
            "event/task_status_changed.json",
            ClientEvent::TaskStatusChanged {
                task_id: "b12345678".to_string(),
                status: TaskStatusDto::Running,
            },
        ),
        (
            "event/coordinator_status.json",
            ClientEvent::CoordinatorStatus {
                active_workers: 0,
                team: None,
            },
        ),
        (
            "event/coordinator_worker.json",
            ClientEvent::CoordinatorWorker {
                worker: canonical_coordinator_worker(),
            },
        ),
        (
            "event/thinking_delta.json",
            ClientEvent::ThinkingDelta {
                thinking: "Let me reason about this.".to_string(),
                signature: Some("sig_abc".to_string()),
            },
        ),
        (
            "event/usage_update.json",
            ClientEvent::UsageUpdate {
                input_tokens: 1200,
                output_tokens: 340,
                cache_read_tokens: 800,
                cache_creation_tokens: 64,
            },
        ),
    ]
}

/// Every `ClientCommand` variant, paired with its golden filename.
fn command_goldens() -> Vec<(&'static str, ClientCommand)> {
    vec![
        (
            "command/send_prompt.json",
            ClientCommand::SendPrompt {
                text: "summarize the diff".to_string(),
                prompt_mode: Some(PromptModeDto::Normal),
                images: vec![ImageRefDto {
                    media_type: "image/png".to_string(),
                    base64: "iVBORw0KGgo=".to_string(),
                }],
                turn_id: Some(1),
            },
        ),
        (
            "command/cancel.json",
            ClientCommand::Cancel { turn_id: Some(1) },
        ),
        (
            "command/approve_permission.json",
            ClientCommand::ApprovePermission {
                request_id: 7,
                response: PermissionResponseDto::AllowOnce,
            },
        ),
        (
            "command/deny_permission.json",
            ClientCommand::DenyPermission { request_id: 7 },
        ),
        (
            "command/set_model.json",
            ClientCommand::SetModel {
                model: "claude-sonnet-4-5".to_string(),
            },
        ),
        ("command/list_models.json", ClientCommand::ListModels),
        (
            "command/run_slash_command.json",
            ClientCommand::RunSlashCommand {
                raw: "/model opus".to_string(),
            },
        ),
        (
            "command/refresh_listings.json",
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Mcp, ListingKindDto::Agents],
            },
        ),
        (
            "command/new_session.json",
            ClientCommand::NewSession {
                cwd: Some("/home/dev/project".to_string()),
                model: Some("claude-opus-4-7".to_string()),
            },
        ),
        (
            "command/resume_session.json",
            ClientCommand::ResumeSession {
                session_id: "44444444-4444-4444-8444-444444444444".to_string(),
                cwd: Some("/home/dev/project".to_string()),
            },
        ),
        (
            "command/list_sessions.json",
            ClientCommand::ListSessions { limit: Some(20) },
        ),
        ("command/login.json", ClientCommand::Login),
        ("command/logout.json", ClientCommand::Logout),
        ("command/force_compact.json", ClientCommand::ForceCompact),
        ("command/clear_session.json", ClientCommand::ClearSession),
        (
            "command/task_list.json",
            ClientCommand::TaskList {
                status_filter: Some(TaskStatusDto::Running),
            },
        ),
        (
            "command/task_output.json",
            ClientCommand::TaskOutput {
                task_id: "b12345678".to_string(),
                offset: 0,
            },
        ),
        (
            "command/task_stop.json",
            ClientCommand::TaskStop {
                task_id: "b12345678".to_string(),
            },
        ),
        ("command/request_exit.json", ClientCommand::RequestExit),
    ]
}

/// The permission DTOs — one golden per `PermissionKindDto` variant + the
/// resolution + the worker-bearing request.
fn permission_request_goldens() -> Vec<(&'static str, PermissionRequest)> {
    vec![
        (
            "permission/request_tool_use_confirm.json",
            PermissionRequest {
                request_id: 7,
                kind: PermissionKindDto::ToolUseConfirm {
                    tool_name: "Bash".to_string(),
                    tool_input_json: r#"{"command":"ls -la"}"#.to_string(),
                    default_allow: false,
                },
                worker: None,
            },
        ),
        (
            "permission/request_exit_plan_mode.json",
            PermissionRequest {
                request_id: 8,
                kind: PermissionKindDto::ExitPlanMode {
                    plan: "1. read files\n2. edit".to_string(),
                },
                worker: None,
            },
        ),
        (
            "permission/request_bypass_permissions_mode.json",
            PermissionRequest {
                request_id: 9,
                kind: PermissionKindDto::BypassPermissionsMode,
                worker: None,
            },
        ),
        (
            "permission/request_with_worker.json",
            PermissionRequest {
                request_id: 10,
                kind: PermissionKindDto::ToolUseConfirm {
                    tool_name: "Edit".to_string(),
                    tool_input_json: r#"{"file_path":"/tmp/x"}"#.to_string(),
                    default_allow: true,
                },
                worker: Some(WorkerInfoDto {
                    name: "reviewer".to_string(),
                    color: "cyan".to_string(),
                    team: None,
                }),
            },
        ),
    ]
}

/// The error DTOs — one golden per `ClientError` variant.
fn error_goldens() -> Vec<(&'static str, ClientError)> {
    vec![
        (
            "error/transport.json",
            ClientError::Transport {
                message: "socket closed".to_string(),
            },
        ),
        (
            "error/protocol.json",
            ClientError::Protocol {
                message: "unknown frame tag".to_string(),
            },
        ),
        (
            "error/rejected.json",
            ClientError::Rejected {
                message: "permission denied".to_string(),
            },
        ),
        (
            "error/not_found.json",
            ClientError::NotFound {
                message: "no such session".to_string(),
            },
        ),
        (
            "error/internal.json",
            ClientError::Internal {
                message: "unexpected state".to_string(),
            },
        ),
    ]
}

// ── shared canonical sub-instances ────────────────────────────────────────────

fn canonical_cost() -> CostDto {
    CostDto {
        total_usd: 0.0123,
        input_tokens: 1200,
        output_tokens: 340,
        api_calls: 3,
        session_duration_secs: 42,
        formatted: "$0.0123".to_string(),
    }
}

/// The `MessageDto` block set — ONE block of each `MessageBlockDto` kind, in the
/// TUI scrollback render order. This is the block-set parity anchor (plan F1-02 /
/// F1-08): the golden enumerates exactly `Text | Thinking | RedactedThinking |
/// ToolUse | ToolResult`.
fn canonical_message() -> MessageDto {
    MessageDto {
        role: "assistant".to_string(),
        blocks: vec![
            MessageBlockDto::Text {
                text: "Here is the plan.".to_string(),
            },
            MessageBlockDto::Thinking {
                thinking: "I should read the file first.".to_string(),
                signature: Some("sig_think".to_string()),
            },
            MessageBlockDto::RedactedThinking {
                data: "REDACTED_BASE64".to_string(),
            },
            MessageBlockDto::ToolUse {
                id: "toolu_01".to_string(),
                tool: "Edit".to_string(),
                input_json: r#"{"file_path":"/tmp/x","old_string":"a","new_string":"b"}"#
                    .to_string(),
            },
            MessageBlockDto::ToolResult {
                id: "toolu_01".to_string(),
                tool: "Edit".to_string(),
                result_json: r#"{"ok":true}"#.to_string(),
                is_error: false,
                old_string: Some("a".to_string()),
                new_string: Some("b".to_string()),
                file_path: Some("/tmp/x".to_string()),
            },
        ],
    }
}

fn canonical_status() -> StatusSnapshotDto {
    StatusSnapshotDto {
        session_id: "11111111-1111-4111-8111-111111111111".to_string(),
        model: "claude-opus-4-7".to_string(),
        n_messages: 17,
        total_cost_usd: 0.0123,
        input_tokens: 1200,
        output_tokens: 340,
        n_mcp_connected: 1,
        n_mcp_total: 2,
        n_hooks: 1,
        n_agents: 1,
        started_at: "2026-06-02T12:00:00Z".to_string(),
        cwd: "/home/dev/project".to_string(),
        status_line: Some("opus | $0.0123".to_string()),
        active_workers: Some(2),
    }
}

fn canonical_doctor() -> DoctorReportDto {
    DoctorReportDto {
        checks: vec![
            DoctorCheckDto {
                name: "config-dir".to_string(),
                status: CheckStatusDto::Pass,
                detail: None,
            },
            DoctorCheckDto {
                name: "api-key".to_string(),
                status: CheckStatusDto::Warn,
                detail: Some("using env override".to_string()),
            },
        ],
        summary: DoctorSummaryDto {
            passed: 1,
            warnings: 1,
            failed: 0,
        },
    }
}

fn canonical_task_row() -> TaskRowDto {
    TaskRowDto {
        task_id: "b12345678".to_string(),
        task_type: "bash".to_string(),
        status: TaskStatusDto::Running,
        description: "run the test suite".to_string(),
    }
}

fn canonical_coordinator_worker() -> CoordinatorWorkerDto {
    CoordinatorWorkerDto {
        agent_id: "agent:00000000-0000-0000-0000-000000000001".to_string(),
        name: "alpha".to_string(),
        agent_type: "explorer".to_string(),
        status: "working".to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// EVERY `ClientEvent` variant has a byte-stable golden. This is the wire-format
/// freeze: a renamed tag / retyped field / dropped variant flips the golden.
#[test]
fn every_client_event_variant_matches_golden() {
    let mut failures = Vec::new();
    for (filename, ev) in event_goldens() {
        check_golden(filename, &ev, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// EVERY `ClientCommand` variant has a byte-stable golden.
#[test]
fn every_client_command_variant_matches_golden() {
    let mut failures = Vec::new();
    for (filename, cmd) in command_goldens() {
        check_golden(filename, &cmd, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// EVERY permission DTO (each `PermissionKindDto`, the worker-bearing request,
/// and `PermissionResolved`) has a byte-stable golden.
#[test]
fn every_permission_dto_matches_golden() {
    let mut failures = Vec::new();
    for (filename, req) in permission_request_goldens() {
        check_golden(filename, &req, &mut failures);
    }
    check_golden(
        "permission/resolved.json",
        &PermissionResolved {
            request_id: 7,
            response: PermissionResponseDto::AllowAlways,
        },
        &mut failures,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// EVERY `ClientError` variant has a byte-stable golden.
#[test]
fn every_client_error_variant_matches_golden() {
    let mut failures = Vec::new();
    for (filename, err) in error_goldens() {
        check_golden(filename, &err, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The `MessageDto` block set golden — the structural parity anchor: ONE block
/// of each `MessageBlockDto` kind (`Text | Thinking | RedactedThinking | ToolUse
/// | ToolResult`) in TUI scrollback order (plan F1-02 / F1-08).
#[test]
fn message_dto_block_set_matches_golden() {
    let mut failures = Vec::new();
    check_golden("message/block_set.json", &canonical_message(), &mut failures);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));

    // Defence-in-depth: the canonical message enumerates exactly the five block
    // kinds the TUI scrollback renders, in render order — so the golden is the
    // full parity set, not a subset.
    let tags: Vec<&str> = canonical_message()
        .blocks
        .iter()
        .map(|b| match b {
            MessageBlockDto::Text { .. } => "text",
            MessageBlockDto::Thinking { .. } => "thinking",
            MessageBlockDto::RedactedThinking { .. } => "redacted_thinking",
            MessageBlockDto::ToolUse { .. } => "tool_use",
            MessageBlockDto::ToolResult { .. } => "tool_result",
            _ => "unknown",
        })
        .collect();
    assert_eq!(
        tags,
        vec![
            "text",
            "thinking",
            "redacted_thinking",
            "tool_use",
            "tool_result"
        ],
        "MessageDto block-set golden must carry exactly the TUI scrollback block kinds"
    );
}

/// The `RenderedMessage` feed-status table golden (plan F1-08): an auditable record
/// of which `RenderedMessage` kinds are LIVE-FED vs. RESERVED / feed-deferred in
/// the foundation (governing decisions §0.7 / §0.9). This makes the §5.3 "~22
/// renderers parity" claim honest — feed-deferred entries are explicitly NOT
/// claimed as live.
#[test]
fn feed_status_table_matches_golden() {
    let table = feed_status_table();
    let mut failures = Vec::new();
    check_golden("feed_status.json", &table, &mut failures);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));

    // Sanity: the LIVE-FED set is exactly the kinds the adapter actually emits.
    // The §0.7 "light up thinking/usage" follow-up adds ThinkingDelta + UsageUpdate
    // to the live set (event_router -> emit_thinking/emit_usage -> AdapterOutputStream).
    let live: Vec<&str> = table
        .iter()
        .filter(|e| e.status == FeedStatus::LiveFed)
        .map(|e| e.rendered_message.as_str())
        .collect();
    assert_eq!(
        live,
        vec![
            "UserText",
            "AssistantText",
            "AssistantToolUse",
            "UserToolResult",
            "CompactBoundary",
            "ThinkingDelta",
            "UsageUpdate",
            "CoordinatorStatus",
        ],
        "the LIVE-FED set must include the §0.9 coordinator-activation's CoordinatorStatus \
         (now wired via emit_coordinator_status -> AdapterOutputStream)"
    );
    // The §0.9 coordinator-activation program wires CoordinatorStatus to a live
    // source (TeamRegistry::active_worker_count flows through
    // OutputStream::emit_coordinator_status -> AdapterOutputStream). The
    // reserved-now-live flip is a feed-status change only — the DTO
    // {active_workers, team} is byte-identical, so no CLIENT_PROTOCOL_VERSION bump.
    let coordinator_status = table
        .iter()
        .find(|e| e.rendered_message == "CoordinatorStatus")
        .expect("CoordinatorStatus present in the feed-status table");
    assert_eq!(coordinator_status.status, FeedStatus::LiveFed);
    // The §0.7 follow-up is taken: ThinkingDelta + UsageUpdate are now LIVE-FED.
    let thinking_delta = table
        .iter()
        .find(|e| e.rendered_message == "ThinkingDelta")
        .expect("ThinkingDelta present in the feed-status table");
    assert_eq!(thinking_delta.status, FeedStatus::LiveFed);
    let usage_update = table
        .iter()
        .find(|e| e.rendered_message == "UsageUpdate")
        .expect("UsageUpdate present in the feed-status table");
    assert_eq!(usage_update.status, FeedStatus::LiveFed);
    // The whole-block AssistantThinking synthesized form stays RESERVED (the
    // live reasoning stream flows through ThinkingDelta, not AssistantThinking).
    let thinking = table
        .iter()
        .find(|e| e.rendered_message == "AssistantThinking")
        .expect("AssistantThinking present in the feed-status table");
    assert_eq!(thinking.status, FeedStatus::Reserved);
}

// ── feed-status table model ────────────────────────────────────────────────────

/// LIVE-FED vs. RESERVED / feed-deferred classification for the feed-status
/// golden. `serde` is `snake_case`-tagged so the golden reads as
/// `"status": "live_fed"` / `"reserved"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FeedStatus {
    /// Wired to a live engine source in the foundation.
    LiveFed,
    /// Defined + frozen, but NOT wired to a live source (round-trip only) in the
    /// foundation (decisions §0.7 / §0.9).
    Reserved,
}

/// One row of the `RenderedMessage` feed-status table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
struct FeedStatusEntry {
    /// The `RenderedMessage` / DTO kind being classified.
    rendered_message: String,
    /// LIVE-FED or RESERVED in the foundation.
    status: FeedStatus,
    /// Human-readable note on the engine source (or why it is deferred).
    note: String,
}

fn entry(rendered_message: &str, status: FeedStatus, note: &str) -> FeedStatusEntry {
    FeedStatusEntry {
        rendered_message: rendered_message.to_string(),
        status,
        note: note.to_string(),
    }
}

/// The full feed-status table. LIVE-FED first (the adapter emits them, now incl.
/// the §0.7 follow-up's `ThinkingDelta` + `UsageUpdate` and the §0.9
/// coordinator-activation's `CoordinatorStatus`), then RESERVED / feed-deferred
/// (`AssistantThinking` whole-block form, `ExitPlanMode`, `BypassPermissionsMode`,
/// the lossy session-replay events, …).
fn feed_status_table() -> Vec<FeedStatusEntry> {
    use FeedStatus::*;
    vec![
        // ── LIVE-FED (~6) ──────────────────────────────────────────────────
        entry(
            "UserText",
            LiveFed,
            "SendPrompt user text echoed into the scrollback",
        ),
        entry(
            "AssistantText",
            LiveFed,
            "OutputStream::emit_text -> ClientEvent::TextDelta",
        ),
        entry(
            "AssistantToolUse",
            LiveFed,
            "OutputStream::emit_tool_call -> ClientEvent::ToolUseStarted",
        ),
        entry(
            "UserToolResult",
            LiveFed,
            "OutputStream::emit_tool_result -> ClientEvent::ToolUseResult",
        ),
        entry(
            "CompactBoundary",
            LiveFed,
            "OutputStream::emit_compaction_completed -> ClientEvent::CompactionCompleted",
        ),
        entry(
            "ThinkingDelta",
            LiveFed,
            "OutputStream::emit_thinking -> ClientEvent::ThinkingDelta (§0.7 follow-up: event_router emits per ThinkingDelta SSE chunk)",
        ),
        entry(
            "UsageUpdate",
            LiveFed,
            "OutputStream::emit_usage -> ClientEvent::UsageUpdate (§0.7 follow-up: event_router emits on MessageStart/MessageDelta usage)",
        ),
        entry(
            "CoordinatorStatus",
            LiveFed,
            "OutputStream::emit_coordinator_status -> ClientEvent::CoordinatorStatus (§0.9 coordinator-activation: CoordinatorStatusSink pushes TeamRegistry::active_worker_count on worker status transitions)",
        ),
        // ── RESERVED / feed-deferred ───────────────────────────────────────
        entry(
            "AssistantThinking",
            Reserved,
            "whole-block thinking synthesized form has no live source; the live stream uses ThinkingDelta (decision §0.7)",
        ),
        entry(
            "RedactedThinking",
            Reserved,
            "carried only inside a synthesized MessageDto, not streamed live",
        ),
        entry(
            "ExitPlanMode",
            Reserved,
            "PermissionGate::check never sources it in the foundation (decision §0.6)",
        ),
        entry(
            "BypassPermissionsMode",
            Reserved,
            "PermissionGate::check never sources it in the foundation (decision §0.6)",
        ),
        entry(
            "WorkerInfo",
            Reserved,
            "no wire worker identity in the foundation; always None on a live request",
        ),
        entry(
            "SessionStarted",
            Reserved,
            "lifecycle event; lossy on replay (carries only session_id, decision §0.5)",
        ),
        entry(
            "SessionEnded",
            Reserved,
            "lifecycle event; not part of the live message feed",
        ),
        entry(
            "SessionResumed",
            Reserved,
            "lifecycle event (not part of the per-turn message feed); now carries the full restored transcript (session_id + messages, oldest-first) from the live ResumeSession path rather than session_id alone",
        ),
        entry(
            "TurnStarted",
            Reserved,
            "adapter-synthesized on SendPrompt receipt; no engine source",
        ),
        entry(
            "MobileInlineImage",
            Reserved,
            "mobile inline image input deferred: needs run_turn_streaming_with_image_sources (§5.12)",
        ),
    ]
}
