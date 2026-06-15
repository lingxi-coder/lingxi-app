//! `/stats` usage-stats screen (claude-code `Stats.tsx` parity): an in-tree
//! aggregation of the `*.jsonl` session transcripts under
//! `<claude_home>/projects/`, presented as a two-tab overlay (`Overview` /
//! `Models`) with a sparkline tokens-per-day chart and a GitHub-style activity
//! heatmap.
//!
//! Four-part split mirroring `skills.rs`/`agents.rs`/`theme.rs`: a
//! [`StatsState`] (aggregated [`StatsData`] + active [`StatsTab`] + an embedded
//! [`crate::screens::scroll::ScrollState`]), a [`StatsOutcome`] enum, a pure
//! [`handle_stats_key`] reducer (Tab/Shift-Tab switch tab, scroll keys via the
//! embedded `ScrollState`, Esc/`q` close), and a pure
//! [`render_stats_to_string`] oracle.
//!
//! Aggregation source: the same session transcripts the M5-08 resume loader
//! discovers (`<claude_home>/projects/<dir>/*.jsonl`), but walked across ALL
//! project dirs (claude-code `getAllSessionFiles`) — not just the cwd's. Each
//! line is the `JsonlMessage` wire shape; we read the inner `message.usage`
//! (input/output/cache-read tokens) + `message.model` from `assistant` rows and
//! the outer `timestamp` to bucket tokens/messages per day per model. The fs
//! walk runs OUTSIDE the `AppState` lock (the async `pump_open_stats` in
//! `root.rs`, mirroring `pump_open_agents`); the parse + aggregate functions in
//! this module are pure (`&str` in, terminal-free out) so they are unit-tested.
//!
//! Literal-lock (claude-code `Stats.tsx`): the two tab titles (`Overview` /
//! `Models`), the empty state `No stats available yet. Start using Claude
//! Code!`, the Overview field labels (`Favorite model:`, `Total tokens:`,
//! `Sessions:`, `Active days:`, `Most active day:`), the `Models` per-row
//! `{model} ({pct}%)` + `  In: {n} · Out: {n}` format, the `No model usage data
//! available` models-empty line, the `Tokens per Day` chart heading, and the
//! footer hint. The charts (sparkline + heatmap) are DATA-DERIVED, so their
//! tests assert STRUCTURAL invariants (bar count, row count) rather than a
//! byte-locked string.
//!
//! Deferred (documented, not dead-coded): claude-code's `ctrl+s`
//! screenshot-to-clipboard (`copyAnsiToClipboard`) has no in-tree clipboard
//! seam, so the `ctrl+s` binding and the `· {copyStatus}` footer suffix are
//! omitted. The `r`-cycles-date-range control is also omitted: the cache-backed
//! `7d`/`30d`/`all` ranges in `stats.ts` depend on a disk `statsCache` that the
//! TUI does not yet own — this screen renders the all-time aggregation only.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

use crate::screens::scroll::{scroll_indicator, visible_slice, ScrollState};

/// Fixed viewport height (body rows shown before scrolling kicks in). A modest
/// constant keeps the pure oracle deterministic; the live render is line-by-
/// line, but the embedded [`ScrollState`] keeps the screen scroll-capable and
/// unit-testable, mirroring `skills.rs`.
const VIEWPORT: usize = 16;

/// Locked tab titles (claude-code `Stats.tsx` `<Tab title=…>`).
pub const TAB_OVERVIEW: &str = "Overview";
/// Locked second tab title.
pub const TAB_MODELS: &str = "Models";
/// Locked empty-state line (claude-code `allTimeResult.type === "empty"`).
pub const EMPTY_LINE: &str = "No stats available yet. Start using Claude Code!";
/// Loading line shown while the background transcript walk runs (the history can
/// be many GB, so the aggregation is done off the UI thread).
pub const LOADING_LINE: &str = "Computing usage stats… (scanning transcript history)";
/// Locked models-tab empty line (claude-code `modelEntries.length === 0`).
pub const MODELS_EMPTY_LINE: &str = "No model usage data available";
/// Locked tokens-chart heading (claude-code `ModelsTab`).
pub const TOKENS_PER_DAY: &str = "Tokens per Day";
/// Locked footer hint. claude-code's footer is
/// `Esc to cancel · r to cycle dates · ctrl+s to copy`; the `r`/`ctrl+s`
/// controls are deferred (see module docs), so only the Esc + Tab affordances
/// this screen actually implements are shown.
pub const FOOTER: &str = "Tab to switch · Esc to close";

/// Per-model aggregated token usage (claude-code `ModelUsage`, the subset this
/// screen reads). Counts are monotonic sums across every `assistant` row that
/// named this model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Sum of `usage.input_tokens`.
    pub input_tokens: u64,
    /// Sum of `usage.output_tokens`.
    pub output_tokens: u64,
    /// Sum of `usage.cache_read_input_tokens`.
    pub cache_read_tokens: u64,
}

impl ModelUsage {
    /// `input + output` (the "total tokens" claude-code sorts + charts by;
    /// cache-read is shown but NOT counted toward the total, matching
    /// `modelTokens = inputTokens + outputTokens`).
    #[must_use]
    pub fn total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// The aggregated, terminal-free stats the screen renders. Built by
/// [`aggregate`] from per-session [`SessionContribution`]s. Carries no `f64`
/// (so `Screen`'s `PartialEq` is satisfiable and there is no Eq pitfall).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsData {
    /// Distinct non-subagent sessions counted.
    pub total_sessions: usize,
    /// Total `assistant`+`user` main-chain messages across sessions.
    pub total_messages: usize,
    /// `date (YYYY-MM-DD) -> message count` for the activity heatmap.
    pub daily_messages: BTreeMap<String, u64>,
    /// `date -> (model -> total tokens that day)` for the per-day chart.
    pub daily_model_tokens: BTreeMap<String, BTreeMap<String, u64>>,
    /// `model -> aggregated usage` for the Models tab.
    pub model_usage: BTreeMap<String, ModelUsage>,
    /// Earliest session date seen (`YYYY-MM-DD`), if any.
    pub first_date: Option<String>,
    /// Latest session date seen (`YYYY-MM-DD`), if any.
    pub last_date: Option<String>,
}

