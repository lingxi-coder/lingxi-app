//! `/stats` usage-stats screen (claude-code `Stats.tsx` parity): an in-tree
//! aggregation of the `*.jsonl` session transcripts under
//! `<lingxi_home>/projects/`, presented as a two-tab overlay (`Overview` /
//! `Models`) with a multi-series asciichart tokens-per-day chart and a
//! GitHub-style activity heatmap.
//!
//! Four-part split mirroring `skills.rs`/`agents.rs`/`theme.rs`: a
//! [`StatsState`] (aggregated [`StatsData`] + active [`StatsTab`] + an embedded
//! [`crate::screens::scroll::ScrollState`]), a [`StatsOutcome`] enum, a pure
//! [`handle_stats_key`] reducer (Tab/Shift-Tab switch tab, scroll keys via the
//! embedded `ScrollState`, Esc/`q` close), and a pure
//! [`render_stats_to_string`] oracle.
//!
//! Aggregation source: the same session transcripts the M5-08 resume loader
//! discovers (`<lingxi_home>/projects/<dir>/*.jsonl`), but walked across ALL
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

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};
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
pub const EMPTY_LINE: &str = "No stats available yet. Start using LingXi!";
/// Loading line shown while the background transcript walk runs (the history can
/// be many GB, so the aggregation is done off the UI thread).
pub const LOADING_LINE: &str = "Computing usage stats… (scanning transcript history)";
/// Locked models-tab empty line (claude-code `modelEntries.length === 0`).
pub const MODELS_EMPTY_LINE: &str = "No model usage data available";
/// Locked tokens-chart heading (claude-code `ModelsTab`).
pub const TOKENS_PER_DAY: &str = "Tokens per Day";
/// Locked footer hint. claude-code's footer is
/// `Esc to cancel · r to cycle dates · ctrl+s to copy`. The `r`-cycle now
/// works (`StatsState::cycle_range`), so its hint is rendered; `ctrl+s` copy
/// is still deferred (no clipboard seam). The Rust-invented `Tab to switch`
/// is NOT rendered — tab switching is discoverable from the tab headers.
pub const FOOTER: &str = "Esc to cancel \u{00B7} r to cycle dates";

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
    /// (session-duration) Longest single-session duration in milliseconds
    /// (claude-code `longestSession.duration`), 0 when no session had a
    /// measurable span. `#[serde(default)]` so a pre-field cache still loads.
    #[serde(default)]
    pub longest_session_ms: u64,
    /// (stats-overview-missing-fields) The fun factoid line shown under the
    /// Overview (claude-code `generateFunFactoid`), `None` when no comparison
    /// applies. Picked once at aggregate time.
    #[serde(default)]
    pub factoid: Option<String>,
    /// (stats-date-range) Per-session `(date, duration_ms)`, retained so a
    /// date-range view can recount sessions + recompute the longest within the
    /// window. One entry per counted (non-subagent, dated) session.
    #[serde(default)]
    pub sessions: Vec<(String, u64)>,
    /// (stats-date-range) `date -> model -> usage` with the In/Out/cache splits
    /// (unlike `daily_model_tokens`, which is date→model→TOTAL), so the Models
    /// tab can be recomputed for a date window.
    #[serde(default)]
    pub daily_model_usage: BTreeMap<String, BTreeMap<String, ModelUsage>>,
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

    /// Total span of days covered (first→last activity date, inclusive) — the
    /// `/N` denominator on the "Active days" line. Falls back to
    /// [`active_days`](Self::active_days) when the dates are missing/unparsable.
    #[must_use]
    pub fn range_days(&self) -> usize {
        match (self.first_date.as_deref(), self.last_date.as_deref()) {
            (Some(f), Some(l)) => {
                match (
                    NaiveDate::parse_from_str(f, "%Y-%m-%d"),
                    NaiveDate::parse_from_str(l, "%Y-%m-%d"),
                ) {
                    (Ok(fd), Ok(ld)) => usize::try_from((ld - fd).num_days() + 1)
                        .unwrap_or(0)
                        .max(1),
                    _ => self.active_days(),
                }
            }
            _ => self.active_days(),
        }
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
    /// Raw ISO-8601 timestamp of the FIRST main-chain message (session start),
    /// used with [`Self::last_ts`] to compute the session duration
    /// (claude-code `lastTimestamp - firstTimestamp`).
    pub first_ts: Option<String>,
    /// Raw ISO-8601 timestamp of the LAST main-chain message (session end).
    pub last_ts: Option<String>,
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
            // (session-duration) Track first + last main-chain timestamp.
            if let Some(ts) = v.get("timestamp").and_then(serde_json::Value::as_str) {
                if out.first_ts.is_none() {
                    out.first_ts = Some(ts.to_string());
                }
                out.last_ts = Some(ts.to_string());
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
                // (session-duration) longest session = max(last_ts - first_ts).
                let dur = match (&c.first_ts, &c.last_ts) {
                    (Some(first), Some(last)) => session_duration_ms(first, last),
                    _ => 0,
                };
                data.longest_session_ms = data.longest_session_ms.max(dur);
                // (stats-date-range) retain per-session (date, duration) +
                // dated per-model usage with In/Out splits.
                data.sessions.push((date.clone(), dur));
                let day = data.daily_model_usage.entry(date.clone()).or_default();
                for (model, usage) in &c.model_usage {
                    let slot = day.entry(model.clone()).or_default();
                    slot.input_tokens = slot.input_tokens.saturating_add(usage.input_tokens);
                    slot.output_tokens = slot.output_tokens.saturating_add(usage.output_tokens);
                    slot.cache_read_tokens = slot
                        .cache_read_tokens
                        .saturating_add(usage.cache_read_tokens);
                }
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
    // (stats-overview-missing-fields) Pick the fun factoid from the final totals.
    data.factoid = pick_factoid(data.total_tokens(), data.longest_session_ms);
    data
}

impl StatsData {
    /// (stats-date-range) A view of this data restricted to `range` ending at
    /// `today`. `All` returns the data unchanged; `Last7`/`Last30` keep only the
    /// dated rows on/after the cutoff and RE-derive every displayed field
    /// (sessions, longest session, daily messages, per-model usage, chart,
    /// factoid) from that window — claude-code `aggregateLingXiStatsForRange`.
    #[must_use]
    pub fn for_range(&self, range: StatsRange, today: chrono::NaiveDate) -> StatsData {
        let Some(days) = range.window_days() else {
            return self.clone();
        };
        // Inclusive window: `today - (days-1) ..= today`.
        let cutoff = today - chrono::Duration::days(days - 1);
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();
        let in_range = |date: &str| *date >= *cutoff_str.as_str();

        let mut out = StatsData::default();
        // Daily messages.
        for (date, &n) in &self.daily_messages {
            if in_range(date) {
                out.daily_messages.insert(date.clone(), n);
                out.total_messages += usize::try_from(n).unwrap_or(0);
                track_date(&mut out, date);
            }
        }
        // Sessions (count + longest).
        for (date, dur) in &self.sessions {
            if in_range(date) {
                out.total_sessions += 1;
                out.longest_session_ms = out.longest_session_ms.max(*dur);
                out.sessions.push((date.clone(), *dur));
            }
        }
        // Per-model usage (In/Out splits) + the per-day chart totals.
        for (date, models) in &self.daily_model_usage {
            if !in_range(date) {
                continue;
            }
            let day_chart = out.daily_model_tokens.entry(date.clone()).or_default();
            for (model, usage) in models {
                out.daily_model_usage
                    .entry(date.clone())
                    .or_default()
                    .insert(model.clone(), usage.clone());
                let agg = out.model_usage.entry(model.clone()).or_default();
                agg.input_tokens = agg.input_tokens.saturating_add(usage.input_tokens);
                agg.output_tokens = agg.output_tokens.saturating_add(usage.output_tokens);
                agg.cache_read_tokens = agg
                    .cache_read_tokens
                    .saturating_add(usage.cache_read_tokens);
                let total = usage.input_tokens.saturating_add(usage.output_tokens);
                if total > 0 {
                    let slot = day_chart.entry(model.clone()).or_default();
                    *slot = slot.saturating_add(total);
                }
            }
        }
        out.factoid = pick_factoid(out.total_tokens(), out.longest_session_ms);
        out
    }
}

/// Milliseconds between two ISO-8601 timestamps (`last - first`), clamped to 0
/// when either fails to parse or the span is negative (claude-code
/// `lastTimestamp.getTime() - firstTimestamp.getTime()`).
#[must_use]
pub fn session_duration_ms(first: &str, last: &str) -> u64 {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    match (parse(first), parse(last)) {
        (Some(a), Some(b)) => {
            let ms = b.signed_duration_since(a).num_milliseconds();
            u64::try_from(ms).unwrap_or(0)
        }
        _ => 0,
    }
}

/// Human-readable duration (claude-code `formatDuration`, no options): `0s`,
/// `{s}s` under a minute, then `{m}m {s}s` / `{h}h {m}m {s}s` / `{d}d {h}h {m}m`
/// with rounding carry-over.
#[must_use]
pub fn format_duration(ms: u64) -> String {
    if ms < 60_000 {
        return format!("{}s", ms / 1000);
    }
    let mut days = ms / 86_400_000;
    let mut hours = (ms % 86_400_000) / 3_600_000;
    let mut minutes = (ms % 3_600_000) / 60_000;
    // `Math.round((ms % 60000) / 1000)` — round to nearest second.
    let mut seconds = ((ms % 60_000) as f64 / 1000.0).round() as u64;
    if seconds == 60 {
        seconds = 0;
        minutes += 1;
    }
    if minutes == 60 {
        minutes = 0;
        hours += 1;
    }
    if hours == 24 {
        hours = 0;
        days += 1;
    }
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// `(book, tokens)` comparisons for the fun factoid (claude-code
/// `BOOK_COMPARISONS`), ascending by token count.
const BOOK_COMPARISONS: &[(&str, u64)] = &[
    ("The Little Prince", 22_000),
    ("The Old Man and the Sea", 35_000),
    ("A Christmas Carol", 37_000),
    ("Animal Farm", 39_000),
    ("Fahrenheit 451", 60_000),
    ("The Great Gatsby", 62_000),
    ("Slaughterhouse-Five", 64_000),
    ("Brave New World", 83_000),
    ("The Catcher in the Rye", 95_000),
    ("Harry Potter and the Philosopher's Stone", 103_000),
    ("The Hobbit", 123_000),
    ("1984", 123_000),
    ("To Kill a Mockingbird", 130_000),
    ("Pride and Prejudice", 156_000),
    ("Dune", 244_000),
    ("Moby-Dick", 268_000),
    ("Crime and Punishment", 274_000),
    ("A Game of Thrones", 381_000),
    ("Anna Karenina", 468_000),
    ("Don Quixote", 520_000),
    ("The Lord of the Rings", 576_000),
    ("The Count of Monte Cristo", 603_000),
    ("Les Misérables", 689_000),
    ("War and Peace", 730_000),
];

/// `(activity, minutes)` comparisons for the fun factoid (claude-code
/// `TIME_COMPARISONS`).
const TIME_COMPARISONS: &[(&str, u64)] = &[
    ("a TED talk", 18),
    ("an episode of The Office", 22),
    ("listening to Abbey Road", 47),
    ("a yoga class", 60),
    ("a World Cup soccer match", 90),
    ("a half marathon (average time)", 120),
    ("the movie Inception", 148),
    ("a transatlantic flight", 420),
    ("a full night of sleep", 480),
];

/// The fun-factoid candidates (claude-code `generateFunFactoid`): token-vs-book
/// + longest-session-vs-activity comparisons. The live caller picks one;
/// claude-code picks at random, this picks deterministically by `total_tokens`
/// (testable; the cosmetic factoid stays stable per stats load either way).
#[must_use]
pub fn generate_factoids(total_tokens: u64, longest_session_ms: u64) -> Vec<String> {
    let mut out = Vec::new();
    if total_tokens > 0 {
        for (name, tokens) in BOOK_COMPARISONS.iter().filter(|(_, t)| total_tokens >= *t) {
            let times = total_tokens / tokens;
            if times >= 2 {
                out.push(format!("You've used ~{times}x more tokens than {name}"));
            } else {
                out.push(format!("You've used the same number of tokens as {name}"));
            }
        }
    }
    if longest_session_ms > 0 {
        let session_minutes = longest_session_ms / 60_000;
        for (name, minutes) in TIME_COMPARISONS.iter() {
            let ratio = session_minutes / minutes;
            if ratio >= 2 {
                out.push(format!(
                    "Your longest session is ~{ratio}x longer than {name}"
                ));
            }
        }
    }
    out
}

/// Deterministic factoid pick (`generate_factoids` indexed by `total_tokens`),
/// or `None` when there are no candidates.
#[must_use]
pub fn pick_factoid(total_tokens: u64, longest_session_ms: u64) -> Option<String> {
    let factoids = generate_factoids(total_tokens, longest_session_ms);
    if factoids.is_empty() {
        None
    } else {
        let idx = usize::try_from(total_tokens).unwrap_or(0) % factoids.len();
        Some(factoids[idx].clone())
    }
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
// `<lingxi_home>/projects/` history, re-computing only when the history has
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
pub const STATS_CACHE_VERSION: u32 = 3;

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

/// Date-range filter for the stats view (claude-code `StatsDateRange`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatsRange {
    /// All time (no filter).
    #[default]
    All,
    /// Last 7 days (inclusive of today).
    Last7,
    /// Last 30 days (inclusive of today).
    Last30,
}

impl StatsRange {
    /// Cycle order (claude-code `DATE_RANGE_ORDER` = all → 7d → 30d → all).
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            StatsRange::All => StatsRange::Last7,
            StatsRange::Last7 => StatsRange::Last30,
            StatsRange::Last30 => StatsRange::All,
        }
    }

    /// Display label (claude-code `DATE_RANGE_LABELS`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StatsRange::All => "All time",
            StatsRange::Last7 => "Last 7 days",
            StatsRange::Last30 => "Last 30 days",
        }
    }

    /// Inclusive day-count of the window, or `None` for `All`.
    #[must_use]
    fn window_days(self) -> Option<i64> {
        match self {
            StatsRange::All => None,
            StatsRange::Last7 => Some(7),
            StatsRange::Last30 => Some(30),
        }
    }
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
    /// (stats-date-range) Active date-range filter, cycled by `r`.
    pub range: StatsRange,
}

