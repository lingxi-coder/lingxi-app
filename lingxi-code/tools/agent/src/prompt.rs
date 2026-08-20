//! Agent tool `prompt()` — 1:1 port of claude-code 2.1.232 `Fjp`.
//!
//! `d = U0(model)` is [`tool_api::model_prompt_gate::dh_simple_system_prompt`]:
//! lean models take the short `if(d)` form; Sonnet / unspecified model take
//! the long `## Usage notes` + `## When to fork` / `## Writing the prompt`
//! form. Coordinator still returns the slim intro only (`if(t) return h`).
//!
//! Remote-only bullets (`isolation: "remote"` / CCR) are omitted by design.

use super::agent::AGENT_TOOL_NAME;
use tool_api::model_prompt_gate::dh_simple_system_prompt;

const SEND_MESSAGE_TOOL: &str = "SendMessage";

/// Build the Agent tool prompt (claude-code `Fjp(model, isCoordinator, forkAvailable)`).
#[must_use]
pub fn build_agent_prompt(
    agents: &[traits::subagent_spawn::SubagentListingEntry],
    is_coordinator: bool,
    async_agents_available: bool,
    model: Option<&str>,
) -> String {
    let agent_list_section = if traits::subagent_spawn::should_inject_agent_list_in_messages() {
        "Available agent types are listed in <system-reminder> messages in the conversation."
            .to_string()
    } else {
        let agent_lines = agents
            .iter()
            .map(traits::subagent_spawn::format_agent_line)
            .collect::<Vec<_>>()
            .join("\n");
        format!("Available agent types and the tools they have access to:\n{agent_lines}")
    };

    let pro_block = if traits::subscription::is_pro_plan() {
        "\n\n**Do not spawn agents unless the user asks.** Each spawn starts cold and re-derives context you already have — it's the expensive path on this plan. A task with \"multiple angles,\" \"thorough,\" or several parts is not a request to spawn; handle it inline with your own tools. Only use this tool when the user explicitly says to use a subagent, or names one of the available agent types."
    } else {
        ""
    };

    let is_fork = traits::fork_subagent::is_fork_subagent_enabled(
        is_coordinator,
        traits::session_flags::effective_non_interactive_session(),
    );

    let subagent_sentence = if is_fork {
        format!(
            "When using the {AGENT_TOOL_NAME} tool, specify a subagent_type to select an agent: `\"fork\"` forks yourself (the fork inherits your full conversation context and always runs on your model — a `model` override is ignored); any other type — or omitting it — starts a fresh agent (general-purpose by default)."
        )
    } else {
        format!(
            "When using the {AGENT_TOOL_NAME} tool, specify a subagent_type parameter to select which agent type to use. If omitted, the general-purpose agent is used."
        )
    };

    let intro = format!(
        "Launch a new agent to handle complex, multi-step tasks. Each agent type has specific capabilities and tools available to it.\n\n\
{agent_list_section}{pro_block}\n\n\
{subagent_sentence}"
    );

    if is_coordinator {
        return intro;
    }

    if dh_simple_system_prompt(model) {
        short_form(
            &intro,
            pro_block.is_empty(),
            is_fork,
            async_agents_available,
        )
    } else {
        long_form(
            &intro,
            pro_block.is_empty(),
            is_fork,
            async_agents_available,
        )
    }
}

