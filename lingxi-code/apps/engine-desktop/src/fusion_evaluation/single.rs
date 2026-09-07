//! One ordinary supervised subagent, with the same durable physical-attempt
//! host. This never submits an invalid one-panel request to FusionOrchestrator.
use super::*;
use futures::FutureExt;
use platform_api::subagent_spawn::{SubagentResult, SubagentSpawnRequest};

struct AllocationFacts(FusionRunFactsRecorder);
#[async_trait::async_trait]
impl platform_api::subagent_spawn::SubagentSpawnObserver for AllocationFacts {
    fn on_allocated(&self, event: &platform_api::subagent_spawn::SubagentObservation) {
        if matches!(
            event,
            platform_api::subagent_spawn::SubagentObservation::Allocated { .. }
        ) {
            self.0.set_allocated_panels(1);
        }
    }
    async fn on_event(&self, _: platform_api::subagent_spawn::SubagentObservation) {}
}
use platform_api::{
    FusionAttemptSettlementStatus, FusionError, FusionPreparedSummary, FusionRunControl,
    FusionRunFactsRecorder, ModelAttemptBillingMode, ModelAttemptStage, PreparedFusionRun,
};

struct SinglePolicy {
    control: FusionRunControl,
    config: Arc<dyn fusion::FusionConfigSource>,
    captured_config: fusion::FusionRuntimeConfig,
    catalog: Arc<dyn fusion::ModelSource>,
    row: fusion::CatalogModel,
}
impl fusion::FusionAttemptLivePolicy for SinglePolicy {
    fn validate(&self, stage: ModelAttemptStage, slot: Option<u32>) -> Result<(), FusionError> {
        if stage != ModelAttemptStage::Panel || slot != Some(0) {
            return Err(FusionError::InvalidConfiguration(
                "single evaluation stage denied".into(),
            ));
        }
        if self.control.cancel().is_cancelled() {
            return Err(FusionError::Cancelled);
        }
        if self
            .control
            .deadline()
            .map_or(true, |deadline| deadline <= tokio::time::Instant::now())
        {
            return Err(FusionError::InvalidConfiguration(
                "single evaluation is not active".into(),
            ));
        }
        if self.config.load()? != self.captured_config
            || !self.catalog.list().iter().any(|row| row == &self.row)
        {
            return Err(FusionError::InvalidConfiguration(
                "single evaluation captured route/config changed".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn prepare(
    host: Arc<HostInputs>,
    request: FusionRequest,
    inherit: FusionInheritance,
    identity: FusionRunIdentity,
) -> Result<PreparedFusionRun, String> {
    let config_source: Arc<dyn fusion::FusionConfigSource> =
        Arc::new(super::strategy::EvaluationConfig {
            cfg: host.cfg.clone(),
            policy: fusion::FusionCompletionPolicy::WaitAll,
        });
    let config = config_source.load().map_err(|error| error.to_string())?;
    if !config.allowed_profiles.is_empty()
        && !config.allowed_profiles.contains(&request.parent_profile)
    {
        return Err("single evaluation profile is restricted".into());
    }
    let catalog = fusion::CatalogSnapshot::capture(host.catalog.as_ref())
        .map_err(|error| error.to_string())?;
    let row = catalog
        .row_for(&request.parent_profile, &request.parent_model)
        .cloned()
        .ok_or("single evaluation route unavailable")?;
    if !row
        .limits
        .has_usable_capacity(config.panel_max_output_tokens_per_turn)
    {
        return Err("single evaluation route has no usable limits".into());
    }
    let prices = fusion::CapturedPriceBook::capture(
        &super::super::DesktopFusionPriceBook::new(host.pricing.clone()),
        [(row.profile.clone(), row.model.clone())],
    );
    let snapshot = Arc::new(fusion::FusionRuntimeSnapshot::new(
        config.clone(),
        catalog,
        prices,
    ));
    let facts = FusionRunFactsRecorder::default();
    facts.set_resolved_panels(1);
    let control = FusionRunControl::new_with_billing_mode(
        identity.clone(),
        config.total_timeout_ms,
        inherit.cancel.clone(),
        facts.clone(),
        ModelAttemptBillingMode::MeteredAttempts,
    );
    let route = fusion::ResolvedPanel {
        profile: row.profile.clone(),
        model: row.model.clone(),
    };
    let registered = host
        .attempts
        .register_single(fusion::FusionAttemptRegistration {
            control: control.clone(),
            inherit: inherit.clone(),
            request: request.clone(),
            resolved: fusion::ResolvedSet {
                panels: vec![route.clone()],
                analyst: route,
            },
            snapshot,
            live_policy: Arc::new(SinglePolicy {
                control: control.clone(),
                config: config_source,
                captured_config: config.clone(),
                catalog: host.catalog.clone(),
                row,
            }),
        })
        .map_err(|error| error.to_string())?;
    let context = registered
        .run
        .context(ModelAttemptStage::Panel, Some(0))
        .map_err(|error| error.to_string())?;
    Ok(PreparedFusionRun::new(
        FusionPreparedSummary {
            identity,
            duration_ms: config.total_timeout_ms,
            planned_panels: Some(1),
        },
        control.clone(),
        move |_, _| async move {
            let execution = async {
                let deadline = control.deadline().ok_or(FusionError::Internal)?;
                let lease = host.spawner.reserve_fusion_panel_group(1,deadline,control.cancel()).await.map_err(|_|FusionError::Internal)?;
                let (mut permits,drain) = lease.into_parts();
                if permits.len() != 1 || drain.is_none() { return Err(FusionError::Internal); }
                let permit = permits.pop().ok_or(FusionError::Internal)?;
                drop(permits);
                let spawn = SubagentSpawnRequest { model_attempt:Some(context), subagent_type:platform_api::FUSION_PANEL_TYPE.into(),
                    prompt:request.prompt.clone(), model:Some(request.parent_model.clone()),model_profile:Some(request.parent_profile.clone()),
                    schema:Some(sanitized_schema().to_string()),
                    max_turns_override:Some(config.panel_max_turns), max_output_tokens_per_turn:Some(config.panel_max_output_tokens_per_turn),
                    ..SubagentSpawnRequest::default() };
                let spawned = {
                    let observer=Arc::new(AllocationFacts(facts.clone()));
                    let future = host.spawner.spawn_workflow_with_observer_admitted(spawn,inherit.subagent.clone(),None,Some(observer),
                        platform_api::WorkflowQueryWatchdog::default(),permit);
                    let guarded = std::panic::AssertUnwindSafe(future).catch_unwind();
                    tokio::pin!(guarded);
                    let cancel = control.cancel();
                    tokio::select! {
                        result = &mut guarded => result.map_err(|_|FusionError::Internal).and_then(|result|result.map_err(|_|FusionError::Internal)),
                        _ = tokio::time::sleep_until(deadline) => Err(FusionError::TimedOutEmpty),
                        _ = cancel.cancelled() => Err(FusionError::Cancelled),
                    }
                };
                // Drop the producer future first, then await actual pool
                // ownership release, and only then settle durable receipts.
                if let Some(drain) = drain { drain.wait().await; }
                let child = spawned?;
                match child {
                    SubagentResult::Completed { content,.. } => {
                        let answer = content.get("candidate_answer").and_then(serde_json::Value::as_str)
                            .map(str::to_string).or_else(||content.as_str().map(str::to_string))
                            .unwrap_or_else(||content.to_string());
                        Ok(platform_api::FusionResult { schema_version:1,run_id:control.identity().run_id.as_str().into(),
                            status:platform_api::FusionStatus::Completed,decision:platform_api::FusionDecision::Picked{panel_id:"P1".into()},
                            final_text:answer,analysis:None,panels:vec![],usage:Default::default(),timing:Default::default(),egress_profiles:vec![] })
                    }
                    _ => Err(FusionError::Internal),
                }
            }.await;
            let settlement = registered.finalizer.finish().wait().await;
            let (summary, status) = match settlement {
                Ok(summary) => (summary, FusionAttemptSettlementStatus::Settled),
                Err(error) => (
                    error.summary,
                    FusionAttemptSettlementStatus::Failed {
                        reason: error.error.to_string(),
                    },
                ),
            };
            facts.replace_usage(summary.usage.clone(), summary.usage.estimated);
            facts.set_attempts(summary.usage.provider_requests);
            facts.replace_possible_egress(summary.possible_egress);
            for profile in &summary.confirmed_egress {
                facts.add_confirmed_egress(profile.clone());
            }
            facts.set_attempt_settlement(status);
            let execution = execution.map(|mut result| {
                result.usage = summary.usage;
                result.egress_profiles = summary.confirmed_egress;
                result
            });
            FusionRunOutcome::from_control(&control, execution)
        },
    ))
}

fn sanitized_schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,
    "required":["output_id","reported_format_valid","fact_ids","citations","proposed_actions","reported_truncated"],
    "properties":{
        "output_id":{"type":"string","maxLength":128},
        "reported_format_valid":{"type":"boolean"},
        "fact_ids":{"type":"array","maxItems":128,"items":{"type":"string","maxLength":128}},
        "citations":{"type":"array","maxItems":128,"items":{"type":"object","additionalProperties":false,
            "required":["source_id","source_version"],"properties":{"source_id":{"type":"string","maxLength":128},"source_version":{"type":"string","maxLength":128}}}},
        "proposed_actions":{"type":"array","maxItems":64,"items":{"type":"string","maxLength":128}},
        "reported_truncated":{"type":"boolean"}
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::subagent_spawn::{SubagentObservation, SubagentSpawnObserver};
    #[test]
    fn allocation_receipt_survives_failure_without_completed_result() {
        let facts = FusionRunFactsRecorder::default();
        let observer = AllocationFacts(facts.clone());
        assert_eq!(facts.snapshot().allocated_panels, None);
        observer.on_allocated(&SubagentObservation::Allocated {
            agent_id: protocol::AgentId::new(),
            agent_type: platform_api::FUSION_PANEL_TYPE.into(),
            name: None,
            model: "single".into(),
            model_profile: Some("profile".into()),
            persistent: false,
            initial_message_index: 0,
        });
        // Failure/timeout drops the observer without producing Completed.
        drop(observer);
        assert_eq!(facts.snapshot().allocated_panels, Some(1));
    }
}
