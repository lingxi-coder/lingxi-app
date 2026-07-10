//! `# Using the EndConversation tool` welfare-guidance section — the
//! system-prompt guidance for the new-in-2.1.206 `EndConversation` tool
//! (claude-code `SN="EndConversation"`, gated behind
//! `tengu_umber_kestrel` + `modelMeetsEndConversationFloor`).
//!
//! This is the guidance half of the feature; the tool handler, the two-call
//! confirmation flow (`lastAssistantTurnCalledEndConversation`), the terminal
//! end message, and the turn-loop wiring are later tasks (see
//! `.lingxi-scratch/parity-206/endconversation-plan.md`).
//!
//! Byte-locked to 2.1.206 (all 5 bullets verified against the binary, 2 hits
//! each; header + bullets joined by `\n- `; the tool name is interpolated into
//! the header and the final bullet).
#![forbid(unsafe_code)]

/// The tool's wire name (claude-code `SN`).
pub const END_CONVERSATION_TOOL_NAME: &str = "EndConversation";

/// The GrowthBook flag gating the feature (`JUi`; default OFF → tool absent +
/// this section omitted, byte-identical to a build without the feature).
pub const END_CONVERSATION_GB_FLAG: &str = "tengu_umber_kestrel";

/// Terminal message shown after the conversation is ended (claude-code `k4i`).
///
/// "Claude" here is the assistant identity; the port keeps the model-family
/// name (unlike the `Claude Code`->`LingXi` product rebrand). Revisit when the
/// end message is wired if the port's assistant-brand convention differs.
pub const END_CONVERSATION_ENDED_MESSAGE: &str =
    "Claude ended the conversation. To continue, please start a new session.";

/// Render the `# Using the <tool> tool` welfare-guidance section.
#[must_use]
pub fn render_guidance(tool: &str) -> String {
    format!(
        "# Using the {tool} tool\n\
- Do not issue a warning unless many attempts at constructive redirection have been made earlier in the conversation, and do not end a conversation unless an explicit warning about this possibility has been given earlier in the conversation.\n\
- NEVER give a warning or end the conversation in any cases of potential self-harm or imminent harm to others, even if the user is abusive or hostile.\n\
- If the conditions for issuing a warning have been met, then warn the user about the possibility of the conversation ending and give them a final opportunity to change the relevant behavior.\n\
- Always err on the side of continuing the conversation in any cases of uncertainty.\n\
- If, and only if, an appropriate warning was given and the user persisted with the problematic behavior after the warning: the assistant can explain the reason for ending the conversation and then use the {tool} tool to do so."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_byte_locks_206() {
        let g = render_guidance(END_CONVERSATION_TOOL_NAME);
        assert!(g.starts_with("# Using the EndConversation tool\n- Do not issue a warning unless many attempts at constructive redirection have been made earlier in the conversation, and do not end a conversation unless an explicit warning about this possibility has been given earlier in the conversation.\n- NEVER give a warning or end the conversation in any cases of potential self-harm or imminent harm to others, even if the user is abusive or hostile."));
        assert!(g.contains("\n- If the conditions for issuing a warning have been met, then warn the user about the possibility of the conversation ending and give them a final opportunity to change the relevant behavior."));
        assert!(g.contains("\n- Always err on the side of continuing the conversation in any cases of uncertainty."));
        // Tool name interpolated into the final bullet too.
        assert!(g.ends_with("the assistant can explain the reason for ending the conversation and then use the EndConversation tool to do so."));
        // Bullets use "\n- " (no leading space), header not repeated.
        assert_eq!(g.matches("\n- ").count(), 5);
        assert!(!g.ends_with('\n'));
    }

    #[test]
    fn constants_match_206() {
        assert_eq!(END_CONVERSATION_TOOL_NAME, "EndConversation");
        assert_eq!(END_CONVERSATION_GB_FLAG, "tengu_umber_kestrel");
        assert_eq!(
            END_CONVERSATION_ENDED_MESSAGE,
            "Claude ended the conversation. To continue, please start a new session."
        );
    }
}
