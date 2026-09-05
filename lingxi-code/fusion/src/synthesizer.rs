//! Merge synthesizer: exactly one parent-model side query.

use crate::config::FusionRuntimeConfig;
use crate::panel::PanelInternal;
use platform_api::subagent_output_guard::sanitize_blocks;
use platform_api::{FusionAnalysis, FusionRequest};
use protocol::{ConversationMessage, MessageId};
use sidequery::{
    QuerySource, SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

/// Synthesizer-stage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthError {
    /// Provider / protocol failure.
    Failed,
    /// Idle / total timeout.
    TimedOut,
}

/// Run the parent-model merge. Caller must invoke this at most once.
pub async fn synthesize(
    client: Arc<dyn SideQueryClient>,
    config: &FusionRuntimeConfig,
    request: &FusionRequest,
    analysis: &FusionAnalysis,
    panels: &[PanelInternal],
) -> Result<(String, cost::Usage), SynthError> {
    let mut reports = Vec::new();
    for panel in panels {
        if let Some(report) = &panel.report {
            reports.push(serde_json::json!({
                "panel_id": panel.anonymous_id,
                "candidate_answer": report.candidate_answer,
                "summary": report.summary,
            }));
        }
    }
    let user = serde_json::json!({
        "task": request.prompt,
        "analysis": analysis,
        "panels": reports,
        "instruction": "Synthesize one improved answer. Do not mention panels, providers, or models."
    })
    .to_string();
    let req = SideQueryRequest {
        model: request.parent_model.clone(),
        profile: Some(request.parent_profile.clone()),
        system_prompt: Some(
            "You are the Fusion synthesizer. Merge the panel answers into one response.".into(),
        ),
        messages: vec![ConversationMessage::user(MessageId::new(), user)],
        tools: Vec::new(),
        tool_choice: None,
        output_format: None,
        max_tokens: config.synthesizer_max_output_tokens,
        max_retries: 0,
        temperature: None,
        thinking: None,
        effort: None,
        stop_sequences: Vec::new(),
        query_source: QuerySource::FusionSynthesizer,
        skip_system_prompt_prefix: true,
    };
    let outcome = timeout(
        Duration::from_millis(config.synthesizer_timeout_ms),
        client.query(req),
    )
    .await;
    match outcome {
        Err(_) => Err(SynthError::TimedOut),
        Ok(Err(
            SideQueryError::Api(_)
            | SideQueryError::InvalidResponse(_)
            | SideQueryError::Partial { .. }
            | SideQueryError::StructuredOutputUnsupported,
        )) => Err(SynthError::Failed),
        Ok(Ok(SideQueryResponse { text, usage, .. })) => {
            let raw = text.unwrap_or_default();
            if raw.trim().is_empty() {
                return Err(SynthError::Failed);
            }
            let sanitized = sanitize_blocks(&[raw.replace('\0', "")]).content.join("");
            Ok((sanitized, usage))
        }
    }
}
