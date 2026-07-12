//! Tests for `orchestrator.rs`, extracted from inline `#[cfg(test)]` blocks. Included via `#[path] mod orchestrator_test;`.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn _trait_objects_compile() {
        fn _f<T: OutputStream + 'static>(t: T) -> Box<dyn OutputStream> {
            Box::new(t)
        }
        fn _g<T: OrchestratorHandle + 'static>(t: T) -> Box<dyn OrchestratorHandle> {
            Box::new(t)
        }
    }

    #[test]
    fn output_event_round_trips_through_json() {
        let ev = OutputEvent::Text {
            text: "hello".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    /// Coordinator-activation T08: the additive `emit_coordinator_status`
    /// PUSH hook ships as a default no-op so every pre-existing `OutputStream`
    /// impl (TUI / CLI / `MockOutputStream`) keeps compiling without an
    /// override. This bare unit struct implements ONLY the four required
    /// methods and relies on the default for `emit_coordinator_status`;
    /// driving it must neither fail to compile nor panic, for both a
    /// `Some(team)` and a `None` team.
    #[tokio::test]
    async fn emit_coordinator_status_default_is_noop() {
        struct BareSink;

        #[async_trait]
        impl OutputStream for BareSink {
            async fn emit_text(&self, _text: &str) {}
            async fn emit_tool_call(
                &self,
                _id: &protocol::ToolUseId,
                _tool: &str,
                _input: &serde_json::Value,
            ) {
            }
            async fn emit_tool_result(
                &self,
                _id: &protocol::ToolUseId,
                _tool: &str,
                _model_text: &str,
                _result: &serde_json::Value,
            ) {
            }
            async fn emit_end_turn(&self, _stop_reason: &str, _cost: &CostSnapshot) {}
        }

        // Object-safe behind `dyn` (matches how engines hold it).
        let sink: Box<dyn OutputStream> = Box::new(BareSink);
        // The default no-op must simply return for both team shapes.
        sink.emit_coordinator_status(3, Some("alpha")).await;
        sink.emit_coordinator_status(0, None).await;
    }

    /// M6-04 Task 1: `OutputEvent::ToolCall` must carry a `ToolUseId` so the
    /// TUI can correlate calls with their results and key the per-tool
    /// expanded-state map.
    #[test]
    fn output_event_tool_call_carries_tool_use_id() {
        use protocol::ToolUseId;
        let id = ToolUseId::new();
        let ev = OutputEvent::ToolCall {
            id,
            tool: "Read".into(),
            input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    /// M6-04 Task 1: same for `OutputEvent::ToolResult`.
    #[test]
    fn output_event_tool_result_carries_tool_use_id() {
        use protocol::ToolUseId;
        let id = ToolUseId::new();
        let ev = OutputEvent::ToolResult {
            id,
            tool: "Read".into(),
            result: serde_json::json!({"content": "fn main() {}"}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn cost_snapshot_default_is_zero() {
        let s = CostSnapshot::default();
        assert_eq!(s.total_nano_usd, 0);
        assert_eq!(s.total_tokens, 0);
        assert!((s.total_usd - 0.0).abs() < f64::EPSILON);
        assert_eq!(s.api_calls, 0);
        assert_eq!(s.session_duration, std::time::Duration::ZERO);
    }

    #[test]
    fn compaction_summary_default_is_zero() {
        let s = CompactionSummary::default();
        assert_eq!(s.messages_before, 0);
        assert_eq!(s.messages_after, 0);
        assert_eq!(s.bytes_saved, 0);
    }

    // M5-11 trait extension tests

    #[allow(dead_code)]
    fn _handle_remains_object_safe_after_m5_11() {
        let _: Option<Box<dyn OrchestratorHandle>> = None;
    }

    #[test]
    fn mcp_server_info_fields() {
        let info = McpServerInfo {
            name: "memory".to_string(),
            status: McpStatus::Connected,
            transport: "stdio".to_string(),
        };
        assert_eq!(info.name, "memory");
        assert!(matches!(info.status, McpStatus::Connected));
        assert_eq!(info.transport, "stdio");
    }

    #[test]
    fn hook_info_fields() {
        let info = HookInfo {
            name: "fmt-on-write".to_string(),
            event: "PostToolUse".to_string(),
            matcher: Some("Write|Edit".to_string()),
            timeout_ms: 60_000,
            ..HookInfo::default()
        };
        assert_eq!(info.timeout_ms, 60_000);
        assert_eq!(info.matcher.as_deref(), Some("Write|Edit"));
    }

    #[test]
    fn agent_info_fields() {
        let info = AgentInfo {
            name: "reviewer".to_string(),
            description: "review code".to_string(),
            tools_allowed: vec!["Read".to_string(), "Grep".to_string()],
            wildcard_tools: false,
            ..AgentInfo::default()
        };
        assert_eq!(info.tools_allowed.len(), 2);
    }

    #[test]
    fn doctor_report_default_is_empty() {
        let r = DoctorReport::default();
        assert_eq!(r.checks.len(), 0);
        assert_eq!(r.summary.passed, 0);
        assert_eq!(r.summary.warnings, 0);
        assert_eq!(r.summary.failed, 0);
    }

    #[test]
    fn status_snapshot_default_is_zero() {
        let s = StatusSnapshot::default();
        assert_eq!(s.n_messages, 0);
        assert_eq!(s.n_mcp_connected, 0);
        assert_eq!(s.n_mcp_total, 0);
        assert_eq!(s.n_hooks, 0);
        assert_eq!(s.n_agents, 0);
    }

    #[test]
    fn check_status_variants() {
        let pass = CheckStatus::Pass;
        let warn = CheckStatus::Warn;
        let fail = CheckStatus::Fail;
        assert_ne!(pass, warn);
        assert_ne!(warn, fail);
    }

    // ── RateLimitSnapshot ─────────────────────────────────────────────────────

    /// `RateLimitSnapshot::default()` has all three fields `None`.
    #[test]
    fn rate_limit_snapshot_default_all_none() {
        let s = RateLimitSnapshot::default();
        assert!(s.rate_limit_type.is_none());
        assert!(s.overage_status.is_none());
        assert!(s.overage_disabled_reason.is_none());
    }

    /// A fully-populated snapshot round-trips through equality checks correctly.
    #[test]
    fn rate_limit_snapshot_fields_round_trip() {
        let s = RateLimitSnapshot {
            rate_limit_type: Some("five_hour".to_string()),
            overage_status: Some("allowed_warning".to_string()),
            overage_disabled_reason: Some("out_of_credits".to_string()),
        };
        assert_eq!(s.rate_limit_type.as_deref(), Some("five_hour"));
        assert_eq!(s.overage_status.as_deref(), Some("allowed_warning"));
        assert_eq!(
            s.overage_disabled_reason.as_deref(),
            Some("out_of_credits"),
            "overage_disabled_reason must round-trip"
        );
    }

    /// Two snapshots with identical fields compare equal (`PartialEq`).
    #[test]
    fn rate_limit_snapshot_eq() {
        let a = RateLimitSnapshot {
            rate_limit_type: Some("seven_day".to_string()),
            overage_status: Some("rejected".to_string()),
            overage_disabled_reason: None,
        };
        let b = a.clone();
        assert_eq!(a, b);
        let c = RateLimitSnapshot {
            overage_disabled_reason: Some("out_of_credits".to_string()),
            ..b
        };
        assert_ne!(
            a, c,
            "different overage_disabled_reason must compare unequal"
        );
    }

    /// The default impl of `last_rate_limit_info` on `OrchestratorHandle`
    /// returns `None` — ensures implementors that don't override get a safe
    /// default.
    #[tokio::test]
    async fn last_rate_limit_info_default_is_none() {
        struct MinimalHandle;

        #[async_trait]
        impl OrchestratorHandle for MinimalHandle {
            async fn current_session_id(&self) -> SessionId {
                SessionId::new()
            }
            async fn clear_session(&self) -> Result<(), HandleError> {
                Ok(())
            }
            async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
                Ok(CompactionSummary::default())
            }
            async fn snapshot_cost(&self) -> CostSnapshot {
                CostSnapshot::default()
            }
            async fn switch_model(&self, _: &str, _: Option<&str>) -> Result<(), HandleError> {
                Ok(())
            }
            async fn request_exit(&self) {}
            async fn current_should_exit(&self) -> bool {
                false
            }
            async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
                Ok(MemoryEditorOutcome {
                    edited_path: PathBuf::new(),
                    exit_code: 0,
                })
            }
            async fn list_mcp_servers(&self) -> Vec<McpServerInfo> {
                Vec::new()
            }
            async fn list_hooks(&self) -> Vec<HookInfo> {
                Vec::new()
            }
            async fn list_agents(&self) -> Vec<AgentInfo> {
                Vec::new()
            }
            async fn run_doctor_checks(&self) -> DoctorReport {
                DoctorReport::default()
            }
            async fn get_status_snapshot(&self) -> StatusSnapshot {
                StatusSnapshot::default()
            }
            async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
                Ok(MemoryEditorOutcome {
                    edited_path: PathBuf::new(),
                    exit_code: 0,
                })
            }
            async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
                Ok(MemoryEditorOutcome {
                    edited_path: PathBuf::new(),
                    exit_code: 0,
                })
            }
            async fn list_available_models(&self) -> Vec<String> {
                Vec::new()
            }
        }

        let h = MinimalHandle;
        assert!(
            h.last_rate_limit_info().await.is_none(),
            "default impl must return None"
        );
    }

    // ── OutputEvent::RateLimit (llm-client future-work batch 3, Task 8) ──────

    /// The additive `RateLimit` variant round-trips through the enum's
    /// default serde conventions (externally tagged, named struct fields),
    /// both fully populated and all-`None`.
    #[test]
    fn output_event_rate_limit_round_trips_through_json() {
        let ev = OutputEvent::RateLimit {
            status: Some("allowed_warning".to_string()),
            rate_limit_type: Some("five_hour".to_string()),
            utilization: Some(0.85),
            resets_at: Some(1_760_000_000),
            claim_resets_at: Some(1_760_000_100),
            overage_status: Some("allowed".to_string()),
            overage_resets_at: Some(1_760_000_200),
            overage_disabled_reason: Some("out_of_credits".to_string()),
            fallback_available: Some(true),
            upgrade_paths: Some(vec!["overage".to_string()]),
            credits_required: true,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);

        let empty = OutputEvent::RateLimit {
            status: None,
            rate_limit_type: None,
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
            upgrade_paths: None,
            credits_required: false,
        };
        let s = serde_json::to_string(&empty).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(empty, back);
    }

    /// Task 8: the additive `emit_rate_limit` PUSH hook ships as a default
    /// no-op so every pre-existing `OutputStream` impl keeps compiling
    /// without an override. A bare sink implementing only the four required
    /// methods must accept the call (populated and all-`None`) and return.
    #[tokio::test]
    async fn emit_rate_limit_default_is_noop() {
        struct BareSink;

        #[async_trait]
        impl OutputStream for BareSink {
            async fn emit_text(&self, _text: &str) {}
            async fn emit_tool_call(
                &self,
                _id: &protocol::ToolUseId,
                _tool: &str,
                _input: &serde_json::Value,
            ) {
            }
            async fn emit_tool_result(
                &self,
                _id: &protocol::ToolUseId,
                _tool: &str,
                _model_text: &str,
                _result: &serde_json::Value,
            ) {
            }
            async fn emit_end_turn(&self, _stop_reason: &str, _cost: &CostSnapshot) {}
        }

        // Object-safe behind `dyn` (matches how the orchestrator holds it).
        let sink: Box<dyn OutputStream> = Box::new(BareSink);
        sink.emit_rate_limit(
            Some("allowed_warning"),
            Some("five_hour"),
            Some(0.85),
            Some(1_760_000_000),
            Some(1_760_000_100),
            Some("allowed"),
            Some(1_760_000_200),
            Some("out_of_credits"),
            Some(true),
            Some(&["overage".to_string()]),
            true,
        )
        .await;
        sink.emit_rate_limit(
            None, None, None, None, None, None, None, None, None, None, false,
        )
        .await;
    }

    // ── OutputEvent::RawUtilization (llm-client future-work batch 5, Task 1) ─

    /// The additive `RawUtilization` variant constructs and round-trips
    /// through the enum's default serde conventions (externally tagged,
    /// named struct fields), both fully populated and all-`None`.
    #[test]
    fn raw_utilization_variant_constructs() {
        let ev = OutputEvent::RawUtilization {
            five_hour_utilization: Some(0.42),
            five_hour_resets_at: Some(1_760_000_000),
            seven_day_utilization: Some(0.9),
            seven_day_resets_at: Some(1_760_500_000),
        };
        assert!(matches!(ev, OutputEvent::RawUtilization { .. }));
        let s = serde_json::to_string(&ev).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);

        let empty = OutputEvent::RawUtilization {
            five_hour_utilization: None,
            five_hour_resets_at: None,
            seven_day_utilization: None,
            seven_day_resets_at: None,
        };
        let s = serde_json::to_string(&empty).unwrap();
        let back: OutputEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(empty, back);
    }

    /// Batch 5, Task 1: the additive `emit_raw_utilization` PUSH hook ships
    /// as a default no-op so every pre-existing `OutputStream` impl keeps
    /// compiling without an override. A bare sink implementing only the four
    /// required methods must accept the call (populated and all-`None`) and
    /// return.
    #[tokio::test]
    async fn emit_raw_utilization_default_is_noop() {
        struct BareSink;

        #[async_trait]
        impl OutputStream for BareSink {
            async fn emit_text(&self, _text: &str) {}
            async fn emit_tool_call(
                &self,
                _id: &protocol::ToolUseId,
                _tool: &str,
                _input: &serde_json::Value,
            ) {
            }
            async fn emit_tool_result(
                &self,
                _id: &protocol::ToolUseId,
                _tool: &str,
                _model_text: &str,
                _result: &serde_json::Value,
            ) {
            }
            async fn emit_end_turn(&self, _stop_reason: &str, _cost: &CostSnapshot) {}
        }

        // Object-safe behind `dyn` (matches how the orchestrator holds it).
        let sink: Box<dyn OutputStream> = Box::new(BareSink);
        sink.emit_raw_utilization(
            Some(0.42),
            Some(1_760_000_000),
            Some(0.9),
            Some(1_760_500_000),
        )
        .await;
        sink.emit_raw_utilization(None, None, None, None).await;
    }
}
