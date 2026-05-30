//! Verified constants from the claude-code reference (see spec §13.2).

/// Buffer tokens before the autocompact threshold kicks in.
pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;
/// Buffer tokens before warning-level threshold.
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
/// Buffer tokens before error-level threshold.
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
/// Buffer tokens kept available after a manual compact request.
pub const MANUAL_COMPACT_BUFFER_TOKENS: u64 = 3_000;
/// Maximum output tokens budgeted for the autocompact summary call.
pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;
/// Maximum consecutive autocompact failures before the circuit breaker trips.
pub const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;
/// Maximum number of recent files restored into the post-compact prompt.
pub const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;
/// Total token budget shared across post-compact file restoration.
pub const POST_COMPACT_TOKEN_BUDGET: u64 = 50_000;
/// Per-file token budget for post-compact file restoration.
pub const POST_COMPACT_MAX_TOKENS_PER_FILE: u64 = 5_000;
/// Per-skill token budget for post-compact skill restoration.
pub const POST_COMPACT_MAX_TOKENS_PER_SKILL: u64 = 5_000;
/// Total token budget shared across post-compact skill restoration.
pub const POST_COMPACT_SKILLS_TOKEN_BUDGET: u64 = 25_000;
/// Maximum prompt-too-long retry attempts before giving up.
pub const MAX_PTL_RETRIES: u32 = 3;
/// Maximum streaming retries for the compaction summary call.
pub const MAX_COMPACT_STREAMING_RETRIES: u32 = 2;

/// Which compaction layer was applied during an iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CompactionLayer {
    /// Drop oldest messages (cheap, no LLM).
    Snip,
    /// Time-based clearing of large tool results.
    Microcompact,
    /// Cached microcompact path (wired in a later plan).
    CachedMicrocompact,
    /// Aggressive context-collapse (wired in a later plan).
    ContextCollapse,
    /// LLM-driven summarization.
    Autocompact,
    /// Partial autocompact path.
    PartialAutocompact,
}

/// Why compaction was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CompactionReason {
    /// Approaching the model's context window.
    TokenLimit,
    /// The user explicitly requested a compact.
    ManualRequest,
    /// Server reported the prompt was too long.
    PromptTooLong,
    /// Microcompact warned that recent results were too large.
    MicrocompactWarn,
}

/// Per-agent tracking state used to coordinate autocompact across iterations.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AutoCompactTrackingState {
    /// Whether autocompact has run for the current turn.
    pub compacted: bool,
    /// Monotonically increasing turn counter.
    pub turn_counter: u32,
    /// Identifier of the current turn (e.g. message ID).
    pub turn_id: String,
    /// How many autocompact attempts have failed in a row.
    pub consecutive_failures: u32,
}
