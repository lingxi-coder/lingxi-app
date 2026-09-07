use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Mutex;

#[derive(Default)]
struct Probe {
    begins: AtomicUsize,
    markers: AtomicUsize,
    drops: AtomicUsize,
    finishes: AtomicUsize,
    waits: AtomicUsize,
    reject_begin: AtomicBool,
    reject_marker: AtomicBool,
    reject_wait: AtomicBool,
    usage: Mutex<Vec<(Usage, ModelAttemptUsageCompleteness)>>,
}
struct Hooks(Arc<Probe>);
#[async_trait]
impl ModelAttemptHooks for Hooks {
    async fn begin(
        &self,
        _: &ModelAttemptContext,
        _: &LlmRequest,
        _: &PreparedLlmCall,
    ) -> Result<Box<dyn ModelAttemptLease>, LlmError> {
        self.0.begins.fetch_add(1, Ordering::SeqCst);
        if self.0.reject_begin.load(Ordering::SeqCst) {
            return Err(refused("unknown registration"));
        }
        Ok(Box::new(Lease(self.0.clone())))
    }
}
struct Lease(Arc<Probe>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl ModelAttemptLease for Lease {
    fn mark_dispatched(&mut self) -> Result<(), LlmError> {
        self.0.markers.fetch_add(1, Ordering::SeqCst);
        if self.0.reject_marker.load(Ordering::SeqCst) {
            Err(refused("inner marker denied"))
        } else {
            Ok(())
        }
    }
    fn observe_usage(&mut self, usage: &Usage, completeness: ModelAttemptUsageCompleteness) {
        self.0
            .usage
            .lock()
            .unwrap()
            .push((usage.clone(), completeness));
    }
    fn finish(self: Box<Self>) -> Box<dyn ModelAttemptSettlement> {
        self.0.finishes.fetch_add(1, Ordering::SeqCst);
        Box::new(Settlement(self.0.clone()))
    }
}
struct Settlement(Arc<Probe>);
#[async_trait]
impl ModelAttemptSettlement for Settlement {
    async fn wait(self: Box<Self>) -> Result<(), LlmError> {
        self.0.waits.fetch_add(1, Ordering::SeqCst);
        if self.0.reject_wait.load(Ordering::SeqCst) {
            Err(refused("durable receipt failed"))
        } else {
            Ok(())
        }
    }
}

async fn prepared() -> (ModelAttemptContext, LlmRequest, PreparedLlmCall) {
    // Codec-only preparation: no credential provider or transport is installed.
    let client = llm_client::DefaultLlmClient::from_config(llm_client::ClientConfig {
        providers: vec![llm_client::ProviderProfile {
            provider_id: llm_client::ProviderId::OpenAI,
            profile_name: "offline".into(),
            base_url: "https://unused.invalid/v1".into(),
            protocol: llm_client::ProtocolFamily::OpenAiChat,
            auth: llm_client::AuthStrategy::None,
            credential: llm_client::CredentialConfig::None,
            models: vec![llm_client::ModelProfile {
                display_model: "fake".into(),
                request_model: "fake".into(),
                billing_model: "fake".into(),
                aliases: vec![],
                description: None,
                metadata: Default::default(),
                capabilities: Default::default(),
            }],
            pricing: Default::default(),
            signing: None,
            azure: None,
            supports_websockets: false,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
            vision_delegate: None,
        }],
    })
    .unwrap();
    let run = platform_api::ModelAttemptRun::new(Arc::new(()));
    let context = run
        .context(platform_api::ModelAttemptStage::Panel, Some(0))
        .unwrap();
    let mut request = LlmRequest::new("fake").with_user_text("offline quota fixture");
    request.profile = Some("offline".into());
    request.model_attempt = Some(context.clone());
    let prepared = client.prepare(&request).await.unwrap();
    (context, request, prepared)
}

#[test]
fn rejects_missing_capacity_without_touching_host() {
    let probe = Arc::new(Probe::default());
    for cap in [0, MAX_MODEL_CALLS + 1, u32::MAX] {
        assert!(AttemptQuota::new(Arc::new(Hooks(probe.clone())), cap).is_err());
    }
    assert_eq!(probe.begins.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_quota_one_delegates_exactly_one_marker() {
    let (context, request, prepared) = prepared().await;
    let probe = Arc::new(Probe::default());
    let quota = AttemptQuota::new(Arc::new(Hooks(probe.clone())), 1).unwrap();
    let mut leases = Vec::new();
    for _ in 0..16 {
        leases.push(quota.begin(&context, &request, &prepared).await.unwrap());
    }
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let threads = leases
        .into_iter()
        .map(|mut lease| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                lease.mark_dispatched().is_ok()
            })
        })
        .collect::<Vec<_>>();
    let successes = threads
        .into_iter()
        .map(|thread| usize::from(thread.join().unwrap()))
        .sum::<usize>();
    assert_eq!(successes, 1);
    assert_eq!(quota.claimed(), 1);
    assert_eq!(probe.markers.load(Ordering::SeqCst), 1);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 16);
}

