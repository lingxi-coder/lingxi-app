//! Prompt-too-long retry: when the server reports `PromptTooLong`, drop oldest
//! API rounds until we've shaved off the suggested token gap with a margin.

use crate::grouping::ApiRoundGroup;
use lingxi_protocol::ConversationMessage;

/// On PTL: drop oldest API rounds until estimated tokens drop by
/// `token_gap + margin`. 20% safety margin per spec §13.7 C5.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::needless_pass_by_value
)]
pub fn truncate_head_for_ptl_retry(
    messages: Vec<ConversationMessage>,
    token_gap: u64,
    _groups: &[ApiRoundGroup],
) -> Result<Vec<ConversationMessage>, &'static str> {
    let target_drop = (token_gap as f64 * 1.2) as u64; // 20% margin (C5)
    let groups = crate::grouping::group_messages_by_api_round(&messages);
    let mut to_drop_end = 0usize;
    let mut dropped = 0u64;
    for g in &groups {
        if dropped >= target_drop {
            break;
        }
        dropped += g.estimated_tokens;
        to_drop_end = g.end;
    }
    if to_drop_end == 0 {
        return Err("could not drop any rounds");
    }
    Ok(messages[to_drop_end..].to_vec())
}
