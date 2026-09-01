//! Analyst side query (strict JSON, no tools).

use crate::config::FusionRuntimeConfig;
use crate::decision::scores_match_request;
use crate::model_resolver::ResolvedPanel;
use crate::panel::PanelInternal;
use platform_api::{FusionAnalysis, FusionError, FusionRequest};
use protocol::{ConversationMessage, MessageId};
use serde_json::Value;
use sidequery::{
    QuerySource, SideQueryClient, SideQueryError, StrictStructuredQueryRequest,
    StrictStructuredQueryResponse,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

/// Analyst-stage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalystError {
    /// JSON / schema failed after retries.
    ParseFailed,
    /// Provider cannot constrain JSON.
    Unsupported,
    /// Transport / timeout.
    Failed(String),
}

impl From<AnalystError> for FusionError {
    fn from(err: AnalystError) -> Self {
        match err {
            AnalystError::Unsupported => Self::StructuredOutputUnsupported,
            AnalystError::ParseFailed | AnalystError::Failed(_) => Self::Internal,
        }
    }
}

/// Call the analyst with one protocol retry.
pub async fn analyze(
    client: Arc<dyn SideQueryClient>,
    config: &FusionRuntimeConfig,
    request: &FusionRequest,
    analyst: &ResolvedPanel,
    panels: &[PanelInternal],
) -> Result<(FusionAnalysis, cost::Usage, u32), AnalystError> {
    let schema = analyst_json_schema();
    let user = analyst_user_message(request, panels);
    let attempts = 1 + u32::from(config.analysis_protocol_retries);
    let mut calls = 0_u32;
    for attempt in 0..attempts {
        let req = StrictStructuredQueryRequest {
            model: analyst.model.clone(),
            profile: Some(analyst.profile.clone()),
            system_prompt: Some(analyst_system_prompt().into()),
            messages: vec![ConversationMessage::user(MessageId::new(), user.clone())],
            schema: schema.clone(),
            max_tokens: config.analyst_max_output_tokens,
            temperature: Some(0.0),
            query_source: QuerySource::FusionAnalyst,
            skip_system_prompt_prefix: true,
        };
        let outcome = timeout(
            Duration::from_millis(config.analyst_timeout_ms),
            client.query_json_schema(req),
        )
        .await;
        calls += 1;
        match outcome {
            Err(_) => {
                if attempt + 1 == attempts {
                    return Err(AnalystError::Failed("timeout".into()));
                }
            }
            Ok(Err(SideQueryError::StructuredOutputUnsupported)) => {
                return Err(AnalystError::Unsupported);
            }
            Ok(Err(SideQueryError::InvalidResponse(_))) => {
                if attempt + 1 == attempts {
                    return Err(AnalystError::ParseFailed);
                }
            }
            Ok(Err(other)) => {
                if attempt + 1 == attempts {
                    return Err(AnalystError::Failed(other.to_string()));
                }
            }
            Ok(Ok(StrictStructuredQueryResponse { value, usage, .. })) => {
                match decode_analysis(&value, request, panels) {
                    Ok(analysis) => return Ok((analysis, usage, calls)),
                    Err(_) if attempt + 1 == attempts => return Err(AnalystError::ParseFailed),
                    Err(_) => {}
                }
            }
        }
    }
    Err(AnalystError::ParseFailed)
}

fn decode_analysis(
    value: &Value,
    request: &FusionRequest,
    panels: &[PanelInternal],
) -> Result<FusionAnalysis, ()> {
    let analysis: FusionAnalysis = serde_json::from_value(value.clone()).map_err(|_| ())?;
    if !scores_match_request(&analysis, panels, &request.dimensions) {
        return Err(());
    }
    Ok(analysis)
}

fn analyst_system_prompt() -> &'static str {
    "You are the Fusion analyst. Score anonymized panel reports. Do not identify \
providers or models. Recommend pick, merge, or needs_parent. Output JSON only."
}

fn analyst_user_message(request: &FusionRequest, panels: &[PanelInternal]) -> String {
    let mut reports = Vec::new();
    let mut successful: Vec<&PanelInternal> = panels
        .iter()
        .filter(|panel| panel.report.is_some())
        .collect();
    successful.sort_by(|a, b| a.anonymous_id.cmp(&b.anonymous_id));
    for panel in successful {
        if let Some(report) = &panel.report {
            reports.push(serde_json::json!({
                "panel_id": panel.anonymous_id,
                "report": report,
            }));
        }
    }
    serde_json::json!({
        "task": request.prompt,
        "dimensions": request.dimensions,
        "panels": reports,
    })
    .to_string()
}

fn analyst_json_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "required": ["confidence", "recommendation"],
        "properties": {
            "schema_version": { "type": "integer" },
            "consensus": { "type": "array", "items": { "type": "string" } },
            "contradictions": { "type": "array" },
            "unique_insights": { "type": "array" },
            "coverage_gaps": { "type": "array", "items": { "type": "string" } },
            "scores": { "type": "object" },
            "confidence": { "type": "integer", "minimum": 0, "maximum": 100 },
            "recommendation": { "type": "object" }
        }
    })
}