impl StatsData {
    /// `true` when nothing was aggregated (claude-code empty state — drives the
    /// `No stats available yet` line). Mirrors `totalSessions === 0`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_sessions == 0
    }

    /// Count of days with any activity (claude-code `activeDays`).
    #[must_use]
    pub fn active_days(&self) -> usize {
        self.daily_messages.len()
    }

    /// The day with the most messages (claude-code `peakActivityDay`). Ties
    /// resolve to the chronologically-earliest date (the `BTreeMap` walks
    /// ascending and a strict `>` keeps the first seen).
    #[must_use]
    pub fn peak_activity_day(&self) -> Option<&str> {
        self.daily_messages
            .iter()
            .fold(None::<(&String, u64)>, |best, (date, &count)| match best {
                Some((_, bc)) if bc >= count => best,
                _ => Some((date, count)),
            })
            .map(|(d, _)| d.as_str())
    }

    /// Sum of input+output tokens across every model (claude-code
    /// `totalTokens`).
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.model_usage
            .values()
            .fold(0u64, |acc, u| acc.saturating_add(u.total()))
    }

    /// Models sorted by total tokens DESC, tie-broken by name ASC (claude-code
    /// `modelEntries.sort((a,b) => b.total - a.total)`, with a stable name tie-
    /// break so the order is deterministic). Returns `(model, usage)` pairs.
    #[must_use]
    pub fn models_by_tokens(&self) -> Vec<(&str, ModelUsage)> {
        let mut entries: Vec<(&str, ModelUsage)> = self
            .model_usage
            .iter()
            .map(|(m, u)| (m.as_str(), *u))
            .collect();
        entries.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then_with(|| a.0.cmp(b.0)));
        entries
    }

    /// The favorite (top-token) model name (claude-code `modelEntries[0]`).
    #[must_use]
    pub fn favorite_model(&self) -> Option<&str> {
        self.models_by_tokens().first().map(|(m, _)| *m)
    }
}

/// One session's contribution to the aggregate — the pure result of parsing a
/// single `.jsonl` transcript's lines. Merged by [`aggregate`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionContribution {
    /// `true` when this file is a real (non-subagent) session that should be
    /// counted as a session + contribute to message/day counts. Subagent
    /// transcripts (`isSidechain`) still contribute token usage but are not
    /// sessions, mirroring claude-code's subagent handling.
    pub is_session: bool,
    /// Main-chain message count (non-sidechain user/assistant rows).
    pub message_count: usize,
    /// The session's date bucket (`YYYY-MM-DD` from the first main-chain
    /// message timestamp), if one was found.
    pub date: Option<String>,
    /// `model -> usage` accumulated from this file's `assistant` rows.
    pub model_usage: BTreeMap<String, ModelUsage>,
    /// `model -> total tokens` for this file's date bucket.
    pub day_model_tokens: BTreeMap<String, u64>,
}

