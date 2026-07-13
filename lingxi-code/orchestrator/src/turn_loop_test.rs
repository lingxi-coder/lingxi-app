//! Tests for `turn_loop.rs`, extracted verbatim from the module's inline
//! `#[cfg(test)]` blocks. Included via `#[path] mod turn_loop_test;` in
//! `turn_loop.rs`, so each `mod` here resolves `crate::turn_loop::*` for the
//! private helpers it exercises (was `super::*` when co-located).

// ============================================================================
// #6 (main-loop parity): a hook's allowlisted `terminalSequence` is FORWARDED
// to the host's terminal-write seam (`OutputStream::emit_terminal_sequence`),
// not merely debug-logged. A rejected sequence is dropped (warned only).
// ============================================================================
#[cfg(test)]
mod terminal_sequence_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::turn_loop::apply_terminal_sequence;
    use crate::OrchestratorConfig;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn orch_with_output(output: Arc<MockOutputStream>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output,
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// An ACCEPTED OSC sequence (here OSC 9 desktop-notification) is forwarded
    /// to the terminal-write seam verbatim (BEL-normalized).
    #[tokio::test]
    async fn accepted_sequence_is_forwarded_to_terminal_seam() {
        let output = Arc::new(MockOutputStream::new());
        let orch = orch_with_output(output.clone());
        apply_terminal_sequence(&orch, "Notification", Some("\u{001B}]9;hello\u{0007}")).await;
        assert_eq!(
            output.terminal_sequences().await,
            vec!["\u{001B}]9;hello\u{0007}".to_string()],
            "an allowlisted terminalSequence must reach emit_terminal_sequence"
        );
    }

    /// A REJECTED sequence (OSC 8 hyperlink is not in the allowlist) is dropped:
    /// nothing reaches the terminal-write seam.
    #[tokio::test]
    async fn rejected_sequence_is_not_forwarded() {
        let output = Arc::new(MockOutputStream::new());
        let orch = orch_with_output(output.clone());
        apply_terminal_sequence(&orch, "Notification", Some("\u{001B}]8;;http://x\u{0007}")).await;
        assert!(
            output.terminal_sequences().await.is_empty(),
            "a rejected terminalSequence must NOT be forwarded"
        );
    }

    /// `None` (no hook returned a sequence) is a strict no-op.
    #[tokio::test]
    async fn none_is_a_noop() {
        let output = Arc::new(MockOutputStream::new());
        let orch = orch_with_output(output.clone());
        apply_terminal_sequence(&orch, "Notification", None).await;
        assert!(output.terminal_sequences().await.is_empty());
    }
}
#[cfg(test)]
mod terminal_api_error_tests {
    use crate::turn_loop::{refusal_explanation_clause, terminal_api_error_text};
    use llm_client::StopDetails;

    fn details(category: &str, explanation: Option<&str>) -> StopDetails {
        StopDetails {
            category: Some(category.to_string()),
            explanation: explanation.map(str::to_string),
        }
    }

    /// The refusal message appends `\n\nRequest ID: {id}` (double newline) when a
    /// request id is present (binary `u = n ? `\n\nRequest ID: ${n}` : ""`), and
    /// omits it otherwise. The suffix is REFUSAL-ONLY.
    #[test]
    fn refusal_appends_request_id_suffix() {
        let with =
            terminal_api_error_text("claude-opus-4-8", true, "refusal", Some("req_011abc"), None)
                .expect("refusal text");
        assert!(
            with.ends_with("\n\nRequest ID: req_011abc"),
            "double newline; got: {with}"
        );

        let without = terminal_api_error_text("claude-opus-4-8", true, "refusal", None, None)
            .expect("refusal text");
        assert!(!without.contains("Request ID:"), "got: {without}");

        // Empty id is treated as absent.
        let empty = terminal_api_error_text("claude-opus-4-8", true, "refusal", Some(""), None)
            .expect("refusal text");
        assert!(!empty.contains("Request ID:"), "got: {empty}");
    }

    /// The Request ID suffix is refusal-only: max_tokens / context-window
    /// messages never carry it, even when a request id is available.
    #[test]
    fn non_refusal_terminals_have_no_request_id() {
        for sr in ["max_tokens", "model_context_window_exceeded"] {
            let t = terminal_api_error_text("claude-opus-4-8", true, sr, Some("req_011abc"), None)
                .unwrap_or_else(|| panic!("{sr} text"));
            assert!(!t.contains("Request ID:"), "{sr}: {t}");
        }
    }

    /// No `stop_details` (the common refusal) → the generic label / Usage-Policy
    /// text, byte-exact (no cyber/bio wording).
    #[test]
    fn refusal_without_stop_details_is_generic() {
        // opus-4-8 HAS a marketing name → the label branch.
        let t = terminal_api_error_text("claude-opus-4-8", true, "refusal", None, None)
            .expect("refusal");
        assert!(t.contains("'s safeguards flagged this message (https://www.anthropic.com/legal/aup). This sometimes happens with safe, normal conversations."), "got: {t}");
        assert!(!t.contains("cybersecurity"), "got: {t}");
    }

    /// LABEL branch + cyber/bio category → the `Saa` variant: "… They may flag
    /// safe, normal content as well. These measures let us bring you Mythos-level
    /// capabilities sooner …" (binary `U2e` `Jct(cat)` path; `Saa`+`elp`).
    #[test]
    fn refusal_label_cyber_or_bio_variant() {
        for cat in ["cyber", "bio"] {
            let sd = details(cat, None);
            let t = terminal_api_error_text("claude-opus-4-8", true, "refusal", None, Some(&sd))
                .expect("refusal");
            assert!(
                t.contains("'s safeguards flagged this message (https://www.anthropic.com/legal/aup). They may flag safe, normal content as well."),
                "{cat}: {t}"
            );
            assert!(
                t.contains("These measures let us bring you Mythos-level capabilities sooner, and we're working to refine them."),
                "{cat}: {t}"
            );
            assert!(
                t.contains("LingXi can't respond to this request with"),
                "{cat}: {t}"
            );
        }
    }

    /// NO-LABEL branch + cyber category → the 2.1.206 Cyber Verification Program
    /// interstitial (replaced the pre-206 "apply for an exemption" message). The
    /// help-center URL is fixed; the feedback tail is interactive-only; there is
    /// no `\n\n{m}\n\n{f}` tail.
    #[test]
    fn refusal_nolabel_cyber_verification_program_206() {
        // A model with NO marketing name → the no-label branch. Use a bare id
        // that `marketing_name_for_model` does not resolve.
        let sd = details("cyber", None);
        // Interactive → includes the feedback tail.
        let ti = terminal_api_error_text("unknown-model-xyz", true, "refusal", None, Some(&sd))
            .expect("refusal");
        assert!(
            ti.starts_with("API Error: This model has safety measures that flagged this message for a cybersecurity topic. To learn about the Cyber Verification Program and apply for access, visit our help center: https://support.claude.com/en/articles/14604842-real-time-cyber-safeguards-on-claude."),
            "got: {ti}"
        );
        assert!(
            ti.ends_with(
                ".\n\nIf you were not engaging in a cybersecurity topic, please send feedback via /feedback."
            ),
            "interactive feedback tail; got: {ti}"
        );
        // 206 dropped the pre-206 exemption wording entirely.
        assert!(!ti.contains("apply for an exemption"), "got: {ti}");
        // Non-interactive → NO feedback tail; ends at the help-center URL.
        let tn = terminal_api_error_text("unknown-model-xyz", false, "refusal", None, Some(&sd))
            .expect("refusal");
        assert!(
            tn.ends_with("14604842-real-time-cyber-safeguards-on-claude."),
            "non-interactive omits the feedback tail; got: {tn}"
        );
        assert!(!tn.contains("please send feedback"), "got: {tn}");
    }

    /// NO-LABEL branch + `military_weapons` category → 2.1.206 REMOVED the
    /// dedicated weapons arm, so it now falls through to the generic
    /// Usage-Policy message ('weapons-related content' = 0 hits in 206).
    #[test]
    fn refusal_nolabel_military_weapons_falls_through_to_generic_206() {
        let sd = details("military_weapons", None);
        let t = terminal_api_error_text("unknown-model-xyz", false, "refusal", None, Some(&sd))
            .expect("refusal");
        assert!(
            t.contains("is unable to respond to this request, which appears to violate our Usage Policy (https://www.anthropic.com/legal/aup)."),
            "got: {t}"
        );
        assert!(!t.contains("weapons-related content"), "206 removed the weapons arm; got: {t}");
        assert!(!t.contains("added safeguards for"), "got: {t}");
    }

    /// `U2e` explanation clause `${a}` — leading space + terminal-`.` when the
    /// explanation lacks one; "" when absent; truncate (+ `…`) past 400 chars.
    #[test]
    fn refusal_explanation_clause_formats() {
        assert_eq!(refusal_explanation_clause(None), "");
        assert_eq!(refusal_explanation_clause(Some("")), "");
        assert_eq!(
            refusal_explanation_clause(Some("see policy X")),
            " see policy X."
        );
        assert_eq!(refusal_explanation_clause(Some("denied.")), " denied.");
        assert_eq!(refusal_explanation_clause(Some("why?")), " why?");
        assert_eq!(refusal_explanation_clause(Some("stop!")), " stop!");
        // Over 400 chars → 400-char head + `…` (terminal punct ⇒ no extra `.`).
        let clause = refusal_explanation_clause(Some(&"x".repeat(450)));
        assert!(clause.starts_with(' ') && clause.ends_with('\u{2026}'));
        assert_eq!(clause.chars().count(), 1 + 400 + 1);
    }

    /// NO-LABEL + non-cyber/non-weapons category + explanation → the generic
    /// usage-policy message appends the explanation clause (binary `…aup).${a} `).
    #[test]
    fn refusal_nolabel_generic_appends_explanation() {
        let sd = details("other", Some("This violates section 2"));
        let t = terminal_api_error_text("unknown-model-xyz", false, "refusal", None, Some(&sd))
            .expect("refusal");
        assert!(
            t.contains("violate our Usage Policy (https://www.anthropic.com/legal/aup). This violates section 2."),
            "got: {t}"
        );
        // No explanation → no clause (period-space-{m}, the prior output).
        let sd2 = details("other", None);
        let t2 = terminal_api_error_text("unknown-model-xyz", false, "refusal", None, Some(&sd2))
            .expect("refusal");
        assert!(
            t2.contains("Usage Policy (https://www.anthropic.com/legal/aup). "),
            "got: {t2}"
        );
        assert!(!t2.contains("section 2"), "got: {t2}");
    }
}
#[cfg(test)]
mod model_text_tests {
    use crate::turn_loop::tool_result_to_model_text;
    use serde_json::json;

    #[test]
    fn prefers_model_content_over_content() {
        // Read-shaped: the model sees the cat -n string, not the raw `content`.
        let data = json!({ "model_content": "1\thi\n2\t", "content": "hi\n" });
        assert_eq!(tool_result_to_model_text(&data), "1\thi\n2\t");
    }

    #[test]
    fn falls_back_to_content_string_verbatim() {
        // Bash/Edit/Write-shaped: no `model_content`, so the model sees the raw
        // `content` string verbatim — NOT a JSON dump of the object.
        let data = json!({ "content": "build ok\n", "exit_code": 0 });
        assert_eq!(tool_result_to_model_text(&data), "build ok\n");
    }

    #[test]
    fn webfetch_result_field_is_model_text() {
        // WebFetch's claude-code result `data` names the model-facing markdown
        // `result` (no `content`/`model_content`). The model must see that
        // markdown verbatim — NOT a JSON dump of the whole {bytes,code,…} object.
        let data = json!({
            "bytes": 11,
            "code": 200,
            "codeText": "OK",
            "result": "# Hello\n\nbody",
            "durationMs": 3,
            "url": "https://e.example/",
        });
        assert_eq!(tool_result_to_model_text(&data), "# Hello\n\nbody");
    }

    #[test]
    fn falls_back_to_json_when_no_string_content() {
        // Structured-only result (no string `content`/`model_content`): legacy
        // JSON serialization is preserved.
        let data = json!({ "matches": ["a", "b"] });
        assert_eq!(tool_result_to_model_text(&data), r#"{"matches":["a","b"]}"#);
        // A non-string `content` also falls through to JSON.
        let data2 = json!({ "content": 42 });
        assert_eq!(tool_result_to_model_text(&data2), r#"{"content":42}"#);
    }
}
#[cfg(test)]
mod read_file_state_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::turn_loop::{absolutize, dispatch_tool_uses, execute_one_turn};
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::ToolUseId;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };
    use traits::OrchestratorHandle;

    /// Minimal Read/Edit/Write-shaped stub. Resolves `file_path` against
    /// `cwd` (mirroring how the real `FileReadTool` resolves against
    /// `getCwd()`) and reads it, so a missing file yields `is_error = true`
    /// exactly like the real tool. `name` is configurable so one stub can
    /// stand in for Read/Edit/Write.
    struct StubFileTool {
        name: &'static str,
        cwd: PathBuf,
    }