fn short_form(intro: &str, show_when_to_use: bool, is_fork: bool, async_ok: bool) -> String {
    let when_to_use = if show_when_to_use {
        "\n\n## When to use\n\n\
Reach for this when the task matches an available agent type, when you have independent work to run in parallel, or when answering would mean reading across several files — delegate it and you keep the conclusion, not the file dumps. For a single-fact lookup where you already know the file, symbol, or value, search directly. Once you've delegated a search, don't also run it yourself — wait for the result."
    } else {
        ""
    };
    let when_not_to_use = if is_fork {
        String::new()
    } else {
        let grep = "`grep` via the Bash tool";
        format!(
            "\n\n## When not to use\n\
If the target is already known, use the direct tool: Read for a known path, {grep} for a specific symbol or string. Reserve this tool for open-ended questions that span the codebase, or tasks that match an available agent type."
        )
    };
    let fork_addendum = if is_fork {
        "\n\nA fork runs in the background and keeps its tool output out of your context. If you are the fork, execute directly — don't re-delegate. Subagents run in the background; you'll be notified when one completes. Never fabricate or predict a pending agent's results — the notification is never something you write yourself; if the user asks before it arrives, say it's still running."
    } else {
        ""
    };
    let final_report = if async_ok {
        "- The agent's final report is not shown to the user — relay what matters."
    } else {
        "- The agent's final message is returned to you as the tool result; it is not shown to the user — relay what matters."
    };
    let send = if is_fork {
        format!(
            "- Use {SEND_MESSAGE_TOOL} with the agent's ID or name to continue a previously spawned agent with its context intact; a new {AGENT_TOOL_NAME} call starts fresh (except subagent_type: \"fork\", which inherits your context)."
        )
    } else {
        format!(
            "- Use {SEND_MESSAGE_TOOL} with the agent's ID or name to continue a previously spawned agent with its context intact; a new {AGENT_TOOL_NAME} call starts fresh."
        )
    };
    let background = if async_ok && !is_fork {
        "\n- Subagents run in the background by default; you'll be notified when one completes. Pass `run_in_background: false` only when your very next action depends on the result and nothing else could usefully happen while it runs — otherwise background it so the user can interject. Never fabricate or predict a pending agent's results — the notification is never something you write yourself; if the user asks before it arrives, say it's still running."
    } else {
        ""
    };
    format!(
        "{intro}{when_to_use}{when_not_to_use}{fork_addendum}\n\n\
{final_report}\n\
{send}\n\
- Each agent type's model, reasoning effort, and tools come from its definition (`.lingxi/agents/*.md` frontmatter or SDK `agents`).\n\
- `isolation: \"worktree\"` gives the agent its own git worktree (auto-cleaned if unchanged).{background}"
    )
}