/// Parse one `.jsonl` transcript body into a [`SessionContribution`]. Pure: no
/// I/O, no terminal. `content` is the raw file text (one JSON object per line);
/// `is_subagent` marks files under a `subagents/` dir (token usage counted, but
/// not as a session), mirroring claude-code `processSessionFiles`.
///
/// For each line we deserialize the outer `JsonlMessage` shape loosely (as a
/// `serde_json::Value`) and:
/// - skip non-transcript / malformed rows,
/// - take the first non-sidechain timestamp as the session date,
/// - count non-sidechain user/assistant rows as messages,
/// - read `message.usage.{input,output,cache_read_input}_tokens` + `message.model`
///   from `assistant` rows into the per-model + per-day token tallies,
/// - skip the synthetic model (`<synthetic>`), matching the TS `SYNTHETIC_MODEL`
///   guard.
#[must_use]
pub fn parse_session(content: &str, is_subagent: bool) -> SessionContribution {
    let mut out = SessionContribution {
        is_session: !is_subagent,
        ..SessionContribution::default()
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(kind) = v.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let is_sidechain = v
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        // Main-chain (non-sidechain) user/assistant rows define the session
        // date + message count. Subagent files are all-sidechain but still
        // contribute their tokens, so we DON'T gate token reads on this.
        let is_main_chat = !is_sidechain && (kind == "user" || kind == "assistant");
        if is_main_chat {
            if out.date.is_none() {
                if let Some(d) = date_bucket(&v) {
                    out.date = Some(d);
                }
            }
            out.message_count += 1;
        }

        if kind != "assistant" {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        let model = msg.get("model").and_then(serde_json::Value::as_str);
        let Some(model) = model else {
            continue;
        };
        // Skip synthetic internal messages (claude-code `SYNTHETIC_MODEL`).
        if model == SYNTHETIC_MODEL {
            continue;
        }
        let usage = msg.get("usage");
        let input = usage_field(usage, "input_tokens");
        let output = usage_field(usage, "output_tokens");
        let cache_read = usage_field(usage, "cache_read_input_tokens");

        let entry = out.model_usage.entry(model.to_string()).or_default();
        entry.input_tokens = entry.input_tokens.saturating_add(input);
        entry.output_tokens = entry.output_tokens.saturating_add(output);
        entry.cache_read_tokens = entry.cache_read_tokens.saturating_add(cache_read);

        let total = input.saturating_add(output);
        if total > 0 {
            let day = out.day_model_tokens.entry(model.to_string()).or_default();
            *day = day.saturating_add(total);
        }
    }

    out
}

/// The synthetic-model sentinel (claude-code `SYNTHETIC_MODEL`).
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Read a `u64` token field from the inner `message.usage` object, treating a
/// missing/non-numeric value as 0 (claude-code `usage.input_tokens || 0`).
fn usage_field(usage: Option<&serde_json::Value>, key: &str) -> u64 {
    usage
        .and_then(|u| u.get(key))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// The `YYYY-MM-DD` bucket from a row's outer `timestamp` (ISO-8601 like
/// `2026-05-25T14:30:00.000Z`). claude-code uses `toDateString(new Date(ts))`;
/// the date portion of an ISO-8601 UTC string is its first 10 chars, which we
/// validate as `dddd-dd-dd` before trusting it.
fn date_bucket(row: &serde_json::Value) -> Option<String> {
    let ts = row.get("timestamp").and_then(serde_json::Value::as_str)?;
    let head: String = ts.chars().take(10).collect();
    if is_iso_date(&head) {
        Some(head)
    } else {
        None
    }
}

/// `true` when `s` looks like `YYYY-MM-DD` (10 chars, digits with `-` at 4/7).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, &c)| matches!(i, 4 | 7) == (c == b'-') && (c == b'-' || c.is_ascii_digit()))
}

/// Merge per-session contributions into the final [`StatsData`]. Pure; the
/// caller (the async pump) supplies the parsed contributions.
#[must_use]
pub fn aggregate(contribs: &[SessionContribution]) -> StatsData {
    let mut data = StatsData::default();
    for c in contribs {
        if c.is_session {
            // A session with no parseable date is skipped entirely (claude-code
            // stats.ts drops files whose first message has no valid timestamp) —
            // counting it would inflate total_sessions/total_messages.
            if let Some(date) = &c.date {
                data.total_sessions += 1;
                data.total_messages += c.message_count;
                *data.daily_messages.entry(date.clone()).or_default() += c.message_count as u64;
                track_date(&mut data, date);
            }
        }
        // Merge per-model usage (subagent files contribute here too).
        for (model, usage) in &c.model_usage {
            let e = data.model_usage.entry(model.clone()).or_default();
            e.input_tokens = e.input_tokens.saturating_add(usage.input_tokens);
            e.output_tokens = e.output_tokens.saturating_add(usage.output_tokens);
            e.cache_read_tokens = e.cache_read_tokens.saturating_add(usage.cache_read_tokens);
        }
        // Merge per-day model tokens into the file's date bucket.
        if let Some(date) = &c.date {
            let day = data.daily_model_tokens.entry(date.clone()).or_default();
            for (model, &tokens) in &c.day_model_tokens {
                let slot = day.entry(model.clone()).or_default();
                *slot = slot.saturating_add(tokens);
            }
        }
    }
    data
}

/// Update `first_date`/`last_date` with `date` (lexicographic order is
/// chronological for `YYYY-MM-DD`).
fn track_date(stats: &mut StatsData, date: &str) {
    match &stats.first_date {
        Some(f) if f.as_str() <= date => {}
        _ => stats.first_date = Some(date.to_string()),
    }
    match &stats.last_date {
        Some(l) if l.as_str() >= date => {}
        _ => stats.last_date = Some(date.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Result cache (claude-code `statsCache.ts` parity). The intent is the same as
// claude-code's `PersistedStatsCache`: a second `/stats` open returns the
// already-aggregated [`StatsData`] WITHOUT re-walking + re-parsing the whole
// `<claude_home>/projects/` history, re-computing only when the history has
// actually changed.
//
// FORCED DIVERGENCE from claude-code: claude-code keys validity on a
// `lastComputedDate` day-watermark (it incrementally merges today's new rows
// into a cache whose historical days never change). We instead key the whole
// cache on a cheap path+mtime+size fingerprint of the transcript files
// ([`HistoryFingerprint`]) and invalidate the WHOLE cache when ANY file
// changes. This is coarser (a busy day forces a full re-walk) but matches the
// candidate design and needs no per-day merge bookkeeping; the fingerprint walk
// (readdir + metadata) is still cheap relative to read+parse, so even a miss is
// no slower than the un-cached path. Accordingly our [`STATS_CACHE_VERSION`] is
// its OWN counter (1), NOT claude-code's `3` — the on-disk schema differs.
// ---------------------------------------------------------------------------

/// One transcript file's identity for the history fingerprint: its path plus the
/// `(mtime, size)` pair claude-code itself trusts as a cheap change signal (it
/// skips re-reading files older than `fromDate`). An in-place edit that
/// preserves BOTH mtime and size can theoretically be missed — an accepted
/// limitation of this design.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileFingerprint {
    /// The file's path (lossy string form), the canonical-sort key.
    pub path: String,
    /// Last-modified time in nanoseconds since the Unix epoch (0 when unknown).
    pub mtime_ns: u128,
    /// File length in bytes.
    pub size: u64,
}

/// A fingerprint of the entire transcript history: the [`FileFingerprint`]s of
/// every walked `*.jsonl` file, kept SORTED by path so a reordered (e.g.
/// parallel or differently-ordered) directory walk produces an identical,
/// comparable fingerprint. Cache validity is `current == cached`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HistoryFingerprint {
    /// The per-file fingerprints, canonically sorted by `path`.
    pub files: Vec<FileFingerprint>,
}

impl HistoryFingerprint {
    /// Build from pre-collected per-file entries, sorting them by path so the
    /// fingerprint is canonical regardless of walk order. Pure: the caller does
    /// the `fs::metadata` reads (the impure shell lives in `root.rs`).
    #[must_use]
    pub fn from_entries(mut v: Vec<FileFingerprint>) -> Self {
        v.sort_by(|a, b| a.path.cmp(&b.path));
        Self { files: v }
    }
}

/// On-disk cache schema version (claude-code `STATS_CACHE_VERSION`). Bumped when
/// the [`PersistedStatsCache`] / [`StatsData`] shape changes so a stale file is
/// rejected by [`decode_stats_cache`] and falls back to a full walk. This is our
/// OWN counter — see the module-section note above for why it is not `3`.
pub const STATS_CACHE_VERSION: u32 = 1;

/// The on-disk cache envelope (claude-code `PersistedStatsCache`): the schema
/// version, the [`HistoryFingerprint`] the [`StatsData`] was computed from, and
/// the aggregated data itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedStatsCache {
    /// Schema version, checked against [`STATS_CACHE_VERSION`] on load.
    pub version: u32,
    /// The history fingerprint this `data` was aggregated from.
    pub fingerprint: HistoryFingerprint,
    /// The cached aggregation result.
    pub data: StatsData,
}

/// Validate + decode a serialized [`PersistedStatsCache`] (claude-code
/// `loadStatsCache`). Returns `Some(data)` only on a clean HIT: the JSON parses,
/// the version matches [`STATS_CACHE_VERSION`], AND the embedded fingerprint
/// equals `current`. A parse error, a version mismatch, OR a fingerprint
/// mismatch (the invalidation) all yield `None`, mirroring claude-code's
/// `getEmptyCache` fallback so a corrupt / old / foreign cache degrades
/// gracefully to a full walk.
#[must_use]
pub fn decode_stats_cache(json: &str, current: &HistoryFingerprint) -> Option<StatsData> {
    serde_json::from_str::<PersistedStatsCache>(json)
        .ok()
        .filter(|c| c.version == STATS_CACHE_VERSION && &c.fingerprint == current)
        .map(|c| c.data)
}

/// Serialize a [`PersistedStatsCache`] for writing to disk (claude-code
/// `saveStatsCache`). Stamps the current [`STATS_CACHE_VERSION`]. A serialization
/// failure yields an empty string (the best-effort writer in `root.rs` swallows
/// it; a subsequent load just misses and re-walks).
#[must_use]
pub fn encode_stats_cache(fingerprint: &HistoryFingerprint, data: &StatsData) -> String {
    serde_json::to_string(&PersistedStatsCache {
        version: STATS_CACHE_VERSION,
        fingerprint: fingerprint.clone(),
        data: data.clone(),
    })
    .unwrap_or_default()
}

/// Which tab the Stats screen shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatsTab {
    /// Activity overview (heatmap + headline numbers).
    #[default]
    Overview,
    /// Per-model token breakdown + tokens-per-day sparkline.
    Models,
}

