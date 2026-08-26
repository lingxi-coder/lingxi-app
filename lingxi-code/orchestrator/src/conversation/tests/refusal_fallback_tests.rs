use super::*;
use crate::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use crate::OrchestratorConfig;
use protocol::SessionId;
use std::path::PathBuf;
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

/// Build an orchestrator whose `refusal_fallback_model` is `cfg_fallback`,
/// returning the orchestrator + a clone of its `MockOutputStream` (shares the
/// same event buffer) so the test can inspect emitted warnings.
fn orch_with_refusal_fallback(
    cfg_fallback: Option<&str>,
) -> (ConversationOrchestrator, MockOutputStream) {
    let out = MockOutputStream::new();
    let cfg = OrchestratorConfig {
        refusal_fallback_model: cfg_fallback.map(str::to_string),
        ..OrchestratorConfig::default()
    };
    let orch = ConversationOrchestrator::new(
        cfg,
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(out.clone()),
        Arc::new(StaticMemoryProvider::with_files(vec![])),
        PathBuf::from("/work/repo"),
    );
    (orch, out)
}

/// Build an orchestrator over a multi-hop refusal CASCADE.
fn orch_with_refusal_chain(chain: &[&str]) -> (ConversationOrchestrator, MockOutputStream) {
    let out = MockOutputStream::new();
    let cfg = OrchestratorConfig {
        refusal_fallback_chain: chain.iter().map(|s| (*s).to_string()).collect(),
        ..OrchestratorConfig::default()
    };
    let orch = ConversationOrchestrator::new(
        cfg,
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(out.clone()),
        Arc::new(StaticMemoryProvider::with_files(vec![])),
        PathBuf::from("/work/repo"),
    );
    (orch, out)
}

/// The point of the cascade: successive refusals walk the chain instead of
/// stopping after one hop. The once-per-session latch does NOT apply to a
/// multi-hop chain — the chain itself is the bound.
#[tokio::test]
async fn a_cascade_walks_each_hop_in_order() {
    let (orch, _out) = orch_with_refusal_chain(&["hop-one", "hop-two"]);

    assert!(orch.maybe_swap_to_refusal_fallback().await, "first hop");
    assert_eq!(orch.session.lock().await.model, "hop-one");

    assert!(orch.maybe_swap_to_refusal_fallback().await, "second hop");
    assert_eq!(orch.session.lock().await.model, "hop-two");

    // Chain exhausted: every stage has been tried, so the walk declines
    // rather than looping back onto a model that already refused.
    assert!(
        !orch.maybe_swap_to_refusal_fallback().await,
        "an exhausted chain declines"
    );
    assert_eq!(orch.session.lock().await.model, "hop-two");
}

/// END TO END, and the whole point of steps 2-4: a multi-hop cascade emits
/// ONE notice, naming where the session ended up — not one notice per hop
/// announcing a model the cascade has already left.
#[tokio::test]
async fn a_cascade_emits_one_collapsed_notice_not_one_per_hop() {
    let (orch, out) = orch_with_refusal_chain(&["hop-one", "hop-two", "hop-three"]);

    assert!(orch.maybe_swap_to_refusal_fallback().await);
    assert!(
        out.text_events().await.is_empty(),
        "an intermediate hop is HELD, not announced"
    );

    assert!(orch.maybe_swap_to_refusal_fallback().await);
    assert!(
        out.text_events().await.is_empty(),
        "the second hop is still intermediate"
    );

    // The last hop has nothing remaining, so the episode settles and the
    // accumulated notice goes out — once.
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    let texts = out.text_events().await;
    assert_eq!(texts.len(), 1, "exactly one notice for the whole cascade");
    assert!(
        texts[0].contains("hop-three"),
        "it names where the session ENDED UP: {}",
        texts[0]
    );
    assert!(
        !texts[0].contains("hop-one") && !texts[0].contains("hop-two"),
        "and not the hops it passed through: {}",
        texts[0]
    );
}

/// A single-hop fallback announces immediately — there is no later hop that
/// could withdraw it, so holding it would just delay the user's notice.
#[tokio::test]
async fn a_single_hop_announces_immediately() {
    let (orch, out) = orch_with_refusal_chain(&["only"]);
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    let texts = out.text_events().await;
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("only"), "{}", texts[0]);
}

/// A ONE-element chain behaves exactly like the historical single
/// `refusal_fallback_model`, latch included — the default path must be
/// unchanged by the cascade's arrival.
#[tokio::test]
async fn a_single_hop_chain_still_latches_once_per_session() {
    let (orch, _out) = orch_with_refusal_chain(&["only"]);
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    assert!(
        orch.model_runtime
            .refusal_fallback_latched
            .load(std::sync::atomic::Ordering::SeqCst),
        "a single-hop chain still sets the latch"
    );
    assert!(
        !orch.maybe_swap_to_refusal_fallback().await,
        "and the latch stops the second attempt"
    );
}