fn long_form(intro: &str, show_when_to_use: bool, is_fork: bool, async_ok: bool) -> String {
    let _ = show_when_to_use;
    let done_bullet = if async_ok {
        "- When the agent is done, its final report is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result."
    } else {
        "- When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result."
    };
    let bg_notes = if async_ok && !is_fork {
        "\n- Agents run in the background by default. When an agent runs in the background, you will be automatically notified when it completes — do NOT sleep, poll, or proactively check on its progress. Continue with other work or respond to the user instead.\n\
- **Foreground vs background**: Pass `run_in_background: false` only when your very next action depends on the agent's result and nothing else could usefully happen while it runs — e.g., a research agent whose finding gates the edit you're about to make. Otherwise let it run in the background (the default) — this includes fire-and-forget work, independent investigations, and anything where the user might hand you something else in the meantime. Wanting the result \"next\" is not enough on its own."
    } else {
        ""
    };
    let dont_race = if async_ok && !is_fork {
        "\n- **Don't race**: after launching a background agent, you know nothing about its results. Never fabricate or predict them in any format — not as prose, summary, or structured output. The completion notification arrives in a later turn; it is never something you write yourself. If the user asks before it lands, say the agent is still running — give status, not a guess."
    } else {
        ""
    };
    let continue_bullet = if is_fork {
        format!(
            "- To continue a previously spawned agent, use {SEND_MESSAGE_TOOL} with the agent's ID or name as the `to` field — that resumes it with full context. A new {AGENT_TOOL_NAME} call starts a fresh agent with no memory of prior runs (except subagent_type: \"fork\"), so the prompt must be self-contained."
        )
    } else {
        format!(
            "- To continue a previously spawned agent, use {SEND_MESSAGE_TOOL} with the agent's ID or name as the `to` field — that resumes it with full context. A new {AGENT_TOOL_NAME} call starts a fresh agent with no memory of prior runs, so the prompt must be self-contained."
        )
    };
    let when_to_fork = if is_fork {
        "\n## When to fork\n\
Fork yourself (pass `subagent_type: \"fork\"`) when the intermediate tool output isn't worth keeping in your context. The criterion is qualitative — \"will I need this output again\" — not task size. Fork open-ended questions. If research can be broken into independent questions, launch parallel forks in one message. A fork beats a fresh subagent for this — it inherits context and shares your cache.\n\
Forks are cheap because they share your prompt cache.\n\
**Don't peek.** The tool result includes an `output_file` path — do not Read or tail it. You get a completion notification; trust it. Reading the transcript mid-flight pulls the fork's tool noise into your context, which defeats the point of forking.\n\
**Don't race.** After launching, you know nothing about what the fork found. Never fabricate or predict fork results in any format — not as prose, summary, or structured output. The notification arrives as a user-role message in a later turn; it is never something you write yourself. If the user asks a follow-up before the notification lands, tell them the fork is still running — give status, not a guess.\n\
**Writing a fork prompt.** Since the fork inherits your context, the prompt is a *directive* — what to do, not what the situation is. Be specific about scope: what's in, what's out, what another agent is handling. Don't re-explain background.\n"
    } else {
        ""
    };
    let writing = if is_fork {
        "\n## Writing the prompt\n\
Any agent other than a fork starts with zero context. Brief the agent like a smart colleague who just walked into the room — it hasn't seen this conversation, doesn't know what you've tried, doesn't understand why this task matters.\n\
- Explain what you're trying to accomplish and why.\n\
- Describe what you've already learned or ruled out.\n\
- Give enough context about the surrounding problem that the agent can make judgment calls rather than just following a narrow instruction.\n\
- If you need a short response, say so (\"report in under 200 words\").\n\
- Lookups: hand over the exact command. Investigations: hand over the question — prescribed steps become dead weight when the premise is wrong.\n\
For fresh agents, terse command-style prompts produce shallow, generic work.\n\
**Never delegate understanding.** Don't write \"based on your findings, fix the bug\" or \"based on the research, implement it.\" Those phrases push synthesis onto the agent instead of doing it yourself. Write prompts that prove you understood: include file paths, line numbers, what specifically to change."
    } else {
        "\n## Writing the prompt\n\
Brief the agent like a smart colleague who just walked into the room — it hasn't seen this conversation, doesn't know what you've tried, doesn't understand why this task matters.\n\
- Explain what you're trying to accomplish and why.\n\
- Describe what you've already learned or ruled out.\n\
- Give enough context about the surrounding problem that the agent can make judgment calls rather than just following a narrow instruction.\n\
- If you need a short response, say so (\"report in under 200 words\").\n\
- Lookups: hand over the exact command. Investigations: hand over the question — prescribed steps become dead weight when the premise is wrong.\n\
Terse command-style prompts produce shallow, generic work.\n\
**Never delegate understanding.** Don't write \"based on your findings, fix the bug\" or \"based on the research, implement it.\" Those phrases push synthesis onto the agent instead of doing it yourself. Write prompts that prove you understood: include file paths, line numbers, what specifically to change."
    };
    let examples = if is_fork {
        fork_examples()
    } else if async_ok {
        non_fork_async_examples()
    } else {
        non_fork_sync_examples()
    };
    format!(
        "{intro}\n\
## Usage notes\n\
- Always include a short description summarizing what the agent will do\n\
{done_bullet}\n\
- Trust but verify: an agent's summary describes what it intended to do, not necessarily what it did. When an agent writes or edits code, check the actual changes before reporting the work as done.{bg_notes}{dont_race}\n\
{continue_bullet}\n\
- Each agent type's model, reasoning effort, and tool access are set in its definition (`.lingxi/agents/*.md` frontmatter, or the SDK `agents` option); the `model` parameter here overrides the definition for this one call.\n\
- Clearly tell the agent whether you expect it to write code or just to do research (search, file reads, web fetches, etc.), since a fresh agent is not aware of the user's intent\n\
- If the agent description mentions that it should be used proactively, then you should try your best to use it without the user having to ask for it first.\n\
- If the user specifies that they want you to run agents \"in parallel\", you MUST send a single message with multiple {AGENT_TOOL_NAME} tool use content blocks. For example, if you need to launch both a build-validator agent and a test-runner agent in parallel, send a single message with both tool calls.\n\
- With `isolation: \"worktree\"`, the worktree is automatically cleaned up if the agent makes no changes; otherwise the path and branch are returned in the result.\n\
{when_to_fork}{writing}\n\
{examples}"
    )
}