impl StatsTab {
    /// The other tab (Tab / Shift-Tab both toggle between the two).
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            StatsTab::Overview => StatsTab::Models,
            StatsTab::Models => StatsTab::Overview,
        }
    }

    /// Locked title for this tab.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            StatsTab::Overview => TAB_OVERVIEW,
            StatsTab::Models => TAB_MODELS,
        }
    }
}

/// Screen state: the aggregated data, the active tab, and the scroll window
/// over the active tab's flattened body lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatsState {
    /// The aggregated stats (empty until the pump fills them).
    pub data: StatsData,
    /// The active tab.
    pub tab: StatsTab,
    /// Scroll window over the active tab's body lines.
    pub scroll: ScrollState,
    /// `true` while the (potentially multi-GB) transcript walk runs on the
    /// blocking pool; the screen shows a "computing" line until [`set_data`]
    /// fills it. Opening via [`loading`](StatsState::loading) sets this.
    pub loading: bool,
}

impl StatsState {
    /// Build from aggregated data, sizing the embedded [`ScrollState`] to the
    /// Overview tab's body-line count and the fixed [`VIEWPORT`].
    #[must_use]
    pub fn new(data: StatsData) -> Self {
        let tab = StatsTab::Overview;
        let len = body_lines(&data, tab).len();
        Self {
            data,
            tab,
            scroll: ScrollState::new(len, VIEWPORT),
            loading: false,
        }
    }

    /// Open in the LOADING state (empty data) while the background aggregation
    /// runs. [`set_data`](StatsState::set_data) replaces the data and clears the
    /// flag. Mirrors claude-code showing a spinner before the stats cache fills.
    #[must_use]
    pub fn loading() -> Self {
        Self {
            loading: true,
            ..Self::new(StatsData::default())
        }
    }

    /// Replace the aggregated data (clears `loading`, re-anchors the scroll to
    /// the current tab's body length).
    pub fn set_data(&mut self, data: StatsData) {
        let len = body_lines(&data, self.tab).len();
        self.data = data;
        self.scroll = ScrollState::new(len, VIEWPORT);
        self.loading = false;
    }

    /// Switch to `tab`, re-sizing the scroll window to that tab's body and
    /// re-anchoring at the top (claude-code resets the Models scroll on tab
    /// switch).
    fn set_tab(&mut self, tab: StatsTab) {
        self.tab = tab;
        let len = body_lines(&self.data, tab).len();
        self.scroll = ScrollState::new(len, VIEWPORT);
    }
}

/// Controller outcome after a key (mirrors `SkillsOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsOutcome {
    /// Stay open (tab switched / scrolled / inert key).
    Stay,
    /// Close the screen (Esc / `q`).
    Close,
}

/// Reduce one key. `Tab`/`BackTab` toggle the active tab (re-anchoring scroll);
/// scroll keys (Up/Down/PageUp/PageDown/Home/End) drive the embedded
/// [`ScrollState`]; Esc and bare `q` close. Everything else is inert. Pure — the
/// caller owns closing the screen + telemetry.
#[must_use]
pub fn handle_stats_key(state: &mut StatsState, key: KeyEvent) -> StatsOutcome {
    match key.code {
        KeyCode::Tab | KeyCode::BackTab => {
            state.set_tab(state.tab.toggled());
            StatsOutcome::Stay
        }
        KeyCode::Esc => StatsOutcome::Close,
        KeyCode::Char('q') if key.modifiers == KeyModifiers::NONE => StatsOutcome::Close,
        _ => {
            // Scroll keys consume Up/Down/Page/Home/End; anything else is inert.
            let _ = state.scroll.handle_scroll_key(key);
            StatsOutcome::Stay
        }
    }
}

/// Port of claude-code `formatNumber` for the counts shown on the screen:
/// values under 1000 render as the plain integer; 1000+ use compact `k`/`m`
/// notation with one fraction digit, trailing `.0` stripped (`1300 -> "1.3k"`,
/// `2_000_000 -> "2m"`). Matches `skills::format_tokens`.
#[must_use]
pub fn format_number(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    #[allow(clippy::cast_precision_loss)]
    let (value, suffix) = if count >= 1_000_000 {
        (count as f64 / 1_000_000.0, 'm')
    } else {
        (count as f64 / 1000.0, 'k')
    };
    let mut s = format!("{value:.1}");
    if let Some(stripped) = s.strip_suffix(".0") {
        s = stripped.to_string();
    }
    format!("{s}{suffix}")
}

