//! `JsonlMessage` — outer JSONL line schema, byte-locked to
//! `claude-code/src/types/logs.ts:8-17, 221-231`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One line of the session JSONL file.
///
/// Outer fields are byte-locked; the `message` field is an opaque `Value`
/// because its inner schema depends on `type` (Anthropic Messages API for
/// `user`/`assistant`, claude-code internal shapes for `system`/`attachment`).
/// All un-named outer fields land in `extra` via `#[serde(flatten)]` so
/// read→write round-trips preserve every byte we read.
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

    /// `false` for the main agent loop; `true` for sub-agent transcripts.
    /// Optional in the schema but emitted by claude-code's writer; we default
    /// to `false` on write and accept missing on read.
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: bool,

    /// `process.env.USER_TYPE || "external"` — lingxi-core lock: `"external"`.
    #[serde(rename = "userType", skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,

    /// Git branch at write time, when available.
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,

    /// All other outer fields (`agentId`, `logicalParentUuid`, `slug`,
    /// `entrypoint`, `agentName`, `agentColor`, `teamName`, `promptId`,
    /// `isMeta`, `toolUseResult`, ...) preserved verbatim across
    /// read->write so round-trips are byte-equivalent.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