impl StatsState {
    /// Build from aggregated data, sizing the embedded [`ScrollState`] to the
    /// Overview tab's body-line count and the fixed [`VIEWPORT`].
    #[must_use]
    pub fn new(data: StatsData) -> Self {
        let tab = StatsTab::Overview;
        // Default range = All → `view()` == data, so size on `data` directly.
        let len = body_lines(&data, tab).len();
        Self {
            data,
            tab,
            scroll: ScrollState::new(len, VIEWPORT),
            loading: false,
            range: StatsRange::All,
        }
    }

    /// (stats-date-range) The data restricted to the active range. `All` (the
    /// default) returns it unchanged; `Last7`/`Last30` filter against the local
    /// `today`. The cutoff uses wall-clock now (like the streak computation).
    #[must_use]
    pub fn view(&self) -> StatsData {
        if self.range == StatsRange::All {
            self.data.clone()
        } else {
            self.data
                .for_range(self.range, chrono::Local::now().date_naive())
        }
    }

    /// (stats-date-range) Cycle to the next range (`r`), re-anchoring the scroll
    /// to the (possibly different) filtered body length.
    fn cycle_range(&mut self) {
        self.range = self.range.next();
        let len = body_lines(&self.view(), self.tab).len();
        self.scroll = ScrollState::new(len, VIEWPORT);
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
        self.data = data;
        // Size on the active range's view (range is `All` by default → == data).
        let len = body_lines(&self.view(), self.tab).len();
        self.scroll = ScrollState::new(len, VIEWPORT);
        self.loading = false;
    }