/// Format a `model_tokens / total * 100` percentage with one fraction digit
/// (claude-code `(modelTokens / totalTokens * 100).toFixed(1)`). `total == 0`
/// yields `"0.0"` (no division by zero).
fn format_pct(model_tokens: u64, total: u64) -> String {
    if total == 0 {
        return "0.0".to_string();
    }
    #[allow(clippy::cast_precision_loss)]
    let pct = (model_tokens as f64 / total as f64) * 100.0;
    format!("{pct:.1}")
}

/// An eight-level asciichart-style sparkline of `values` (one bar glyph per
/// value), scaled so the max value maps to the tallest bar. An empty input or
/// an all-zero series yields an empty string (claude-code renders no chart for
/// `< 2` points / no data; the caller gates the heading on this).
///
/// Structural invariant (unit-tested, NOT byte-locked — bars are data-derived):
/// the output has exactly `values.len()` glyphs, each one of the 8 block-bar
/// characters `▁▂▃▄▅▆▇█`.
#[must_use]
pub fn sparkline(values: &[u64]) -> String {
    const BARS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return String::new();
    }
    values
        .iter()
        .map(|&v| {
            // Map v in 0..=max to a bar index in 0..=7. Zero stays at the
            // shortest bar; the max hits the tallest.
            #[allow(clippy::cast_possible_truncation)]
            let idx = ((v.saturating_mul(7)) / max) as usize;
            BARS[idx.min(7)]
        })
        .collect()
}

/// A GitHub-style activity heatmap of `daily` (`date -> message count`), as a
/// `Vec` of rows — a structural port of claude-code `generateHeatmap`. We keep
/// it COMPACT (a single intensity strip ordered by date, one glyph per active
/// day) since the screen is a string-render overlay without the Ink fixed-width
/// week grid; the glyph set + the `Less … More` legend match the TS heatmap.
///
/// Returns `[]` when there is no activity (caller omits the section).
///
/// Structural invariant (unit-tested): the strip has exactly `daily.len()`
/// glyphs (one per active day), each one of `· ░ ▒ ▓ █`, and the legend line is
/// present.
#[must_use]
pub fn heatmap(daily: &BTreeMap<String, u64>) -> Vec<String> {
    if daily.is_empty() {
        return Vec::new();
    }
    let counts: Vec<u64> = daily.values().copied().filter(|&c| c > 0).collect();
    let pct = percentiles(&counts);
    // One glyph per day, ascending by date (BTreeMap iteration order).
    let strip: String = daily
        .values()
        .map(|&c| heatmap_char(intensity(c, pct)))
        .collect();
    vec![
        strip,
        format!(
            "Less {} {} {} {} More",
            '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'
        ),
    ]
}

/// `(p25, p50, p75)` of `counts` (claude-code `calculatePercentiles`, which
/// sorts then indexes `floor(len * q)`). `None` when there is no positive
/// count.
fn percentiles(counts: &[u64]) -> Option<(u64, u64, u64)> {
    if counts.is_empty() {
        return None;
    }
    let mut sorted = counts.to_vec();
    sorted.sort_unstable();
    let at = |q: f64| -> u64 {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let idx = ((sorted.len() as f64) * q) as usize;
        sorted[idx.min(sorted.len() - 1)]
    };
    Some((at(0.25), at(0.5), at(0.75)))
}

/// Intensity bucket 0..=4 for a day's message count (claude-code
/// `getIntensity`).
fn intensity(count: u64, pct: Option<(u64, u64, u64)>) -> u8 {
    let Some((p25, p50, p75)) = pct else {
        return 0;
    };
    if count == 0 {
        0
    } else if count >= p75 {
        4
    } else if count >= p50 {
        3
    } else if count >= p25 {
        2
    } else {
        1
    }
}

/// Heatmap glyph for an intensity bucket (claude-code `getHeatmapChar`).
fn heatmap_char(level: u8) -> char {
    match level {
        1 => '\u{2591}', // ░
        2 => '\u{2592}', // ▒
        3 => '\u{2593}', // ▓
        4 => '\u{2588}', // █
        _ => '\u{00B7}', // ·
    }
}

/// Format a `YYYY-MM-DD` date as `Mon D` (claude-code `formatPeakDay`'s
/// `toLocaleDateString('en-US', { month:'short', day:'numeric' })`). Falls back
/// to the raw string when it is not a well-formed date.
fn format_peak_day(date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !is_iso_date(date) {
        return date.to_string();
    }
    let month: usize = date[5..7].parse::<usize>().unwrap_or(0);
    let day: u32 = date[8..10].parse::<u32>().unwrap_or(0);
    if (1..=12).contains(&month) && day >= 1 {
        format!("{} {day}", MONTHS[month - 1])
    } else {
        date.to_string()
    }
}

/// The Overview tab's body lines (heatmap + headline fields), claude-code
/// `OverviewTab` order: heatmap, then Favorite model / Total tokens / Sessions
/// / Active days / Most active day.
fn overview_lines(data: &StatsData) -> Vec<String> {
    let mut out = Vec::new();
    for row in heatmap(&data.daily_messages) {
        out.push(row);
    }
    if let Some(fav) = data.favorite_model() {
        out.push(format!("Favorite model: {fav}"));
    }
    out.push(format!(
        "Total tokens: {}",
        format_number(data.total_tokens())
    ));
    out.push(format!(
        "Sessions: {}",
        format_number(data.total_sessions as u64)
    ));
    out.push(format!("Active days: {}", data.active_days()));
    if let Some(day) = data.peak_activity_day() {
        out.push(format!("Most active day: {}", format_peak_day(day)));
    }
    out
}

