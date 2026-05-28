//! `/cost` — snapshot the orchestrator's cost via
//! [`OrchestratorHandle::snapshot_cost`].
//!
//! Locked display template (`LingXi` UX, M5-11 T0 step 2 L1):
//!   `"Cost: ${total:.4} ({calls} calls, {input}+{output} tokens, {dur} session time)"`
//! Failure prefix: `"Could not snapshot cost: "`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md`
//! Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;
use std::time::Duration;

/// `/cost` handler — renders the cumulative cost snapshot.
#[derive(Clone)]
pub struct CostHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl CostHandler {
    /// Construct a `CostHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for CostHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit_command_started(cmd_evt::COST_STARTED);
        let cost = self.handle.snapshot_cost().await;
        let dur = format_duration(cost.session_duration);
        let display = format!(
            "Cost: ${:.4} ({} calls, {}+{} tokens, {} session time)",
            cost.total_usd, cost.api_calls, cost.input_tokens, cost.output_tokens, dur
        );
        lingxi_telemetry::emit_command_completed(cmd_evt::COST_COMPLETED, "");
        CommandResult::Done {
            display: Some(display),
        }
    }
    fn name(&self) -> &str {
        "cost"
    }
    fn description(&self) -> &str {
        core_description("cost")
    }
}

/// Format an elapsed `Duration` as `"{H}h {M}m {S}s"`, falling back to
/// `"{M}m {S}s"` or `"{S}s"` when leading components are zero.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use lingxi_traits::CostSnapshot;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "cost".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn success_short_duration() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let snap = CostSnapshot {
            total_usd: 0.0042,
            input_tokens: 1500,
            output_tokens: 230,
            api_calls: 3,
            session_duration: Duration::from_secs(45),
            ..CostSnapshot::default()
        };
        mock.set_cost_snapshot(snap);
        let h = CostHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(
                    s,
                    "Cost: $0.0042 (3 calls, 1500+230 tokens, 45s session time)"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn success_hour_duration() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_cost_snapshot(CostSnapshot {
            total_usd: 1.2345,
            input_tokens: 50_000,
            output_tokens: 10_000,
            api_calls: 42,
            session_duration: Duration::from_secs(3 * 3600 + 25 * 60 + 17),
            ..CostSnapshot::default()
        });
        let h = CostHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(
                s,
                "Cost: $1.2345 (42 calls, 50000+10000 tokens, 3h 25m 17s session time)"
            );
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn zero_cost_zero_calls_zero_duration() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = CostHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Cost: $0.0000 (0 calls, 0+0 tokens, 0s session time)");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = CostHandler::new(mock);
        assert_eq!(h.name(), "cost");
        assert_eq!(
            h.description(),
            "Show total cost and duration of the current session"
        );
    }

    // M6-06 T7: end-to-end smoke against a real ConversationOrchestrator
    // with a wired CostTracker — confirms the /cost slash command reflects
    // real numbers (not the M5-10 zero stub).
    fn end_turn_response_with_usage(
        input: u64,
        output: u64,
    ) -> lingxi_api_client::types::MessageResponse {
        lingxi_api_client::types::MessageResponse {
            id: "msg_mock".to_string(),
            model: "claude-opus-4-6".to_string(),
            content: Vec::new(),
            stop_reason: Some("end_turn".to_string()),
            usage: lingxi_api_client::types::UsageApi {
                input_tokens: input,
                output_tokens: output,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
        }
    }

    #[tokio::test]
    async fn real_orchestrator_renders_non_zero_cost() {
        use lingxi_cost::pricing::PricingCatalog;
        use lingxi_cost::CostTracker;
        use lingxi_orchestrator::test_support::{
            noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
            StaticMemoryProvider,
        };
        use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
        use lingxi_protocol::SessionId;
        use tokio::sync::mpsc;

        let api = Arc::new(MockApiClient::new(vec![end_turn_response_with_usage(
            1_000, 500,
        )]));
        let (tx, _rx) = mpsc::channel(64);
        let tracker = Arc::new(CostTracker::new(
            SessionId::new(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ));

        let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
        let hooks = noop_hook_executor();
        let perms = Arc::new(NoOpPermissionGate);
        let output = Arc::new(MockOutputStream::new());
        let memory = Arc::new(StaticMemoryProvider::empty());

        let mut cfg = OrchestratorConfig::default();
        cfg.model = "claude-opus-4-6".into();

        let orch = Arc::new(
            ConversationOrchestrator::new(
                cfg,
                api,
                tools,
                hooks,
                perms,
                output,
                memory,
                std::env::temp_dir(),
            )
            .with_cost_tracker(tracker),
        );
        orch.run_turn("hi").await.unwrap();

        let handle: Arc<dyn lingxi_traits::OrchestratorHandle> = orch.clone();
        let h = CostHandler::new(handle);

        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                // 1000*5000 + 500*25000 = 17_500_000 nano-USD = $0.0175
                assert!(
                    s.starts_with("Cost: $0.0175 (1 calls, 1000+500 tokens, "),
                    "got: {s}"
                );
                // session_duration is non-deterministic; just assert the
                // suffix shape ends with " session time)".
                assert!(s.ends_with(" session time)"), "got: {s}");
                // No-op rendering must NOT match: $0.0000 + 0 calls.
                assert!(!s.contains("$0.0000 (0 calls"), "stub leaked: {s}");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cost_command_matches_snapshot_cost_value() {
        // Locks the invariant that the slash command and the trait method
        // read from the same source. (`/cost` already calls `snapshot_cost`
        // — this test wires real numbers and confirms identity.)
        use lingxi_cost::pricing::PricingCatalog;
        use lingxi_cost::CostTracker;
        use lingxi_orchestrator::test_support::{
            noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
            StaticMemoryProvider,
        };
        use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
        use lingxi_protocol::SessionId;
        use tokio::sync::mpsc;

        let api = Arc::new(MockApiClient::new(vec![end_turn_response_with_usage(
            2_000, 1_000,
        )]));
        let (tx, _rx) = mpsc::channel(64);
        let tracker = Arc::new(CostTracker::new(
            SessionId::new(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ));
        let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());
        let hooks = noop_hook_executor();
        let perms = Arc::new(NoOpPermissionGate);
        let output = Arc::new(MockOutputStream::new());
        let memory = Arc::new(StaticMemoryProvider::empty());
        let mut cfg = OrchestratorConfig::default();
        cfg.model = "claude-opus-4-6".into();
        let orch = Arc::new(
            ConversationOrchestrator::new(
                cfg,
                api,
                tools,
                hooks,
                perms,
                output,
                memory,
                std::env::temp_dir(),
            )
            .with_cost_tracker(tracker),
        );
        orch.run_turn("hi").await.unwrap();

        let snap = orch.snapshot_cost().await;
        let handle: Arc<dyn lingxi_traits::OrchestratorHandle> = orch.clone();
        let h = CostHandler::new(handle);
        let display = match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => s,
            _ => panic!("expected Done"),
        };
        // Cost handler renders `${total:.4}` — extract and compare.
        assert!(
            display.contains(&format!("${:.4}", snap.total_usd)),
            "display={display}, snap.total_usd={}",
            snap.total_usd
        );
        assert!(
            display.contains(&format!("({} calls", snap.api_calls)),
            "display={display}, snap.api_calls={}",
            snap.api_calls
        );
    }
}