    #[async_trait]
    impl Tool for StubFileTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| {
                    json!({
                        "type": "object",
                        "properties": { "file_path": { "type": "string" } },
                        "required": ["file_path"],
                    })
                });
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "stub file tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("file_path required".into()))?;
            // Resolve against the tool's cwd (mirrors getCwd()), then touch
            // disk so a missing file is a genuine error (mirrors Read).
            let resolved = self.cwd.join(path);
            let content = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| ToolError::Io(format!("read {}: {e}", resolved.display())))?;
            Ok(ToolCallResult {
                data: json!({ "content": content }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator whose registry contains the given stub tools,
    /// rooted at `cwd`. The API queue is empty (these tests drive
    /// `dispatch_tool_uses` directly, never `run_turn`).
    fn orch_with_tools(cwd: PathBuf, tools: Vec<Arc<dyn Tool>>) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        for t in tools {
            registry.register_builtin(t);
        }
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            crate::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd,
        )
    }

    /// Drive one `(name, input)` `tool_use` through the dispatch chokepoint.
    async fn dispatch_one(orch: &ConversationOrchestrator, name: &str, input: serde_json::Value) {
        let uses = vec![(ToolUseId::new(), name.to_string(), input, None)];
        dispatch_tool_uses(orch, &uses).await.expect("dispatch");
    }

    // ===== Orphaned-permission recovery (run_orphaned_permission) ===========
    // 1:1 with claude-code's handleOrphanedPermission (queryHelpers.ts:224-343).

    fn assistant_with_tool_use(
        tuid: &ToolUseId,
        name: &str,
        input: serde_json::Value,
    ) -> protocol::ConversationMessage {
        protocol::ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolUse {
                id: tuid.clone(),
                name: name.to_string(),
                input,
                provider_id: None,
            }],
            stop_reason: None,
        }
    }

    fn last_tool_result(history: &[protocol::ConversationMessage]) -> (String, bool) {
        for m in history.iter().rev() {
            if let protocol::ConversationMessage::User { content, .. } = m {
                for b in content {
                    if let protocol::ContentBlock::ToolResult {
                        content, is_error, ..
                    } = b
                    {
                        return (content.clone(), *is_error);
                    }
                }
            }
        }
        panic!("no tool_result in history");
    }

    #[tokio::test]
    async fn orphaned_permission_allow_applies_updated_input_and_executes() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("real.txt"), "RECOVERED")
            .await
            .unwrap();
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);

        let tuid = ToolUseId::new();
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            // Original input points at a MISSING file; the orphaned ALLOW carries
            // an `updatedInput` that rewrites it to the real one.
            s.history.push(assistant_with_tool_use(
                &tuid,
                "Read",
                json!({"file_path":"missing.txt"}),
            ));
        }

        let recovered = orch
            .run_orphaned_permission(
                &tuid,
                traits::permission_gate::PermissionOutcome::Allow {
                    updated_input: Some(json!({"file_path":"real.txt"})),
                    permission_updates: vec![],
                },
            )
            .await
            .expect("recovery");
        assert!(recovered, "a found unresolved tool_use must recover");

        let sess = orch.session();
        let s = sess.lock().await;
        let assistants = s
            .history
            .iter()
            .filter(|m| matches!(m, protocol::ConversationMessage::Assistant { .. }))
            .count();
        assert_eq!(
            assistants, 1,
            "assistant must not be re-pushed (alreadyPresent)"
        );
        let (content, is_error) = last_tool_result(&s.history);
        assert!(
            !is_error,
            "updatedInput should make the tool read the real file"
        );
        assert!(
            content.contains("RECOVERED"),
            "tool ran with the rewritten input: {content}"
        );
    }

    #[tokio::test]
    async fn orphaned_permission_deny_pushes_error_result_without_executing() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("real.txt"), "SHOULD_NOT_READ")
            .await
            .unwrap();
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);

        let tuid = ToolUseId::new();
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            s.history.push(assistant_with_tool_use(
                &tuid,
                "Read",
                json!({"file_path":"real.txt"}),
            ));
        }

        let recovered = orch
            .run_orphaned_permission(
                &tuid,
                traits::permission_gate::PermissionOutcome::Deny {
                    reason: "Permission to use Read has been denied.".into(),
                },
            )
            .await
            .expect("recovery");
        assert!(recovered);

        let sess = orch.session();
        let s = sess.lock().await;
        let (content, is_error) = last_tool_result(&s.history);
        assert!(is_error, "deny must produce an error tool_result");
        assert_eq!(content, "Permission to use Read has been denied.");
        assert!(
            !content.contains("SHOULD_NOT_READ"),
            "deny must not execute the tool"
        );
    }

    #[tokio::test]
    async fn orphaned_permission_noop_when_already_resolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);

        let tuid = ToolUseId::new();
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            s.history.push(assistant_with_tool_use(
                &tuid,
                "Read",
                json!({"file_path":"x.txt"}),
            ));
            // A matching tool_result already exists ⇒ resolved.
            s.history.push(protocol::ConversationMessage::User {
                id: protocol::MessageId::new(),
                content: vec![protocol::ContentBlock::ToolResult {
                    tool_use_id: tuid.clone(),
                    content: "prior".into(),
                    is_error: false,
                    provider_tool_use_id: None,
                    content_blocks: None,
                }],
                is_meta: false,
            });
        }
        let before = orch.session().lock().await.history.len();
        let recovered = orch
            .run_orphaned_permission(
                &tuid,
                traits::permission_gate::PermissionOutcome::Allow {
                    updated_input: None,
                    permission_updates: vec![],
                },
            )
            .await
            .expect("recovery");
        assert!(!recovered, "already-resolved tool_use must not recover");
        assert_eq!(
            orch.session().lock().await.history.len(),
            before,
            "no mutation on a resolved tool_use"
        );
    }

    #[tokio::test]
    async fn orphaned_permission_noop_when_tool_use_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);
        let recovered = orch
            .run_orphaned_permission(
                &ToolUseId::new(),
                traits::permission_gate::PermissionOutcome::Allow {
                    updated_input: None,
                    permission_updates: vec![],
                },
            )
            .await
            .expect("recovery");
        assert!(!recovered);
        assert!(orch.session().lock().await.history.is_empty());
    }

    #[tokio::test]
    async fn orphaned_permission_unknown_tool_returns_handled_without_result() {
        // The recovered tool is no longer registered (findToolByName → return,
        // queryHelpers.ts:256-259): emit/push NOTHING but report handled
        // (`Ok(true)`) so the caller's per-id gate is consumed.
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);

        let tuid = ToolUseId::new();
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            s.history
                .push(assistant_with_tool_use(&tuid, "Ghost", json!({"x":1})));
        }
        let before = orch.session().lock().await.history.len();
        let recovered = orch
            .run_orphaned_permission(
                &tuid,
                traits::permission_gate::PermissionOutcome::Allow {
                    updated_input: None,
                    permission_updates: vec![],
                },
            )
            .await
            .expect("recovery");
        assert!(recovered, "unknown tool consumes the gate (Ok(true))");
        assert_eq!(
            orch.session().lock().await.history.len(),
            before,
            "unknown tool must push NO tool_result"
        );
    }

    #[tokio::test]
    async fn orphaned_permission_distinct_ids_each_recover() {
        // A --resume that lost TWO can_use_tool requests must recover BOTH
        // (no spurious single-orphan cap). Per-id idempotency lives in the
        // history found-check; the run-loop gate is a per-id Set on top.
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("a.txt"), "AAA")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "BBB")
            .await
            .unwrap();
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);

        let (id1, id2) = (ToolUseId::new(), ToolUseId::new());
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            s.history.push(assistant_with_tool_use(
                &id1,
                "Read",
                json!({"file_path":"a.txt"}),
            ));
            s.history.push(assistant_with_tool_use(
                &id2,
                "Read",
                json!({"file_path":"b.txt"}),
            ));
        }
        let allow = || traits::permission_gate::PermissionOutcome::Allow {
            updated_input: None,
            permission_updates: vec![],
        };
        assert!(orch.run_orphaned_permission(&id1, allow()).await.unwrap());
        assert!(orch.run_orphaned_permission(&id2, allow()).await.unwrap());

        let sess = orch.session();
        let s = sess.lock().await;
        let results: Vec<&str> = s
            .history
            .iter()
            .filter_map(|m| match m {
                protocol::ConversationMessage::User { content, .. } => {
                    content.iter().find_map(|b| match b {
                        protocol::ContentBlock::ToolResult { content, .. } => {
                            Some(content.as_str())
                        }
                        _ => None,
                    })
                }
                _ => None,
            })
            .collect();
        assert!(results.iter().any(|c| c.contains("AAA")), "id1 recovered");
        assert!(results.iter().any(|c| c.contains("BBB")), "id2 recovered");
    }

    #[tokio::test]
    async fn orphaned_permission_same_id_second_call_is_noop() {
        // After recovery pushes a tool_result, a same-id re-delivery finds the
        // call resolved (findUnresolvedToolUse → null) and no-ops — the real
        // same-id dedup, independent of the run-loop gate.
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("real.txt"), "ONCE")
            .await
            .unwrap();
        let tool = Arc::new(StubFileTool {
            name: "Read",
            cwd: dir.path().to_path_buf(),
        });
        let orch = orch_with_tools(dir.path().to_path_buf(), vec![tool]);

        let tuid = ToolUseId::new();
        {
            let sess = orch.session();
            let mut s = sess.lock().await;
            s.history.push(assistant_with_tool_use(
                &tuid,
                "Read",
                json!({"file_path":"real.txt"}),
            ));
        }
        let allow = || traits::permission_gate::PermissionOutcome::Allow {
            updated_input: None,
            permission_updates: vec![],
        };
        assert!(orch.run_orphaned_permission(&tuid, allow()).await.unwrap());
        assert!(
            !orch.run_orphaned_permission(&tuid, allow()).await.unwrap(),
            "second same-id recovery must no-op (already resolved)"
        );
    }

    // ----- JSON-schema input-validation gate -------------------------------
    // (claude-code `toolExecution.ts:615` `inputSchema.safeParse`). BEHAVIORAL
    // parity only — the message bytes intentionally differ from claude-code's
    // Zod `formatZodValidationError` output (unportable).

    fn schema_gate_tool_result(block: &protocol::ContentBlock) -> (&str, bool) {
        match block {
            protocol::ContentBlock::ToolResult {
                content, is_error, ..
            } => (content.as_str(), *is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    /// A tool whose `input_schema()` requires a string `path`, with a `call()`
    /// that records (via an `AtomicBool`) whether it was reached. Lets the
    /// pass-through test assert the gate did NOT short-circuit a valid input.
    struct SchemaCallTrackerTool {
        called: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl Tool for SchemaCallTrackerTool {
        fn name(&self) -> &str {
            "Schemic"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| {
                    json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"],
                    })
                });
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "schema tracker".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolCallResult {
                data: json!({ "ok": true }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// Missing a required field → the schema gate short-circuits with an
    /// `InputValidationError` `<tool_use_error>` block, and `call()` is never
    /// reached.
    #[tokio::test]
    async fn schema_gate_rejects_missing_required_field() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let orch = orch_with_tools(
            PathBuf::from("/tmp"),
            vec![Arc::new(SchemaCallTrackerTool {
                called: called.clone(),
            })],
        );
        let uses = vec![(ToolUseId::new(), "Schemic".to_string(), json!({}), None)];
        let results = dispatch_tool_uses(&orch, &uses).await.expect("dispatch");
        assert_eq!(results.len(), 1);
        let (content, is_error) = schema_gate_tool_result(&results[0]);
        assert!(is_error, "missing-required input must be an error");
        assert!(
            content.starts_with("<tool_use_error>InputValidationError:"),
            "expected InputValidationError wrapper, got: {content}"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "call() must NOT run when the schema gate rejects the input"
        );
    }

    /// A schema-valid input passes the gate and reaches `call()` without
    /// producing an `InputValidationError`.
    #[tokio::test]
    async fn schema_gate_passes_valid_input_through_to_call() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let orch = orch_with_tools(
            PathBuf::from("/tmp"),
            vec![Arc::new(SchemaCallTrackerTool {
                called: called.clone(),
            })],
        );
        let uses = vec![(
            ToolUseId::new(),
            "Schemic".to_string(),
            json!({ "path": "/x" }),
            None,
        )];
        let results = dispatch_tool_uses(&orch, &uses).await.expect("dispatch");
        assert_eq!(results.len(), 1);
        let (content, _is_error) = schema_gate_tool_result(&results[0]);
        assert!(
            !content.contains("InputValidationError"),
            "valid input must not trip the schema gate, got: {content}"
        );
        assert!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            "call() must run for schema-valid input"
        );
    }

    // ----- build_wire_tools (registry -> wire `tools` array) -----

    #[tokio::test]
    async fn build_wire_tools_serializes_enabled_registry_tools() {
        // The orchestrator's wire tool array carries each enabled registry tool
        // as `{name, description, input_schema}`, sorted by name (the batched +
        // streaming legs both source their `tools` arg from here).
        let cwd = PathBuf::from("/tmp");
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "Bash",
                cwd: cwd.clone(),
            }),
        ];
        let orch = orch_with_tools(cwd, tools);
        let wire = orch.build_wire_tools().await;

        let names: Vec<&str> = wire.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bash", "Read"], "sorted by name");
        for t in &wire {
            // The base triple, nothing else.
            assert_eq!(t.as_object().unwrap().len(), 3, "base triple only: {t}");
            assert!(t.get("description").is_some());
            assert_eq!(t["input_schema"]["type"], "object");
        }
    }

    #[tokio::test]
    async fn build_wire_tools_empty_registry_is_empty() {
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        assert!(orch.build_wire_tools().await.is_empty());
    }

    // ----- FIX 1: tool-wide deny filter on the wire `tools` array -----------
    // claude-code `getTools`/`assembleToolPool` strip blanket-denied tools BEFORE
    // the model sees them (`filterToolsByDenyRules`, tools.ts:307-310). The
    // orchestrator now does the same in `build_wire_tools` via the gate's
    // `tool_wide_deny_names`.

    /// Build an orchestrator whose registry holds `builtins` + the MCP `mcp_tools`
    /// (a single connection), gated by a `PolicyPermissionGate` carrying the given
    /// tool-wide `deny` rule strings (e.g. `"WebFetch"`, `"mcp__github"`).
    fn orch_with_deny_rules(
        builtins: Vec<Arc<dyn Tool>>,
        mcp_tools: Vec<Arc<dyn Tool>>,
        deny: &[&str],
    ) -> ConversationOrchestrator {
        use permission::{
            PermissionBehavior, PermissionPolicy, PermissionRule, PermissionRuleSource,
            PermissionRuleValue, PolicyPermissionGate,
        };
        let mut registry = ToolRegistry::new();
        for t in builtins {
            registry.register_builtin(t);
        }
        if !mcp_tools.is_empty() {
            registry.register_mcp_tools(protocol::McpConnectionId::new(), mcp_tools);
        }
        let rules = deny.iter().map(|d| PermissionRule {
            value: PermissionRuleValue {
                tool_name: (*d).to_string(),
                rule_content: None,
            },
            behavior: PermissionBehavior::Deny,
            source: PermissionRuleSource::ProjectSettings,
        });
        let policy = Arc::new(PermissionPolicy::from_rules(
            permission::PermissionMode::Default,
            rules,
        ));
        let gate: Arc<dyn traits::permission_gate::PermissionGate> = Arc::new(
            PolicyPermissionGate::new(policy, Arc::new(NoOpPermissionGate)),
        );
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            crate::test_support::noop_hook_executor(),
            gate,
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    fn wire_tool_names(wire: &[serde_json::Value]) -> Vec<String> {
        wire.iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn build_wire_tools_hides_tool_wide_denied_tool() {
        let cwd = PathBuf::from("/tmp");
        let builtins: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "WebFetch",
                cwd: cwd.clone(),
            }),
        ];
        let orch = orch_with_deny_rules(builtins, vec![], &["WebFetch"]);
        let names = wire_tool_names(&orch.build_wire_tools().await);
        assert_eq!(names, vec!["Read"], "deny:[WebFetch] hides WebFetch");
    }

    #[tokio::test]
    async fn build_wire_tools_mcp_server_prefix_deny_hides_all_server_tools() {
        // A tool-wide `mcp__github` deny strips EVERY `mcp__github__*` tool
        // (claude-code MCP server-prefix blanket strip) but leaves other servers.
        let cwd = PathBuf::from("/tmp");
        let builtins: Vec<Arc<dyn Tool>> = vec![Arc::new(StubFileTool {
            name: "Read",
            cwd: cwd.clone(),
        })];
        let mcp: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubFileTool {
                name: "mcp__github__issue",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "mcp__github__pr",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "mcp__slack__post",
                cwd: cwd.clone(),
            }),
        ];
        let orch = orch_with_deny_rules(builtins, mcp, &["mcp__github"]);
        let names = wire_tool_names(&orch.build_wire_tools().await);
        assert_eq!(
            names,
            vec!["Read", "mcp__slack__post"],
            "deny:[mcp__github] hides all mcp__github__* but keeps mcp__slack__post"
        );
    }

    #[tokio::test]
    async fn build_wire_tools_empty_deny_is_byte_identical() {
        // Regression safety: with ZERO deny rules the filtered output must equal
        // the unfiltered output (the default-gate path must not perturb anything).
        let cwd = PathBuf::from("/tmp");
        let mk = || -> Vec<Arc<dyn Tool>> {
            vec![
                Arc::new(StubFileTool {
                    name: "Bash",
                    cwd: cwd.clone(),
                }) as Arc<dyn Tool>,
                Arc::new(StubFileTool {
                    name: "WebFetch",
                    cwd: cwd.clone(),
                }) as Arc<dyn Tool>,
            ]
        };
        // No-deny gate (PolicyPermissionGate with empty rules) vs the default
        // NoOp gate: both must yield the same wire bytes as the plain registry.
        let baseline = orch_with_tools(cwd.clone(), mk()).build_wire_tools().await;
        let gated = orch_with_deny_rules(mk(), vec![], &[])
            .build_wire_tools()
            .await;
        assert_eq!(
            gated, baseline,
            "empty deny must be byte-identical to the unfiltered wire tools"
        );
        assert_eq!(wire_tool_names(&gated), vec!["Bash", "WebFetch"]);
    }

    #[tokio::test]
    async fn batched_turn_forwards_wire_tools_to_messages_create() {
        // End-to-end (batched leg): `execute_one_turn` must build the registry's
        // wire tools and pass them to `messages_create`. A no-tool `end_turn`
        // response terminates the step after a single round-trip. (The streaming
        // leg's twin is `streaming_concurrent_tools_test`.)
        let cwd = PathBuf::from("/tmp");
        let api = Arc::new(MockApiClient::new(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        )]));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(StubFileTool {
            name: "Read",
            cwd: cwd.clone(),
        }));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api.clone(),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd,
        );

        let _ = execute_one_turn(&orch, None).await.expect("turn step");

        let captured = api.captured_tools().await;
        assert_eq!(captured.len(), 1, "exactly one messages_create round-trip");
        let names: Vec<&str> = captured[0]
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["Read"],
            "batched turn must advertise the registry's wire tools to messages_create"
        );
    }

    // ----- absolutize (pure, lexical — NOT realpath) -----

    #[test]
    fn absolutize_helper_resolves_lexically() {
        let cwd = PathBuf::from("/repo");
        assert_eq!(
            absolutize(&cwd, "src/main.rs"),
            PathBuf::from("/repo/src/main.rs")
        );
        assert_eq!(absolutize(&cwd, "/abs/x.rs"), PathBuf::from("/abs/x.rs"));
        // `.` dropped, `..` popped — purely lexical.
        assert_eq!(absolutize(&cwd, "./a/../b.rs"), PathBuf::from("/repo/b.rs"));
        // Surrounding whitespace is trimmed (mirrors expandPath).
        assert_eq!(
            absolutize(&cwd, "  src/a.rs  "),
            PathBuf::from("/repo/src/a.rs")
        );
    }

    #[test]
    fn absolutize_does_not_canonicalize_disk() {
        // A path that does NOT exist must still resolve to the joined string
        // (expandPath is lexical, not realpath — no fs canonicalization).
        let cwd = PathBuf::from("/nonexistent-root-xyz");
        let got = absolutize(&cwd, "does/not/exist.rs");
        assert_eq!(
            got,
            PathBuf::from("/nonexistent-root-xyz/does/not/exist.rs")
        );
    }

    #[test]
    fn absolutize_expands_tilde() {
        let Some(home) = dirs::home_dir() else {
            return; // no home dir on this platform — skip
        };
        assert_eq!(absolutize(&PathBuf::from("/repo"), "~/x"), home.join("x"));
        assert_eq!(absolutize(&PathBuf::from("/repo"), "~"), home);
    }

    // ----- cache population through the dispatch loop -----

    #[tokio::test]
    async fn cache_records_successful_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).expect("mkdir");
        std::fs::write(sub.join("a.rs"), b"fn a() {}").expect("write");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }) as Arc<dyn Tool>],
        );
        dispatch_one(&orch, "Read", json!({ "file_path": "src/a.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("src").join("a.rs")]);
        // The richer `read_state_map` is a SEPARATE registry from the `/files`
        // `Vec`. `record_read_file_state` (which a `StubFileTool` dispatch
        // exercises) only touches the `Vec`; the map is populated by the real
        // file tools' `readFileState.set`, which the stub does not call. So the
        // `/files` ordering semantics above are unaffected by Batch B.
        assert!(
            orch.read_state_map.lock().unwrap().is_empty(),
            "the richer read-state map is independent of the /files Vec"
        );
    }

    #[tokio::test]
    async fn read_state_map_starts_empty_and_is_distinct_from_files_vec() {
        // Behavior-neutral wiring check: a fresh orchestrator has an empty
        // read-state registry, separate from the `/files` `Vec`.
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        assert!(orch.read_state_map.lock().unwrap().is_empty());
        assert!(orch.files_in_context().await.is_empty());
    }

    #[tokio::test]
    async fn read_state_map_arc_is_shareable_and_visible_through_orchestrator() {
        // Proves the composition-root contract: the SAME `Arc` the orchestrator
        // holds in `read_state_map` is what the file tools' `BuiltinToolContext`
        // share, so a `readFileState.set` performed against a clone of that
        // `Arc` (as the real `FileReadTool` does — see the `tool-file`
        // `read_populates_read_file_state_map_with_offset_limit` test) is
        // visible through `orch.read_state_map`. Simulated here with a direct
        // `set` (the orchestrator crate cannot depend on `tool-file`), keeping
        // the wiring assertion crate-local. The `/files` `Vec` is untouched.
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        let shared = orch.read_state_map.clone();
        tool_api::read_file_state::set(
            &shared,
            PathBuf::from("/tmp/a.txt"),
            tool_api::read_file_state::ReadFileEntry {
                content: "line2\n".into(),
                mtime_ms: 42,
                offset: Some(2),
                limit: Some(1),
                from_read: true,
            },
        );
        let entry = tool_api::read_file_state::get(
            &orch.read_state_map,
            std::path::Path::new("/tmp/a.txt"),
        )
        .expect("orchestrator registry sees the shared-Arc set");
        assert_eq!(entry.content, "line2\n");
        assert_eq!(entry.offset, Some(2));
        assert_eq!(entry.limit, Some(1));
        // The `/files` `Vec` remains independent and empty.
        assert!(orch.files_in_context().await.is_empty());
    }

    // ----- #59 post-compact file/skill attachment restoration -----

    #[tokio::test]
    async fn force_compact_restores_recent_files_and_clears_read_state() {
        use compaction::CompactionOrchestrator;
        use protocol::{ConversationMessage, MessageId};
        use traits::OrchestratorHandle;

        // Compaction with a tiny threshold so a small seeded history compacts.
        let mut orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        orch = orch.with_compaction(Arc::new(CompactionOrchestrator::new(10)));
        let orch = Arc::new(orch);

        // Seed enough history to trip the compactor.
        {
            let session = orch.session();
            let mut s = session.lock().await;
            for i in 0..20 {
                s.history.push(ConversationMessage::user(
                    MessageId::new(),
                    format!(
                        "turn-{i} padded body text to push the token estimate over the threshold"
                    ),
                ));
            }
        }

        // Seed the read-file-state registry with two files at distinct mtimes.
        // P2-12: the restore RE-READS from disk (not the snapshot content), so
        // the files must exist on disk with the asserted content.
        let dir = tempfile::tempdir().expect("tempdir");
        let old_path = dir.path().join("old.rs");
        let new_path = dir.path().join("new.rs");
        std::fs::write(&old_path, "fn old() {}\n").expect("write old.rs");
        std::fs::write(&new_path, "fn fresh() {}\n").expect("write new.rs");
        tool_api::read_file_state::set(
            &orch.read_state_map,
            old_path.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: "fn old() {}\n".into(),
                mtime_ms: 100,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
        tool_api::read_file_state::set(
            &orch.read_state_map,
            new_path.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: "fn fresh() {}\n".into(),
                mtime_ms: 200,
                offset: None,
                limit: None,
                from_read: true,
            },
        );

        orch.force_compact().await.expect("force_compact ok");

        // The read-state registry is cleared post-compact (`readFileState.clear`).
        assert!(
            orch.read_state_map.lock().unwrap().is_empty(),
            "read_state_map must be cleared after compaction"
        );

        // The restored file attachments ride after the boundary marker + summary.
        let session = orch.session();
        let s = session.lock().await;
        let restored: Vec<&String> = s
            .history
            .iter()
            .filter_map(|m| match m {
                ConversationMessage::User {
                    content,
                    is_meta: true,
                    ..
                } => content.iter().find_map(|b| match b {
                    protocol::ContentBlock::Text { text }
                        if text.contains("restored after compaction") =>
                    {
                        Some(text)
                    }
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(
            restored.len(),
            2,
            "both seeded files should be restored as attachments"
        );
        // Most-recent file content is present.
        assert!(
            restored.iter().any(|t| t.contains("fn fresh() {}")),
            "the freshest file content must be restored"
        );
        let new_disp = new_path.display().to_string();
        assert!(
            restored.iter().any(|t| t.contains(&new_disp)),
            "the restored attachment names the file path"
        );
    }

    // ----- P1-06 composition-root read-state-map sharing -----

    #[tokio::test]
    async fn with_read_state_map_adopts_the_composition_root_arc() {
        // The crux of P1-06: the builder OVERWRITES the constructor's fresh
        // default with the SAME `Arc` the composition root passes into the file
        // tools' `BuiltinToolContext`, so both sides observe one registry.
        let shared = tool_api::read_file_state::new_read_file_state_map();
        let orch =
            orch_with_tools(PathBuf::from("/tmp"), vec![]).with_read_state_map(shared.clone());
        assert!(
            Arc::ptr_eq(&orch.read_state_map, &shared),
            "with_read_state_map must adopt the shared Arc, not keep the default"
        );
        // A set through the composition-root handle is visible on the
        // orchestrator's field (the same allocation).
        tool_api::read_file_state::set(
            &shared,
            PathBuf::from("/tmp/wired.txt"),
            tool_api::read_file_state::ReadFileEntry {
                content: "wired\n".into(),
                mtime_ms: 7,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
        assert!(
            tool_api::read_file_state::get(
                &orch.read_state_map,
                std::path::Path::new("/tmp/wired.txt")
            )
            .is_some(),
            "a set through the shared map must be visible on the orchestrator"
        );
    }

    #[tokio::test]
    async fn shared_read_state_map_feeds_post_compact_restore() {
        // End-to-end wiring proof: when the composition root shares its map (the
        // one the file tools write via `readFileState.set`) into the
        // orchestrator, a tool's read feeds the post-compact file restore. Before
        // P1-06 the orchestrator held a THIRD, unshared map, so this restore was
        // always empty in production.
        use compaction::CompactionOrchestrator;
        use protocol::{ConversationMessage, MessageId};
        use traits::OrchestratorHandle;

        // The composition-root-owned map (also handed to `BuiltinToolContext`).
        let shared = tool_api::read_file_state::new_read_file_state_map();
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![])
            .with_compaction(Arc::new(CompactionOrchestrator::new(10)))
            .with_read_state_map(shared.clone());
        let orch = Arc::new(orch);

        {
            let session = orch.session();
            let mut s = session.lock().await;
            for i in 0..20 {
                s.history.push(ConversationMessage::user(
                    MessageId::new(),
                    format!(
                        "turn-{i} padded body text to push the token estimate over the threshold"
                    ),
                ));
            }
        }

        // A tool's `readFileState.set` goes through the SHARED handle (as the
        // real `FileReadTool` does via its `BuiltinToolContext.read_file_state`).
        // P2-12: the post-compact restore RE-READS from disk, so the file must
        // exist on disk with the asserted content.
        let dir = tempfile::tempdir().expect("tempdir");
        let tool_read_path = dir.path().join("tool_read.rs");
        std::fs::write(&tool_read_path, "fn tool_read() {}\n").expect("write tool_read.rs");
        tool_api::read_file_state::set(
            &shared,
            tool_read_path.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: "fn tool_read() {}\n".into(),
                mtime_ms: 321,
                offset: None,
                limit: None,
                from_read: true,
            },
        );

        orch.force_compact().await.expect("force_compact ok");

        // The shared map is drained/cleared post-compact.
        assert!(
            orch.read_state_map.lock().unwrap().is_empty(),
            "shared read_state_map must be cleared after compaction"
        );

        // The tool's read was restored as a post-compact attachment — proving the
        // share, not the orchestrator's own now-removed default, fed the restore.
        let session = orch.session();
        let s = session.lock().await;
        let restored = s.history.iter().any(|m| match m {
            ConversationMessage::User {
                content,
                is_meta: true,
                ..
            } => content.iter().any(|b| match b {
                protocol::ContentBlock::Text { text } => {
                    text.contains("restored after compaction")
                        && text.contains(&tool_read_path.display().to_string())
                        && text.contains("fn tool_read() {}")
                }
                _ => false,
            }),
            _ => false,
        });
        assert!(
            restored,
            "a tool's read through the shared map must feed post-compact restore"
        );
    }

    #[tokio::test]
    async fn cache_skips_errored_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd }) as Arc<dyn Tool>],
        );
        // File does not exist → the tool errors → nothing is cached.
        dispatch_one(&orch, "Read", json!({ "file_path": "missing.rs" })).await;
        assert!(orch.files_in_context().await.is_empty());
    }

    #[tokio::test]
    async fn cache_insertion_order_and_dedup() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), b"a").expect("write a");
        std::fs::write(dir.path().join("b.rs"), b"b").expect("write b");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }) as Arc<dyn Tool>],
        );
        // a, b, then a again — first-insertion order [a, b], a not duplicated.
        // This 2-file re-read case coincides with TS's MRU LRU (also [a, b]);
        // the divergence only appears at ≥3 files — see
        // `three_file_reread_locks_first_insertion_order` below.
        dispatch_one(&orch, "Read", json!({ "file_path": "a.rs" })).await;
        dispatch_one(&orch, "Read", json!({ "file_path": "b.rs" })).await;
        dispatch_one(&orch, "Read", json!({ "file_path": "a.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("a.rs"), cwd.join("b.rs")]);
    }

    #[tokio::test]
    async fn three_file_reread_locks_first_insertion_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        for f in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.path().join(f), b"x").expect("write");
        }
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }) as Arc<dyn Tool>],
        );
        // read a, b, c, then a again. This `Vec` keeps first-insertion order
        // [a, b, c]; TS's MRU-promoting LRU would diverge to [a, c, b]. Locking
        // [a, b, c] pins the documented divergence so a future switch to MRU
        // semantics cannot pass silently.
        for f in ["a.rs", "b.rs", "c.rs", "a.rs"] {
            dispatch_one(&orch, "Read", json!({ "file_path": f })).await;
        }
        let files = orch.files_in_context().await;
        assert_eq!(
            files,
            vec![cwd.join("a.rs"), cwd.join("b.rs"), cwd.join("c.rs")]
        );
    }

    #[tokio::test]
    async fn notebook_edit_records_notebook_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("nb.ipynb"), b"{}").expect("write nb");
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool {
                name: "NotebookEdit",
                cwd: cwd.clone(),
            }) as Arc<dyn Tool>],
        );
        // `NotebookEdit` keys the cache on `notebook_path`; the stub reads
        // `file_path` to confirm the file exists, so pass both (same path).
        dispatch_one(
            &orch,
            "NotebookEdit",
            json!({ "notebook_path": "nb.ipynb", "file_path": "nb.ipynb" }),
        )
        .await;
        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("nb.ipynb")]);
    }

    #[tokio::test]
    async fn edit_and_write_record_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("e.rs"), b"e").expect("write e");
        std::fs::write(dir.path().join("w.rs"), b"w").expect("write w");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![
                Arc::new(StubFileTool {
                    name: "Edit",
                    cwd: cwd.clone(),
                }) as Arc<dyn Tool>,
                Arc::new(StubFileTool {
                    name: "Write",
                    cwd: cwd.clone(),
                }) as Arc<dyn Tool>,
            ],
        );
        dispatch_one(&orch, "Edit", json!({ "file_path": "e.rs" })).await;
        dispatch_one(&orch, "Write", json!({ "file_path": "w.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("e.rs"), cwd.join("w.rs")]);
    }
}
// ============================================================================
// A1: max_output_tokens recovery (multi-turn nudge + escalation/exhaustion).
// Drives `execute_one_turn_with_recovery` directly with a `max_tokens`-scripted
// MockApiClient and asserts the nudge injection, counter increments, and
// disposition (Continue while under the limit; Ended on exhaustion).
// ============================================================================
#[cfg(test)]
mod max_output_tokens_recovery_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::turn_loop::{
        execute_one_turn_with_recovery, RecoveryState, TurnStepOutcome, ESCALATED_MAX_TOKENS,
        MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
    };
    use crate::OrchestratorConfig;
    use llm_client::LlmResponse;
    use protocol::{ContentBlock, ConversationMessage};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator whose batched API returns the given scripted
    /// `LlmResponse`s in order. No tools registered (recovery never needs
    /// them).
    fn orch_with_responses(responses: Vec<LlmResponse>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A `max_tokens` response carrying one text block.
    fn max_tokens_response() -> LlmResponse {
        mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "partial".into(),
                cache_control: None,
            }],
            Some("max_tokens"),
        )
    }

    /// Snapshot the current session history.
    async fn history(orch: &ConversationOrchestrator) -> Vec<ConversationMessage> {
        orch.session.lock().await.history.clone()
    }

    /// The exact-bytes nudge string is byte-faithful to TS `query.ts:1226-1227`,
    /// including the U+2014 em-dash and the single space joining the two literals.
    #[test]
    fn nudge_string_is_byte_exact() {
        assert_eq!(
            MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
            "Output token limit hit. Resume directly \u{2014} no apology, no recap of what you were doing. Pick up mid-thought if that is where the cut happened. Break remaining work into smaller pieces."
        );
        // The em-dash is U+2014, not an ASCII hyphen or U+2013 en-dash.
        assert!(MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.contains('\u{2014}'));
        assert!(!MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.contains("directly -"));
    }

    #[test]
    fn recovery_limit_is_three() {
        assert_eq!(MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, 3);
    }

    /// (Test plan 1) `max_tokens` at recovery_count 0 → Continue, the exact
    /// nudge is appended as a User message, and the counter becomes 1.
    #[tokio::test]
    async fn max_tokens_at_count_zero_continues_and_injects_nudge() {
        let orch = orch_with_responses(vec![max_tokens_response()]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_recovery_count, 1);
        assert_eq!(state.max_output_tokens_override, None);

        // History: [assistant(max_tokens), user(nudge)].
        let h = history(&orch).await;
        let last = h.last().expect("nudge appended");
        match last {
            ConversationMessage::User {
                content, is_meta, ..
            } => {
                assert_eq!(content.len(), 1, "single text block");
                // (parity 2.1.207 P2-05) the recovery nudge is a META user
                // message (`createUserMessage({…, isMeta:!0})`).
                assert!(*is_meta, "max-output-tokens recovery nudge must be is_meta");
                match &content[0] {
                    // (Test plan 4) the nudge is a User message with exact bytes.
                    ContentBlock::Text { text } => {
                        assert_eq!(text, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE);
                    }
                    other => panic!("expected text block, got {other:?}"),
                }
            }
            other => panic!("expected User nudge message, got {other:?}"),
        }
    }

    /// (parity 2.1.207 P2-05) End-to-end persist: driving a real `max_tokens`
    /// recovery turn through the orchestrator with a wired JSONL writer stamps the
    /// injected recovery nudge with the top-level `isMeta:true` envelope flag, so
    /// title / first-prompt / fork-name / visible-count extraction skips it.
    #[tokio::test]
    async fn max_tokens_recovery_nudge_persists_with_top_level_is_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(
            platform_posix::fs::PosixFileSystem::new(dir.path().to_path_buf()),
        );
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![max_tokens_response()])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
        .with_jsonl_writer(writer);

        let mut state = RecoveryState::default();
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));

        let contents = tokio::fs::read_to_string(&session_path)
            .await
            .expect("jsonl written");
        let nudge_line = contents
            .lines()
            .find(|l| l.contains(MAX_OUTPUT_TOKENS_RECOVERY_NUDGE))
            .expect("recovery nudge line persisted to JSONL");
        let v: serde_json::Value =
            serde_json::from_str(nudge_line).expect("nudge line is valid JSON");
        assert_eq!(v["type"], "user", "nudge persists as a user line");
        assert_eq!(
            v["isMeta"],
            serde_json::Value::Bool(true),
            "recovery nudge must carry top-level isMeta:true: {nudge_line}"
        );
    }

    /// (Test plan 1) `max_tokens` at counts 1 and 2 → Continue, counter
    /// increments to 2 then 3. A fresh `max_tokens` is queued per step.
    #[tokio::test]
    async fn max_tokens_at_counts_one_and_two_continue_and_increment() {
        let orch = orch_with_responses(vec![max_tokens_response(), max_tokens_response()]);
        let mut state = RecoveryState {
            max_output_tokens_recovery_count: 1,
            max_output_tokens_override: None,
            max_output_tokens_escalated: false,
            ..Default::default()
        };

        // count 1 → 2
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_recovery_count, 2);

        // count 2 → 3 (still < limit, so still nudges)
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_recovery_count, 3);

        // Two nudges were appended (one per step).
        let h = history(&orch).await;
        let nudges = h
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    ConversationMessage::User { content, .. }
                        if matches!(content.first(), Some(ContentBlock::Text { text })
                            if text == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
                )
            })
            .count();
        assert_eq!(nudges, 2);
    }

    /// (Test plan 2) the 4th consecutive `max_tokens` (count already at the
    /// limit of 3) → Ended with stop_reason `max_tokens`, no further nudge.
    #[tokio::test]
    async fn fourth_consecutive_max_tokens_ends_turn() {
        let orch = orch_with_responses(vec![max_tokens_response()]);
        let mut state = RecoveryState {
            max_output_tokens_recovery_count: MAX_OUTPUT_TOKENS_RECOVERY_LIMIT,
            max_output_tokens_override: None,
            max_output_tokens_escalated: false,
            ..Default::default()
        };

        let len_before = history(&orch).await.len();
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "max_tokens");
            }
            TurnStepOutcome::Continue => panic!("expected Ended on exhaustion"),
        }
        // The counter is NOT incremented past the limit, and NO nudge is
        // appended on exhaustion. The step appends the response assistant message
        // AND the surfaced terminal `API Error: …` assistant message (#24 batched
        // parity with the streaming terminal arm) → +2.
        assert_eq!(
            state.max_output_tokens_recovery_count,
            MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
        );
        let h = history(&orch).await;
        assert_eq!(
            h.len(),
            len_before + 2,
            "the step's assistant msg + the surfaced terminal API-error msg"
        );
        // The last message is the surfaced terminal API-error assistant.
        match h.last() {
            Some(ConversationMessage::Assistant {
                content,
                stop_reason,
                ..
            }) => {
                assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
                let ContentBlock::Text { text } = &content[0] else {
                    panic!("expected a text block");
                };
                assert!(
                    text.starts_with("API Error: Claude's response exceeded"),
                    "got: {text}"
                );
            }
            other => panic!("expected the surfaced Assistant API-error, got {other:?}"),
        }
    }

    /// #24 batched parity: a terminal `model_context_window_exceeded` surfaces
    /// the byte-locked `API Error: …` assistant message AND ends the turn
    /// (previously fell through to `_ => Continue` and bare-re-called the API).
    #[tokio::test]
    async fn model_context_window_exceeded_surfaces_error_and_ends() {
        let orch = orch_with_responses(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "partial".into(),
                cache_control: None,
            }],
            Some("model_context_window_exceeded"),
        )]);
        let mut state = RecoveryState::default();
        let len_before = history(&orch).await.len();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "model_context_window_exceeded");
            }
            TurnStepOutcome::Continue => panic!("expected Ended, not a bare re-call"),
        }
        let h = history(&orch).await;
        assert_eq!(
            h.len(),
            len_before + 2,
            "response asst + surfaced API-error asst"
        );
        let Some(ConversationMessage::Assistant {
            content,
            stop_reason,
            ..
        }) = h.last()
        else {
            panic!("expected the surfaced Assistant API-error");
        };
        assert_eq!(
            stop_reason.as_deref(),
            Some("model_context_window_exceeded")
        );
        let ContentBlock::Text { text } = &content[0] else {
            panic!("expected a text block");
        };
        assert_eq!(
            text,
            "API Error: The model has reached its context window limit."
        );
    }

    /// #24 batched parity: a terminal `refusal` with NO `refusalFallbackModel`
    /// (the swap arm returns false) surfaces the byte-locked Usage-Policy
    /// `API Error: …` message AND ends the turn (previously bare-re-called).
    #[tokio::test]
    async fn terminal_refusal_without_fallback_surfaces_error_and_ends() {
        // Default config has no refusalFallbackModel → maybe_swap returns false.
        let orch = orch_with_responses(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "partial".into(),
                cache_control: None,
            }],
            Some("refusal"),
        )]);
        let mut state = RecoveryState::default();
        let len_before = history(&orch).await.len();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "refusal"),
            TurnStepOutcome::Continue => panic!("expected Ended, not a bare re-call"),
        }
        let h = history(&orch).await;
        assert_eq!(
            h.len(),
            len_before + 2,
            "response asst + surfaced API-error asst"
        );
        let Some(ConversationMessage::Assistant {
            content,
            stop_reason,
            ..
        }) = h.last()
        else {
            panic!("expected the surfaced Assistant API-error");
        };
        assert_eq!(stop_reason.as_deref(), Some("refusal"));
        let ContentBlock::Text { text } = &content[0] else {
            panic!("expected a text block");
        };
        // Either the labelled "safety measures" or the generic Usage-Policy
        // variant — both are `API Error: …` and cite the AUP URL.
        assert!(text.starts_with("API Error:"), "got: {text}");
        assert!(
            text.contains("https://www.anthropic.com/legal/aup"),
            "got: {text}"
        );
    }

    /// (Test plan 3) a normal `end_turn` is unaffected by the recovery wiring:
    /// it Ends with `end_turn`, never touches the recovery counter, and appends
    /// no nudge.
    #[tokio::test]
    async fn normal_end_turn_unaffected_by_recovery() {
        let orch = orch_with_responses(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        )]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            TurnStepOutcome::Continue => panic!("expected Ended"),
        }
        assert_eq!(state.max_output_tokens_recovery_count, 0);
        let h = history(&orch).await;
        // [assistant] only — no nudge.
        assert!(matches!(
            h.last(),
            Some(ConversationMessage::Assistant { .. })
        ));
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
        )));
    }

    /// As [`orch_with_responses`] but with the REC.A1 8k→64k escalation enabled.
    fn orch_with_responses_escalating(responses: Vec<LlmResponse>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig {
                escalate_max_output_tokens: true,
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// REC.A1: with escalation ON, the FIRST `max_tokens` arms the 64k override
    /// and the once-per-episode gate, and returns `Continue` WITHOUT a nudge —
    /// the single-shot retry fires before the multi-turn nudge
    /// (TS `query.ts:1199-1221`).
    #[tokio::test]
    async fn escalation_arms_override_and_continues_without_nudge() {
        let orch = orch_with_responses_escalating(vec![max_tokens_response()]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_override, Some(ESCALATED_MAX_TOKENS));
        assert!(state.max_output_tokens_escalated);
        // No nudge counted/injected — the escalation precedes the nudge path.
        assert_eq!(state.max_output_tokens_recovery_count, 0);
        let h = history(&orch).await;
        assert!(
            !matches!(h.last(), Some(ConversationMessage::User { .. })),
            "escalation must not inject a nudge; got {:?}",
            h.last()
        );
    }

    /// REC.A1: once escalated, a SECOND `max_tokens` TAKEs the armed override
    /// (so the retry used 64k) and, since the episode already escalated, falls
    /// through to the multi-turn nudge instead of escalating again — no
    /// escalate-forever loop.
    #[tokio::test]
    async fn second_max_tokens_after_escalation_takes_override_then_nudges() {
        let orch = orch_with_responses_escalating(vec![max_tokens_response()]);
        let mut state = RecoveryState {
            max_output_tokens_override: Some(ESCALATED_MAX_TOKENS),
            max_output_tokens_escalated: true,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        // The one-shot override was consumed for this call; the nudge path ran.
        assert_eq!(state.max_output_tokens_override, None);
        assert!(
            state.max_output_tokens_escalated,
            "stays escalated for the rest of this episode"
        );
        assert_eq!(state.max_output_tokens_recovery_count, 1);
        assert!(
            matches!(
                history(&orch).await.last(),
                Some(ConversationMessage::User { .. })
            ),
            "nudge appended after the escalation was exhausted"
        );
    }

    /// The legacy 2-arg shim (`recovery = None`) preserves the bare behavior:
    /// `max_tokens` falls through to Continue WITHOUT injecting a nudge — the
    /// cancelable REPL driver depends on this no-op.
    #[tokio::test]
    async fn legacy_shim_does_not_recover_on_max_tokens() {
        let orch = orch_with_responses(vec![max_tokens_response()]);
        let step = crate::turn_loop::execute_one_turn(&orch, None)
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        let h = history(&orch).await;
        // Only the assistant message; no nudge appended by the shim.
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
        )));
    }
}
// ============================================================================
// #77 malformed-tool-use retry + #78 thinking-only nudge (BATCHED path).
// Drives `execute_one_turn_with_recovery_tracked` with a scripted `LlmResponse`
// whose stop_reason / blocks force each branch, then asserts the byte-exact
// nudge injection, the per-turn guard transitions, and the disposition.
// ============================================================================
#[cfg(test)]
mod malformed_and_thinking_only_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::turn_loop::{
        execute_one_turn, execute_one_turn_with_recovery, prior_assistant_used_structured_output,
        RecoveryState, TurnStepOutcome, MALFORMED_TOOL_USE_RETRY_FAILED,
        MALFORMED_TOOL_USE_RETRY_NUDGE, STRUCTURED_OUTPUT_TOOL_NAME, THINKING_ONLY_NUDGE,
    };
    use crate::OrchestratorConfig;
    use llm_client::LlmResponse;
    use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn orch_with_responses(responses: Vec<LlmResponse>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A response whose `stop_reason` is `tool_use` but which carries ZERO
    /// `tool_use` blocks (only a text block) — the #77 malformed shape.
    fn malformed_tool_use_response() -> LlmResponse {
        mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "I'll call the tool".into(),
                cache_control: None,
            }],
            Some("tool_use"),
        )
    }

    /// A thinking-only response: `end_turn` `stop_reason` but only a `Reasoning`
    /// (thinking) block — no visible text. The #78 shape.
    fn thinking_only_response(stop_reason: &str) -> LlmResponse {
        mock_message_response(
            vec![llm_client::ContentBlock::Reasoning {
                text: "thinking quietly".into(),
                signature: None,
            }],
            Some(stop_reason),
        )
    }

    async fn history(orch: &ConversationOrchestrator) -> Vec<ConversationMessage> {
        orch.session.lock().await.history.clone()
    }

    fn last_user_text(h: &[ConversationMessage]) -> Option<String> {
        match h.last()? {
            ConversationMessage::User { content, .. } => match content.first()? {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// `is_meta` flag of the last history entry when it is a User message.
    /// (parity 2.1.207 P2-05: recovery/continuation nudges are `isMeta:!0`.)
    fn last_user_is_meta(h: &[ConversationMessage]) -> Option<bool> {
        match h.last()? {
            ConversationMessage::User { is_meta, .. } => Some(*is_meta),
            _ => None,
        }
    }

    // ---- byte-exact strings ------------------------------------------------

    #[test]
    fn malformed_nudge_strings_are_byte_exact() {
        // Default build: clean-retry feature flag OFF (`PZa()` defaults false),
        // so the first-failure string is the non-clean-retry variant.
        assert_eq!(
            MALFORMED_TOOL_USE_RETRY_NUDGE,
            "Your tool call was malformed and could not be parsed. Please retry."
        );
        assert_eq!(
            MALFORMED_TOOL_USE_RETRY_FAILED,
            "The model's tool call could not be parsed (retry also failed)."
        );
    }

    #[test]
    fn thinking_only_nudge_is_byte_exact() {
        assert_eq!(
            THINKING_ONLY_NUDGE,
            "[Your previous response had no visible output. Please continue and produce a user-visible response.]"
        );
    }

    // ---- #77 malformed-tool-use -------------------------------------------

    /// First malformed `tool_use` → Continue, byte-exact nudge appended as a
    /// user message, guard armed, recovery reset.
    #[tokio::test]
    async fn malformed_tool_use_first_failure_continues_and_nudges() {
        let orch = orch_with_responses(vec![malformed_tool_use_response()]);
        let mut state = RecoveryState {
            // Pre-seed a non-zero recovery count to prove it gets reset.
            max_output_tokens_recovery_count: 2,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.malformed_tool_use_retried, "guard armed");
        assert_eq!(
            state.max_output_tokens_recovery_count, 0,
            "recovery reset on retry transition"
        );

        let h = history(&orch).await;
        assert_eq!(
            last_user_text(&h).as_deref(),
            Some(MALFORMED_TOOL_USE_RETRY_NUDGE)
        );
        assert_eq!(
            last_user_is_meta(&h),
            Some(true),
            "malformed-tool retry nudge must be a META user message (isMeta:!0)"
        );
    }

    /// Second malformed `tool_use` (guard already armed) → Ended with
    /// stop_reason `end_turn`, the non-meta terminal message appended as an
    /// ASSISTANT api-error message (binary `ql`→`mcc`: stop_reason
    /// "stop_sequence"), NOT a user message.
    #[tokio::test]
    async fn malformed_tool_use_second_failure_ends_turn() {
        let orch = orch_with_responses(vec![malformed_tool_use_response()]);
        let mut state = RecoveryState {
            malformed_tool_use_retried: true,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "end_turn");
            }
            TurnStepOutcome::Continue => panic!("expected Ended on second failure"),
        }
        let h = history(&orch).await;
        // Terminal message = ASSISTANT api-error message, stop_reason
        // "stop_sequence"; it is NOT persisted as a user message.
        match h.last().expect("history non-empty") {
            ConversationMessage::Assistant {
                content,
                stop_reason,
                ..
            } => {
                assert_eq!(stop_reason.as_deref(), Some("stop_sequence"));
                assert!(matches!(
                    content.first(),
                    Some(ContentBlock::Text { text }) if text == MALFORMED_TOOL_USE_RETRY_FAILED
                ));
            }
            other => panic!("expected Assistant api-error message, got {other:?}"),
        }
        assert_eq!(last_user_text(&h), None);
    }

    /// A NORMAL `tool_use` response (with an actual tool_use block) must NOT
    /// trigger the malformed path — it Continues to dispatch as usual and
    /// injects no malformed nudge.
    #[tokio::test]
    async fn normal_tool_use_does_not_trigger_malformed_path() {
        let resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: "toolu_1".into(),
                name: "Nope".into(), // unknown tool → synthetic error, still dispatched
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        );
        let orch = orch_with_responses(vec![resp]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(!state.malformed_tool_use_retried, "guard NOT armed");
        let h = history(&orch).await;
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MALFORMED_TOOL_USE_RETRY_NUDGE)
        )));
    }

    /// The legacy shim (`recovery == None`) keeps the historical `_ => Continue`
    /// no-op on a malformed `tool_use` — no nudge.
    #[tokio::test]
    async fn legacy_shim_does_not_handle_malformed_tool_use() {
        let orch = orch_with_responses(vec![malformed_tool_use_response()]);
        let step = execute_one_turn(&orch, None).await.expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        let h = history(&orch).await;
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MALFORMED_TOOL_USE_RETRY_NUDGE)
        )));
    }

    // ---- #78 nudge guard `!Pt(ce)` (StructuredOutput exchange) -------------

    fn so_tool_use_assistant() -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id: ToolUseId::new(),
                name: STRUCTURED_OUTPUT_TOOL_NAME.to_string(),
                input: serde_json::json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        }
    }

    fn tool_result_user() -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content: "Structured output provided successfully".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
        }
    }

    fn real_user(text: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_meta: false,
        }
    }

    fn empty_assistant() -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![],
            stop_reason: None,
        }
    }

    #[test]
    fn pt_true_skips_tool_result_carrier_to_reach_structured_output() {
        // [real user] [assistant StructuredOutput tool_use] [user tool_result]
        // [current empty assistant]: scanning back, the carrier is skipped (Jde)
        // and the StructuredOutput assistant is reached BEFORE any real user ⇒ true.
        let h = vec![
            real_user("emit JSON"),
            so_tool_use_assistant(),
            tool_result_user(),
            empty_assistant(),
        ];
        assert!(prior_assistant_used_structured_output(&h));
    }

    #[test]
    fn pt_false_when_real_user_precedes_any_structured_output() {
        // No StructuredOutput before the most recent real user turn ⇒ false (the
        // `Sn.type==="user" && !isMeta && !Jde ⇒ return!1` short-circuit).
        let h = vec![
            so_tool_use_assistant(),
            real_user("new question"),
            empty_assistant(),
        ];
        assert!(!prior_assistant_used_structured_output(&h));
    }

    #[test]
    fn pt_false_for_non_structured_output_tool_use() {
        // An assistant that used a DIFFERENT tool is not a match; scanning hits
        // the real user and returns false.
        let other = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id: ToolUseId::new(),
                name: "Read".into(),
                input: serde_json::json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        };
        let h = vec![
            real_user("read it"),
            other,
            tool_result_user(),
            empty_assistant(),
        ];
        assert!(!prior_assistant_used_structured_output(&h));
    }

    #[test]
    fn pt_skips_meta_user_messages() {
        // A meta user message (isMeta) between the StructuredOutput assistant and
        // the current response is skipped, not treated as a real user turn.
        let mut meta = real_user("[meta nudge]");
        if let ConversationMessage::User { is_meta, .. } = &mut meta {
            *is_meta = true;
        }
        let h = vec![so_tool_use_assistant(), meta, empty_assistant()];
        assert!(prior_assistant_used_structured_output(&h));
    }

    #[test]
    fn pt_false_on_empty_history() {
        assert!(!prior_assistant_used_structured_output(&[]));
    }

    // ---- #78 thinking-only -------------------------------------------------

    /// An `end_turn` thinking-only response (not yet nudged) → Continue with
    /// the byte-exact nudge appended, guard armed.
    #[tokio::test]
    async fn thinking_only_end_turn_first_time_nudges() {
        let orch = orch_with_responses(vec![thinking_only_response("end_turn")]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.thinking_only_nudged, "guard armed");
        let h = history(&orch).await;
        assert_eq!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
        assert_eq!(
            last_user_is_meta(&h),
            Some(true),
            "thinking-only nudge must be a META user message (isMeta:!0)"
        );
    }

    /// A `stop_sequence` thinking-only response also triggers the nudge.
    #[tokio::test]
    async fn thinking_only_stop_sequence_nudges() {
        let orch = orch_with_responses(vec![thinking_only_response("stop_sequence")]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.thinking_only_nudged);
        let h = history(&orch).await;
        assert_eq!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
        assert_eq!(
            last_user_is_meta(&h),
            Some(true),
            "thinking-only nudge must be a META user message (isMeta:!0)"
        );
    }

    /// Once nudged, a still-thinking-only `end_turn` ends the turn normally
    /// (no second nudge).
    #[tokio::test]
    async fn thinking_only_already_nudged_ends_turn() {
        let orch = orch_with_responses(vec![thinking_only_response("end_turn")]);
        let mut state = RecoveryState {
            thinking_only_nudged: true,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            TurnStepOutcome::Continue => panic!("expected Ended once already nudged"),
        }
        let h = history(&orch).await;
        assert_ne!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }

    /// An `end_turn` response WITH visible text ends the turn — never nudged.
    #[tokio::test]
    async fn end_turn_with_visible_text_does_not_nudge() {
        let resp = mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "Here is the answer.".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = orch_with_responses(vec![resp]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Ended { .. }));
        assert!(!state.thinking_only_nudged);
        let h = history(&orch).await;
        assert_ne!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }

    /// Whitespace-only text counts as NOT visible (`.trim()` empty) → nudged.
    #[tokio::test]
    async fn whitespace_only_text_is_not_visible() {
        let resp = mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "   \n  ".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = orch_with_responses(vec![resp]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.thinking_only_nudged);
    }

    /// The legacy shim (`recovery == None`) does NOT nudge on a thinking-only
    /// `end_turn`; it ends the turn as before.
    #[tokio::test]
    async fn legacy_shim_does_not_handle_thinking_only() {
        let orch = orch_with_responses(vec![thinking_only_response("end_turn")]);
        let step = execute_one_turn(&orch, None).await.expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            TurnStepOutcome::Continue => panic!("legacy shim should end on end_turn"),
        }
        let h = history(&orch).await;
        assert_ne!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }
}
/// HOOK.1 / HOOK.2 / HOOK.3 — `PreToolUse` hook behaviors surfaced by the turn
/// loop's `dispatch_tool_uses` chokepoint (TS `services/tools/toolExecution.ts`
/// + `toolHooks.ts` + `query.ts:1518-1521`).
#[cfg(test)]
mod pre_tool_hook_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, MockApiClient, MockOutputStream, PermissionDecision,
        PermissionDecisionSource, PermissionGate, PermissionResolution, StaticMemoryProvider,
    };
    use crate::turn_loop::{dispatch_tool_uses_tracked, execute_one_turn, TurnStepOutcome};
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::{HookEvent, HookEventType};
    use hooks::executor::BuiltinHookHandler;
    use hooks::registry::{HookContext, HookRegistry};
    use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
    use hooks::HookExecutorImpl;
    use protocol::{ContentBlock, ConversationMessage, HookId, MessageId, ToolUseId};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    // ----- unused transport/runtime stubs for the builtin-only executor -----
    struct UnusedHttp;
    #[async_trait]
    impl traits::HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }
    struct UnusedRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _d: std::time::Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    /// Builtin `PreToolUse` handler that returns a fixed [`HookResponse`].
    struct FixedPreHook {
        response: HookResponse,
    }
    #[async_trait]
    impl BuiltinHookHandler for FixedPreHook {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(self.response.clone()),
            }
        }
        fn id(&self) -> &str {
            "fixed-pre"
        }
    }

    /// Build a `HookExecutorImpl` with a single unconditional `PreToolUse` hook
    /// that yields `response`.
    fn pre_hook_executor(response: HookResponse) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-pre".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-pre".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPreHook { response }));
        Arc::new(exec)
    }

    /// Build a `HookExecutorImpl` with a single hook registered for `event` that
    /// yields `response` (reusing the `FixedPreHook` handler, which answers any
    /// event it is invoked for). Used to register a `PermissionRequest` hook.
    fn event_hook_executor(event: HookEventType, response: HookResponse) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-evt".into(),
            events: vec![event],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-pre".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPreHook { response }));
        Arc::new(exec)
    }

    /// Builtin hook handler that COUNTS its invocations (so a test can assert a
    /// hook event fired — or did NOT fire), returning a default success response.
    struct RecordingHook {
        fired: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl BuiltinHookHandler for RecordingHook {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            self.fired.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }
        fn id(&self) -> &str {
            "recording"
        }
    }

    /// Build a `HookExecutorImpl` with a single counting hook registered for
    /// `event`; `fired` is incremented each time the hook runs.
    fn recording_executor(
        event: HookEventType,
        fired: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "recording".into(),
            events: vec![event],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "recording".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(RecordingHook { fired }));
        Arc::new(exec)
    }

    // ----- FIX 2: HookContext transcript_path + permission_mode -------------

    /// Builtin PreToolUse hook that CAPTURES the [`HookContext`] it was handed,
    /// so a test can assert the fire-site populated `transcript_path` +
    /// `permission_mode` (claude-code `createBaseHookInput` always sets
    /// `transcript_path`; PreToolUse/PostToolUse add `permission_mode`).
    struct CtxCapturingHook {
        seen: Arc<std::sync::Mutex<Option<HookContext>>>,
    }
    #[async_trait]
    impl BuiltinHookHandler for CtxCapturingHook {
        async fn handle(&self, _event: &HookEvent, ctx: &HookContext) -> HookResult {
            *self.seen.lock().unwrap() = Some(ctx.clone());
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }
        fn id(&self) -> &str {
            "ctx-capture"
        }
    }

    fn ctx_capturing_executor(
        seen: Arc<std::sync::Mutex<Option<HookContext>>>,
    ) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "ctx-capture".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "ctx-capture".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(CtxCapturingHook { seen }));
        Arc::new(exec)
    }

    #[tokio::test]
    async fn pre_tool_hook_ctx_carries_transcript_path_and_permission_mode() {
        // Wire a real JSONL writer so `transcript_path` is non-empty (it sources
        // the live writer's path), register a PreToolUse hook that captures the
        // context, dispatch a tool, and assert the fields are populated.
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));

        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            ctx_capturing_executor(seen.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
        .with_jsonl_writer(writer);

        let uses = vec![(ToolUseId::new(), "Echo".into(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let ctx = seen.lock().unwrap().clone().expect("PreToolUse hook fired");
        assert_eq!(
            ctx.transcript_path, session_path,
            "transcript_path must be the live JSONL writer's path (claude-code createBaseHookInput)"
        );
        assert_eq!(
            ctx.permission_mode.as_deref(),
            Some("default"),
            "permission_mode must be 'default' outside plan mode (toolHooks.ts:471)"
        );
    }

    #[tokio::test]
    async fn pre_tool_hook_ctx_transcript_path_is_computed_when_no_writer() {
        // FIX A PRODUCTION PATH: with NO `JsonlWriter` wired (the real production
        // shape — every `with_jsonl_writer` call site is a test) but a `config_home`
        // set, the PreToolUse hook's `transcript_path` must be the
        // deterministically-computed `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`
        // — claude-code `getTranscriptPathForSession`, which `createBaseHookInput`
        // ALWAYS stamps — instead of the empty string the old `unwrap_or_default()`
        // produced.
        use protocol::SessionId;

        let config_home = std::path::PathBuf::from("/home/user/.lingxi");
        let cwd = std::path::PathBuf::from("/Users/me/proj");
        // Pin a known session id so the expected path is deterministic.
        let session_id = SessionId::new();
        let expected = session::jsonl::path::session_path(
            &config_home,
            &cwd.to_string_lossy(),
            &session_id.as_uuid().to_string(),
        );

        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            ctx_capturing_executor(seen.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd.clone(),
        )
        // NOTE: deliberately NO `.with_jsonl_writer(...)` — this is the production
        // shape. Only the config home + a pinned session id are wired.
        .with_config_home(config_home.clone())
        .with_session_id(session_id);

        let uses = vec![(ToolUseId::new(), "Echo".into(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let ctx = seen.lock().unwrap().clone().expect("PreToolUse hook fired");
        assert!(
            !ctx.transcript_path.as_os_str().is_empty(),
            "production transcript_path must be NON-EMPTY when a config_home is wired"
        );
        assert_eq!(
            ctx.transcript_path, expected,
            "transcript_path must be the computed <config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl \
             (claude-code getTranscriptPathForSession) when no JsonlWriter is wired"
        );
        // Correctly shaped: under <config_home>/projects and a `.jsonl` leaf named
        // by the BARE uuid (no `sess:` prefix), matching the on-disk filename.
        assert!(
            ctx.transcript_path
                .starts_with(config_home.join("projects")),
            "computed path must live under <config_home>/projects"
        );
        assert_eq!(
            ctx.transcript_path.file_name().and_then(|s| s.to_str()),
            Some(format!("{}.jsonl", session_id.as_uuid()).as_str()),
            "leaf must be <bare-uuid>.jsonl"
        );
    }

    #[tokio::test]
    async fn pre_tool_hook_ctx_permission_mode_is_plan_in_plan_mode() {
        // When the session is in plan mode, `permission_mode` is "plan" — the
        // faithful approximation of claude-code's permission-mode enum.
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            ctx_capturing_executor(seen.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        orch.session().lock().await.plan_mode = true;

        let uses = vec![(ToolUseId::new(), "Echo".into(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let ctx = seen.lock().unwrap().clone().expect("PreToolUse hook fired");
        assert_eq!(
            ctx.permission_mode.as_deref(),
            Some("plan"),
            "permission_mode must be 'plan' in plan mode"
        );
    }

    /// Permission gate that denies every tool call AT THE PROMPT (`check`), but
    /// leaves `check_after_hook_allow` at the default (Allow) — modeling a gate
    /// with NO deny RULE, only a would-be prompt. A hook 'allow' therefore skips
    /// the prompt and the tool runs (HOOK.3 issue 1: hook-allow skips the prompt).
    struct DenyAllGate;
    #[async_trait]
    impl PermissionGate for DenyAllGate {
        async fn check(&self, _tool: &str, _input: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-gate".into(),
            }
        }
    }

    /// Permission gate modeling an explicit DENY RULE: it denies on BOTH `check`
    /// and `check_after_hook_allow`, so even a hook 'allow' cannot override it
    /// (HOOK.3 issue 1 / claude-code `checkRuleBasedPermissions`).
    struct DenyRuleGate;
    #[async_trait]
    impl PermissionGate for DenyRuleGate {
        async fn check(&self, _tool: &str, _input: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-rule".into(),
            }
        }
        async fn check_after_hook_allow(
            &self,
            _tool: &str,
            _input: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-rule".into(),
            }
        }
    }

    /// Permission gate that returns a DISTINGUISHABLE denial from each entry
    /// point, so a test can assert WHICH method the turn loop routed to:
    /// `check` → "via-check", `check_after_hook_allow` → "via-hook-allow",
    /// `check_in_plan_mode` → "via-plan-mode".
    /// Gate that ALLOWS every call on every path (used by the #37 defer tests
    /// where an IGNORED defer must fall through to a gate that lets the tool
    /// run).
    struct AllowAllGate;
    #[async_trait]
    impl PermissionGate for AllowAllGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn check_after_hook_allow(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Allow
        }
    }

    struct RouteProbeGate;
    #[async_trait]
    impl PermissionGate for RouteProbeGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "via-check".into(),
            }
        }
        async fn check_after_hook_allow(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "via-hook-allow".into(),
            }
        }
        async fn check_in_plan_mode(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "via-plan-mode".into(),
            }
        }
    }

    /// Gate that is ABOUT TO ASK (`resolve_detailed` → `Ask`). Its `check` denies
    /// (models the prompt / headless auto-deny) and `check_after_hook_allow`
    /// allows (no deny rule), so a `PermissionRequest` 'allow' rescues an
    /// otherwise-denied ask, while no PermissionRequest decision delegates to the
    /// (denying) inner.
    struct AskGate;
    #[async_trait]
    impl PermissionGate for AskGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "prompt-denied".into(),
            }
        }
        async fn check_after_hook_allow(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn resolve_detailed(&self, _t: &str, _i: &serde_json::Value) -> PermissionResolution {
            PermissionResolution::Ask
        }
    }

    /// Gate that denies with a configurable SOURCE from `resolve_detailed` (and
    /// denies on `check`), to assert PermissionDenied fires only on a classifier
    /// deny.
    struct SourcedDenyGate(PermissionDecisionSource);
    #[async_trait]
    impl PermissionGate for SourcedDenyGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "sourced-deny".into(),
            }
        }
        async fn resolve_detailed(&self, _t: &str, _i: &serde_json::Value) -> PermissionResolution {
            PermissionResolution::Deny {
                reason: "sourced-deny".into(),
                source: self.0,
                behavior_ask: false,
                content_blocks: Vec::new(),
            }
        }
    }

    /// A tool that always succeeds with the fixed string `ECHOED-OUTPUT`.
    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "Echo"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "echo".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "content": "ECHOED-OUTPUT" }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// FORK (codex #5 follow-up): a tool that records the
    /// `fork_parent_system_prompt` from the `ToolUseContext` it is dispatched
    /// with, so a test can assert `dispatch_tool_uses_tracked` threads the
    /// turn's recorded system prompt onto every tool's context.
    struct CaptureSystemPromptTool {
        captured: Arc<std::sync::Mutex<Option<Option<String>>>>,
    }
    #[async_trait]
    impl Tool for CaptureSystemPromptTool {
        fn name(&self) -> &str {
            "Capture"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "capture".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            *self.captured.lock().unwrap() = Some(ctx.fork_parent_system_prompt.clone());
            Ok(ToolCallResult {
                data: json!({ "content": "ok" }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// FORK (codex #5 follow-up): after the turn driver records the rendered
    /// system prompt via `save_current_turn_system_prompt`,
    /// `dispatch_tool_uses_tracked` must thread those exact bytes onto every
    /// tool's `ToolUseContext::fork_parent_system_prompt` (the field the fork
    /// path reads to give the child a byte-identical cache prefix).
    #[tokio::test]
    async fn dispatch_threads_recorded_system_prompt_onto_tool_ctx() {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(CaptureSystemPromptTool {
            captured: captured.clone(),
        }) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let parent_bytes = "PARENT RENDERED SYSTEM PROMPT";
        orch.save_current_turn_system_prompt(Some(parent_bytes))
            .await;

        let uses = vec![(ToolUseId::new(), "Capture".to_string(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(
            got,
            Some(Some(parent_bytes.to_string())),
            "tool ctx must carry the turn's recorded system prompt bytes"
        );
    }

    /// FORK (codex #5 follow-up): when no system prompt has been recorded (no
    /// successful turn yet, or a turn with no system prompt), the tool ctx
    /// carries `None` — the existing non-fork behavior is unchanged.
    #[tokio::test]
    async fn dispatch_threads_none_when_no_system_prompt_recorded() {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(CaptureSystemPromptTool {
            captured: captured.clone(),
        }) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );

        let uses = vec![(ToolUseId::new(), "Capture".to_string(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(
            got,
            Some(None),
            "tool ctx must carry None with no recorded prompt"
        );
    }

    /// SKILLEXEC.3 (Part A): a tool that succeeds AND injects a follow-up
    /// conversation message (the Skill-tool shape — `ToolCallResult.new_messages`
    /// carrying the expanded skill prompt). Mirrors `EchoTool` but with a
    /// non-empty `new_messages`.
    struct InjectingTool;
    #[async_trait]
    impl Tool for InjectingTool {
        fn name(&self) -> &str {
            "Inject"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "inject".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({
                    "content": "TOOL-RESULT",
                    "model_content": "Launching skill: demo",
                }),
                model_content: None,
                new_messages: vec![ConversationMessage::user(
                    MessageId::new(),
                    "EXPANDED-SKILL-PROMPT".into(),
                )],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator wired with the given hook executor + permission gate
    /// and a single `Echo` tool.
    fn orch_with(
        hooks: Arc<HookExecutorImpl>,
        perms: Arc<dyn PermissionGate>,
        responses: Vec<llm_client::LlmResponse>,
    ) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(registry),
            hooks,
            perms,
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    fn uses() -> Vec<(ToolUseId, String, serde_json::Value, Option<String>)> {
        vec![(ToolUseId::new(), "Echo".into(), json!({}), None)]
    }

    fn tool_result(block: &ContentBlock) -> (&str, bool) {
        match block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => (content.as_str(), *is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // ----- SKILLEXEC.3 (Part A): tool-injected new_messages -----------------

    /// A tool that returns `new_messages` has those messages threaded out of
    /// `dispatch_tool_uses_tracked` as the third tuple element (the Skill-tool
    /// expanded-prompt injection path).
    #[tokio::test]
    async fn dispatch_threads_out_tool_injected_new_messages() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(InjectingTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let skill_tu = ToolUseId::new();
        let uses = vec![(skill_tu.clone(), "Inject".to_string(), json!({}), None)];
        let (results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        // The tool_result block still rides the first tuple element.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error);
        assert_eq!(content, "Launching skill: demo");
        // The injected message is surfaced for the caller to append, PAIRED
        // with the injecting tool's `tool_use_id` (TS `sourceToolUseID`).
        assert_eq!(injected.len(), 1);
        let (injected_msg, injected_tu) = &injected[0];
        assert_eq!(
            *injected_tu, skill_tu,
            "injected message is tagged with the injecting tool's tool_use_id"
        );
        match injected_msg {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(text, "EXPANDED-SKILL-PROMPT"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// SOURCE-TOOL-USE-ID parity: after a skill-style tool injects `new_messages`
    /// through a full turn step, `SessionState::injected_message_sources` maps
    /// each injected message's id → the injecting tool's `tool_use_id` (faithful
    /// port of TS `tagMessagesWithToolUseID` stamping `sourceToolUseID`).
    #[tokio::test]
    async fn injected_message_sources_records_tool_use_id() {
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Inject".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(InjectingTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![api_resp])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let _ = execute_one_turn(&orch, None).await.expect("turn step");
        let s = orch.session.lock().await;
        // Find the injected expanded-skill-prompt message in history.
        let injected = s
            .history
            .iter()
            .find(|m| {
                matches!(m, ConversationMessage::User { content, .. }
                    if content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "EXPANDED-SKILL-PROMPT")))
            })
            .expect("injected skill-prompt message present in history");
        assert_eq!(
            s.injected_message_sources.get(&injected.id()),
            Some(&tu),
            "injected message id maps to the Skill tool's tool_use_id"
        );
        assert_eq!(
            s.injected_message_sources.len(),
            1,
            "exactly one association recorded for one injected message"
        );
    }

    /// SOURCE-TOOL-USE-ID parity (negative): a normal tool that injects NO
    /// `new_messages` (e.g. `Echo`) records NOTHING in the side-table, and the
    /// in-memory association is `#[serde(skip)]` so the JSONL transcript bytes
    /// are unchanged (no `sourceToolUseID` ever written, matching TS).
    #[tokio::test]
    async fn normal_tool_records_no_source_and_serializes_no_field() {
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Echo".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![api_resp],
        );
        let _ = execute_one_turn(&orch, None).await.expect("turn step");
        let s = orch.session.lock().await;
        assert!(
            s.injected_message_sources.is_empty(),
            "a tool with no injected messages records no source associations"
        );
        // The side-table is `#[serde(skip)]`: serializing the session never
        // emits a `sourceToolUseID`/`injected_message_sources` key, so the
        // persisted JSONL bytes stay byte-identical to before this change.
        let json = serde_json::to_string(&*s).expect("serialize session");
        assert!(
            !json.contains("injected_message_sources"),
            "side-table must not serialize: {json}"
        );
        assert!(
            !json.contains("sourceToolUseID"),
            "sourceToolUseID must never reach the wire: {json}"
        );
    }

    /// End-to-end through `execute_one_turn`: the injected message lands in
    /// history IMMEDIATELY AFTER this turn's tool_result user message, in order.
    #[tokio::test]
    async fn new_messages_appended_to_history_after_tool_result() {
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Inject".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(InjectingTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![api_resp])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let _ = execute_one_turn(&orch, None).await.expect("turn step");
        let h = orch.session.lock().await.history.clone();
        // Locate the tool_result user message; the very next message must be the
        // injected expanded-skill-prompt user message.
        let tr_idx = h
            .iter()
            .position(|m| {
                matches!(m, ConversationMessage::User { content, .. }
                    if content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. })))
            })
            .expect("tool_result user message present");
        let injected = &h[tr_idx + 1];
        match injected {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(text, "EXPANDED-SKILL-PROMPT"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message after tool_result, got {other:?}"),
        }
    }

    /// Byte-identical guard: a tool with EMPTY `new_messages` (every existing
    /// tool, e.g. `Echo`) threads out an empty injected vec → no extra history.
    #[tokio::test]
    async fn empty_new_messages_injects_nothing() {
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (_results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .unwrap();
        assert!(
            injected.is_empty(),
            "Echo injects no messages → history is byte-identical to before"
        );
    }

    // ----- HOOK.1: additionalContext / systemMessage surfaced ---------------

    #[tokio::test]
    async fn hook1_additional_context_is_a_separate_message_not_folded() {
        // Parity with claude-code `toolExecution.ts:845` — a PreToolUse hook's
        // `additionalContext` is pushed as its OWN message into
        // `resultingMessages`, INDEPENDENT of the tool_result. It must NOT be
        // concatenated onto the tool_result content. The faithful message shape
        // (`messages.ts:4117-4128`) is a meta user message:
        // `<system-reminder>\nPreToolUse:{tool} hook additional context:
        // {content}\n</system-reminder>`. The hook supplies `additionalContext`
        // (the model-facing channel) — NOT `systemMessage`.
        let resp = HookResponse {
            additional_context: Some("INJECTED-CTX".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = uses();
        let tool_use_id = uses[0].0.clone();
        let (results, prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        assert!(!prevent);

        // (a) the tool_result is the tool's ORIGINAL output, NO appended context.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        assert!(
            !content.contains("INJECTED-CTX"),
            "additionalContext must NOT be folded into the tool_result content: {content:?}"
        );

        // (b) a SEPARATE message carries the additionalContext, tagged with this
        //     tool's `tool_use_id` so it rides the existing `injected` channel
        //     (appended AFTER the tool_result by both drivers, matching the TS
        //     `resultingMessages` push order).
        assert_eq!(
            injected.len(),
            1,
            "additionalContext surfaces as one separate injected message"
        );
        let (msg, tagged_tu) = &injected[0];
        assert_eq!(*tagged_tu, tool_use_id, "tagged with the dispatching tool");
        match msg {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(
                    text,
                    "<system-reminder>\nPreToolUse:Echo hook additional context: INJECTED-CTX\n</system-reminder>"
                ),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hook1_system_message_does_not_reach_the_model() {
        // Parity with claude-code `messages.ts:4258` — a PreToolUse hook's
        // `systemMessage` is routed to a `hook_system_message` attachment whose
        // `normalizeAttachmentForAPI` returns `[]`: it is transcript/user-facing
        // only and NEVER reaches the model. So a hook returning ONLY
        // `systemMessage` (no `additionalContext`) must produce NO model-facing
        // additionalContext message — the `injected` channel stays empty and the
        // tool_result content is untouched.
        let resp = HookResponse {
            system_message: Some("USER-ONLY-NOTE".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = uses();
        let (results, prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        assert!(!prevent);

        // (a) the tool_result is the tool's ORIGINAL output, untouched.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        assert!(
            !content.contains("USER-ONLY-NOTE"),
            "systemMessage must NOT leak into the tool_result content: {content:?}"
        );

        // (b) NO model-facing additionalContext message is emitted.
        assert!(
            injected.is_empty(),
            "systemMessage must NOT reach the model — no injected message expected, got {injected:?}"
        );
    }

    // ----- HOOK.2: continue:false stops the loop ----------------------------

    #[tokio::test]
    async fn hook2_prevent_continuation_flag_is_tracked() {
        let resp = HookResponse {
            prevent_continuation: true,
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (_results, prevent, _injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .unwrap();
        assert!(
            prevent,
            "continue:false must surface as prevent_continuation"
        );
    }

    // ----- FIX C: hook_stopped_continuation meta message --------------------

    #[tokio::test]
    async fn fix_c_pretooluse_prevent_continuation_emits_stopped_message() {
        // Parity with claude-code `toolExecution.ts:1571-1582` — a PreToolUse
        // hook's `continue:false` (preventContinuation) yields a
        // `hook_stopped_continuation` attachment AFTER the tool_result, rendered
        // as `<system-reminder>\nPreToolUse:{tool} hook stopped continuation:
        // {stopReason}\n</system-reminder>` (`messages.ts:4130-4137`). The tool
        // STILL runs (the message is emitted post-execution).
        let resp = HookResponse {
            prevent_continuation: true,
            reason: Some("STOP-NOW".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = uses();
        let tool_use_id = uses[0].0.clone();
        let (results, prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        // continue:false still surfaces as prevent_continuation (ends the turn).
        assert!(prevent, "continue:false surfaces as prevent_continuation");
        // The tool ran: its original output is the tool_result content.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        // A SEPARATE stopped-continuation message rides the injected channel,
        // tagged with this tool's tool_use_id (ordered after the tool_result).
        assert_eq!(
            injected.len(),
            1,
            "exactly one hook_stopped_continuation message"
        );
        let (msg, tagged_tu) = &injected[0];
        assert_eq!(*tagged_tu, tool_use_id, "tagged with the dispatching tool");
        match msg {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(
                    text,
                    "<system-reminder>\nPreToolUse:Echo hook stopped continuation: STOP-NOW\n</system-reminder>"
                ),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fix_c_pretooluse_prevent_continuation_default_reason() {
        // No `stopReason` → claude's default `'Execution stopped by hook'`
        // (`toolExecution.ts:1576`).
        let resp = HookResponse {
            prevent_continuation: true,
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (_results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .unwrap();
        assert_eq!(injected.len(), 1);
        match &injected[0].0 {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(
                    text,
                    "<system-reminder>\nPreToolUse:Echo hook stopped continuation: Execution stopped by hook\n</system-reminder>"
                ),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fix_c_posttooluse_prevent_continuation_emits_stopped_message() {
        // Parity with claude-code `toolHooks.ts:118-130` — a PostToolUse hook's
        // `continue:false` yields a `hook_stopped_continuation` attachment AFTER
        // the tool_result, rendered as `<system-reminder>\nPostToolUse:{tool}
        // hook stopped continuation: {stopReason}\n</system-reminder>`.
        let resp = HookResponse {
            prevent_continuation: true,
            reason: Some("POST-STOP".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            event_hook_executor(HookEventType::PostToolUse, resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = uses();
        let tool_use_id = uses[0].0.clone();
        let (results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        // The tool ran and its result is unchanged.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        assert_eq!(
            injected.len(),
            1,
            "exactly one hook_stopped_continuation message"
        );
        let (msg, tagged_tu) = &injected[0];
        assert_eq!(*tagged_tu, tool_use_id, "tagged with the dispatching tool");
        match msg {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(
                    text,
                    "<system-reminder>\nPostToolUse:Echo hook stopped continuation: POST-STOP\n</system-reminder>"
                ),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hook2_prevent_continuation_ends_the_turn_step() {
        // A turn step that runs a tool whose PreToolUse hook set continue:false
        // ends with stop_reason "hook_stopped" (TS query.ts `{reason:'hook_stopped'}`).
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Echo".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let resp = HookResponse {
            prevent_continuation: true,
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![api_resp],
        );
        match execute_one_turn(&orch, None).await.expect("turn step") {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "hook_stopped");
            }
            TurnStepOutcome::Continue => panic!("expected Ended(hook_stopped), got Continue"),
        }
    }

    #[tokio::test]
    async fn hook2_no_prevent_continuation_continues() {
        // Without continue:false a tool-bearing step keeps looping (Continue).
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Echo".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![api_resp],
        );
        assert!(matches!(
            execute_one_turn(&orch, None).await.expect("turn step"),
            TurnStepOutcome::Continue
        ));
    }

    // ----- HOOK.3: allow bypasses / deny denies / ask falls through ---------

    #[tokio::test]
    async fn hook3_allow_skips_the_prompt_when_no_deny_rule() {
        // permissionDecision "allow"/legacy "approve" parses to Approve and SKIPS
        // the interactive prompt (claude-code `resolveHookPermissionDecision`).
        // `DenyAllGate` would deny at the PROMPT (`check`) but has no deny RULE
        // (`check_after_hook_allow` defaults to Allow), so the hook-allow skips
        // the prompt and the tool runs.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(DenyAllGate), vec![]);
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "hook allow skipped the prompt; tool ran");
        assert!(content.contains("ECHOED-OUTPUT"));
        assert!(!content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn hook3_allow_cannot_override_a_deny_rule() {
        // (HOOK.3 issue 1) A hook 'allow' skips the prompt but must NOT override
        // an explicit deny RULE (claude-code `checkRuleBasedPermissions`).
        // `DenyRuleGate.check_after_hook_allow` denies, so the tool is DENIED even
        // though the hook approved.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(DenyRuleGate), vec![]);
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "a deny rule must override a hook 'allow'");
        // The deny reason reaches the model VERBATIM (no "Permission denied: "
        // wrapper); this test gate emits a raw reason string.
        assert!(content.contains("denied-by-rule"));
        assert!(!content.contains("Permission denied: denied-by-rule"));
        assert!(!content.contains("ECHOED-OUTPUT"), "tool never ran");
    }

    #[tokio::test]
    async fn hook3_deny_denies_before_the_tool_runs() {
        // permissionDecision "deny"/legacy "block" parses to Block → error result.
        let resp = HookResponse {
            decision: Some(HookDecision::Block),
            reason: Some("nope".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            // allow-all gate proves the BLOCK came from the hook, not the gate.
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(content.contains("PreToolUse:Echo hook error: nope"));
        assert!(!content.contains("ECHOED-OUTPUT"), "tool never ran");
    }

    #[tokio::test]
    async fn hook3_ask_falls_through_to_the_gate() {
        // No decision (the "ask"/passthrough case) leaves the gate authoritative.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(DenyAllGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(
            is_error,
            "gate denial applies when the hook makes no decision"
        );
        // Verbatim deny reason (no "Permission denied: " wrapper).
        assert!(content.contains("denied-by-gate"));
        assert!(!content.contains("Permission denied: denied-by-gate"));
    }

    /// UNKNOWN-TOOL: when the model calls a tool name that is not in the
    /// registry, `dispatch_tool_uses_tracked` must return a `ToolResult` whose
    /// content is wrapped in `<tool_use_error>…</tool_use_error>` and whose
    /// `is_error` flag is `true` — matching claude-code byte-for-byte
    /// (`toolExecution.ts`: `"<tool_use_error>Error: No such tool available: …</tool_use_error>"`).
    #[tokio::test]
    async fn unknown_tool_returns_tool_use_error_wrapper() {
        // `orch_with` registers only `EchoTool`, so "NoSuchTool" is not in the registry.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = vec![(ToolUseId::new(), "NoSuchTool".to_string(), json!({}), None)];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "unknown tool must set is_error=true");
        assert_eq!(
            content, "<tool_use_error>Error: No such tool available: NoSuchTool</tool_use_error>",
            "content must match claude-code format byte-for-byte"
        );
    }

    /// A tool whose `call()` always returns `Err(ToolError::Internal("kaboom"))`.
    /// Used to drive the tool-execution-error path in `dispatch_tool_uses_tracked`.
    struct AlwaysFailTool;
    #[async_trait]
    impl Tool for AlwaysFailTool {
        fn name(&self) -> &str {
            "AlwaysFail"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "always fails".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Err(ToolError::Internal("kaboom".into()))
        }
    }

    /// TOOL-EXEC-ERROR (parity): when a registered tool's `call()` returns
    /// `Err(ToolError)`, `dispatch_tool_uses_tracked` must pass the error text
    /// BARE — NOT wrapped in `<tool_use_error>` — matching claude-code's
    /// `toolExecution.ts:1691`:
    ///
    ///   ```js
    ///   const content = formatError(error)   // bare string, e.g. "Error: …"
    ///   ```
    ///
    /// followed by `tool_result.content = content` (line 1721), and every
    /// per-tool `mapToolResultToToolResultBlockParam` (e.g. `NotebookEditTool.ts:137`,
    /// `BashTool.tsx:617`, `ConfigTool.ts:427`) returns raw error content.
    ///
    /// Only PRE-execution paths wrap: unknown-tool (inlined literal) and
    /// input-schema validation — NOT tool execution errors.
    ///
    /// Reference: claude-code/src/services/tools/toolExecution.ts:1691 +
    ///            claude-code/src/utils/toolErrors.ts (formatError returns bare)
    #[tokio::test]
    async fn tool_execution_error_is_bare_not_wrapped() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(AlwaysFailTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(ToolUseId::new(), "AlwaysFail".to_string(), json!({}), None)];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "a failing tool must set is_error=true");
        // Exact content: bare "Error: kaboom" — no XML envelope AND no
        // LingXi-internal `ToolError` variant prefix. claude-code's
        // `toolExecution.ts:1691` passes `formatError(error)` (= `error.message`,
        // bare) RAW into `tool_result.content`; the model never sees an
        // `internal: `/`invalid input: ` prefix (that prefix is `Display`-only,
        // for logging). Only unknown-tool and schema-validation paths wrap.
        assert_eq!(
            content, "Error: kaboom",
            "tool-execution errors must be BARE (no <tool_use_error> wrapper, no variant prefix)"
        );
        assert!(
            !content.contains("<tool_use_error>"),
            "tool-execution error must NOT be wrapped in <tool_use_error>, got: {content:?}"
        );
    }

    /// A registered tool whose `validate_input` ALWAYS fails with a fixed
    /// message. Drives the new pre-execution `validate_input` gate
    /// (claude-code `toolExecution.ts:683-723`, which wraps a `validateInput`
    /// failure in `<tool_use_error>${message}</tool_use_error>`). Its `call`
    /// panics: a passing validate gate would (incorrectly) reach `call`, so the
    /// panic surfaces any regression that lets a validation failure through.
    struct ValidatingFailTool;
    #[async_trait]
    impl Tool for ValidatingFailTool {
        fn name(&self) -> &str {
            "ValidatingFail"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Err(ValidationError("bad path".into()))
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "always-invalid".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            panic!("validate_input gate must short-circuit before call()");
        }
    }

    /// `PreToolUse` handler that flips a shared flag the instant it fires, so a
    /// test can assert whether the hook ran. Returns the default (no-op)
    /// response otherwise.
    struct SpyPreHook {
        fired: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait]
    impl BuiltinHookHandler for SpyPreHook {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            self.fired.store(true, std::sync::atomic::Ordering::SeqCst);
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }
        fn id(&self) -> &str {
            "spy-pre"
        }
    }

    /// Build a `HookExecutorImpl` with a single `PreToolUse` hook that sets
    /// `fired` when invoked.
    fn spy_pre_hook_executor(fired: Arc<std::sync::atomic::AtomicBool>) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "spy-pre".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "spy-pre".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(SpyPreHook { fired }));
        Arc::new(exec)
    }

    /// VALIDATE-INPUT GATE (parity): when a registered tool's `validate_input`
    /// returns `Err(ValidationError(msg))`, `dispatch_tool_uses_tracked` must
    /// return a `ToolResult` whose content is `<tool_use_error>${msg}</tool_use_error>`
    /// with `is_error = true`, and the tool's `call` must NOT run — matching
    /// claude-code `toolExecution.ts:683-723`.
    #[tokio::test]
    async fn validate_input_failure_returns_tool_use_error_wrapper() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(ValidatingFailTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(
            ToolUseId::new(),
            "ValidatingFail".to_string(),
            json!({}),
            None,
        )];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "validate_input failure must set is_error=true");
        assert_eq!(
            content, "<tool_use_error>bad path</tool_use_error>",
            "content must match claude-code <tool_use_error>${{message}}</tool_use_error>"
        );
    }

    /// VALIDATE-INPUT runs BEFORE the PreToolUse hook (claude-code validates at
    /// `toolExecution.ts:683` BEFORE `runPreToolUseHooks` at ~800). A tool whose
    /// `validate_input` fails must short-circuit so the registered PreToolUse
    /// hook NEVER fires.
    #[tokio::test]
    async fn validate_input_gate_runs_before_pre_tool_use_hook() {
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(ValidatingFailTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            spy_pre_hook_executor(fired.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(
            ToolUseId::new(),
            "ValidatingFail".to_string(),
            json!({}),
            None,
        )];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert_eq!(content, "<tool_use_error>bad path</tool_use_error>");
        assert!(
            !fired.load(std::sync::atomic::Ordering::SeqCst),
            "validate_input gate must run BEFORE the PreToolUse hook; the hook must not fire"
        );
    }

    // ----- HOOK.4: plan-mode dynamic gate routing --------------------------

    #[tokio::test]
    async fn hook4_plan_mode_routes_to_check_in_plan_mode() {
        // With the session in plan mode, the gate is consulted via
        // check_in_plan_mode (the dynamic Plan-mode path) — NOT the boot-mode
        // check — so a runtime EnterPlanMode activates the mutation backstop.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(RouteProbeGate),
            vec![],
        );
        orch.session.lock().await.plan_mode = true;
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("via-plan-mode"),
            "plan mode must route to check_in_plan_mode, got: {content}"
        );
    }

    #[tokio::test]
    async fn hook4_plan_mode_binds_over_a_hook_allow() {
        // Plan mode binds OVER a PreToolUse hook 'allow': even when a hook
        // approved the call, an active plan mode still routes through
        // check_in_plan_mode (a hook cannot push a mutation through during
        // planning — same principle as HOOK.3 issue 1's deny-rule binding).
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(RouteProbeGate), vec![]);
        orch.session.lock().await.plan_mode = true;
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "plan mode binds over the hook 'allow'");
        assert!(
            content.contains("via-plan-mode"),
            "plan mode must override the hook-allow path, got: {content}"
        );
    }

    #[tokio::test]
    async fn hook4_non_plan_mode_still_routes_to_check() {
        // The default (non-plan, no-hook) path is unchanged: route to `check`.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(RouteProbeGate),
            vec![],
        );
        // plan_mode defaults to false.
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("via-check"),
            "non-plan mode must route to check, got: {content}"
        );
    }

    // ----- HOOK.3 issue 2: PermissionRequest on the ask path ---------------

    #[tokio::test]
    async fn hook3_issue2_permission_request_allow_rescues_an_ask() {
        // The gate is about to ASK (resolve_detailed → Ask). A PermissionRequest
        // hook 'allow' RESCUES the call (resolved via check_after_hook_allow →
        // Allow), so the tool runs — the headless rescue claude-code provides.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(
            event_hook_executor(HookEventType::PermissionRequest, resp),
            Arc::new(AskGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(
            !is_error,
            "PermissionRequest 'allow' rescued the ask; tool ran: {content}"
        );
        assert!(content.contains("ECHOED-OUTPUT"));
    }

    #[tokio::test]
    async fn hook3_issue2_permission_request_deny_denies_an_ask() {
        // A PermissionRequest hook 'deny' denies the about-to-ask call.
        let resp = HookResponse {
            decision: Some(HookDecision::Block),
            reason: Some("hook-said-no".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            event_hook_executor(HookEventType::PermissionRequest, resp),
            Arc::new(AskGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("hook-said-no"),
            "PermissionRequest 'deny' reason surfaces: {content}"
        );
    }

    // ----- #37 permissionDecision "defer" ----------------------------------

    /// A `PreToolUse` hook returning `permissionDecision: "defer"` in
    /// NON-interactive (print) mode for a SOLO tool call defers the tool: it is
    /// NOT executed (no tool_result), the turn is terminated
    /// (`prevent_continuation`), and a `hook_deferred_tool` meta message is
    /// injected carrying the faithful fields.
    #[tokio::test]
    async fn defer_in_print_mode_solo_tool_defers_and_terminates() {
        let resp = HookResponse {
            decision: Some(HookDecision::Defer),
            ..HookResponse::default()
        };
        // OrchestratorConfig::default() has interactive_permissions=false
        // (= non-interactive / print mode), and uses() is a single tool — so
        // both defer gates pass and the gated path fires.
        let orch = orch_with(
            event_hook_executor(HookEventType::PreToolUse, resp),
            Arc::new(AllowAllGate),
            vec![],
        );
        let (results, prevent, injected, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        assert!(
            results.is_empty(),
            "the deferred tool produces NO tool_result: {results:?}"
        );
        assert!(prevent, "defer terminates the turn (prevent_continuation)");
        // a hook_deferred_tool meta message was injected
        let joined: String = injected
            .iter()
            .map(|(m, _)| match m {
                ConversationMessage::User { content, .. } => content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
                _ => String::new(),
            })
            .collect();
        assert!(
            joined.contains("hook_deferred_tool"),
            "a hook_deferred_tool meta message must be injected: {joined}"
        );
        assert!(
            joined.contains("\"hookEvent\":\"PreToolUse\""),
            "the meta carries hookEvent=PreToolUse: {joined}"
        );
    }

    /// A `PreToolUse` `defer` in INTERACTIVE mode is IGNORED (warn) — the tool
    /// proceeds through the normal permission gate and runs.
    #[tokio::test]
    async fn defer_in_interactive_mode_is_ignored_and_tool_runs() {
        let resp = HookResponse {
            decision: Some(HookDecision::Defer),
            ..HookResponse::default()
        };
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let cfg = OrchestratorConfig {
            interactive_permissions: true, // interactive → defer ignored
            ..OrchestratorConfig::default()
        };
        let orch = ConversationOrchestrator::new(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            event_hook_executor(HookEventType::PreToolUse, resp),
            Arc::new(AllowAllGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let (results, prevent, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        assert!(!prevent, "ignored defer does NOT terminate the turn");
        assert_eq!(results.len(), 1, "the tool ran and produced a tool_result");
        let (_, is_error) = tool_result(&results[0]);
        assert!(!is_error, "the tool ran successfully (defer ignored)");
    }

    /// A `PreToolUse` `defer` in a MULTI-tool batch is IGNORED (solo-only) — the
    /// tools proceed normally.
    #[tokio::test]
    async fn defer_in_multi_tool_batch_is_ignored() {
        let resp = HookResponse {
            decision: Some(HookDecision::Defer),
            ..HookResponse::default()
        };
        // non-interactive (default) but TWO tool_use blocks → solo-only gate
        // ignores the defer.
        let orch = orch_with(
            event_hook_executor(HookEventType::PreToolUse, resp),
            Arc::new(AllowAllGate),
            vec![],
        );
        let two = vec![
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
        ];
        let (results, prevent, _, _) = dispatch_tool_uses_tracked(&orch, &two, None).await.unwrap();
        assert!(!prevent, "multi-tool defer does NOT terminate the turn");
        assert_eq!(results.len(), 2, "both tools ran (defer ignored)");
    }

    /// #39 PostToolBatch fires ONCE after a batch of resolved tools, carrying
    /// the full batch in `tool_calls`.
    #[tokio::test]
    async fn post_tool_batch_fires_once_with_the_full_batch() {
        use std::sync::Mutex as StdMutex;
        // a capturing PostToolBatch hook recording the tool_calls count it saw.
        struct CaptureBatch {
            seen: Arc<StdMutex<Vec<usize>>>,
        }
        #[async_trait]
        impl BuiltinHookHandler for CaptureBatch {
            async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
                if let HookEvent::PostToolBatch { tool_calls } = event {
                    self.seen.lock().unwrap().push(tool_calls.len());
                }
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    response: Some(HookResponse::default()),
                }
            }
            fn id(&self) -> &str {
                "capture-batch"
            }
        }
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let hook = HookDefinition {
            id: HookId::new(),
            name: "capture-batch".into(),
            events: vec![HookEventType::PostToolBatch],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "capture-batch".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(CaptureBatch { seen: seen.clone() }));
        let orch = orch_with(Arc::new(exec), Arc::new(AllowAllGate), vec![]);
        let two = vec![
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
        ];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &two, None).await.unwrap();
        assert_eq!(results.len(), 2, "both tools ran");
        let captured = seen.lock().unwrap().clone();
        assert_eq!(
            captured,
            vec![2],
            "PostToolBatch fires exactly once with the full 2-tool batch"
        );
    }

    /// #39 PostToolBatch is a strict no-op when NO PostToolBatch hook is
    /// registered (the common path) — the batch still dispatches normally.
    #[tokio::test]
    async fn post_tool_batch_no_hook_is_noop() {
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(AllowAllGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "the tool still ran (no PostToolBatch hook)"
        );
    }

    #[tokio::test]
    async fn hook3_issue2_ask_without_request_hook_delegates_to_inner() {
        // With no PermissionRequest hook the ask delegates to the inner transport
        // (AskGate.check denies) — the prior behavior is preserved.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(AskGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("prompt-denied"),
            "ask delegated to the inner transport: {content}"
        );
    }

    // ----- HOOK.3 issue 3: PermissionDenied only on a classifier deny ------

    #[tokio::test]
    async fn hook3_issue3_permission_denied_fires_only_on_classifier_deny() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A RULE deny must NOT fire PermissionDenied (claude-code fires it only on
        // an auto-mode classifier deny, toolExecution.ts:1075).
        let fired_rule = Arc::new(AtomicUsize::new(0));
        let orch = orch_with(
            recording_executor(HookEventType::PermissionDenied, fired_rule.clone()),
            Arc::new(SourcedDenyGate(PermissionDecisionSource::Rule)),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .unwrap();
        assert!(
            tool_result(&results[0]).1,
            "rule deny still denies the tool"
        );
        assert_eq!(
            fired_rule.load(Ordering::SeqCst),
            0,
            "rule deny must NOT fire the PermissionDenied hook"
        );

        // A CLASSIFIER deny DOES fire PermissionDenied.
        let fired_cls = Arc::new(AtomicUsize::new(0));
        let orch2 = orch_with(
            recording_executor(HookEventType::PermissionDenied, fired_cls.clone()),
            Arc::new(SourcedDenyGate(PermissionDecisionSource::Classifier)),
            vec![],
        );
        let (results2, _, _, _) = dispatch_tool_uses_tracked(&orch2, &uses(), None)
            .await
            .unwrap();
        assert!(
            tool_result(&results2[0]).1,
            "classifier deny denies the tool"
        );
        assert_eq!(
            fired_cls.load(Ordering::SeqCst),
            1,
            "classifier deny MUST fire the PermissionDenied hook"
        );
    }
}
/// Pre-cancellation guard in `dispatch_tool_uses_tracked`:
/// when the cancel token is already fired at dispatch entry,
/// the function must return a `ToolResult` with `is_error:true`
/// and content = `CANCEL_MESSAGE` for every pending tool.
#[cfg(test)]
mod pre_cancel_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::turn_loop::{dispatch_tool_uses_tracked, CANCEL_MESSAGE};
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::{ContentBlock, ToolUseId};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// A tool that always succeeds — if the pre-cancel guard lets it run, the
    /// test will get a success result instead of the CANCEL_MESSAGE error.
    struct NeverShouldRunTool;
    #[async_trait]
    impl Tool for NeverShouldRunTool {
        fn name(&self) -> &str {
            "NeverRun"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
            "never-run".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _: serde_json::Value,
            _: ToolUseContext,
            _: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            panic!(
                "NeverShouldRunTool::call must not be reached when cancel fires before dispatch"
            );
        }
    }

    fn orch_with_never_run() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(NeverShouldRunTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// When the cancel token is already fired at dispatch entry,
    /// `dispatch_tool_uses_tracked` must return a `ToolResult` with
    /// `is_error:true` and content = `CANCEL_MESSAGE` for every pending tool.
    #[tokio::test]
    async fn pre_cancel_emits_cancel_message_for_pending_tools() {
        let orch = orch_with_never_run();
        let cancel = CancellationToken::new();
        cancel.cancel(); // fire BEFORE dispatch

        let uses = vec![(ToolUseId::new(), "NeverRun".to_string(), json!({}), None)];
        let (results, prevent_continuation, injected, _) =
            dispatch_tool_uses_tracked(&orch, &uses, Some(cancel))
                .await
                .expect("dispatch must succeed even on pre-cancel");

        assert_eq!(results.len(), 1, "must return one result per tool");
        match &results[0] {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(
                    *is_error,
                    "pre-cancel tool_result must have is_error=true, got content={content:?}"
                );
                assert_eq!(
                    content, CANCEL_MESSAGE,
                    "pre-cancel content must be the bare CANCEL_MESSAGE (not <tool_use_error>-wrapped)"
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        // The tool did not run → no injected messages and no post-batch entries.
        assert!(
            injected.is_empty(),
            "a pre-cancelled tool must inject no new messages"
        );
        assert!(
            !prevent_continuation,
            "a pre-cancelled tool must not set prevent_continuation"
        );
    }
}
// RECOV.4: the `max_output_tokens` recovery-reset helper used by both the
// token-budget continuation and the Stop-hook blocking continuation.
#[cfg(test)]
mod recovery_state_reset_tests {
    use crate::turn_loop::{RecoveryState, ESCALATED_MAX_TOKENS};

    /// `reset_max_output_tokens_recovery` zeroes the consecutive nudge count,
    /// drops any armed escalation override, and re-arms the 8k→64k single-shot
    /// — exactly the TS continuation reset (`query.ts:1291`/`1332`,
    /// `maxOutputTokensRecoveryCount: 0` + `maxOutputTokensOverride: undefined`).
    #[test]
    fn reset_zeroes_all_three_fields() {
        let mut s = RecoveryState {
            max_output_tokens_recovery_count: 2,
            max_output_tokens_override: Some(ESCALATED_MAX_TOKENS),
            max_output_tokens_escalated: true,
            ..Default::default()
        };
        s.reset_max_output_tokens_recovery();
        assert_eq!(s.max_output_tokens_recovery_count, 0);
        assert_eq!(s.max_output_tokens_override, None);
        assert!(!s.max_output_tokens_escalated);
    }
}