/// The Models tab's body lines: the `Tokens per Day` sparkline (when there are
/// ≥2 days of token data) then one two-line block per model
/// (`{model} ({pct}%)` + `  In: {n} · Out: {n}`), claude-code `ModelsTab` +
/// `ModelEntry`. `No model usage data available` when there is no model data.
fn models_lines(data: &StatsData) -> Vec<String> {
    let entries = data.models_by_tokens();
    if entries.is_empty() {
        return vec![MODELS_EMPTY_LINE.to_string()];
    }
    let mut out = Vec::new();

    // Tokens-per-day sparkline for the top model (claude-code charts the top
    // models; we sparkline the favorite's daily totals). Needs ≥2 days.
    if let Some((top, _)) = entries.first() {
        let series: Vec<u64> = data
            .daily_model_tokens
            .values()
            .map(|day| day.get(*top).copied().unwrap_or(0))
            .collect();
        if series.len() >= 2 {
            let line = sparkline(&series);
            if !line.is_empty() {
                out.push(TOKENS_PER_DAY.to_string());
                out.push(line);
            }
        }
    }

    let total = data.total_tokens();
    for (model, usage) in entries {
        out.push(format!("{model} ({}%)", format_pct(usage.total(), total)));
        out.push(format!(
            "  In: {} \u{00B7} Out: {}",
            format_number(usage.input_tokens),
            format_number(usage.output_tokens)
        ));
    }
    out
}

/// The active tab's body lines (the list the embedded [`ScrollState`] scrolls).
fn body_lines(data: &StatsData, tab: StatsTab) -> Vec<String> {
    match tab {
        StatsTab::Overview => overview_lines(data),
        StatsTab::Models => models_lines(data),
    }
}

/// The tab header line: the two titles with the active one marked. Locked
/// format: `[Overview] Models` (active in brackets) / `Overview [Models]`.
fn tab_header(active: StatsTab) -> String {
    let mark = |t: StatsTab| -> String {
        if t == active {
            format!("[{}]", t.title())
        } else {
            t.title().to_string()
        }
    };
    format!("{} {}", mark(StatsTab::Overview), mark(StatsTab::Models))
}