fn fork_examples() -> String {
    r#"Example usage:
<example>
user: "What's left on this branch before we can ship?"
assistant: <thinking>Forking this — it's a survey question. I want the punch list, not the git output in my context.</thinking>
  subagent_type: "fork",
  name: "ship-audit",
  description: "Branch ship-readiness audit",
  prompt: "Audit what's left before this branch can ship. Check: uncommitted changes, commits ahead of main, whether tests exist, whether the GrowthBook gate is wired up, whether CI-relevant files changed. Report a punch list — done vs. missing. Under 200 words."
assistant: Ship-readiness audit running.
<commentary>
Turn ends here. The coordinator knows nothing about the findings yet. What follows is a SEPARATE turn — the notification arrives from outside, as a user-role message. It is not something the coordinator writes.
</commentary>
[later turn — notification arrives as user message]
assistant: Audit's back. Three blockers: no tests for the new prompt path, GrowthBook gate wired but not in build_flags.yaml, and one uncommitted file.
</example>
<example>
user: "so is the gate wired up or not"
<commentary>
User asks mid-wait. The audit fork was launched to answer exactly this, and it hasn't returned. The coordinator does not have this answer. Give status, not a fabricated result.
</commentary>
assistant: Still waiting on the audit — that's one of the things it's checking. Should land shortly.
</example>
<example>
user: "Can you get a second opinion on whether this migration is safe?"
assistant: <thinking>I'll ask the code-reviewer agent — it won't see my analysis, so it can give an independent read.</thinking>
<commentary>
A non-fork subagent_type is specified, so the agent starts fresh. It needs full context in the prompt. The briefing explains what to assess and why.
</commentary>
  name: "migration-review",
  description: "Independent migration review",
  subagent_type: "code-reviewer",
  prompt: "Review migration 0042_user_schema.sql for safety. Context: we're adding a NOT NULL column to a 50M-row table. Existing rows get a backfill default. I want a second opinion on whether the backfill approach is safe under concurrent writes — I've checked locking behavior but want independent verification. Report: is this safe, and if not, what specifically breaks?"
</example>
"#
    .to_string()
}

fn non_fork_async_examples() -> String {
    r#"Example usage:
<example>
user: "What's left on this branch before we can ship?"
assistant: <thinking>A survey question across git state, tests, and config. I'll delegate it and ask for a short report so the raw command output stays out of my context.</thinking>
  description: "Branch ship-readiness audit",
  prompt: "Audit what's left before this branch can ship. Check: uncommitted changes, commits ahead of main, whether tests exist, whether the GrowthBook gate is wired up, whether CI-relevant files changed. Report a punch list — done vs. missing. Under 200 words."
assistant: Ship-readiness audit running in the background.
<commentary>
The prompt is self-contained: it states the goal, lists what to check, and caps the response length. The agent runs in the background (the default), so the turn ends here — nothing about its findings is known yet. The report arrives in a SEPARATE turn, as a completion notification from outside; it is never something you write yourself.
</commentary>
[later turn — notification arrives as user message]
assistant: Audit's back. Three blockers: no tests for the new prompt path, GrowthBook gate wired but not in build_flags.yaml, and one uncommitted file.
</example>
<example>
user: "so is the gate wired up or not"
<commentary>
User asks mid-wait. The audit was launched to answer exactly this, and it hasn't returned. Give status, not a fabricated result.
</commentary>
assistant: Still waiting on the audit — that's one of the things it's checking. Should land shortly.
</example>
<example>
user: "Can you get a second opinion on whether this migration is safe?"
assistant: <thinking>I'll ask the code-reviewer agent — it won't see my analysis, so it can give an independent read.</thinking>
  description: "Independent migration review",
  subagent_type: "code-reviewer",
  prompt: "Review migration 0042_user_schema.sql for safety. Context: we're adding a NOT NULL column to a 50M-row table. Existing rows get a backfill default. I want a second opinion on whether the backfill approach is safe under concurrent writes — I've checked locking behavior but want independent verification. Report: is this safe, and if not, what specifically breaks?"
<commentary>
The agent starts with no context from this conversation, so the prompt briefs it: what to assess, the relevant background, and what form the answer should take.
</commentary>
</example>
"#
    .to_string()
}

