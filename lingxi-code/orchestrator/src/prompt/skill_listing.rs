//! `skill_listing` system-reminder formatter + provider seam.
//!
//! 1:1 with claude-code `src/tools/SkillTool/prompt.ts` (the
//! `formatCommandsWithinBudget` budgeter) and the `messages.ts:3728-3738`
//! renderer that wraps the listing in a `<system-reminder>` meta user message.
//!
//! IMPORTANT: the skill listing is NOT a system-prompt section. It is a
//! per-turn, TRANSIENT meta user message appended to the outgoing message
//! snapshot (never to `session.history` / JSONL), exactly like the OUTSTYLE.3
//! output-style reminder. See
//! [`crate::conversation::ConversationOrchestrator::skill_listing_reminder_message`].
#![forbid(unsafe_code)]

use async_trait::async_trait;

/// Minimal per-skill view the formatter needs. The composition root builds
/// these from `command_api::SlashCommand` so the orchestrator crate need not
/// depend on `command-api`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillListingEntry {
    /// Skill name (TS `cmd.name`).
    pub name: String,
    /// Short description (TS `cmd.description`).
    pub description: String,
    /// Optional "when to use" guidance (TS `cmd.whenToUse`).
    pub when_to_use: Option<String>,
    /// TS `cmd.source === 'bundled'` — bundled skills are never truncated.
    pub is_bundled: bool,
}

/// Supplies the model-invocable skill entries for the per-turn `skill_listing`
/// reminder. The production impl wraps the shared `CommandRegistry`; tests
/// inject a static fixture. Mirrors TS `getSkillToolCommands(cwd)`
/// (`commands.ts:565`).
#[async_trait]
pub trait SkillListingProvider: Send + Sync {
    /// Return the eligible skill entries (already filtered to model-invocable
    /// prompt skills). May be empty.
    async fn skill_entries(&self) -> Vec<SkillListingEntry>;
}

// prompt.ts:20-29
const SKILL_BUDGET_CONTEXT_PERCENT: f64 = 0.01;
const CHARS_PER_TOKEN: usize = 4;
const DEFAULT_CHAR_BUDGET: usize = 8_000;
const MAX_LISTING_DESC_CHARS: usize = 250;
const MIN_DESC_LENGTH: usize = 20;

/// prompt.ts:31-41. Honors the `SLASH_COMMAND_TOOL_CHAR_BUDGET` override 1:1.
fn char_budget(context_window_tokens: Option<usize>) -> usize {
    if let Some(v) = std::env::var("SLASH_COMMAND_TOOL_CHAR_BUDGET")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n != 0)
    {
        return v;
    }
    match context_window_tokens {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        Some(t) => ((t * CHARS_PER_TOKEN) as f64 * SKILL_BUDGET_CONTEXT_PERCENT).floor() as usize,
        None => DEFAULT_CHAR_BUDGET,
    }
}

/// prompt.ts:43-50. `whenToUse ? "{desc} - {when}" : desc`, capped at 250 with `…`.
fn skill_description(e: &SkillListingEntry) -> String {
    let desc = match &e.when_to_use {
        Some(w) if !w.is_empty() => format!("{} - {}", e.description, w),
        _ => e.description.clone(),
    };
    if desc.chars().count() > MAX_LISTING_DESC_CHARS {
        let kept: String = desc.chars().take(MAX_LISTING_DESC_CHARS - 1).collect();
        format!("{kept}\u{2026}")
    } else {
        desc
    }
}

fn full_line(e: &SkillListingEntry) -> String {
    format!("- {}: {}", e.name, skill_description(e))
}

