//! Compact-boundary message: the typed transition marker the session loader
//! and `get_messages_after_compact_boundary` slice on.
//!
//! TS ref: `src/utils/messages.ts:4530-4555` (`createCompactBoundaryMessage`),
//! `:4608-4656` (`isCompactBoundaryMessage`, `findLastCompactBoundaryIndex`,
//! `getMessagesAfterCompactBoundary`); `src/services/compact/compact.ts:349-368`
//! (`annotateBoundaryWithPreservedSegment`), `:600-611` / `:330-338`
//! (`preCompactDiscoveredTools`, `buildPostCompactMessages`).
//!
//! Wire-shape note (divergence — flagged BLOCKED in the batch report):
//! TS represents the boundary as `{ type:'system', subtype:'compact_boundary',
//! content:'Conversation compacted', compactMetadata:{…} }`. Rust's
//! [`protocol::ConversationMessage::System`] currently carries only `{ id,
//! content }` with no `subtype` discriminant, and `protocol/` is frozen for
//! this batch (additive variant/field must be raised with the user). So the
//! in-history marker is a `System` message whose `content` equals the exact TS
//! sentinel string [`BOUNDARY_CONTENT`] (`"Conversation compacted"`), and the
//! rich [`CompactBoundaryMetadata`] is carried alongside as a typed value (the
//! TUI already renders the boundary from a `CompactionCompleted` event, not by
//! re-reading message content, so the metadata does not need to round-trip
//! through `protocol` to render today). When `protocol` gains a
//! `compact_boundary` subtype the sentinel-string detection in
//! [`is_compact_boundary`] should switch to the subtype check.

use protocol::{ConversationMessage, MessageId};
use serde::{Deserialize, Serialize};

/// Exact content string TS stamps on every compact-boundary system message
/// (`createCompactBoundaryMessage` sets `content` to `Conversation compacted`).
/// Also the sentinel [`is_compact_boundary`] matches on until `protocol`
/// grows a real `subtype` discriminant.
pub const BOUNDARY_CONTENT: &str = "Conversation compacted";

/// Whether a compaction was user-initiated (`/compact`) or fired
/// automatically by the token-threshold autocompactor.
///
/// TS: the `trigger` field of `compactMetadata` — `'manual' | 'auto'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactTrigger {
    /// User ran `/compact`.
    Manual,
    /// Token threshold tripped the autocompactor.
    Auto,
}

impl CompactTrigger {
    /// The exact TS string for this trigger (`"manual"` / `"auto"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

/// Relink metadata for preserved (`messagesToKeep`) segments — used by the
/// session loader to patch head→anchor and anchor's-other-children→tail when
/// a partial/directional compaction keeps a tail of original messages.
///
/// TS: `compactMetadata.preservedSegment` (`{ headUuid?, anchorUuid?,
/// tailUuid? }`). The Batch 5 `annotate_boundary_with_preserved_segment`
/// fills this in; Batch 4 only carries the field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreservedSegment {
    /// `uuid` of the first preserved (kept) message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_uuid: Option<String>,
    /// `uuid` of whatever sits immediately before keep[0] in the desired chain
    /// (last summary message for suffix-preserving, the boundary for prefix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_uuid: Option<String>,
    /// `uuid` of the last preserved (kept) message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_uuid: Option<String>,
}

/// Rich metadata stamped onto a compact-boundary marker.
///
/// TS: `SystemCompactBoundaryMessage.compactMetadata` plus the
/// `preCompactDiscoveredTools` carry (`compact.ts:608`/`:1025`) and the
/// `logicalParentUuid` relink (`createCompactBoundaryMessage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactBoundaryMetadata {
    /// `'manual' | 'auto'`.
    pub trigger: CompactTrigger,
    /// Token count of the conversation just before compaction
    /// (`preTokens`). TS passes `preCompactTokenCount ?? 0`.
    pub pre_tokens: u64,
    /// User-context string snapshot at compaction time (`userContext`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_context: Option<String>,
    /// How many messages the summary replaced (`messagesSummarized`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages_summarized: Option<u32>,
    /// Already-loaded deferred-tool names at compaction time, **sorted**
    /// (`preCompactDiscoveredTools`). Empty when none were discovered (TS only
    /// sets the field when the set is non-empty; the empty `Vec` is the same
    /// observable state).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pre_compact_discovered_tools: Vec<String>,
    /// Relink metadata for a preserved tail (Batch 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_segment: Option<PreservedSegment>,
    /// `uuid` of the last pre-compact message, used to relink the boundary into
    /// the on-disk chain (`logicalParentUuid`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_parent_uuid: Option<String>,
}

