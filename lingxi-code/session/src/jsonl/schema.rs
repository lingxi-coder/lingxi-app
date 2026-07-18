//! `JsonlMessage` — outer JSONL line schema.
//!
//! Field NAMES **and ORDER** are parity-locked to claude-code's transcript
//! schema (verified against real 2.1.195 on-disk transcripts —
//! ~810k lines). `Serialize` is hand-written ([`impl Serialize for
//! JsonlMessage`]) to emit claude's EXACT per-kind outer-key order instead of
//! the struct-declaration order; `Deserialize` stays derived (the reader is
//! order-independent).
//!
//! Per-kind head order (everything before the common trailer), for the fields
//! this engine actually emits:
//! - **user**: `parentUuid, isSidechain, [promptId,] type, message, [isMeta,]
//!   [isVisibleInTranscriptOnly,] [isCompactSummary,] uuid, timestamp` (the
//!   two compact-summary flags ride between `message` and `uuid`, per real
//!   2.1.207 summary lines).
//! - **assistant (normal)**: `parentUuid, isSidechain, message, [requestId,]
//!   type, uuid, timestamp`
//! - **assistant (api-error)**: `parentUuid, isSidechain, type, uuid,
//!   timestamp, message, [requestId,] [error,] isApiErrorMessage,
//!   [apiErrorStatus]`
//! - **system (compact boundary, `extra.subtype == "compact_boundary"`)**:
//!   `parentUuid, [logicalParentUuid,] isSidechain, type, subtype, content,
//!   [isMeta,] level, compactMetadata, uuid, timestamp` — claude's FLATTENED
//!   boundary envelope (real 2.1.207 transcripts); these lines carry NO inner
//!   `message`.
//! - **system (other)**: `parentUuid, [logicalParentUuid,] isSidechain, type,
//!   message, [isMeta,] uuid, timestamp` (the port emits an inner
//!   `{role,content}` `message`; claude's flattened `subtype`/`content`
//!   top-level system envelope is ported only for the compact-boundary
//!   subtype, so on other system lines those siblings — when present in
//!   `extra` — are tail-appended verbatim rather than synthesized into
//!   claude's positions).
//!
//! Common trailer (every kind): `userType, [entrypoint,] cwd, sessionId,
//! version, [gitBranch,] [slug]`. Any UNRECOGNIZED `extra` key (an unported
//! claude field, e.g. `agentId`/`toolUseResult`/`attachment`) is appended after
//! the trailer in `extra` iteration order, so read→write round-trips of
//! unported fields stay byte-faithful.

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

/// One line of the session JSONL file.
///
/// Outer field NAMES **and ORDER** match claude: the hand-written
/// [`impl Serialize for JsonlMessage`] emits claude's per-kind envelope order
/// (see the module docs) rather than this struct's declaration order — so the
/// on-disk byte layout is 1:1, not just the field set. (This corrects an
/// earlier note that claimed the order "does not yet" match; the hand-written
/// `Serialize` closed that gap.) The `message` field is an opaque `Value`
/// because its inner schema depends on `type` (Anthropic
/// Messages API for `user`/`assistant`, claude-code internal shapes for
/// `system`/`attachment`). All un-named outer fields land in `extra` via
/// `#[serde(flatten)]` so read→write round-trips preserve every byte we read.
#[derive(Debug, Clone, Deserialize)]
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
    #[serde(
        rename = "entrypoint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
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

/// `extra` keys this serializer pulls into their claude-ordered positions.
/// Anything NOT in this set is appended verbatim after the common trailer
/// (forward-compat for unported claude fields like `agentId`/`toolUseResult`).
const RECOGNIZED_EXTRA: &[&str] = &[
    "isMeta",
    "requestId",
    "error",
    "errorDetails",
    "isApiErrorMessage",
    "apiErrorStatus",
    "effort",
];

/// `extra` keys the USER head consumes (between `message` and `uuid`): the
/// compact-summary envelope flags, emitted in claude's on-disk order
/// (`..., "message":…, "isVisibleInTranscriptOnly":true, "isCompactSummary":
/// true, "uuid":…` — real 2.1.207 transcripts). Skipped from the tail ONLY on
/// user lines, so a non-user line carrying them still round-trips verbatim.
const USER_HEAD_EXTRA: &[&str] = &["isVisibleInTranscriptOnly", "isCompactSummary"];