#[tokio::test]
async fn rejected_begin_uses_host_authority_and_never_claims_slot() {
    let (context, request, prepared) = prepared().await;
    let probe = Arc::new(Probe::default());
    probe.reject_begin.store(true, Ordering::SeqCst);
    let quota = AttemptQuota::new(Arc::new(Hooks(probe.clone())), 1).unwrap();
    assert!(quota.begin(&context, &request, &prepared).await.is_err());
    assert_eq!(probe.begins.load(Ordering::SeqCst), 1);
    assert_eq!(quota.claimed(), 0);
    assert_eq!(probe.markers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_or_repeated_marker_never_refunds_or_delegates_twice() {
    let (context, request, prepared) = prepared().await;
    let probe = Arc::new(Probe::default());
    probe.reject_marker.store(true, Ordering::SeqCst);
    let quota = AttemptQuota::new(Arc::new(Hooks(probe.clone())), 1).unwrap();
    let mut first = quota.begin(&context, &request, &prepared).await.unwrap();
    assert!(first.mark_dispatched().is_err());
    probe.reject_marker.store(false, Ordering::SeqCst);
    assert!(first.mark_dispatched().is_err());
    drop(first);
    let mut retry = quota.begin(&context, &request, &prepared).await.unwrap();
    assert!(retry.mark_dispatched().is_err());
    assert!(retry.mark_dispatched().is_err());
    assert_eq!(quota.claimed(), 1);
    assert_eq!(probe.markers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successful_marker_is_single_use_and_usage_settlement_pass_through() {
    let (context, request, prepared) = prepared().await;
    let probe = Arc::new(Probe::default());
    let quota = AttemptQuota::new(Arc::new(Hooks(probe.clone())), 2).unwrap();
    let mut lease = quota.begin(&context, &request, &prepared).await.unwrap();
    lease.mark_dispatched().unwrap();
    assert!(lease.mark_dispatched().is_err());
    let mut usage = Usage::default();
    usage.billable_tokens.input = 17;
    usage.billable_tokens.output = 9;
    usage.provider_reported_total_tokens = Some(26);
    lease.observe_usage(&usage, ModelAttemptUsageCompleteness::Partial);
    lease.observe_usage(&usage, ModelAttemptUsageCompleteness::Complete);
    let settlement = lease.finish();
    assert_eq!(probe.finishes.load(Ordering::SeqCst), 1);
    assert_eq!(probe.waits.load(Ordering::SeqCst), 0);
    probe.reject_wait.store(true, Ordering::SeqCst);
    assert!(settlement.wait().await.is_err());
    assert_eq!(probe.waits.load(Ordering::SeqCst), 1);
    assert_eq!(quota.claimed(), 1);
    let observations = probe.usage.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].0, usage);
    assert_eq!(observations[1].0, usage);
    assert_eq!(observations[0].1, ModelAttemptUsageCompleteness::Partial);
    assert_eq!(observations[1].1, ModelAttemptUsageCompleteness::Complete);
}

#[tokio::test]
async fn undispatched_drop_delegates_cleanup_without_spending_call_quota() {
    let (context, request, prepared) = prepared().await;
    let probe = Arc::new(Probe::default());
    let quota = AttemptQuota::new(Arc::new(Hooks(probe.clone())), 1).unwrap();
    let lease = quota.begin(&context, &request, &prepared).await.unwrap();
    drop(lease);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
    assert_eq!(quota.claimed(), 0);
}

#[tokio::test]
async fn stages_and_new_logical_calls_share_invocation_ceiling() {
    let (_, mut request, prepared) = prepared().await;
    let probe = Arc::new(Probe::default());
    let quota = AttemptQuota::new(Arc::new(Hooks(probe.clone())), 2).unwrap();
    let run = platform_api::ModelAttemptRun::new(Arc::new(()));
    for (stage, slot, allowed) in [
        (platform_api::ModelAttemptStage::Panel, Some(0), true),
        (platform_api::ModelAttemptStage::Analyst, None, true),
        (platform_api::ModelAttemptStage::Synthesis, None, false),
    ] {
        let context = run.context(stage, slot).unwrap();
        request.model_attempt = Some(context.clone());
        let mut lease = quota.begin(&context, &request, &prepared).await.unwrap();
        assert_eq!(lease.mark_dispatched().is_ok(), allowed);
        // Settlement ownership transfers even when the marker was denied.
        drop(lease.finish());
    }
    assert_eq!(quota.claimed(), 2);
    assert_eq!(probe.markers.load(Ordering::SeqCst), 2);
    assert_eq!(probe.finishes.load(Ordering::SeqCst), 3);
    assert_eq!(probe.waits.load(Ordering::SeqCst), 0);
}