    /// Switch to `tab`, re-sizing the scroll window to that tab's body and
    /// re-anchoring at the top (claude-code resets the Models scroll on tab
    /// switch).
    fn set_tab(&mut self, tab: StatsTab) {
        self.tab = tab;
        let len = body_lines(&self.view(), tab).len();
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
        // (stats-date-range) `r` cycles the date range (All → 7d → 30d → All).
        KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
            state.cycle_range();
            StatsOutcome::Stay
        }
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

// ---------------------------------------------------------------------------
// (stats-chart-sparkline-vs-asciichart) Multi-series tokens-per-day chart, a
// port of claude-code `generateTokenChart` (`Stats.tsx`). claude-code replaced
// the single-line sparkline with an 8-row `asciichart` plot of the top-3
// models, a padStart(6) k/M y-axis, an x-axis date line, and a `●`-bulleted
// legend. The asciichart glyphs + layout are byte-faithful to the npm
// `asciichart` `plot()` algorithm (kroitor); the only forced divergence is
// COLOR: claude-code colors each series + bullet via chalk/ANSI, but the pure
// string oracle has no color seam, so the chart + legend render monochrome
// (the per-series ANSI escapes are simply omitted, the box-drawing glyphs are
// identical).
// ---------------------------------------------------------------------------

/// The chart-window width budget (claude-code `generateTokenChart`): a fixed
/// `terminalWidth` of 80 (the screenshot path's constant) minus the 7-char
/// y-axis gutter, clamped to `20..=52` (`Math.min(52, Math.max(20, …))`). The
/// 52 cap aligns the chart with the heatmap's one-year width.
const CHART_TERMINAL_WIDTH: usize = 80;
/// Y-axis gutter width (claude-code `yAxisWidth = 7`): the 6-char label
/// (`format(...).padStart(6)`) plus the 1-char axis rule.
const CHART_Y_AXIS_WIDTH: usize = 7;
/// Plot height passed to asciichart (`height: 8` in `generateTokenChart`).
const CHART_HEIGHT: i64 = 8;

/// One model's prepared chart input: its renderable display name (for the
/// legend) and its per-day token series (one value per window column).
struct ChartSeries {
    /// `renderModelName(model)` — the legend label.
    display_name: String,
    /// Per-day total tokens across the resampled window.
    values: Vec<u64>,
}

/// The result of [`generate_token_chart`] (claude-code `ChartOutput`): the
/// rendered `asciichart` body rows, the x-axis date-label line, and the legend
/// line. `None` when there is nothing to chart (claude-code returns `null`).
struct ChartOutput {
    /// The 8-row (height+1) asciichart body, one `String` per row.
    chart_rows: Vec<String>,
    /// The x-axis date labels (already y-axis-indented).
    x_axis_labels: String,
    /// The `● {name} · ● {name}` legend line.
    legend: String,
}

/// The y-axis label formatter (claude-code `generateTokenChart`'s `format`):
/// `>=1M` → `{x/1M:.1}M`, `>=1k` → `{x/1k:.0}k`, else the integer — then
/// `padStart(6)` (always exactly 6 chars, space-padded on the left).
#[must_use]
fn format_y_axis_label(value: f64) -> String {
    let label = if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        // (review) JS `.toFixed(0)` rounds half AWAY from zero; Rust's `{:.0}`
        // rounds half to even. They diverge at `.5k` (e.g. 2500 → JS "3k" vs
        // Rust "2k"). Match JS for these positive values via `(x + 0.5).floor()`.
        let k = (value / 1_000.0 + 0.5).floor() as i64;
        format!("{k}k")
    } else {
        format!("{value:.0}")
    };
    format!("{label:>6}")
}

/// Resample `dailies` (chronological `(date, model→tokens)` rows) to exactly
/// `chart_width` columns, mirroring claude-code's window logic: when there is
/// more data than space, keep the most recent `chart_width` days
/// (`slice(-chartWidth)`); when there is less, repeat each day
/// `floor(chartWidth / len)` times (so the expanded length can be `<
/// chart_width`, exactly as the TS `repeatCount` loop produces).
fn resample_window<'a>(
    dailies: &[(&'a str, &'a BTreeMap<String, u64>)],
    chart_width: usize,
) -> Vec<(&'a str, &'a BTreeMap<String, u64>)> {
    if dailies.len() >= chart_width {
        dailies[dailies.len() - chart_width..].to_vec()
    } else {
        let repeat = chart_width / dailies.len().max(1);
        let mut out = Vec::with_capacity(repeat * dailies.len());
        for day in dailies {
            for _ in 0..repeat {
                out.push(*day);
            }
        }
        out
    }
}