/// Construct a compact-boundary system message plus its typed metadata.
///
/// 1:1 with `createCompactBoundaryMessage` (TS `messages.ts:4530`):
///  - `content` is the exact `"Conversation compacted"` sentinel,
///  - `trigger` / `pre_tokens` / `user_context` / `messages_summarized` map to
///    `compactMetadata`,
///  - `last_pre_compact_message_uuid` → `logicalParentUuid` (only when present),
///  - `discovered_tools` are sorted and deduped to mirror the TS
///    `[...preCompactDiscovered].sort()` carry (`compact.ts:608`).
///
/// Returns the in-history [`ConversationMessage`] marker and the rich
/// [`CompactBoundaryMetadata`] (the latter cannot ride inside the frozen
/// `protocol::System` variant — see the module docs).
#[must_use]
pub fn create_compact_boundary(
    trigger: CompactTrigger,
    pre_tokens: u64,
    last_pre_compact_message_uuid: Option<MessageId>,
    user_context: Option<String>,
    messages_summarized: Option<u32>,
    discovered_tools: &[String],
) -> (ConversationMessage, CompactBoundaryMetadata) {
    // Mirror `[...preCompactDiscovered].sort()`: sort + dedup. The TS set is
    // already unique; sorting a deduped vec yields the identical array.
    let mut tools: Vec<String> = discovered_tools.to_vec();
    tools.sort();
    tools.dedup();

    let metadata = CompactBoundaryMetadata {
        trigger,
        pre_tokens,
        user_context,
        messages_summarized,
        pre_compact_discovered_tools: tools,
        preserved_segment: None,
        logical_parent_uuid: last_pre_compact_message_uuid.map(|u| u.to_string()),
    };

    let marker = ConversationMessage::System {
        id: MessageId::new(),
        content: BOUNDARY_CONTENT.to_string(),
    };

    (marker, metadata)
}

/// Whether `message` is a compact-boundary marker.
///
/// TS `isCompactBoundaryMessage`: `type === 'system' && subtype ===
/// 'compact_boundary'`. Until `protocol` grows a `subtype`, we match a
/// `System` message whose content is the exact [`BOUNDARY_CONTENT`] sentinel.
#[must_use]
pub fn is_compact_boundary(message: &ConversationMessage) -> bool {
    matches!(
        message,
        ConversationMessage::System { content, .. } if content == BOUNDARY_CONTENT
    )
}

/// Index of the last compact boundary, scanning backward; `None` if none.
///
/// TS `findLastCompactBoundaryIndex` returns `-1` for "none"; the Rust idiom
/// is `Option<usize>`.
#[must_use]
pub fn find_last_compact_boundary_index(messages: &[ConversationMessage]) -> Option<usize> {
    messages.iter().rposition(is_compact_boundary)
}