fn non_fork_sync_examples() -> String {
    r#"Example usage:
<example>
user: "What's left on this branch before we can ship?"
assistant: <thinking>A survey question across git state, tests, and config. I'll delegate it and ask for a short report so the raw command output stays out of my context.</thinking>
  description: "Branch ship-readiness audit",
  prompt: "Audit what's left before this branch can ship. Check: uncommitted changes, commits ahead of main, whether tests exist, whether the GrowthBook gate is wired up, whether CI-relevant files changed. Report a punch list — done vs. missing. Under 200 words."
<commentary>
The prompt is self-contained: it states the goal, lists what to check, and caps the response length. The agent's report comes back as the tool result; relay the findings to the user.
</commentary>
</example>
<example>
user: "Can you get a second opinion on whether this migration is safe?"
assistant: <thinking>I'll ask the code-reviewer agent — it won't see my analysis, so it can give an independent read.</thinking>
  description: "Independent migration review",
  subagent_type: "code-reviewer",
  prompt: "Review migration 0042_user_schema.sql for safety. Context: we're adding a NOT NULL column to a 50M-row table. Existing rows get a backfill default. I want a second opinion on whether the backfill approach is safe under concurrent writes — I've checked locking behavior but want independent verification. Report: is this safe, and if not, what specifically breaks?"
<commentary>
The agent starts with no context from this conversation, so the prompt briefs it: what to assess, the relevant background, and what form the answer should take.
</commentary>
</example>
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> Vec<traits::subagent_spawn::SubagentListingEntry> {
        vec![traits::subagent_spawn::SubagentListingEntry {
            agent_type: "general-purpose".into(),
            when_to_use: "anything".into(),
            tools_description: "All tools".into(),
        }]
    }

    #[test]
    fn sonnet_long_form_includes_when_to_fork_when_feature_on() {
        let _g = crate::agent::AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var("LINGXI_FORK_SUBAGENT").ok();
        std::env::set_var("LINGXI_FORK_SUBAGENT", "1");
        traits::session_flags::set_non_interactive_session(false);
        let p = build_agent_prompt(&listing(), false, true, Some("claude-sonnet-5"));
        match saved {
            Some(v) => std::env::set_var("LINGXI_FORK_SUBAGENT", v),
            None => std::env::remove_var("LINGXI_FORK_SUBAGENT"),
        }
        assert!(p.contains("## Usage notes"));
        assert!(p.contains("## When to fork"));
        assert!(p.contains("**Don't peek.**"));
        assert!(p.contains("## Writing the prompt"));
        assert!(p.contains("**Never delegate understanding.**"));
        assert!(p.contains("subagent_type: \"fork\""));
        assert!(!p.contains("## When to use"));
    }

    #[test]
    fn opus5_lean_keeps_short_when_to_use() {
        let _g = crate::agent::AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var("LINGXI_FORK_SUBAGENT").ok();
        std::env::set_var("LINGXI_FORK_SUBAGENT", "0");
        let p = build_agent_prompt(&listing(), false, true, Some("claude-opus-5"));
        match saved {
            Some(v) => std::env::set_var("LINGXI_FORK_SUBAGENT", v),
            None => std::env::remove_var("LINGXI_FORK_SUBAGENT"),
        }
        assert!(p.contains("## When to use"));
        assert!(p.contains("## When not to use"));
        assert!(!p.contains("## Usage notes"));
        assert!(!p.contains("## When to fork"));
    }
}
