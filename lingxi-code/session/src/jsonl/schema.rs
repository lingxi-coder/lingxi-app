//! `JsonlMessage` — outer JSONL line schema.
//!
//! Field NAMES are parity-locked to claude-code's transcript schema (verified
//! against real 2.1.195 on-disk transcripts). Field ORDER is NOT yet 1:1: this
//! struct emits a single flat declared order, whereas claude wraps a per-kind
//! message envelope in the middle of common fields — real on-disk order is
//! `parentUuid, [logicalParentUuid,] isSidechain, [promptId,] <inner envelope:
//! type/uuid/timestamp/message + kind-specific siblings>, [sessionKind,]
//! userType, entrypoint, cwd, sessionId, version, gitBranch, [slug]`. Matching
//! that byte-for-byte needs an ordered/per-kind serializer (tracked as a
//! dedicated parity task; see memory `session-jsonl-keyorder-oracle`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One line of the session JSONL file.
///
/// Outer field NAMES match claude; field ORDER does not yet (see module
/// docs — claude spreads a per-kind envelope mid-line). The `message` field is
/// an opaque `Value` because its inner schema depends on `type` (Anthropic
/// Messages API for `user`/`assistant`, claude-code internal shapes for
/// `system`/`attachment`). All un-named outer fields land in `extra` via
/// `#[serde(flatten)]` so read→write round-trips preserve every byte we read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonlMessage {
    /// `"user" | "assistant" | "system" | "attachment" | "summary" | ...`
    /// — see `claude-code/src/types/logs.ts:297` `Entry` union.
    #[serde(rename = "type")]
    pub message_type: String,

    /// Stable identifier for this entry. UUID v4 lowercase.
    pub uuid: String,

    /// Parent entry's `uuid`, or JSON `null` for the first turn.
    /// MUST serialize as `null` (NOT omitted) — `claude-code/src/types/logs.ts:222`.
    #[serde(rename = "parentUuid")]
    pub parent_uuid: Option<String>,

    /// Session UUID. Matches the filename without `.jsonl`.
    #[serde(rename = "sessionId")]
    pub session_id: String,

    /// ISO-8601 UTC timestamp with millisecond precision
    /// (`new Date().toISOString()`), e.g. `"2026-05-25T14:30:00.000Z"`.
    ///
    /// TOLERANT: `user`/`assistant` lines always carry it, but `attachment` /
    /// `system` chain-participant lines (and some legacy rows) may omit it —
    /// `claude-code`'s `isTranscriptMessage` admits those into the chain without
    /// requiring a timestamp, so we default to the empty string on read rather
    /// than rejecting the line. The branch-aware DAG walk
    /// ([`crate::jsonl::loader::build_conversation_chain`]) parses this via
    /// `chrono` and treats an unparsable / empty value as epoch (oldest), so a
    /// missing timestamp simply can't win the newest-leaf race.
    #[serde(default)]
    pub timestamp: String,

    /// Canonical absolute cwd at the time this entry was written.
    /// TOLERANT (`#[serde(default)]`): present on `user`/`assistant`, may be
    /// absent on `attachment`/`system`.
    #[serde(default)]
    pub cwd: String,

    /// Engine version string. claude-code: `MACRO.VERSION`; lingxi-core: `CARGO_PKG_VERSION`.
    /// TOLERANT (`#[serde(default)]`) for the same reason as `cwd`.
    #[serde(default)]
    pub version: String,

    /// Inner message object — Anthropic Messages API shape for `user`/`assistant`,
    /// other shapes for system entries. Preserved verbatim.
    ///
    /// TOLERANT: `attachment` and `system` chain-participant lines frequently
    /// lack a `message` field entirely (their payload lives in sibling outer
    /// fields captured by [`Self::extra`]). `#[serde(default)]` yields
    /// `Value::Null` when absent so the line still parses and joins the chain,
    /// matching `claude-code`'s `isTranscriptMessage` (it gates on `type` only,
    /// never on the presence of `message`).
    #[serde(default)]
    pub message: Value,

    // The Anthropic `request-id` response header (`req_…`) is persisted as the
    // top-level `requestId` field on REAL assistant lines (the SDK's
    // `response._request_id`). It is carried through [`Self::extra`] (the same
    // flatten channel as `isMeta`), set by the writer only for real assistant
    // lines — so no struct-literal constructor churn, and read-side round-trips
    // it transparently. See `orchestrator::conversation::to_jsonl_message_with_inner_id`.
    /// `false` for the main agent loop; `true` for sub-agent transcripts.
    /// Optional in the schema but emitted by claude-code's writer; we default
    /// to `false` on write and accept missing on read.
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: bool,

    /// `process.env.USER_TYPE || "external"` — lingxi-core lock: `"external"`.
    #[serde(rename = "userType", skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,

    /// Git branch at write time, when available.
    /// `getBranch()` once per chain (`sessionStorage.ts:1012-1019`); on a
    /// non-repo / git failure the writer leaves it `None` → omitted.
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,

    /// CLI entrypoint string — `getEntrypoint()` (`sessionStorage.ts:1058`),
    /// e.g. `"cli"`. Stamped on EVERY line by claude-code's writer. Optional on
    /// read (legacy rows omit it); `skip_serializing_if = "Option::is_none"`
    /// so an unset value is omitted, matching TS `undefined`.
    #[serde(rename = "entrypoint", default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,

    /// Per-session plan slug — `getPlanSlugCache().get(sessionId)`
    /// (`sessionStorage.ts:1023, 1063`). `undefined` when the session has no
    /// plan slug; omitted on write when `None`.
    #[serde(rename = "slug", default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,

    /// Stable per-prompt id — `getPromptId()` (`sessionStorage.ts:1045-1046`),
    /// set ONLY on `user` lines (TS: `type === 'user' ? getPromptId() : undefined`).
    /// `None` (omitted) on every non-`user` line and on `user` lines when no
    /// prompt id is available.
    #[serde(rename = "promptId", default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,

    /// Compact-boundary back-link — `logicalParentUuid`
    /// (`sessionStorage.ts:1041`). On a compaction boundary TS sets
    /// `parentUuid: null` and stashes the real parent here
    /// (`isCompactBoundary ? parentUuid : undefined`); on every other line it is
    /// `undefined`. Omitted on write when `None`.
    #[serde(
        rename = "logicalParentUuid",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub logical_parent_uuid: Option<String>,

    /// All other outer fields (`agentId`, `agentName`, `agentColor`,
    /// `teamName`, `isMeta`, `toolUseResult`, ...) preserved verbatim across
    /// read->write so round-trips are byte-equivalent.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