/// Build the multi-series chart input (claude-code `generateTokenChart` up to
/// the `asciichart(...)` call). `daily_model_tokens` is the per-date model
/// token map; `models` is the token-ranked model id list (top-3 are charted).
/// Returns `None` when there are `< 2` days, no models, or no series has any
/// positive value (claude-code's three `null` returns).
fn build_chart_series(
    daily_model_tokens: &BTreeMap<String, BTreeMap<String, u64>>,
    models: &[&str],
) -> Option<(Vec<ChartSeries>, Vec<String>)> {
    if daily_model_tokens.len() < 2 || models.is_empty() {
        return None;
    }
    let chart_width = (CHART_TERMINAL_WIDTH - CHART_Y_AXIS_WIDTH).clamp(20, 52);
    // BTreeMap iterates dates ascending → chronological, matching the TS
    // `dailyModelTokens` array order.
    let dailies: Vec<(&str, &BTreeMap<String, u64>)> = daily_model_tokens
        .iter()
        .map(|(d, m)| (d.as_str(), m))
        .collect();
    let window = resample_window(&dailies, chart_width);

    let mut series = Vec::new();
    for &model in models.iter().take(3) {
        let values: Vec<u64> = window
            .iter()
            .map(|(_, day)| day.get(model).copied().unwrap_or(0))
            .collect();
        // Only include a series that has actual data (`data.some(v => v > 0)`).
        if values.iter().any(|&v| v > 0) {
            series.push(ChartSeries {
                display_name: crate::render::model_name::render_model_name(model),
                values,
            });
        }
    }
    if series.is_empty() {
        return None;
    }
    let dates: Vec<String> = window.iter().map(|(d, _)| (*d).to_string()).collect();
    Some((series, dates))
}

/// The full tokens-per-day chart (claude-code `generateTokenChart`): the
/// 8-row asciichart body, the x-axis date line, and the legend. `None` when
/// there is nothing to chart.
#[must_use]
fn generate_token_chart(
    daily_model_tokens: &BTreeMap<String, BTreeMap<String, u64>>,
    models: &[&str],
) -> Option<ChartOutput> {
    let (series, dates) = build_chart_series(daily_model_tokens, models)?;
    let value_series: Vec<&[u64]> = series.iter().map(|s| s.values.as_slice()).collect();
    let chart_rows = asciichart_plot(&value_series);
    let x_axis_labels = generate_x_axis_labels(&dates, CHART_Y_AXIS_WIDTH);
    let legend = series
        .iter()
        .map(|s| format!("\u{25CF} {}", s.display_name))
        .collect::<Vec<_>>()
        .join(" \u{00B7} ");
    Some(ChartOutput {
        chart_rows,
        x_axis_labels,
        legend,
    })
}

/// asciichart's default y-axis cell offset (`cfg.offset ?? 3`). claude-code's
/// `generateTokenChart` passes only `{height, colors, format}` — NO `offset` —
/// so the library default `3` applies: a 6-char `format` label is written as a
/// single string-cell at index `max(offset - 6, 0) = 0`, the axis rule sits at
/// index `offset - 1 = 2`, and the series plot starts at index `offset = 3`.
const CHART_OFFSET: usize = 3;

