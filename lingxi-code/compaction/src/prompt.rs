//! Byte-faithful port of the compaction prompt machinery.
//!
//! Mirrors `claude-code/src/services/compact/prompt.ts`:
//! - `NO_TOOLS_PREAMBLE` (prompt.ts:19-26)
//! - `NO_TOOLS_TRAILER` (prompt.ts:269-272)
//! - `BASE_COMPACT_PROMPT` (prompt.ts:61-143, with the
//!   `DETAILED_ANALYSIS_INSTRUCTION_BASE` interpolation inlined so the
//!   const equals the TS runtime string byte-for-byte)
//! - [`get_compact_prompt`] (prompt.ts:293-303)
//! - [`format_compact_summary`] (prompt.ts:311-335)
//! - [`get_compact_user_summary_message`] (prompt.ts:337-374)
//!
//! 1:1-fidelity notes:
//! - The XML stripping in `formatCompactSummary` uses regex in TS
//!   (`/<analysis>[\s\S]*?<\/analysis>/`, etc.). To avoid adding a new
//!   dependency (the crate has no `regex`), the scan is hand-rolled here.
//!   The transform is byte-faithful: same first-match semantics, same
//!   `Summary:\n{trimmed}` rewrite, same `\n\n+` → `\n\n` collapse, same
//!   final `.trim()`.
//! - The `up_to`/`partial` prompt variants are not exercised by the base
//!   summarizer (base/manual compact only) and are noted as a deferral.
//!   The `recentMessagesPreserved` branch (#58) IS now wired: when a partial
//!   compaction preserves a verbatim tail of recent messages, the continuation
//!   message gains the byte-exact `Recent messages are preserved verbatim.`
//!   sentence (TS `UOt`'s `r` parameter). The `replVmCleared` branch (`UOt`'s
//!   `o` parameter — a REPL-subsystem addendum) remains an intentional
//!   deferral; the REPL VM-state reset is a separate finding.

/// Aggressive no-tools preamble — prompt.ts:19-26.
///
/// Byte-faithful (trailing blank line included, matching the template
/// literal that ends with `\n\n`).
pub const NO_TOOLS_PREAMBLE: &str = "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

";

/// No-tools trailer — prompt.ts:269-272.
pub const NO_TOOLS_TRAILER: &str = "\n\nREMINDER: Do NOT call any tools. Respond with plain text only — an <analysis> block followed by a <summary> block. Tool calls will be rejected and you will fail the task.";

/// Base compact prompt — prompt.ts:61-143.
///
/// The TS source builds this with `${DETAILED_ANALYSIS_INSTRUCTION_BASE}`
/// (prompt.ts:31-44) interpolated at line 64; that interpolation is inlined
/// here so the const equals the TS runtime string byte-for-byte.
pub const BASE_COMPACT_PROMPT: &str = "Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
   - Note any security-relevant instructions or constraints the user stated (e.g., sensitive files or data to avoid, operations that must not be performed, credential or secret handling rules). These MUST be preserved verbatim in the summary so they continue to apply after compaction.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.

Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to the most recent messages and include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results. These are critical for understanding the users' feedback and changing intent. Preserve any security-relevant instructions or constraints verbatim so they remain in effect after compaction.
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this summary request, paying special attention to the most recent messages from both user and assistant. Include file names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's most recent explicit requests, and the task you were working on immediately before this summary request. If your last task was concluded, then only list next steps if they are explicitly in line with the users request. Do not start on tangential requests or really old requests that were already completed without confirming with the user first.
                       If there is a next step, include direct quotes from the most recent conversation showing exactly what task you were working on and where you left off. This should be verbatim to ensure there's no drift in task interpretation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Errors and fixes:
    - [Detailed description of error 1]:
      - [How you fixed the error]
      - [User feedback on the error if any]
    - [...]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages: 
    - [Detailed non tool use user message]
    - [...]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the conversation so far, following this structure and ensuring precision and thoroughness in your response. 

There may be additional summarization instructions provided in the included context. If so, remember to follow these instructions when creating the above summary. Examples of instructions include:
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also remember the mistakes you made and how you fixed them.
</example>

<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. Include file reads verbatim.
</example>
";

