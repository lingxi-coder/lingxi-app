//! Explicit experimental decision intervention, never installed in normal boot.
use super::evaluation::harness::ComparisonMode;
use async_trait::async_trait;
use platform_api::{FusionAnalysis, FusionRecommendation};
use sidequery::{
    CanonicalSideQueryRequest, SideQueryClient, SideQueryError, SideQueryEstimate,
    SideQueryRequest, SideQueryResponse, StrictStructuredQueryRequest,
    StrictStructuredQueryResponse,
};
use std::sync::{Arc, Mutex};

pub(super) struct EvaluationConfig {
    pub cfg: crate::DesktopConfig,
    pub policy: fusion::FusionCompletionPolicy,
}
impl fusion::FusionConfigSource for EvaluationConfig {
    fn load(&self) -> Result<fusion::FusionRuntimeConfig, platform_api::FusionError> {
        let mut config = crate::desktop_fusion_runtime_config(&self.cfg)?;
        config.completion_policy = self.policy;
        Ok(config)
    }
}
pub(super) struct EvaluationStrategy {
    inner: Arc<dyn SideQueryClient>,
    mode: ComparisonMode,
    natural: Mutex<Vec<FusionRecommendation>>,
}
impl EvaluationStrategy {
    pub(super) fn new(inner: Arc<dyn SideQueryClient>, mode: ComparisonMode) -> Self {
        Self {
            inner,
            mode,
            natural: Mutex::new(Vec::new()),
        }
    }
    pub(super) fn natural(&self) -> Vec<FusionRecommendation> {
        self.natural.lock().unwrap().clone()
    }
}

fn intervene(analysis: &mut FusionAnalysis, mode: ComparisonMode) {
    // Confidence/contradictions/scores stay untouched. Ordinary host validation
    // and its critical-risk/low-confidence Merge refusal remain authoritative.
    analysis.recommendation = match mode {
        ComparisonMode::PanelMerge => FusionRecommendation::Merge {
            reason: "evaluation intervention: merge".into(),
        },
        ComparisonMode::PanelPick => {
            let winner = analysis.scores.iter().max_by(|(a_id, a), (b_id, b)| {
                let sum = |row: &std::collections::BTreeMap<String, u8>| {
                    row.values().map(|value| u64::from(*value)).sum::<u64>()
                };
                sum(a).cmp(&sum(b)).then_with(|| b_id.cmp(a_id))
            });
            match winner {
                Some((id, _)) => FusionRecommendation::Pick {
                    panel_id: id.clone(),
                    reason: "evaluation intervention: maximum score sum, anonymous-ID tie break"
                        .into(),
                },
                None => return,
            }
        }
        ComparisonMode::Single => return,
    };
}

#[async_trait]
impl SideQueryClient for EvaluationStrategy {
    fn last_retry_count(&self) -> u32 {
        self.inner.last_retry_count()
    }
    fn has_canonical_estimator(&self) -> bool {
        self.inner.has_canonical_estimator()
    }
    fn estimate_request(
        &self,
        request: CanonicalSideQueryRequest,
    ) -> Result<SideQueryEstimate, SideQueryError> {
        self.inner.estimate_request(request)
    }
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError> {
        self.inner.query(request).await
    }
    async fn query_json_schema(
        &self,
        request: StrictStructuredQueryRequest,
    ) -> Result<StrictStructuredQueryResponse, SideQueryError> {
        let mut response = self.inner.query_json_schema(request).await?;
        // If malformed, leave it to the normal parser/retry policy. Known usage
        // remains attached to the response and the durable physical attempt.
        if let Ok(mut analysis) = serde_json::from_value::<FusionAnalysis>(response.value.clone()) {
            self.natural
                .lock()
                .unwrap()
                .push(analysis.recommendation.clone());
            intervene(&mut analysis, self.mode);
            if let Ok(value) = serde_json::to_value(analysis) {
                response.value = value;
            }
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decision_intervention_preserves_safety_fields_and_has_stable_ties() {
        let mut analysis: FusionAnalysis = serde_json::from_value(serde_json::json!({
            "schema_version":1,"consensus":[],"contradictions":[],"unique_insights":[],"coverage_gaps":[],
            "scores":{"P2":{"safety":80},"P1":{"safety":80}},"confidence":20,
            "recommendation":{"type":"needs_parent","reason":"natural uncertainty"}
        })).unwrap();
        let scores = analysis.scores.clone();
        intervene(&mut analysis, ComparisonMode::PanelPick);
        assert!(
            matches!(&analysis.recommendation,FusionRecommendation::Pick{panel_id,..} if panel_id=="P1")
        );
        intervene(&mut analysis, ComparisonMode::PanelMerge);
        assert_eq!(analysis.confidence, 20);
        assert_eq!(analysis.scores, scores);
        assert!(matches!(
            analysis.recommendation,
            FusionRecommendation::Merge { .. }
        ));
    }
}