/// A monochrome port of the npm `asciichart` `plot(series, {height: 8})`
/// algorithm (kroitor). Returns one `String` per chart row (top to bottom),
/// each beginning with the 6-char y-axis label + axis rule, then the
/// box-drawing series glyphs.
///
/// Faithful to asciichart's *string-cell* grid: each grid cell holds a string
/// (a single glyph, except the y-axis cell which holds the whole padded
/// label), and a row is the `''`-join of its cells — so a 6-char label at cell
/// 0 + the axis at cell 2 render as `"   588 ┤…"` exactly like the library.
/// The only divergence is COLOR (the per-series ANSI escapes are dropped); the
/// glyphs + layout are identical.
///
/// Structural (NOT byte-locked) invariants the tests assert: exactly
/// `height + 1` rows; each row's leading 6 chars are the y-axis label; the
/// plotted glyphs are drawn from asciichart's box-drawing set.
#[must_use]
fn asciichart_plot(series: &[&[u64]]) -> Vec<String> {
    // asciichart symbols: ┼ ┤ ╶ ╴ ─ ╰ ╮ ╭ ╯ │
    const SYM: [&str; 10] = [
        "\u{253C}", "\u{2524}", "\u{2576}", "\u{2574}", "\u{2500}", "\u{2570}", "\u{256E}",
        "\u{256D}", "\u{256F}", "\u{2502}",
    ];
    if series.is_empty() || series.iter().all(|s| s.is_empty()) {
        return Vec::new();
    }
    // min/max across all series.
    let mut min = u64::MAX;
    let mut max = 0u64;
    for s in series {
        for &v in *s {
            min = min.min(v);
            max = max.max(v);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let (minf, maxf) = (min as f64, max as f64);
    let range = (maxf - minf).abs();
    let offset = CHART_OFFSET;
    #[allow(clippy::cast_precision_loss)]
    let height = CHART_HEIGHT as f64;
    let ratio = if range != 0.0 { height / range } else { 1.0 };
    let min2 = (minf * ratio).round() as i64;
    let max2 = (maxf * ratio).round() as i64;
    let rows = (max2 - min2).unsigned_abs() as usize;

    let series_width = series.iter().map(|s| s.len()).max().unwrap_or(0);
    let width = series_width + offset;

    // String-cell grid of (rows+1) × width, single-space cells by default.
    let mut grid: Vec<Vec<String>> = vec![vec![" ".to_string(); width]; rows + 1];
    let set = |grid: &mut Vec<Vec<String>>, r: usize, c: usize, v: &str| {
        if r < grid.len() && c < grid[r].len() {
            grid[r][c] = v.to_string();
        }
    };

    // Y-axis labels + axis rule (`result[row][max(offset-len,0)] = label`,
    // `result[row][offset-1] = (y==0)? ┼ : ┤`).
    #[allow(clippy::cast_precision_loss)]
    let rows_f = rows.max(1) as f64;
    for y in min2..=max2 {
        let row = (y - min2) as usize;
        let value = maxf - ((y - min2) as f64) * range / rows_f;
        let label = format_y_axis_label(value);
        let label_len = label.chars().count();
        let label_col = offset.saturating_sub(label_len);
        set(&mut grid, row, label_col, &label);
        set(
            &mut grid,
            row,
            offset - 1,
            if y == 0 { SYM[0] } else { SYM[1] },
        );
    }

    // Plot each series.
    #[allow(clippy::cast_precision_loss)]
    let scaled = |v: u64| ((v as f64 * ratio).round() as i64) - min2;
    let row_of = |y: i64| (rows as i64 - y).clamp(0, rows as i64) as usize;
    for s in series {
        if s.is_empty() {
            continue;
        }
        // First point marker (`result[rows - y0][offset-1] = ┼`).
        let r0 = row_of(scaled(s[0]));
        set(&mut grid, r0, offset - 1, SYM[0]);
        for i in 0..s.len().saturating_sub(1) {
            let ya = scaled(s[i]);
            let yb = scaled(s[i + 1]);
            let col = i + offset;
            if col >= width {
                continue;
            }
            if ya == yb {
                set(&mut grid, row_of(ya), col, SYM[4]);
            } else {
                set(
                    &mut grid,
                    row_of(yb),
                    col,
                    if ya > yb { SYM[5] } else { SYM[6] },
                );
                set(
                    &mut grid,
                    row_of(ya),
                    col,
                    if ya > yb { SYM[7] } else { SYM[8] },
                );
                let (from, to) = (ya.min(yb), ya.max(yb));
                for y in (from + 1)..to {
                    set(&mut grid, row_of(y), col, SYM[9]);
                }
            }
        }
    }

    grid.into_iter()
        .map(|row| row.concat().trim_end().to_string())
        .collect()
}

/// The x-axis date-label line (claude-code `generateXAxisLabels`): 2-4 `Mon D`
/// date labels evenly spaced across the window, prefixed by a `y_axis_offset`
/// space gutter. `dates` is the resampled window's `YYYY-MM-DD` keys.
#[must_use]
fn generate_x_axis_labels(dates: &[String], y_axis_offset: usize) -> String {
    if dates.is_empty() {
        return String::new();
    }
    // numLabels = min(4, max(2, floor(len / 8))).
    let num_labels = (dates.len() / 8).clamp(2, 4);
    // usableLength = len - 6 (reserve ~6 chars for the last label).
    let usable = dates.len() as i64 - 6;
    let step = (usable / (num_labels as i64 - 1).max(1)).max(1);
    let mut result = " ".repeat(y_axis_offset);
    let mut current_pos: i64 = 0;
    for i in 0..num_labels {
        let idx = ((i as i64) * step).min(dates.len() as i64 - 1) as usize;
        let label = format_peak_day(&dates[idx]);
        let pos = idx as i64;
        let spaces = (pos - current_pos).max(1);
        for _ in 0..spaces {
            result.push(' ');
        }
        result.push_str(&label);
        current_pos = pos + label.chars().count() as i64;
    }
    result
}

/// (stats-heatmap-grid) `today`-defaulting wrapper around
/// [`heatmap_with_today`] — the live render path's entry point (`today` is
/// `Local::now()`'s date; the parameterized form exists purely for
/// deterministic tests).
///
/// Returns `[]` when there is no activity (caller omits the section).
#[must_use]
pub fn heatmap(daily: &BTreeMap<String, u64>) -> Vec<String> {
    heatmap_with_today(daily, chrono::Local::now().date_naive())
}

/// GitHub-style 7-row × N-week activity grid — a faithful port of claude-code
/// `generateHeatmap` (`utils/heatmap.ts`). The grid ends at `today`'s week
/// (anchored on that week's Sunday) and walks back `width-1` weeks; future days
/// are blank, past days carry their intensity glyph (`·` for no activity).
/// Output rows: a month-label line, the 7 weekday rows (`Mon`/`Wed`/`Fri`
/// labels on rows 1/3/5), a blank line, and the `Less … More` legend.
/// `today` is injected so the layout is deterministic in tests.
#[must_use]
pub fn heatmap_with_today(daily: &BTreeMap<String, u64>, today: NaiveDate) -> Vec<String> {
    if daily.is_empty() {
        return Vec::new();
    }
    const TERMINAL_WIDTH: i64 = 80;
    const DAY_LABEL_WIDTH: i64 = 4;
    // width = min(52, max(10, terminalWidth - dayLabelWidth)).
    let width = (TERMINAL_WIDTH - DAY_LABEL_WIDTH).clamp(10, 52) as usize;

    let counts: Vec<u64> = daily.values().copied().filter(|&c| c > 0).collect();
    let pct = percentiles(&counts);

    // Sunday of the current week, then back (width-1) weeks.
    let dow = i64::from(today.weekday().num_days_from_sunday());
    let current_week_start = today - ChronoDuration::days(dow);
    let start_date = current_week_start - ChronoDuration::days((width as i64 - 1) * 7);

    let mut grid = vec![vec![' '; width]; 7];
    let mut month_order: Vec<u32> = Vec::new();
    let mut last_month: i32 = -1;
    let mut current = start_date;
    for week in 0..width {
        for day in 0..7usize {
            if current > today {
                grid[day][week] = ' ';
            } else {
                let date_str = current.format("%Y-%m-%d").to_string();
                let count = daily.get(&date_str).copied().unwrap_or(0);
                if day == 0 {
                    let month = current.month0();
                    if month as i32 != last_month {
                        month_order.push(month);
                        last_month = month as i32;
                    }
                }
                grid[day][week] = heatmap_char(intensity(count, pct));
            }
            current += ChronoDuration::days(1);
        }
    }

    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let mut lines: Vec<String> = Vec::new();

    // Month labels: each unique month, padEnd(floor(width / max(months,1))).
    let label_w = width / month_order.len().max(1);
    let labels: String = month_order
        .iter()
        .map(|&m| format!("{:<w$}", MONTHS[m as usize], w = label_w))
        .collect();
    lines.push(format!("    {labels}"));

    // 7 weekday rows; labels only on Mon(1)/Wed(3)/Fri(5).
    for day in 0..7usize {
        let label = if day == 1 || day == 3 || day == 5 {
            format!("{:<3}", DAYS[day])
        } else {
            "   ".to_string()
        };
        let row: String = grid[day].iter().collect();
        lines.push(format!("{label} {row}"));
    }

    // Legend (blank line + 4-space indent).
    lines.push(String::new());
    lines.push(format!(
        "    Less {} {} {} {} More",
        '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'
    ));
    lines
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
/// Consecutive-active-day streaks (claude-code `calculateStreaks`): `(longest,
/// current)`. `longest` = the longest run of consecutive calendar days that are
/// all active; `current` = the run of consecutive active days ending at `today`
/// (0 when `today` itself is inactive). `today` is injected for tests.
#[must_use]
pub fn streaks(daily: &BTreeMap<String, u64>, today: NaiveDate) -> (u64, u64) {
    use std::collections::BTreeSet;
    let active: BTreeSet<NaiveDate> = daily
        .keys()
        .filter_map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .collect();
    if active.is_empty() {
        return (0, 0);
    }
    // Current streak: walk back from today while each day is active.
    let mut current = 0u64;
    let mut check = today;
    while active.contains(&check) {
        current += 1;
        match check.pred_opt() {
            Some(p) => check = p,
            None => break,
        }
    }
    // Longest streak: longest run of consecutive days in the sorted active set.
    let sorted: Vec<NaiveDate> = active.into_iter().collect();
    let (mut longest, mut temp) = (1u64, 1u64);
    for w in sorted.windows(2) {
        if (w[1] - w[0]).num_days() == 1 {
            temp += 1;
            longest = longest.max(temp);
        } else {
            temp = 1;
        }
    }
    (longest, current)
}

fn overview_lines(data: &StatsData) -> Vec<String> {
    let mut out = Vec::new();
    for row in heatmap(&data.daily_messages) {
        out.push(row);
    }
    if let Some(fav) = data.favorite_model() {
        // (stats-model-name-raw) friendly display name (renderModelName).
        out.push(format!(
            "Favorite model: {}",
            crate::render::model_name::render_model_name(fav)
        ));
    }
    out.push(format!(
        "Total tokens: {}",
        format_number(data.total_tokens())
    ));
    out.push(format!(
        "Sessions: {}",
        format_number(data.total_sessions as u64)
    ));
    // (stats-overview-missing-fields) Longest session duration, `N/A` when no
    // session had a measurable span (claude-code `Longest session`).
    let longest = if data.longest_session_ms > 0 {
        format_duration(data.longest_session_ms)
    } else {
        "N/A".to_string()
    };
    out.push(format!("Longest session: {longest}"));
    // (stats-overview-missing-fields) Active days `/rangeDays` + streaks.
    out.push(format!(
        "Active days: {}/{}",
        data.active_days(),
        data.range_days()
    ));
    let (longest, current) = streaks(&data.daily_messages, chrono::Local::now().date_naive());
    let plural = |n: u64| if n == 1 { "day" } else { "days" };
    out.push(format!("Longest streak: {longest} {}", plural(longest)));
    out.push(format!("Current streak: {current} {}", plural(current)));
    if let Some(day) = data.peak_activity_day() {
        out.push(format!("Most active day: {}", format_peak_day(day)));
    }
    // (stats-overview-missing-fields) The fun factoid (claude-code shows it in
    // the suggestion accent below the overview).
    if let Some(factoid) = &data.factoid {
        out.push(factoid.clone());
    }
    out
}

/// The Models tab's body lines: the `Tokens per Day` multi-series asciichart
/// (when there are ≥2 days of token data) — the 8-row chart, x-axis date line,
/// and `●`-bulleted top-3 legend — then one two-line block per model
/// (`{model} ({pct}%)` + `  In: {n} · Out: {n}`), claude-code `ModelsTab` +
/// `ModelEntry`. `No model usage data available` when there is no model data.
fn models_lines(data: &StatsData) -> Vec<String> {
    let entries = data.models_by_tokens();
    if entries.is_empty() {
        return vec![MODELS_EMPTY_LINE.to_string()];
    }
    let mut out = Vec::new();

    // (stats-chart-sparkline-vs-asciichart) Tokens-per-day multi-series
    // asciichart for the top-3 models (claude-code `generateTokenChart`):
    // the 8-row chart, an x-axis date line, and a `●`-bulleted legend, all
    // under the `Tokens per Day` heading. Rendered only when there are ≥2 days
    // of data with at least one non-empty top-3 series (else the section is
    // omitted, mirroring the TS `null` return).
    let model_ids: Vec<&str> = entries.iter().map(|(m, _)| *m).collect();
    if let Some(chart) = generate_token_chart(&data.daily_model_tokens, &model_ids) {
        out.push(TOKENS_PER_DAY.to_string());
        for row in chart.chart_rows {
            out.push(row);
        }
        out.push(chart.x_axis_labels);
        out.push(chart.legend);
    }

    let total = data.total_tokens();
    for (model, usage) in entries {
        // (stats-models-row-bullet) figures.bullet (●) prefix; bold name + dim
        // (pct%) await a structured render. (stats-model-name-raw) friendly
        // display name via renderModelName.
        out.push(format!(
            "\u{25CF} {} ({}%)",
            crate::render::model_name::render_model_name(model),
            format_pct(usage.total(), total)
        ));
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

/// (stats-date-range-selector) All three range options joined by ` · `, the
/// active one bracketed (claude-code `DateRangeSelector`: active bold+claude,
/// others dim — color/bold are invisible in the string oracle, so the
/// active one is `[…]`-bracketed, matching [`tab_header`]).
fn range_selector_line(active: StatsRange) -> String {
    let mark = |r: StatsRange| -> String {
        if r == active {
            format!("[{}]", r.label())
        } else {
            r.label().to_string()
        }
    };
    format!(
        "{} \u{00B7} {} \u{00B7} {}",
        mark(StatsRange::All),
        mark(StatsRange::Last7),
        mark(StatsRange::Last30),
    )
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
    // (stats-date-range-selector) claude-code's DateRangeSelector renders ALL
    // three range options joined by ` · `, the active one bold+claude (the
    // string oracle marks it with `[…]` brackets, like tab_header — color is
    // invisible here). The `r`-cycle hint moves to the footer.
    out.push_str(&range_selector_line(state.range));
    out.push('\n');

    let view = state.view();
    let lines = body_lines(&view, state.tab);
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
            "No stats available yet. Start using LingXi!\nEsc to cancel \u{00B7} r to cycle dates"
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
        assert!(out.contains("Active days: 1/1"), "got: {out}");
        assert!(out.ends_with(FOOTER), "got: {out}");
        // The taller overview (heatmap grid + streaks) pushes the lower fields
        // below the fold; assert them against the full body.
        let body = overview_lines(&st.data).join("\n");
        assert!(body.contains("Most active day: May 1"), "body: {body}");
        assert!(body.contains("Longest streak:"), "body: {body}");
        assert!(body.contains("Current streak:"), "body: {body}");
    }

    #[test]
    fn streaks_longest_and_current() {
        let mut daily = BTreeMap::new();
        // A 3-day run, a gap, then a 2-day run ending on the 10th.
        for d in [
            "2026-06-01",
            "2026-06-02",
            "2026-06-03",
            "2026-06-09",
            "2026-06-10",
        ] {
            daily.insert(d.to_string(), 1u64);
        }
        // today = 2026-06-10 → current streak = 2 (09, 10); longest = 3.
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        assert_eq!(streaks(&daily, today), (3, 2));
        // today = 2026-06-12 (inactive) → current streak = 0.
        let today2 = NaiveDate::from_ymd_opt(2026, 6, 12).unwrap();
        assert_eq!(streaks(&daily, today2), (3, 0));
        // Empty → (0, 0).
        assert_eq!(streaks(&BTreeMap::new(), today), (0, 0));
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

    // ---- (stats-chart-sparkline-vs-asciichart) multi-series chart ----

    /// The asciichart box-drawing glyph set + the y-axis chars, for validation.
    fn chart_glyphs() -> Vec<char> {
        vec![
            ' ', '\u{253C}', '\u{2524}', '\u{2576}', '\u{2574}', '\u{2500}', '\u{2570}',
            '\u{256E}', '\u{256D}', '\u{256F}', '\u{2502}',
            // y-axis label chars (digits, '.', 'k', 'M', minus sign).
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', 'k', 'M', '-',
        ]
    }

    #[test]
    fn format_y_axis_label_pads_to_six_and_uses_k_m() {
        // <1k → integer, padStart(6).
        assert_eq!(format_y_axis_label(0.0), "     0");
        assert_eq!(format_y_axis_label(42.0), "    42");
        // 1k..1M → {x/1k:.0}k (JS toFixed(0) = round half AWAY from zero).
        assert_eq!(format_y_axis_label(1_500.0), "    2k"); // 1.5 → 2
        assert_eq!(format_y_axis_label(2_500.0), "    3k"); // (review) 2.5 → 3 (away), not 2 (banker's)
        assert_eq!(format_y_axis_label(12_000.0), "   12k");
        // >=1M → {x/1M:.1}M.
        assert_eq!(format_y_axis_label(2_000_000.0), "  2.0M");
        assert_eq!(format_y_axis_label(2_500_000.0), "  2.5M");
        // Always exactly 6 chars.
        for v in [0.0, 999.0, 1_000.0, 999_999.0, 5_000_000.0] {
            assert_eq!(format_y_axis_label(v).chars().count(), 6, "v={v}");
        }
    }

    #[test]
    fn asciichart_plot_has_height_plus_one_rows_and_valid_glyphs() {
        // Two series of 10 columns: one climbing, one flat.
        let climbing: Vec<u64> = (0..10).map(|i| i * 100).collect();
        let flat: Vec<u64> = vec![500; 10];
        let rows = asciichart_plot(&[climbing.as_slice(), flat.as_slice()]);
        // asciichart height=8 → 9 rows.
        assert_eq!(rows.len(), 9, "rows: {rows:#?}");
        let valid = chart_glyphs();
        for (r, row) in rows.iter().enumerate() {
            assert!(
                row.chars().all(|c| valid.contains(&c)),
                "row {r} has invalid glyph: {row:?}"
            );
            // Each row's leading 6 chars are the y-axis label (space-padded).
            let label: String = row.chars().take(6).collect();
            assert_eq!(label.chars().count(), 6, "row {r} label width: {row:?}");
        }
        // Empty input → no rows.
        assert!(asciichart_plot(&[]).is_empty());
    }

    #[test]
    fn generate_x_axis_labels_evenly_spaced_dates() {
        let dates: Vec<String> = (1..=30).map(|d| format!("2026-06-{d:02}")).collect();
        let line = generate_x_axis_labels(&dates, CHART_Y_AXIS_WIDTH);
        // Leading 7-space y-axis gutter.
        assert!(line.starts_with("       "), "gutter: {line:?}");
        // 30 days / 8 = 3 labels; first is the earliest date 'Jun 1'.
        assert!(line.contains("Jun 1"), "got: {line}");
        // 2..=4 labels → count the 'Jun ' occurrences.
        let n = line.matches("Jun ").count();
        assert!((2..=4).contains(&n), "label count {n}: {line}");
        // Empty → empty.
        assert!(generate_x_axis_labels(&[], 7).is_empty());
    }

    #[test]
    fn generate_token_chart_none_cases() {
        // <2 days → None.
        let mut one_day = BTreeMap::new();
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), 100u64);
        one_day.insert("2026-06-01".to_string(), m);
        assert!(generate_token_chart(&one_day, &["a"]).is_none());
        // No models → None.
        let mut two_day = BTreeMap::new();
        two_day.insert("2026-06-01".to_string(), BTreeMap::new());
        two_day.insert("2026-06-02".to_string(), BTreeMap::new());
        assert!(generate_token_chart(&two_day, &[]).is_none());
        // 2 days but all-zero series → None.
        let mut z = BTreeMap::new();
        let mut z1 = BTreeMap::new();
        z1.insert("a".to_string(), 0u64);
        z.insert("2026-06-01".to_string(), z1.clone());
        z.insert("2026-06-02".to_string(), z1);
        assert!(generate_token_chart(&z, &["a"]).is_none());
    }

    #[test]
    fn generate_token_chart_top3_legend_and_rows() {
        // 5 models over 4 days; only top-3 charted.
        let mut daily = BTreeMap::new();
        for (di, date) in ["2026-06-01", "2026-06-02", "2026-06-03", "2026-06-04"]
            .iter()
            .enumerate()
        {
            let mut day = BTreeMap::new();
            day.insert("model-a".to_string(), 1000 + di as u64 * 100);
            day.insert("model-b".to_string(), 500);
            day.insert("model-c".to_string(), 200);
            day.insert("model-d".to_string(), 50);
            day.insert("model-e".to_string(), 10);
            daily.insert((*date).to_string(), day);
        }
        let models = ["model-a", "model-b", "model-c", "model-d", "model-e"];
        let chart = generate_token_chart(&daily, &models).expect("chart");
        // 8-row asciichart → 9 rows.
        assert_eq!(chart.chart_rows.len(), 9);
        // Legend: top-3 bullets joined by ' · '.
        assert_eq!(
            chart.legend.matches('\u{25CF}').count(),
            3,
            "legend: {}",
            chart.legend
        );
        assert!(chart.legend.contains("model-a"), "legend: {}", chart.legend);
        assert!(chart.legend.contains("model-c"), "legend: {}", chart.legend);
        // model-d/e are below top-3 → not in legend.
        assert!(
            !chart.legend.contains("model-d"),
            "legend: {}",
            chart.legend
        );
        // x-axis line carries a 'Mon D' date.
        assert!(
            chart.x_axis_labels.contains("Jun "),
            "xaxis: {}",
            chart.x_axis_labels
        );
    }

    #[test]
    fn asciichart_label_then_axis_layout_is_byte_faithful() {
        // asciichart offset=3 string-cell model: a 6-char padStart label at
        // cell 0, a space, then the axis rule — so each row begins with the
        // 6-char label, char[6] is a space, char[7] is the ┤/┼ axis glyph.
        let s1: Vec<u64> = vec![200, 800, 400, 1500, 600, 2200, 900, 3000, 1200, 4000];
        let rows = asciichart_plot(&[s1.as_slice()]);
        assert_eq!(rows.len(), 9, "rows: {rows:#?}");
        for row in &rows {
            let chars: Vec<char> = row.chars().collect();
            // Leading 6-char label field.
            assert!(chars.len() >= 8, "row too short: {row:?}");
            assert_eq!(chars[6], ' ', "char 6 should be the gap space: {row:?}");
            assert!(
                chars[7] == '\u{2524}' || chars[7] == '\u{253C}',
                "char 7 should be the axis rule: {row:?}"
            );
        }
        // Top label is the max value (4000 → "4k"), bottom is the min (200).
        assert!(rows[0].starts_with("    4k"), "top: {:?}", rows[0]);
        assert!(
            rows[rows.len() - 1].starts_with("   200"),
            "bottom: {:?}",
            rows[rows.len() - 1]
        );
    }

    #[test]
    fn models_tab_renders_asciichart_above_rows() {
        // 10 days of 3 models, descending totals → chart + legend + model rows.
        let mut contribs = Vec::new();
        for d in 1..=10u32 {
            let date = format!("2026-06-{d:02}");
            contribs.push(parse_session(
                &format!(
                    "{}\n{}\n{}\n",
                    assistant_line(&date, "claude-opus", 1000, 200),
                    assistant_line(&date, "claude-sonnet", 500, 100),
                    assistant_line(&date, "claude-haiku", 200, 50),
                ),
                false,
            ));
        }
        let data = aggregate(&contribs);
        let body = models_lines(&data).join("\n");
        // Heading + chart + legend present.
        assert!(body.contains(TOKENS_PER_DAY), "body: {body}");
        // Legend with 3 bullets.
        let legend_line = body
            .lines()
            .find(|l| l.matches('\u{25CF}').count() == 3 && l.contains('\u{00B7}'))
            .expect("legend line");
        assert!(legend_line.contains("claude-opus"), "legend: {legend_line}");
        // Model rows still follow.
        assert!(
            body.contains("claude-opus (") || body.contains("Opus"),
            "body: {body}"
        );
        // The old single-line sparkline (8 contiguous block bars, no y-axis
        // label) is gone: no line is composed purely of sparkline bars.
        let spark_bars: &[char] = &[
            '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
            '\u{2588}',
        ];
        assert!(
            !body
                .lines()
                .any(|l| !l.is_empty() && l.chars().all(|c| spark_bars.contains(&c))),
            "old sparkline line still present: {body}"
        );
    }

    #[test]
    fn heatmap_renders_7row_grid_with_labels_and_legend() {
        let mut daily = BTreeMap::new();
        daily.insert("2026-05-01".to_string(), 1u64);
        daily.insert("2026-05-02".to_string(), 5u64);
        daily.insert("2026-06-03".to_string(), 9u64);
        let today = NaiveDate::from_ymd_opt(2026, 6, 24).unwrap();
        let rows = heatmap_with_today(&daily, today);
        // month-label line + 7 weekday rows + blank + legend = 10 rows.
        assert_eq!(rows.len(), 10, "got: {rows:#?}");
        // Output rows: 0=months, 1=Sun, 2=Mon, 3=Tue, 4=Wed, 5=Thu, 6=Fri, 7=Sat.
        assert!(rows[2].starts_with("Mon"), "Mon label: {}", rows[2]);
        assert!(rows[4].starts_with("Wed"), "Wed label: {}", rows[4]);
        assert!(rows[6].starts_with("Fri"), "Fri label: {}", rows[6]);
        assert!(
            rows[1].starts_with("   "),
            "Sun has blank label: {}",
            rows[1]
        );
        // Each weekday row's grid is `width` glyphs (52 weeks at terminalWidth=80).
        let valid: &[char] = &[
            ' ', '\u{00B7}', '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}',
        ];
        let grid: String = rows[2].chars().skip(4).collect();
        assert_eq!(grid.chars().count(), 52, "Mon row grid width");
        assert!(grid.chars().all(|c| valid.contains(&c)), "glyphs: {grid}");
        // Legend.
        assert!(rows[8].is_empty(), "blank line before legend");
        assert!(rows[9].starts_with("    Less "));
        assert!(rows[9].ends_with(" More"));
        // Empty -> no rows.
        assert!(heatmap_with_today(&BTreeMap::new(), today).is_empty());
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

    #[test]
    fn format_duration_matches_claude_code() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30_000), "30s");
        assert_eq!(format_duration(59_000), "59s");
        assert_eq!(format_duration(90_000), "1m 30s");
        assert_eq!(format_duration(3_661_000), "1h 1m 1s");
        assert_eq!(format_duration(90_061_000), "1d 1h 1m");
        // Rounding carry: 59.5s under a minute → still <60000 → floor "59s".
        assert_eq!(format_duration(59_500), "59s");
    }

    #[test]
    fn session_duration_from_iso_timestamps() {
        let d = session_duration_ms("2026-05-25T14:00:00.000Z", "2026-05-25T15:30:00.000Z");
        assert_eq!(d, 90 * 60 * 1000); // 1h30m
                                       // Negative / unparseable → 0.
        assert_eq!(
            session_duration_ms("2026-05-25T15:00:00Z", "2026-05-25T14:00:00Z"),
            0
        );
        assert_eq!(session_duration_ms("bad", "also-bad"), 0);
    }

    #[test]
    fn factoid_compares_tokens_to_books_and_session_to_activities() {
        // 250k tokens ≥ several books; ~11x The Little Prince (22k).
        let f = generate_factoids(250_000, 0);
        assert!(
            f.iter()
                .any(|s| s == "You've used ~11x more tokens than The Little Prince"),
            "got: {f:?}"
        );
        // A book just under 2x → "same number of tokens as".
        let f2 = generate_factoids(40_000, 0);
        assert!(
            f2.iter()
                .any(|s| s == "You've used the same number of tokens as Animal Farm"),
            "got: {f2:?}"
        );
        // 60-minute session is ~3x a TED talk (18m), ~2x an Office episode (22m).
        let f3 = generate_factoids(0, 60 * 60 * 1000);
        assert!(
            f3.iter()
                .any(|s| s == "Your longest session is ~3x longer than a TED talk"),
            "got: {f3:?}"
        );
        // No tokens, no session → empty.
        assert!(generate_factoids(0, 0).is_empty());
        // pick_factoid is deterministic + within bounds.
        assert!(pick_factoid(250_000, 0).is_some());
        assert!(pick_factoid(0, 0).is_none());
    }

    #[test]
    fn for_range_filters_sessions_and_tokens_by_window() {
        use chrono::NaiveDate;
        let today = NaiveDate::from_ymd_opt(2026, 6, 24).unwrap();
        let recent = "2026-06-22"; // 2 days before today → within Last7 + Last30
        let old = "2026-05-10"; // > 30 days → outside both windows
        let mk = |date: &str, inp: u64, out: u64| {
            parse_session(
                &format!(
                    "{}\n{}\n",
                    user_line(date),
                    assistant_line(date, "claude-opus-4-6", inp, out)
                ),
                false,
            )
        };
        let data = aggregate(&[mk(recent, 100, 50), mk(old, 999, 999)]);
        assert_eq!(data.total_sessions, 2);

        // All → unchanged.
        assert_eq!(data.for_range(StatsRange::All, today).total_sessions, 2);
        // Last7 / Last30 → only the recent session + its tokens.
        let l7 = data.for_range(StatsRange::Last7, today);
        assert_eq!(l7.total_sessions, 1);
        assert_eq!(l7.total_tokens(), 150);
        assert_eq!(data.for_range(StatsRange::Last30, today).total_sessions, 1);
    }

    #[test]
    fn r_key_cycles_the_date_range() {
        let mut st = StatsState::new(StatsData::default());
        assert_eq!(st.range, StatsRange::All);
        let _ = handle_stats_key(&mut st, k(KeyCode::Char('r')));
        assert_eq!(st.range, StatsRange::Last7);
        let _ = handle_stats_key(&mut st, k(KeyCode::Char('r')));
        assert_eq!(st.range, StatsRange::Last30);
        let _ = handle_stats_key(&mut st, k(KeyCode::Char('r')));
        assert_eq!(st.range, StatsRange::All);
    }

    #[test]
    fn render_shows_active_range_label() {
        let data = aggregate(&[parse_session(
            &format!(
                "{}\n{}\n",
                user_line("2026-05-01"),
                assistant_line("2026-05-01", "claude-opus-4-6", 10, 5)
            ),
            false,
        )]);
        let mut st = StatsState::new(data);
        // (stats-date-range-selector) all three options; active bracketed.
        assert!(
            render_stats_to_string(&st)
                .contains("[All time] \u{00B7} Last 7 days \u{00B7} Last 30 days"),
            "got: {}",
            render_stats_to_string(&st)
        );
        st.cycle_range();
        assert!(render_stats_to_string(&st)
            .contains("All time \u{00B7} [Last 7 days] \u{00B7} Last 30 days"));
        // (stats-footer-text) the r-cycle hint now lives in the footer.
        assert!(render_stats_to_string(&st).contains("Esc to cancel \u{00B7} r to cycle dates"));
    }

    #[test]
    fn longest_session_flows_into_overview() {
        // A transcript whose first→last main-chain timestamps span 2 hours.
        let content = [
            r#"{"type":"user","timestamp":"2026-05-25T10:00:00.000Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"assistant","timestamp":"2026-05-25T12:00:00.000Z","message":{"model":"claude-opus-4-6","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ]
        .join("\n");
        let contrib = parse_session(&content, false);
        assert_eq!(
            contrib.first_ts.as_deref(),
            Some("2026-05-25T10:00:00.000Z")
        );
        assert_eq!(contrib.last_ts.as_deref(), Some("2026-05-25T12:00:00.000Z"));
        let data = aggregate(&[contrib]);
        assert_eq!(data.longest_session_ms, 2 * 60 * 60 * 1000);
        let overview = overview_lines(&data).join("\n");
        assert!(
            overview.contains("Longest session: 2h 0m 0s"),
            "got: {overview}"
        );
    }
}