/// Assemble the base compact prompt — prompt.ts:293-303 (`getCompactPrompt`).
///
/// `custom_instructions`, when present and non-blank, is appended under an
/// `Additional Instructions:` header exactly as TS does. The `NO_TOOLS_*`
/// preamble/trailer bracket the body.
#[must_use]
pub fn get_compact_prompt(custom_instructions: Option<&str>) -> String {
    let mut prompt = String::with_capacity(
        NO_TOOLS_PREAMBLE.len() + BASE_COMPACT_PROMPT.len() + NO_TOOLS_TRAILER.len(),
    );
    prompt.push_str(NO_TOOLS_PREAMBLE);
    prompt.push_str(BASE_COMPACT_PROMPT);

    if let Some(custom) = custom_instructions {
        if !custom.trim().is_empty() {
            // Binary: `t += `\n\nAdditional Instructions:\n${e}`` — DOUBLE leading
            // `\n` (verified via `od -c` on the 2.1.195 binary JS source at the
            // `t+=` template; the `strings` dump splits real newlines and misled
            // an earlier pass into a single `\n`). BASE_COMPACT_PROMPT ends with
            // `</example>\n`, so the boundary nets `\n\n\n` (two blank lines).
            prompt.push_str("\n\nAdditional Instructions:\n");
            prompt.push_str(custom);
        }
    }

    prompt.push_str(NO_TOOLS_TRAILER);
    prompt
}

/// Find the first `[start, end)` span delimited by `open`/`close` (the
/// close tag immediately following the matched open tag). Returns the byte
/// range covering `open..=close` (i.e. inclusive of the closing tag), or
/// `None` when no well-formed pair is present.
///
/// Mirrors the JS regex `/<open>[\s\S]*?<\/close>/` first-match,
/// non-greedy semantics.
fn first_tag_span(haystack: &str, open: &str, close: &str) -> Option<(usize, usize)> {
    let open_at = haystack.find(open)?;
    let after_open = open_at + open.len();
    let close_rel = haystack[after_open..].find(close)?;
    let end = after_open + close_rel + close.len();
    Some((open_at, end))
}