/// Pure render oracle: tab header + the active tab's visible body window +
/// (when scrolled) a scroll indicator + the footer.
///
/// Empty (no sessions): the locked `No stats available yet…` line + footer.
#[must_use]
pub fn render_stats_to_string(state: &StatsState) -> String {
    if state.loading {
        return format!("{LOADING_LINE}\n{FOOTER}");
    }
    if state.data.is_empty() {
        return format!("{EMPTY_LINE}\n{FOOTER}");
    }
    let mut out = tab_header(state.tab);
    out.push('\n');

    let lines = body_lines(&state.data, state.tab);
    for line in visible_slice(&lines, &state.scroll) {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(ind) = scroll_indicator(&state.scroll) {
        out.push_str(&ind);
        out.push('\n');
    }
    out.push_str(FOOTER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Build a minimal one-line assistant JSONL row with the given model +
    /// token counts and a fixed date.
    fn assistant_line(date: &str, model: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":false,"timestamp":"{date}T10:00:00.000Z","message":{{"model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0}}}}}}"#
        )
    }

    fn user_line(date: &str) -> String {
        format!(
            r#"{{"type":"user","isSidechain":false,"timestamp":"{date}T09:00:00.000Z","message":{{"role":"user","content":"hi"}}}}"#
        )
    }

    #[test]
    fn parse_counts_messages_tokens_and_date() {
        let content = format!(
            "{}\n{}\n",
            user_line("2026-05-01"),
            assistant_line("2026-05-01", "claude-opus", 100, 50)
        );
        let c = parse_session(&content, false);
        assert!(c.is_session);
        assert_eq!(c.message_count, 2);
        assert_eq!(c.date.as_deref(), Some("2026-05-01"));
        let u = c
            .model_usage
            .get("claude-opus")
            .copied()
            .unwrap_or_default();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(c.day_model_tokens.get("claude-opus").copied(), Some(150));
    }

    #[test]
    fn parse_skips_synthetic_and_malformed() {
        let content = format!(
            "not json\n{}\n{}\n",
            assistant_line("2026-05-02", SYNTHETIC_MODEL, 999, 999),
            assistant_line("2026-05-02", "claude-sonnet", 10, 5)
        );
        let c = parse_session(&content, false);
        // Synthetic model excluded; only the real one tallied.
        assert!(!c.model_usage.contains_key(SYNTHETIC_MODEL));
        assert_eq!(
            c.model_usage
                .get("claude-sonnet")
                .copied()
                .unwrap_or_default()
                .total(),
            15
        );
    }

    #[test]
    fn subagent_file_contributes_tokens_but_not_sessions() {
        let content = assistant_line("2026-05-03", "claude-opus", 20, 30);
        let c = parse_session(&content, true);
        assert!(!c.is_session);
        // Tokens still tallied.
        assert_eq!(
            c.model_usage
                .get("claude-opus")
                .copied()
                .unwrap_or_default()
                .total(),
            50
        );
    }

    #[test]
    fn aggregate_merges_sessions_and_models() {
        let s1 = parse_session(
            &format!(
                "{}\n{}\n",
                user_line("2026-05-01"),
                assistant_line("2026-05-01", "claude-opus", 100, 50)
            ),
            false,
        );
        let s2 = parse_session(
            &format!(
                "{}\n{}\n",
                user_line("2026-05-02"),
                assistant_line("2026-05-02", "claude-opus", 10, 5)
            ),
            false,
        );
        let data = aggregate(&[s1, s2]);
        assert_eq!(data.total_sessions, 2);
        assert_eq!(data.total_messages, 4);
        assert_eq!(data.active_days(), 2);
        assert_eq!(data.total_tokens(), 165);
        assert_eq!(data.first_date.as_deref(), Some("2026-05-01"));
        assert_eq!(data.last_date.as_deref(), Some("2026-05-02"));
        // Per-model merged.
        assert_eq!(
            data.model_usage
                .get("claude-opus")
                .copied()
                .unwrap_or_default()
                .total(),
            165
        );
    }

    #[test]
    fn peak_activity_day_picks_the_busiest() {
        let mut data = StatsData::default();
        data.daily_messages.insert("2026-05-01".into(), 3);
        data.daily_messages.insert("2026-05-02".into(), 9);
        data.daily_messages.insert("2026-05-03".into(), 1);
        assert_eq!(data.peak_activity_day(), Some("2026-05-02"));
    }

    #[test]
    fn models_sorted_by_tokens_desc() {
        let mut data = StatsData::default();
        data.model_usage.insert(
            "small".into(),
            ModelUsage {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
            },
        );
        data.model_usage.insert(
            "big".into(),
            ModelUsage {
                input_tokens: 100,
                output_tokens: 100,
                cache_read_tokens: 0,
            },
        );
        let order: Vec<&str> = data
            .models_by_tokens()
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        assert_eq!(order, vec!["big", "small"]);
        assert_eq!(data.favorite_model(), Some("big"));
    }

    #[test]
    fn empty_state_is_byte_locked() {
        let s = StatsState::new(StatsData::default());
        assert!(s.data.is_empty());
        let out = render_stats_to_string(&s);
        assert_eq!(
            out,
            "No stats available yet. Start using Claude Code!\nTab to switch · Esc to close"
        );
    }

    #[test]
    fn loading_state_renders_loading_line_then_set_data_clears_it() {
        // A loading screen shows the LOADING_LINE (not the empty/data view) so
        // the user gets instant feedback while the off-thread walk runs.
        let mut s = StatsState::loading();
        assert!(s.loading);
        assert_eq!(
            render_stats_to_string(&s),
            format!("{LOADING_LINE}\n{FOOTER}")
        );

        // Filling data clears the loading flag and switches to the real render.
        let data = StatsData {
            total_sessions: 3,
            ..Default::default()
        };
        s.set_data(data);
        assert!(!s.loading);
        assert!(!render_stats_to_string(&s).contains(LOADING_LINE));
    }

    #[test]
    fn render_shows_tab_header_and_overview_fields() {
        let s = parse_session(
            &format!(
                "{}\n{}\n",
                user_line("2026-05-01"),
                assistant_line("2026-05-01", "claude-opus", 100, 50)
            ),
            false,
        );
        let st = StatsState::new(aggregate(&[s]));
        let out = render_stats_to_string(&st);
        assert!(out.starts_with("[Overview] Models\n"), "got: {out}");
        assert!(out.contains("Favorite model: claude-opus"), "got: {out}");
        assert!(out.contains("Total tokens: 150"), "got: {out}");
        assert!(out.contains("Sessions: 1"), "got: {out}");
        assert!(out.contains("Active days: 1"), "got: {out}");
        assert!(out.contains("Most active day: May 1"), "got: {out}");
        assert!(out.ends_with(FOOTER), "got: {out}");
    }

    #[test]
    fn tab_cycles_and_models_tab_renders_rows() {
        let s = parse_session(
            &format!(
                "{}\n{}\n",
                user_line("2026-05-01"),
                assistant_line("2026-05-01", "claude-opus", 1200, 300)
            ),
            false,
        );
        let mut st = StatsState::new(aggregate(&[s]));
        assert_eq!(st.tab, StatsTab::Overview);
        // Tab -> Models.
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::Tab)),
            StatsOutcome::Stay
        );
        assert_eq!(st.tab, StatsTab::Models);
        let out = render_stats_to_string(&st);
        assert!(out.starts_with("Overview [Models]\n"), "got: {out}");
        // Per-model row: name (pct%) + In/Out, with compact formatting (1200 -> 1.2k).
        assert!(out.contains("claude-opus (100.0%)"), "got: {out}");
        assert!(out.contains("  In: 1.2k \u{00B7} Out: 300"), "got: {out}");
        // BackTab cycles back to Overview.
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::BackTab)),
            StatsOutcome::Stay
        );
        assert_eq!(st.tab, StatsTab::Overview);
    }

    #[test]
    fn models_empty_when_no_model_data() {
        // A session with only a user message: a session, but no model usage.
        let s = parse_session(&format!("{}\n", user_line("2026-05-01")), false);
        let mut st = StatsState::new(aggregate(&[s]));
        st.set_tab(StatsTab::Models);
        let out = render_stats_to_string(&st);
        assert!(out.contains(MODELS_EMPTY_LINE), "got: {out}");
    }

    #[test]
    fn esc_and_q_close_other_keys_stay() {
        let s = parse_session(
            &format!("{}\n", assistant_line("2026-05-01", "m", 1, 1)),
            false,
        );
        let mut st = StatsState::new(aggregate(&[s]));
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::Esc)),
            StatsOutcome::Close
        );
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::Char('q'))),
            StatsOutcome::Close
        );
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::Enter)),
            StatsOutcome::Stay
        );
    }

    #[test]
    fn scroll_keys_move_window_and_stay() {
        // Many models -> a tall Models body so scrolling is live.
        let lines: Vec<SessionContribution> = (0..40)
            .map(|i| {
                parse_session(
                    &format!(
                        "{}\n",
                        assistant_line("2026-05-01", &format!("model-{i:02}"), i + 1, 1)
                    ),
                    false,
                )
            })
            .collect();
        let mut st = StatsState::new(aggregate(&lines));
        st.set_tab(StatsTab::Models);
        assert!(st.scroll.is_scrollable());
        assert_eq!(st.scroll.offset(), 0);
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::Down)),
            StatsOutcome::Stay
        );
        assert_eq!(st.scroll.offset(), 1);
        assert_eq!(
            handle_stats_key(&mut st, k(KeyCode::End)),
            StatsOutcome::Stay
        );
        assert_eq!(st.scroll.offset(), st.scroll.max_offset());
    }

    #[test]
    fn sparkline_has_one_bar_per_value_and_valid_glyphs() {
        let bars: &[char] = &[
            '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
            '\u{2588}',
        ];
        let s = sparkline(&[1, 5, 3, 8, 2]);
        assert_eq!(s.chars().count(), 5);
        assert!(s.chars().all(|c| bars.contains(&c)), "got: {s}");
        // Max value -> tallest bar.
        assert!(s.ends_with('\u{2582}')); // last value 2 of max 8 -> idx (2*7)/8=1
        assert_eq!(s.chars().nth(3), Some('\u{2588}')); // value 8 == max -> idx 7
                                                        // Empty / all-zero -> empty string.
        assert_eq!(sparkline(&[]), "");
        assert_eq!(sparkline(&[0, 0, 0]), "");
    }

    #[test]
    fn heatmap_strip_has_one_glyph_per_day_plus_legend() {
        let valid: &[char] = &['\u{00B7}', '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'];
        let mut daily = BTreeMap::new();
        daily.insert("2026-05-01".to_string(), 1u64);
        daily.insert("2026-05-02".to_string(), 5u64);
        daily.insert("2026-05-03".to_string(), 9u64);
        let rows = heatmap(&daily);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].chars().count(), 3);
        assert!(
            rows[0].chars().all(|c| valid.contains(&c)),
            "got: {}",
            rows[0]
        );
        assert!(rows[1].starts_with("Less "));
        assert!(rows[1].ends_with(" More"));
        // Empty -> no rows.
        assert!(heatmap(&BTreeMap::new()).is_empty());
    }

    #[test]
    fn format_number_compact() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1k");
        assert_eq!(format_number(1300), "1.3k");
        assert_eq!(format_number(2_000_000), "2m");
    }

    #[test]
    fn format_pct_handles_zero_total() {
        assert_eq!(format_pct(0, 0), "0.0");
        assert_eq!(format_pct(50, 200), "25.0");
    }

    #[test]
    fn format_peak_day_renders_month_name() {
        assert_eq!(format_peak_day("2026-05-01"), "May 1");
        assert_eq!(format_peak_day("2026-12-25"), "Dec 25");
        // Malformed falls back to the raw string.
        assert_eq!(format_peak_day("not-a-date"), "not-a-date");
    }

    // ---- Result-cache primitives (claude-code `statsCache.ts` parity). ----

    /// A non-empty `StatsData` built via the real `aggregate` path, to exercise
    /// the serde round-trip over its `BTreeMap`s + `ModelUsage`.
    fn sample_data() -> StatsData {
        let s = parse_session(
            &format!(
                "{}\n{}\n",
                user_line("2026-05-01"),
                assistant_line("2026-05-01", "claude-opus", 100, 50)
            ),
            false,
        );
        aggregate(&[s])
    }

    fn fp_one() -> HistoryFingerprint {
        HistoryFingerprint::from_entries(vec![FileFingerprint {
            path: "a.jsonl".into(),
            mtime_ns: 1,
            size: 10,
        }])
    }

    #[test]
    fn cache_roundtrip_hit() {
        let data = sample_data();
        assert!(!data.is_empty());
        let fp = fp_one();
        let json = encode_stats_cache(&fp, &data);
        // Same fingerprint + version → HIT, data recovered verbatim.
        assert_eq!(decode_stats_cache(&json, &fp), Some(data));
    }

    #[test]
    fn cache_miss_on_fingerprint_change() {
        let data = sample_data();
        let fp = fp_one();
        let json = encode_stats_cache(&fp, &data);
        // Bumped mtime → MISS (an edited file invalidates the cache).
        let fp_mtime = HistoryFingerprint::from_entries(vec![FileFingerprint {
            path: "a.jsonl".into(),
            mtime_ns: 2,
            size: 10,
        }]);
        assert_eq!(decode_stats_cache(&json, &fp_mtime), None);
        // Bumped size → MISS.
        let fp_size = HistoryFingerprint::from_entries(vec![FileFingerprint {
            path: "a.jsonl".into(),
            mtime_ns: 1,
            size: 11,
        }]);
        assert_eq!(decode_stats_cache(&json, &fp_size), None);
        // Extra file → MISS (a new transcript invalidates the cache).
        let fp_extra = HistoryFingerprint::from_entries(vec![
            FileFingerprint {
                path: "a.jsonl".into(),
                mtime_ns: 1,
                size: 10,
            },
            FileFingerprint {
                path: "b.jsonl".into(),
                mtime_ns: 1,
                size: 10,
            },
        ]);
        assert_eq!(decode_stats_cache(&json, &fp_extra), None);
    }

    #[test]
    fn cache_miss_on_version_mismatch() {
        let data = sample_data();
        let fp = fp_one();
        // Serialize at the current version, then string-replace it with a future
        // one the decoder must reject (claude-code version gate).
        let json = encode_stats_cache(&fp, &data).replace(
            &format!("\"version\":{STATS_CACHE_VERSION}"),
            "\"version\":999",
        );
        assert!(
            json.contains("\"version\":999"),
            "version bump applied: {json}"
        );
        assert_eq!(decode_stats_cache(&json, &fp), None);
    }

    #[test]
    fn cache_miss_on_garbage() {
        let fp = fp_one();
        // Non-JSON and empty input both decode to None (no panic).
        assert_eq!(decode_stats_cache("not json", &fp), None);
        assert_eq!(decode_stats_cache("", &fp), None);
    }

    #[test]
    fn fingerprint_is_order_independent() {
        // A reordered directory walk must produce an EQUAL fingerprint (→ HIT),
        // so equality survives a parallel / differently-ordered readdir.
        let a = FileFingerprint {
            path: "a.jsonl".into(),
            mtime_ns: 1,
            size: 10,
        };
        let b = FileFingerprint {
            path: "b.jsonl".into(),
            mtime_ns: 2,
            size: 20,
        };
        let from_ab = HistoryFingerprint::from_entries(vec![a.clone(), b.clone()]);
        let from_ba = HistoryFingerprint::from_entries(vec![b, a]);
        assert_eq!(from_ab, from_ba);
        // And the canonical order is by path ascending.
        assert_eq!(from_ab.files[0].path, "a.jsonl");
    }

    #[test]
    fn statsdata_serde_roundtrip() {
        // Covers the new `Serialize`/`Deserialize` derives over `StatsData`'s
        // `BTreeMap`s (incl. the nested `daily_model_tokens`) and `ModelUsage`.
        let data = sample_data();
        let json = serde_json::to_string(&data).expect("serialize StatsData");
        let back: StatsData = serde_json::from_str(&json).expect("deserialize StatsData");
        assert_eq!(back, data);
    }
}