/// An empty chain defers to `refusal_fallback_model`, so a config written
/// before the cascade existed keeps working and the two never disagree.
#[tokio::test]
async fn an_empty_chain_defers_to_the_single_model_field() {
    let (orch, _out) = orch_with_refusal_fallback(Some("legacy-model"));
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    assert_eq!(orch.session.lock().await.model, "legacy-model");
}

/// The cascade's tried-models list resets with the latch, so a cleared
/// session can walk the same chain again from its first hop.
#[tokio::test]
async fn clearing_the_session_lets_the_cascade_start_over() {
    let (orch, _out) = orch_with_refusal_chain(&["hop-one", "hop-two"]);
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    assert!(!orch.maybe_swap_to_refusal_fallback().await);

    orch.model_runtime.refusal_tried_models.lock().await.clear();
    orch.model_runtime
        .refusal_fallback_latched
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(
        orch.maybe_swap_to_refusal_fallback().await,
        "a reset session walks the chain again"
    );
    assert_eq!(orch.session.lock().await.model, "hop-one");
}

#[tokio::test]
async fn no_fallback_configured_is_a_strict_noop() {
    let (orch, out) = orch_with_refusal_fallback(None);
    let before = orch.session.lock().await.model.clone();
    assert!(
        !orch.maybe_swap_to_refusal_fallback().await,
        "no fallback → false"
    );
    assert_eq!(
        orch.session.lock().await.model,
        before,
        "model must NOT change"
    );
    assert!(out.text_events().await.is_empty(), "no warning emitted");
    assert!(
        !orch
            .model_runtime
            .refusal_fallback_latched
            .load(std::sync::atomic::Ordering::SeqCst),
        "latch must stay unset when nothing was configured"
    );
}

#[tokio::test]
async fn swaps_once_then_latches() {
    let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
    // First refusal → swap.
    assert!(
        orch.maybe_swap_to_refusal_fallback().await,
        "first call swaps"
    );
    assert_eq!(
        orch.session.lock().await.model,
        "claude-sonnet-4-6",
        "session model must be swapped to the fallback"
    );
    assert!(
        orch.session.lock().await.model_profile.is_none(),
        "fallback model carries no provider profile"
    );
    // Second refusal → latched, no re-swap, terminal behavior preserved.
    assert!(
        !orch.maybe_swap_to_refusal_fallback().await,
        "second call is latched (no re-swap)"
    );
    assert_eq!(
        orch.session.lock().await.model,
        "claude-sonnet-4-6",
        "model unchanged on the second (latched) call"
    );
}

#[tokio::test]
async fn emits_byte_exact_warning_on_swap() {
    let (orch, out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
    assert!(orch.maybe_swap_to_refusal_fallback().await);
    let texts = out.text_events().await;
    assert_eq!(texts.len(), 1, "exactly one warning emitted");
    // Byte-exact reproduction of 2.1.206 `VPn` for category == "other":
    // the generic `$7m`/`hmi` prefix, then "Switched to {Mf(fallback)}" — the
    // fallback's MARKETING NAME ("Sonnet 4.6"), not the raw id — then `bxr`.
    assert_eq!(
        texts[0],
        "This model's safeguards flagged this message. \
This sometimes happens with safe, normal conversations. Switched to Sonnet 4.6. \
Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
    );
}

#[tokio::test]
async fn clear_session_resets_refusal_fallback_latch() {
    let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
    assert!(
        orch.maybe_swap_to_refusal_fallback().await,
        "first session swaps"
    );

    <ConversationOrchestrator as traits::OrchestratorHandle>::clear_session(&orch)
        .await
        .expect("clear_session succeeds");

    assert!(
        !orch
            .model_runtime
            .refusal_fallback_latched
            .load(std::sync::atomic::Ordering::SeqCst),
        "clear_session must reset the per-session refusal fallback latch"
    );
    assert!(
        orch.maybe_swap_to_refusal_fallback().await,
        "a fresh cleared session must be able to swap on its first refusal"
    );
}

#[tokio::test]
async fn resume_session_resets_refusal_fallback_latch() {
    let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
    assert!(
        orch.maybe_swap_to_refusal_fallback().await,
        "first session swaps"
    );

    <ConversationOrchestrator as traits::OrchestratorHandle>::resume_session(
        &orch,
        SessionId::new(),
        vec![],
        None,
        None,
        traits::ResumeRuntimeSnapshot::default(),
    )
    .await
    .expect("resume_session succeeds");

    assert!(
        !orch
            .model_runtime
            .refusal_fallback_latched
            .load(std::sync::atomic::Ordering::SeqCst),
        "resume_session must reset the per-session refusal fallback latch"
    );
    assert!(
        orch.maybe_swap_to_refusal_fallback().await,
        "a resumed session must be able to swap on its first refusal"
    );
}