/// Messages from the last compact boundary onward (including the boundary).
/// Returns all messages when no boundary exists.
///
/// TS `getMessagesAfterCompactBoundary`. The TS `includeSnipped` /
/// `projectSnippedView` path is the `HISTORY_SNIP` projection (a separate
/// subsystem ported in `snip.rs`) — out of scope here; this returns the raw
/// slice, matching TS with `includeSnipped: true` / `HISTORY_SNIP` off.
#[must_use]
pub fn get_messages_after_compact_boundary(
    messages: &[ConversationMessage],
) -> &[ConversationMessage] {
    match find_last_compact_boundary_index(messages) {
        Some(i) => &messages[i..],
        None => messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ConversationMessage;

    fn user(text: &str) -> ConversationMessage {
        ConversationMessage::user(MessageId::new(), text.to_string())
    }

    fn boundary() -> ConversationMessage {
        create_compact_boundary(CompactTrigger::Manual, 0, None, None, None, &[]).0
    }

    #[test]
    fn create_populates_trigger_pre_tokens_and_discovered_tools() {
        let last = MessageId::new();
        let (marker, meta) = create_compact_boundary(
            CompactTrigger::Auto,
            123_456,
            Some(last),
            Some("ctx".to_string()),
            Some(7),
            &["Zebra".to_string(), "alpha".to_string(), "Mango".to_string()],
        );

        assert_eq!(meta.trigger, CompactTrigger::Auto);
        assert_eq!(meta.pre_tokens, 123_456);
        assert_eq!(meta.user_context.as_deref(), Some("ctx"));
        assert_eq!(meta.messages_summarized, Some(7));
        // sorted (byte order: uppercase before lowercase)
        assert_eq!(
            meta.pre_compact_discovered_tools,
            vec!["Mango".to_string(), "Zebra".to_string(), "alpha".to_string()]
        );
        assert_eq!(meta.logical_parent_uuid.as_deref(), Some(&*last.to_string()));
        assert_eq!(meta.preserved_segment, None);

        // The in-history marker carries the exact TS sentinel content.
        assert!(is_compact_boundary(&marker));
        match marker {
            ConversationMessage::System { content, .. } => {
                assert_eq!(content, "Conversation compacted");
            }
            _ => panic!("expected a System marker"),
        }
    }

    #[test]
    fn create_dedups_discovered_tools() {
        let (_m, meta) = create_compact_boundary(
            CompactTrigger::Manual,
            0,
            None,
            None,
            None,
            &["b".to_string(), "a".to_string(), "b".to_string(), "a".to_string()],
        );
        assert_eq!(meta.pre_compact_discovered_tools, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn create_omits_logical_parent_when_absent() {
        let (_m, meta) =
            create_compact_boundary(CompactTrigger::Manual, 0, None, None, None, &[]);
        assert_eq!(meta.logical_parent_uuid, None);
    }

    #[test]
    fn trigger_as_str_matches_ts() {
        assert_eq!(CompactTrigger::Manual.as_str(), "manual");
        assert_eq!(CompactTrigger::Auto.as_str(), "auto");
    }

    #[test]
    fn is_boundary_distinguishes_system_marker_from_plain_system() {
        assert!(is_compact_boundary(&boundary()));
        let plain = ConversationMessage::System {
            id: MessageId::new(),
            content: "some other system text".to_string(),
        };
        assert!(!is_compact_boundary(&plain));
        assert!(!is_compact_boundary(&user("hi")));
    }

    #[test]
    fn find_last_index_returns_none_when_no_boundary() {
        let msgs = vec![user("a"), user("b")];
        assert_eq!(find_last_compact_boundary_index(&msgs), None);
    }

    #[test]
    fn find_last_index_returns_most_recent() {
        let msgs = vec![
            user("a"),
            boundary(), // index 1
            user("b"),
            boundary(), // index 3 — most recent
            user("c"),
        ];
        assert_eq!(find_last_compact_boundary_index(&msgs), Some(3));
    }

    #[test]
    fn slice_returns_all_when_no_boundary() {
        let msgs = vec![user("a"), user("b"), user("c")];
        let sliced = get_messages_after_compact_boundary(&msgs);
        assert_eq!(sliced.len(), 3);
        assert_eq!(sliced, &msgs[..]);
    }

    #[test]
    fn slice_starts_at_last_boundary_inclusive() {
        let msgs = vec![
            user("a"),
            boundary(),
            user("b"),
            boundary(), // index 3
            user("c"),
            user("d"),
        ];
        let sliced = get_messages_after_compact_boundary(&msgs);
        // boundary at 3 + the two trailing users = 3 messages, boundary first
        assert_eq!(sliced.len(), 3);
        assert!(is_compact_boundary(&sliced[0]));
        assert_eq!(sliced[1].text_content(), "c");
        assert_eq!(sliced[2].text_content(), "d");
    }

    #[test]
    fn slice_includes_boundary_when_it_is_last() {
        let msgs = vec![user("a"), boundary()];
        let sliced = get_messages_after_compact_boundary(&msgs);
        assert_eq!(sliced.len(), 1);
        assert!(is_compact_boundary(&sliced[0]));
    }

    #[test]
    fn metadata_serializes_omitting_empty_optionals() {
        let (_m, meta) =
            create_compact_boundary(CompactTrigger::Auto, 42, None, None, None, &[]);
        let v = serde_json::to_value(&meta).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("trigger").and_then(|t| t.as_str()), Some("auto"));
        assert_eq!(obj.get("pre_tokens").and_then(serde_json::Value::as_u64), Some(42));
        // empty / None fields are omitted from the wire shape
        assert!(!obj.contains_key("user_context"));
        assert!(!obj.contains_key("messages_summarized"));
        assert!(!obj.contains_key("pre_compact_discovered_tools"));
        assert!(!obj.contains_key("preserved_segment"));
        assert!(!obj.contains_key("logical_parent_uuid"));
    }
}