/// Collapse runs of two-or-more newlines down to `\n\n` (one preserved blank
/// line) — mirrors the binary `aup` (`formatCompactSummary`) trailing
/// `replace(/\n\n+/g, '\n\n')`. (The replacement is `\n\n`, verified via
/// `od -c` on the 2.1.195 binary — the `strings` dump misled an earlier pass
/// into collapsing to a single `\n`.)
fn collapse_blank_lines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // Count the run of consecutive newlines.
            let mut run = 0;
            while i + run < bytes.len() && bytes[i + run] == b'\n' {
                run += 1;
            }
            // `/\n\n+/g` → `\n\n`: a run of 2+ newlines collapses to one
            // preserved blank line; a lone `\n` stays single.
            out.push_str(if run >= 2 { "\n\n" } else { "\n" });
            i += run;
        } else {
            // Copy this (possibly multibyte) UTF-8 scalar verbatim.
            let ch_len = utf8_char_len(bytes[i]);
            out.push_str(&input[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// Length in bytes of the UTF-8 sequence starting with `first_byte`.
#[inline]
fn utf8_char_len(first_byte: u8) -> usize {
    match first_byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Format a raw summary — prompt.ts:311-335 (`formatCompactSummary`).
///
/// Steps, in order, mirroring the TS regex pipeline:
/// 1. Strip the first `<analysis>…</analysis>` block (drafting scratchpad).
/// 2. Extract the first `<summary>…</summary>` block's inner content and
///    rewrite it in place as `Summary:\n{content.trim()}`.
/// 3. Collapse `\n\n+` runs to `\n\n` (preserve one blank line).
/// 4. `trim()` the whole result.
#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    // 1. Strip analysis section (first match only).
    let stripped: String = match first_tag_span(summary, "<analysis>", "</analysis>") {
        Some((start, end)) => {
            let mut s = String::with_capacity(summary.len());
            s.push_str(&summary[..start]);
            s.push_str(&summary[end..]);
            s
        }
        None => summary.to_string(),
    };

    // 2. Extract & rewrite summary section (first match only).
    let rewritten = match first_tag_span(&stripped, "<summary>", "</summary>") {
        Some((start, end)) => {
            // Inner content lives between the open and close tags.
            let inner_start = start + "<summary>".len();
            let inner_end = end - "</summary>".len();
            let content = stripped[inner_start..inner_end].trim();
            let mut s = String::with_capacity(stripped.len());
            s.push_str(&stripped[..start]);
            s.push_str("Summary:\n");
            s.push_str(content);
            s.push_str(&stripped[end..]);
            s
        }
        None => stripped,
    };

    // 3. Collapse extra whitespace between sections.
    let collapsed = collapse_blank_lines(&rewritten);

    // 4. Final trim.
    collapsed.trim().to_string()
}

/// Build the user-facing continuation message — `getCompactUserSummaryMessage`
/// (TS `UOt(e,t,n,r,o)`, `bin/claude.exe` offset 197355616).
///
/// Byte-faithful order, each segment appended only when its flag/arg is set
/// (segment prefixes verified via `od -c` on the 2.1.195 binary `K9t` JS source
/// — every appended segment uses `\n\n`; ONLY the final continuation uses a
/// single `\n`. An earlier pass misread the `strings` dump as single `\n`.):
/// 1. base: `This session is being continued… covers the earlier portion…\n\n{summary}`
/// 2. `transcript_path` (`n`): `\n\nIf you need specific details…read the full transcript at: {path}`
/// 3. `recent_messages_preserved` (`r`, #58): `\n\nRecent messages are preserved verbatim.`
/// 4. (`o` `replVmCleared` — the REPL VM-state addendum — is an intentional
///    deferral; the REPL VM reset is a separate finding.)
/// 5. `suppress_follow_up_questions` (`t`): `\nContinue the conversation…`
///
/// The `recent_messages_preserved` sentence is emitted exactly when the
/// partial/suffix-preserving path kept a verbatim tail after the summary —
/// matching TS, which threads `r = messagesToKeep.length > 0`.
#[must_use]
pub fn get_compact_user_summary_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    transcript_path: Option<&str>,
    recent_messages_preserved: bool,
) -> String {
    let formatted_summary = format_compact_summary(summary);

    let mut base_summary = format!(
        "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n{formatted_summary}"
    );

    if let Some(path) = transcript_path {
        base_summary.push_str(&format!(
            "\n\nIf you need specific details from before compaction (like exact code snippets, error messages, or content you generated), read the full transcript at: {path}"
        ));
    }

    // #58: when a verbatim tail rides after the summary, tell the model so it
    // does not re-derive recent state from the summary (`UOt`'s `r` arg).
    if recent_messages_preserved {
        base_summary.push_str("\n\nRecent messages are preserved verbatim.");
    }

    if suppress_follow_up_questions {
        let mut continuation = base_summary;
        continuation.push_str(
            "\nContinue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, do not preface with \"I'll continue\" or similar. Pick up the last task as if the break never happened.",
        );
        return continuation;
    }

    base_summary
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binary oracle: BASE_COMPACT_PROMPT must be exactly 5427 bytes in v2.1.193
    /// — the base literal ends `</example>\n` (verified via `od -c` at the closing
    /// backtick: `…verbatim.\n</example>\n` then `` `;…dea=`\n\nREMINDER… ``). This
    /// is a 2.1.186→2.1.193 drift: v2.1.186 was 5426 bytes (no trailing `\n`); a
    /// `\n` was added after the final `</example>` so the assembled prompt reads
    /// `</example>\n\n\nREMINDER…` (base `\n` + the trailer's `\n\n`).
    #[test]
    fn base_compact_prompt_byte_length_matches_binary() {
        assert_eq!(
            BASE_COMPACT_PROMPT.len(),
            5427,
            "BASE_COMPACT_PROMPT must be 5427 bytes (binary oracle v2.1.193)"
        );
        // Spot-check the two trailing-space lines that account for the
        // 5424→5426 difference vs the older TS source.
        assert!(
            BASE_COMPACT_PROMPT.contains("6. All user messages: \n"),
            "example header must have trailing space before \\n"
        );
        assert!(
            BASE_COMPACT_PROMPT.contains("thoroughness in your response. \n"),
            "closing instruction must have trailing space before \\n"
        );
    }

    #[test]
    fn get_compact_prompt_is_byte_faithful_golden() {
        let p = get_compact_prompt(None);
        // Brackets: preamble first, trailer last.
        assert!(p.starts_with(NO_TOOLS_PREAMBLE));
        assert!(p.ends_with(NO_TOOLS_TRAILER));
        // The body equals preamble + base + trailer exactly when no
        // custom instructions are supplied.
        let expected = format!("{NO_TOOLS_PREAMBLE}{BASE_COMPACT_PROMPT}{NO_TOOLS_TRAILER}");
        assert_eq!(p, expected);
        // Spot-check anchor lines from the TS const.
        assert!(p.contains("CRITICAL: Respond with TEXT ONLY. Do NOT call any tools."));
        assert!(p.contains("Your task is to create a detailed summary of the conversation so far"));
        assert!(p.contains("9. Optional Next Step:"));
        assert!(p.contains("REMINDER: Do NOT call any tools."));
    }

    #[test]
    fn get_compact_prompt_appends_custom_instructions_when_non_blank() {
        let p = get_compact_prompt(Some("focus on rust"));
        // Base ends `</example>\n`; the custom block adds a DOUBLE leading `\n`
        // (binary `t += `\n\nAdditional Instructions:\n${e}``), netting a triple
        // newline (two blank lines) at the boundary.
        assert!(p.contains("\n\nAdditional Instructions:\nfocus on rust"));
        assert!(
            p.contains("\n\n\nAdditional Instructions:"),
            "base `</example>\\n` + `\\n\\n` prefix nets a triple newline"
        );
        // Trailer still last.
        assert!(p.ends_with(NO_TOOLS_TRAILER));
        // And the additional-instructions block sits before the trailer.
        let ai_idx = p.find("Additional Instructions:").unwrap();
        let trailer_idx = p.find("REMINDER: Do NOT call any tools.").unwrap();
        assert!(ai_idx < trailer_idx);
    }

    #[test]
    fn get_compact_prompt_skips_blank_custom_instructions() {
        let with_blank = get_compact_prompt(Some("   \n\t  "));
        let without = get_compact_prompt(None);
        assert_eq!(with_blank, without);
        let empty = get_compact_prompt(Some(""));
        assert_eq!(empty, without);
    }

    #[test]
    fn format_strips_analysis_block() {
        let raw = "<analysis>scratch thoughts\nmore</analysis>\n<summary>S body</summary>";
        let out = format_compact_summary(raw);
        assert!(!out.contains("scratch thoughts"));
        assert!(!out.contains("<analysis>"));
        assert!(out.starts_with("Summary:\nS body"));
    }

    #[test]
    fn format_unwraps_summary_to_header() {
        let raw = "<summary>\n  hello world  \n</summary>";
        let out = format_compact_summary(raw);
        assert_eq!(out, "Summary:\nhello world");
    }

    #[test]
    fn format_collapses_blank_lines() {
        let raw = "<summary>line1\n\n\n\nline2</summary>";
        let out = format_compact_summary(raw);
        // Binary `aup` collapses any run of 2+ newlines to `\n\n` (one blank line).
        assert_eq!(out, "Summary:\nline1\n\nline2");
    }

    #[test]
    fn format_passthrough_without_tags() {
        let raw = "just a plain summary with no xml";
        let out = format_compact_summary(raw);
        assert_eq!(out, "just a plain summary with no xml");
    }

    #[test]
    fn format_passthrough_collapses_and_trims_plain_text() {
        let raw = "\n\n  alpha\n\n\nbeta  \n\n";
        let out = format_compact_summary(raw);
        // Leading/trailing whitespace trimmed, internal run collapsed to `\n\n`.
        assert_eq!(out, "alpha\n\nbeta");
    }

    #[test]
    fn format_full_pipeline_matches_ts_semantics() {
        let raw = "preface\n<analysis>\nthinking...\n</analysis>\n\n<summary>\n1. Primary Request\n\n\n2. Concepts\n</summary>\ntrailer";
        let out = format_compact_summary(raw);
        assert!(!out.contains("thinking..."));
        assert!(out.contains("Summary:\n1. Primary Request"));
        // The triple-newline inside the summary collapsed to `\n\n` (one blank line).
        assert!(out.contains("1. Primary Request\n\n2. Concepts"));
        assert!(!out.contains("1. Primary Request\n\n\n2. Concepts"));
        // Preface and trailer survive (only analysis stripped, summary
        // rewritten in place).
        assert!(out.starts_with("preface"));
        assert!(out.trim_end().ends_with("trailer"));
    }

    #[test]
    fn format_only_strips_first_analysis_block() {
        // JS regex without /g strips only the first match.
        let raw = "<analysis>one</analysis>X<analysis>two</analysis>";
        let out = format_compact_summary(raw);
        assert!(!out.contains("one"));
        assert!(out.contains("two"));
    }

    #[test]
    fn format_handles_multibyte_content() {
        let raw = "<summary>café — naïve\n\n\nrésumé</summary>";
        let out = format_compact_summary(raw);
        assert_eq!(out, "Summary:\ncafé — naïve\n\nrésumé");
    }

    #[test]
    fn user_summary_message_base_only() {
        let msg = get_compact_user_summary_message("<summary>S</summary>", false, None, false);
        assert!(msg.starts_with(
            "This session is being continued from a previous conversation that ran out of context."
        ));
        assert!(msg.contains("Summary:\nS"));
        // No continuation sentence when not suppressing follow-ups.
        assert!(!msg.contains("Continue the conversation from where it left off"));
        // No transcript line when path absent.
        assert!(!msg.contains("read the full transcript at:"));
        // No preserved-tail sentence when no tail was kept.
        assert!(!msg.contains("Recent messages are preserved verbatim."));
    }

    #[test]
    fn user_summary_message_with_suppress_adds_continuation() {
        let msg = get_compact_user_summary_message("<summary>S</summary>", true, None, false);
        assert!(msg.contains("Summary:\nS"));
        assert!(msg.contains("Continue the conversation from where it left off without asking the user any further questions."));
        assert!(msg.contains("Pick up the last task as if the break never happened."));
    }

    #[test]
    fn user_summary_message_with_transcript_path() {
        let msg = get_compact_user_summary_message(
            "<summary>S</summary>",
            true,
            Some("/tmp/transcript.jsonl"),
            false,
        );
        assert!(msg.contains(
            "read the full transcript at: /tmp/transcript.jsonl"
        ));
        // Transcript line precedes the continuation sentence.
        let t = msg.find("read the full transcript at:").unwrap();
        let c = msg.find("Continue the conversation from where it left off").unwrap();
        assert!(t < c);
    }

    // --- #58 recentMessagesPreserved branch (TS `UOt`'s `r` arg) ---------- //

    #[test]
    fn user_summary_message_recent_preserved_appends_sentence() {
        let msg =
            get_compact_user_summary_message("<summary>S</summary>", false, None, true);
        assert!(msg.contains("Summary:\nS"));
        // Byte-exact preserved-tail sentence (binary K9t joins with `\n\n`).
        assert!(msg.contains("\n\nRecent messages are preserved verbatim."));
    }

    #[test]
    fn user_summary_message_recent_preserved_ordering() {
        // Order: base → transcript → recent-preserved → continuation.
        let msg = get_compact_user_summary_message(
            "<summary>S</summary>",
            true,
            Some("/t.jsonl"),
            true,
        );
        let transcript = msg.find("read the full transcript at:").unwrap();
        let preserved = msg.find("Recent messages are preserved verbatim.").unwrap();
        let cont = msg
            .find("Continue the conversation from where it left off")
            .unwrap();
        assert!(transcript < preserved, "transcript precedes preserved sentence");
        assert!(preserved < cont, "preserved sentence precedes continuation");
    }

    #[test]
    fn user_summary_message_no_preserved_when_flag_false() {
        // The full-replacement path (no kept tail) must be byte-identical to
        // before: no preserved-tail sentence anywhere.
        let with_tail = get_compact_user_summary_message("<summary>S</summary>", true, None, true);
        let without = get_compact_user_summary_message("<summary>S</summary>", true, None, false);
        assert!(!without.contains("Recent messages are preserved verbatim."));
        // The only delta between them is the inserted preserved-tail sentence.
        assert_eq!(
            with_tail.replace("\n\nRecent messages are preserved verbatim.", ""),
            without
        );
    }
}