/// `extra` keys the `subtype:"compact_boundary"` SYSTEM head consumes —
/// claude's flattened boundary envelope (`type, subtype, content, level,
/// compactMetadata, uuid, timestamp`, real 2.1.207 transcripts; boundary lines
/// carry NO inner `message`). Skipped from the tail only when the boundary
/// head emitted them.
const BOUNDARY_HEAD_EXTRA: &[&str] = &["subtype", "content", "level", "compactMetadata"];

// Hand-written `Serialize` so the outer JSONL keys land in claude-code's EXACT
// per-kind order (see module docs). `Deserialize` stays derived — the reader is
// order-independent. VALUES are byte-identical to the derived impl; only key
// POSITION changes. Skip predicates mirror the struct's `skip_serializing_if`
// so presence parity is preserved.
impl Serialize for JsonlMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // serde_json's map serializer preserves `serialize_entry` insertion
        // order, so emitting entries in sequence yields claude's on-disk order.
        let mut map = serializer.serialize_map(None)?;

        let is_user = self.message_type == "user";
        let is_assistant = self.message_type == "assistant";
        let is_system = self.message_type == "system";
        let is_api_error = self.extra.contains_key("isApiErrorMessage");
        // Compact-boundary system line: claude flattens the system envelope
        // (`subtype`/`content`/`level`/`compactMetadata` are top-level
        // siblings, no inner `message`). Only THIS system subtype gets the
        // flattened head; other system lines keep the generic arm below.
        let is_compact_boundary = is_system
            && self.extra.get("subtype").and_then(Value::as_str) == Some("compact_boundary");

        // (a) parentUuid — ALWAYS first, emitted even when null.
        map.serialize_entry("parentUuid", &self.parent_uuid)?;
        // (b) logicalParentUuid — system places it right after parentUuid. The
        //     port sets it on compact-boundary lines (which carry
        //     `parentUuid: null` + the real parent here, the claude chain
        //     reset); omitted when None to match the TS `undefined` skip.
        if self.logical_parent_uuid.is_some() {
            map.serialize_entry("logicalParentUuid", &self.logical_parent_uuid)?;
        }
        // (c) isSidechain — always.
        map.serialize_entry("isSidechain", &self.is_sidechain)?;

        if is_user {
            // (d) user head: promptId?, type, message, isMeta?,
            //     isVisibleInTranscriptOnly?, isCompactSummary?, uuid,
            //     timestamp. The two compact-summary flags sit between
            //     `message` and `uuid` on claude's persisted summary lines
            //     (real 2.1.207 transcripts).
            if let Some(pid) = &self.prompt_id {
                map.serialize_entry("promptId", pid)?;
            }
            map.serialize_entry("type", &self.message_type)?;
            map.serialize_entry("message", &self.message)?;
            if let Some(v) = self.extra.get("isMeta") {
                map.serialize_entry("isMeta", v)?;
            }
            if let Some(v) = self.extra.get("isVisibleInTranscriptOnly") {
                map.serialize_entry("isVisibleInTranscriptOnly", v)?;
            }
            if let Some(v) = self.extra.get("isCompactSummary") {
                map.serialize_entry("isCompactSummary", v)?;
            }
            map.serialize_entry("uuid", &self.uuid)?;
            map.serialize_entry("timestamp", &self.timestamp)?;
        } else if is_assistant && is_api_error {
            // (e1) assistant api-error head: type, uuid, timestamp, message,
            //      requestId?, error?, isApiErrorMessage, apiErrorStatus?.
            map.serialize_entry("type", &self.message_type)?;
            map.serialize_entry("uuid", &self.uuid)?;
            map.serialize_entry("timestamp", &self.timestamp)?;
            map.serialize_entry("message", &self.message)?;
            if let Some(v) = self.extra.get("requestId") {
                map.serialize_entry("requestId", v)?;
            }
            if let Some(v) = self.extra.get("error") {
                map.serialize_entry("error", v)?;
            }
            if let Some(v) = self.extra.get("errorDetails") {
                map.serialize_entry("errorDetails", v)?;
            }
            // isApiErrorMessage is guaranteed present (gated `is_api_error`).
            if let Some(v) = self.extra.get("isApiErrorMessage") {
                map.serialize_entry("isApiErrorMessage", v)?;
            }
            if let Some(v) = self.extra.get("apiErrorStatus") {
                map.serialize_entry("apiErrorStatus", v)?;
            }
        } else if is_assistant {
            // (e2) assistant normal head: message, requestId?, type, uuid,
            //      timestamp, effort?. `effort` (2.1.212) is the last field of
            //      the in-memory assistant message object `d` (after
            //      `advisorModel`), spread into the transcript record before the
            //      `userType`/`cwd`/`version`/`gitBranch` trailer — the level
            //      string from `Y4n(effort).level`, emitted only when present
            //      (claude's `...effort!==void 0&&{effort}` guard).
            map.serialize_entry("message", &self.message)?;
            if let Some(v) = self.extra.get("requestId") {
                map.serialize_entry("requestId", v)?;
            }
            map.serialize_entry("type", &self.message_type)?;
            map.serialize_entry("uuid", &self.uuid)?;
            map.serialize_entry("timestamp", &self.timestamp)?;
            if let Some(v) = self.extra.get("effort") {
                map.serialize_entry("effort", v)?;
            }
        } else if is_compact_boundary {
            // (f1) compact-boundary system head — claude's flattened envelope
            //      (real 2.1.207 transcripts): type, subtype, content,
            //      [isMeta?,] level, compactMetadata, uuid, timestamp. NO
            //      inner `message` — claude never writes one on boundary
            //      lines, so `self.message` (Null) is intentionally skipped.
            //      (`isMeta` only appears on pre-2.1.199 boundary lines; kept
            //      near its historical slot for tolerant round-trips.)
            map.serialize_entry("type", &self.message_type)?;
            if let Some(v) = self.extra.get("subtype") {
                map.serialize_entry("subtype", v)?;
            }
            if let Some(v) = self.extra.get("content") {
                map.serialize_entry("content", v)?;
            }
            if let Some(v) = self.extra.get("isMeta") {
                map.serialize_entry("isMeta", v)?;
            }
            if let Some(v) = self.extra.get("level") {
                map.serialize_entry("level", v)?;
            }
            if let Some(v) = self.extra.get("compactMetadata") {
                map.serialize_entry("compactMetadata", v)?;
            }
            map.serialize_entry("uuid", &self.uuid)?;
            map.serialize_entry("timestamp", &self.timestamp)?;
        } else {
            // (f) system + any other kind: type, message, isMeta?, uuid,
            //     timestamp. The port emits an inner `message`; claude's
            //     flattened system envelope (subtype/content as top-level
            //     siblings) is ported ONLY for the compact-boundary subtype
            //     (arm f1); other system subtypes tail-append their `extra`
            //     keys as before.
            map.serialize_entry("type", &self.message_type)?;
            map.serialize_entry("message", &self.message)?;
            if let Some(v) = self.extra.get("isMeta") {
                map.serialize_entry("isMeta", v)?;
            }
            map.serialize_entry("uuid", &self.uuid)?;
            map.serialize_entry("timestamp", &self.timestamp)?;
        }

        // (h) Common trailer — same emit/skip predicates as the derived impl,
        //     only the POSITION moves (it now follows the per-kind envelope).
        if let Some(v) = &self.user_type {
            map.serialize_entry("userType", v)?;
        }
        if let Some(v) = &self.entrypoint {
            map.serialize_entry("entrypoint", v)?;
        }
        map.serialize_entry("cwd", &self.cwd)?;
        map.serialize_entry("sessionId", &self.session_id)?;
        map.serialize_entry("version", &self.version)?;
        if let Some(v) = &self.git_branch {
            map.serialize_entry("gitBranch", v)?;
        }
        if let Some(v) = &self.slug {
            map.serialize_entry("slug", v)?;
        }

        // (i) Tail — any UNRECOGNIZED extra key (unported claude field), in
        //     `extra` iteration order, so round-trips of those survive. Keys a
        //     per-kind head already emitted are skipped ONLY for that kind.
        for (k, v) in &self.extra {
            if RECOGNIZED_EXTRA.contains(&k.as_str()) {
                continue;
            }
            if is_user && USER_HEAD_EXTRA.contains(&k.as_str()) {
                continue;
            }
            if is_compact_boundary && BOUNDARY_HEAD_EXTRA.contains(&k.as_str()) {
                continue;
            }
            map.serialize_entry(k, v)?;
        }

        map.end()
    }
}