/// prompt.ts:70-171. Returns the listing body (no `<system-reminder>` wrapper);
/// empty string when there are no entries.
///
/// PARITY-NOTE: TS measures width with `stringWidth` (grapheme / East-Asian
/// aware). This uses `chars().count()`, exact for ASCII descriptions and a
/// close divergence only at the truncation boundary for wide/emoji text.
#[must_use]
pub fn format_within_budget(
    entries: &[SkillListingEntry],
    context_window_tokens: Option<usize>,
) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let budget = char_budget(context_window_tokens);

    let full: Vec<String> = entries.iter().map(full_line).collect();
    let join_overhead = full.len().saturating_sub(1); // newlines between lines
    let full_total: usize = full.iter().map(|l| l.chars().count()).sum::<usize>() + join_overhead;
    if full_total <= budget {
        return full.join("\n");
    }

    // Over budget: bundled skills stay full; the rest share the remaining budget
    // evenly across their descriptions, falling back to names-only (prompt.ts:92-170).
    let bundled_chars: usize = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_bundled)
        .map(|(i, _)| full[i].chars().count() + 1) // +1 newline
        .sum();
    let rest: Vec<usize> = (0..entries.len())
        .filter(|&i| !entries[i].is_bundled)
        .collect();
    if rest.is_empty() {
        return full.join("\n");
    }
    let remaining = budget.saturating_sub(bundled_chars);
    let rest_name_overhead: usize = rest
        .iter()
        .map(|&i| entries[i].name.chars().count() + "- ".len() + ": ".len())
        .sum::<usize>()
        + rest.len().saturating_sub(1); // newlines among rest
    let max_desc_len = remaining.saturating_sub(rest_name_overhead) / rest.len();

    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            if e.is_bundled {
                full[i].clone()
            } else if max_desc_len < MIN_DESC_LENGTH {
                format!("- {}", e.name) // names-only fallback (prompt.ts:137-141)
            } else {
                let d = skill_description(e);
                let d: String = d.chars().take(max_desc_len).collect();
                format!("- {}: {d}", e.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// messages.ts:3728-3738 renderer + `wrapInSystemReminder` (`messages.ts:3097`).
/// Returns the full meta-message body (including the `<system-reminder>`
/// envelope), or `None` when the listing is empty.
#[must_use]
pub fn render_reminder(
    entries: &[SkillListingEntry],
    context_window_tokens: Option<usize>,
) -> Option<String> {
    let body = format_within_budget(entries, context_window_tokens);
    if body.is_empty() {
        return None;
    }
    Some(format!(
        "<system-reminder>\nThe following skills are available for use with the Skill tool:\n\n{body}\n</system-reminder>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serializes the two tests that mutate the process-global
    // SLASH_COMMAND_TOOL_CHAR_BUDGET env var. Poison is benign — recover.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn entry(name: &str, desc: &str, when: Option<&str>, bundled: bool) -> SkillListingEntry {
        SkillListingEntry {
            name: name.to_string(),
            description: desc.to_string(),
            when_to_use: when.map(str::to_string),
            is_bundled: bundled,
        }
    }

    #[test]
    fn empty_entries_render_none() {
        assert_eq!(render_reminder(&[], Some(200_000)), None);
        assert_eq!(format_within_budget(&[], None), "");
    }

    #[test]
    fn renders_byte_exact_system_reminder() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("SLASH_COMMAND_TOOL_CHAR_BUDGET");
        let entries = vec![
            entry("debug", "Debug a failing test", None, false),
            entry(
                "loop",
                "Run a prompt on a loop",
                Some("for repeated tasks"),
                false,
            ),
        ];
        let got = render_reminder(&entries, Some(200_000)).expect("some");
        let want = "<system-reminder>\n\
            The following skills are available for use with the Skill tool:\n\n\
            - debug: Debug a failing test\n\
            - loop: Run a prompt on a loop - for repeated tasks\n\
            </system-reminder>";
        assert_eq!(got, want);
    }

    #[test]
    fn when_to_use_is_appended_to_description() {
        let e = entry("x", "Base desc", Some("when X happens"), false);
        assert_eq!(skill_description(&e), "Base desc - when X happens");
    }

    #[test]
    fn description_capped_at_250_chars_with_ellipsis() {
        let long = "d".repeat(400);
        let e = entry("x", &long, None, false);
        let d = skill_description(&e);
        assert_eq!(d.chars().count(), MAX_LISTING_DESC_CHARS);
        assert!(d.ends_with('\u{2026}'));
    }

    #[test]
    fn tiny_budget_falls_back_to_names_only_for_non_bundled() {
        // A tiny budget forces the truncation path; non-bundled entries collapse
        // to "- name" while bundled entries keep their full line.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("SLASH_COMMAND_TOOL_CHAR_BUDGET");
        let entries = vec![
            entry("bundled_one", "Bundled stays full", None, true),
            entry(
                "user_a",
                "Some user skill description that is long",
                None,
                false,
            ),
            entry(
                "user_b",
                "Another user skill description that is long",
                None,
                false,
            ),
        ];
        let body = format_within_budget(&entries, Some(1_000)); // budget = 40 chars
        assert!(body.contains("- bundled_one: Bundled stays full"));
        assert!(body.contains("- user_a"));
        assert!(body.contains("- user_b"));
        // Non-bundled descriptions are dropped under a tiny budget.
        assert!(!body.contains("Some user skill description"));
    }

    #[test]
    fn slash_command_tool_char_budget_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("SLASH_COMMAND_TOOL_CHAR_BUDGET", "5");
        let b = char_budget(Some(200_000));
        std::env::remove_var("SLASH_COMMAND_TOOL_CHAR_BUDGET");
        assert_eq!(b, 5);
    }

    #[test]
    fn default_budget_matches_one_percent_of_window() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("SLASH_COMMAND_TOOL_CHAR_BUDGET");
        assert_eq!(char_budget(Some(200_000)), 8_000); // 200k * 4 * 0.01
        assert_eq!(char_budget(None), DEFAULT_CHAR_BUDGET);
    }
}
