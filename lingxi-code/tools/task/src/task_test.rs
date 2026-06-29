//! Extracted tests for task.rs.

use super::*;
#[cfg(test)]
mod tests {
    use super::*;

    /// Process-global lock shared by every test that mutates the env vars the
    /// file-backed [`TodoStore`] resolves at call time (`LINGXI_CONFIG_DIR`,
    /// `LINGXI_TASK_LIST_ID`) or the swarm gate
    /// (`LINGXI_EXPERIMENTAL_AGENT_TEAMS`). Without serialization these
    /// tests race on the shared env and a store read can land in another test's
    /// throwaway config dir (→ spurious "Task not found").
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn six_tool_name_constants_locked() {
        assert_eq!(TASK_CREATE_TOOL_NAME, "TaskCreate");
        assert_eq!(TASK_GET_TOOL_NAME, "TaskGet");
        assert_eq!(TASK_LIST_TOOL_NAME, "TaskList");
        assert_eq!(TASK_UPDATE_TOOL_NAME, "TaskUpdate");
        assert_eq!(TASK_STOP_TOOL_NAME, "TaskStop");
        assert_eq!(TASK_OUTPUT_TOOL_NAME, "TaskOutput");
    }

    // ── retrieval_status (TaskOutputTool.tsx `call` lines 219-281) ───────────

    #[test]
    fn retrieval_status_done_is_success_regardless_of_block() {
        // TS: a terminal task → `success` in both the blocking and
        // non-blocking branches.
        assert_eq!(task_output_retrieval_status(true, true), "success");
        assert_eq!(task_output_retrieval_status(true, false), "success");
    }

    #[test]
    fn retrieval_status_running_non_blocking_is_not_ready() {
        // TS non-blocking branch: running/pending → `not_ready`.
        assert_eq!(task_output_retrieval_status(false, false), "not_ready");
    }

    #[test]
    fn retrieval_status_running_blocking_is_timeout() {
        // TS blocking branch: still running/pending after the wait → `timeout`.
        assert_eq!(task_output_retrieval_status(false, true), "timeout");
    }

    #[test]
    fn parse_timeout_ms_handles_floats_and_clamps() {
        // T30: TS `z.number().min(0).max(600000).default(30000)`.
        // Absent → default 30000.
        assert_eq!(parse_timeout_ms(&json!({})), 30_000);
        // An integer JSON number parses as-is.
        assert_eq!(parse_timeout_ms(&json!({ "timeout": 1234 })), 1234);
        // A FLOAT JSON number must parse (the old `as_u64` path dropped these and
        // silently fell back to the 30s default).
        assert_eq!(parse_timeout_ms(&json!({ "timeout": 1234.0 })), 1234);
        assert_eq!(parse_timeout_ms(&json!({ "timeout": 1500.9 })), 1500);
        // Above the max is clamped DOWN to 600000 (zod `.max(600000)`).
        assert_eq!(parse_timeout_ms(&json!({ "timeout": 999_999 })), 600_000);
        assert_eq!(parse_timeout_ms(&json!({ "timeout": 600_000.5 })), 600_000);
        // Below the min is clamped UP to 0 (zod `.min(0)`).
        assert_eq!(parse_timeout_ms(&json!({ "timeout": -50 })), 0);
        // A non-numeric value falls back to the default.
        assert_eq!(parse_timeout_ms(&json!({ "timeout": "nope" })), 30_000);
    }

    #[test]
    fn task_output_schema_declares_block_default_true() {
        let block = &TASK_OUTPUT_SCHEMA["properties"]["block"];
        assert_eq!(block["type"], "boolean");
        assert_eq!(block["default"], true);
    }

    #[test]
    fn validate_task_id_accepts_valid_9_char_ids() {
        assert!(validate_task_id("b3f9zk2x1").is_ok());
        assert!(validate_task_id("a2k1m9pq0").is_ok());
        assert!(validate_task_id("d000abcde").is_ok());
        assert!(validate_task_id("r12345678").is_ok());
        assert!(validate_task_id("t12345678").is_ok());
        assert!(validate_task_id("w12345678").is_ok());
        assert!(validate_task_id("m12345678").is_ok());
    }

    #[test]
    fn validate_task_id_rejects_bad() {
        assert!(validate_task_id("").is_err());
        assert!(validate_task_id("toolong0000").is_err());
        assert!(validate_task_id("X12345678").is_err());
        assert!(validate_task_id("b1234567Z").is_err());
        assert!(validate_task_id("b1234567_").is_err());
    }

    #[test]
    fn task_id_regex_matches_fresh_generated() {
        use regex::Regex;
        let re = Regex::new(r"^[bartwmd][0-9a-z]{8}$").unwrap();
        for c in ['b', 'a', 'r', 't', 'w', 'm', 'd'] {
            let id = fresh_task_id(c);
            assert!(re.is_match(&id), "generated id {id} fails regex");
            assert!(validate_task_id(&id).is_ok());
        }
    }

    #[test]
    fn task_types_byte_aligned_with_m1_surface() {
        assert_eq!(
            TASK_TYPES,
            &[
                "local_bash",
                "local_agent",
                "remote_agent",
                "in_process_teammate",
                "local_workflow",
                "monitor_mcp",
                "dream"
            ]
        );
    }

    #[test]
    fn task_statuses_locked() {
        assert_eq!(
            TASK_STATUSES,
            &["pending", "running", "completed", "failed", "killed"]
        );
    }

    // ── Product-A V2 gating (sub-batch [2]) ──────────────────────────────

    #[test]
    fn todo_v2_enabled_inner_matches_ts_predicate() {
        // Binary TE(): enabled = NOT(_l(LINGXI_ENABLE_TASKS)), i.e.
        // NOT(env is defined-falsy). No non-interactive term.
        assert!(todo_v2_enabled_inner(false)); // env not defined-falsy (unset/empty/garbage/truthy) → on
        assert!(!todo_v2_enabled_inner(true)); // env defined-falsy (0/false/no/off) → off
    }

    #[test]
    fn is_todo_v2_enabled_matches_te_defined_falsy() {
        // The ctx is unused by TE(); the gate is purely the LINGXI_ENABLE_TASKS
        // defined-falsy check. Default (unset env) → enabled. We don't mutate the
        // global env here (to avoid races); the defined-falsy truth table is
        // covered by `todo_v2_enabled_inner` + `traits::env::is_env_defined_falsy`.
        let ctx = ToolStaticContext::default();
        // Holds whenever LINGXI_ENABLE_TASKS is NOT a defined-falsy value.
        if !traits::env::is_env_defined_falsy(
            std::env::var("LINGXI_ENABLE_TASKS").ok().as_deref(),
        ) {
            assert!(is_todo_v2_enabled(&ctx));
        } else {
            assert!(!is_todo_v2_enabled(&ctx));
        }
    }

    // ── Product-A V2 result-string rendering ─────────────────────────────

    fn mk_task(id: &str, subject: &str, status: TodoState) -> TodoTask {
        TodoTask {
            id: id.into(),
            subject: subject.into(),
            description: "desc".into(),
            active_form: None,
            owner: None,
            status,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: Map::new(),
        }
    }

    #[test]
    fn render_task_get_missing_and_present() {
        assert_eq!(render_task_get(None), "Task not found");
        let mut t = mk_task("1", "Do thing", TodoState::InProgress);
        t.blocked_by = vec!["2".into(), "3".into()];
        t.blocks = vec!["4".into()];
        assert_eq!(
            render_task_get(Some(&t)),
            "Task #1: Do thing\nStatus: in_progress\nDescription: desc\nBlocked by: #2, #3\nBlocks: #4"
        );
        let bare = mk_task("5", "Bare", TodoState::Pending);
        assert_eq!(
            render_task_get(Some(&bare)),
            "Task #5: Bare\nStatus: pending\nDescription: desc"
        );
    }

    #[test]
    fn render_task_list_empty_and_rows() {
        assert_eq!(render_task_list(&[]), "No tasks found");
        let rows = vec![
            TaskListRow {
                id: "1".into(),
                subject: "First".into(),
                status: TodoState::Pending,
                owner: Some("alice".into()),
                blocked_by: vec!["2".into()],
            },
            TaskListRow {
                id: "2".into(),
                subject: "Second".into(),
                status: TodoState::Completed,
                owner: None,
                blocked_by: Vec::new(),
            },
        ];
        assert_eq!(
            render_task_list(&rows),
            "#1 [pending] First (alice) [blocked by #2]\n#2 [completed] Second"
        );
    }

    #[test]
    fn render_task_update_strings() {
        assert_eq!(
            render_task_update_success("7", &["status".into(), "owner".into()]),
            "Updated task #7 status, owner"
        );
        assert_eq!(render_task_update_fail("9", Some("Task not found")), "Task not found");
        assert_eq!(render_task_update_fail("9", None), "Task #9 not found");
        assert_eq!(render_task_update_fail("9", Some("")), "Task #9 not found");
    }

    // ── verification nudge (sub-batch [5]) ───────────────────────────────

    #[test]
    fn verification_nudge_suffix_is_byte_exact() {
        // Byte-locked against TaskUpdateTool.ts:397 / TodoWriteTool.ts:107 with
        // VERIFICATION_AGENT_TYPE = 'verification'. Note the em-dash (U+2014).
        assert_eq!(VERIFICATION_AGENT_TYPE, "verification");
        assert_eq!(
            verification_nudge_suffix(),
            "\n\nNOTE: You just closed out 3+ tasks and none of them was a verification step. Before writing your final summary, spawn the verification agent (subagent_type=\"verification\"). You cannot self-assign PARTIAL by listing caveats in your summary \u{2014} only the verifier issues a verdict."
        );
    }

    #[test]
    fn matches_verif_is_case_insensitive_substring() {
        assert!(matches_verif("Run verification tests"));
        assert!(matches_verif("VERIFY the build"));
        assert!(matches_verif("Reverify outputs"));
        assert!(!matches_verif("Ship the feature"));
        assert!(!matches_verif("verfy")); // typo: not a /verif/ match
    }

    #[test]
    fn verification_nudge_fires_on_main_thread_all_done_3plus_no_verif() {
        // main thread (agent_id none) + interactive + all completed + 3 items
        // + none /verif/ ⇒ nudge.
        assert!(verification_nudge_needed(
            true,
            false,
            true,
            3,
            ["Implement", "Wire it up", "Document"].into_iter(),
        ));
    }

    #[test]
    fn verification_nudge_absent_when_fewer_than_three() {
        assert!(!verification_nudge_needed(
            true,
            false,
            true,
            2,
            ["Implement", "Document"].into_iter(),
        ));
    }

    #[test]
    fn verification_nudge_absent_when_an_item_matches_verif() {
        assert!(!verification_nudge_needed(
            true,
            false,
            true,
            3,
            ["Implement", "Verify the fix", "Document"].into_iter(),
        ));
    }

    #[test]
    fn verification_nudge_absent_for_subagent() {
        // agent_id present (!context.agentId is false) ⇒ no nudge.
        assert!(!verification_nudge_needed(
            false,
            false,
            true,
            3,
            ["Implement", "Wire it up", "Document"].into_iter(),
        ));
    }

    #[test]
    fn verification_nudge_absent_when_not_all_completed() {
        assert!(!verification_nudge_needed(
            true,
            false,
            false,
            3,
            ["Implement", "Wire it up", "Document"].into_iter(),
        ));
    }

    #[test]
    fn verification_nudge_absent_in_non_interactive_session() {
        // PARITY-GAP approximation of the unexpressible feature gate.
        assert!(!verification_nudge_needed(
            true,
            true,
            true,
            3,
            ["Implement", "Wire it up", "Document"].into_iter(),
        ));
    }

    // ── TaskUpdate nudge gates on the COMPUTED transition (TaskUpdateTool.ts:
    //    230,267,338) — a no-op `completed` re-send must NOT fire ────────────
    mod task_update_nudge_transition_gate {
        use super::*;
        use std::sync::Arc;
        use telemetry::AnalyticsBus;
        use tool_api::test_support::{ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs};

        /// Restore-on-drop guard for the two process-global env vars this test
        /// flips, plus cleanup of the throwaway store dir — runs even if an
        /// assertion panics. Holds the shared [`super::ENV_LOCK`] so it does not
        /// race other env-mutating tests on `LINGXI_CONFIG_DIR`.
        struct EnvGuard {
            prev_config: Option<std::ffi::OsString>,
            prev_list: Option<std::ffi::OsString>,
            prev_verif: Option<std::ffi::OsString>,
            dir: std::path::PathBuf,
            _lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match &self.prev_config {
                    Some(v) => std::env::set_var("LINGXI_CONFIG_DIR", v),
                    None => std::env::remove_var("LINGXI_CONFIG_DIR"),
                }
                match &self.prev_list {
                    Some(v) => std::env::set_var("LINGXI_TASK_LIST_ID", v),
                    None => std::env::remove_var("LINGXI_TASK_LIST_ID"),
                }
                match &self.prev_verif {
                    Some(v) => std::env::set_var("LINGXI_VERIFICATION_AGENT", v),
                    None => std::env::remove_var("LINGXI_VERIFICATION_AGENT"),
                }
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn bctx() -> BuiltinToolContext {
            let bus = Arc::new(AnalyticsBus::new());
            ctx_for_file_tools(make_dummy_fs(), bus, vec![std::env::temp_dir()])
        }

        fn task(subject: &str, status: TodoState) -> TodoTask {
            let mut t = TodoTask::new(subject.into(), "desc".into(), None, Map::new());
            t.status = status;
            t
        }

        #[tokio::test]
        async fn no_op_completed_does_not_fire_but_real_transition_does() {
            // Isolate the file-backed store to a throwaway config dir + list id.
            let unique = format!(
                "lingxi-task-nudge-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let dir = std::env::temp_dir().join(&unique);
            let _guard = EnvGuard {
                prev_config: std::env::var_os("LINGXI_CONFIG_DIR"),
                prev_list: std::env::var_os("LINGXI_TASK_LIST_ID"),
                prev_verif: std::env::var_os("LINGXI_VERIFICATION_AGENT"),
                dir: dir.clone(),
                _lock: super::ENV_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
            std::env::set_var("LINGXI_CONFIG_DIR", &dir);
            std::env::set_var("LINGXI_TASK_LIST_ID", &unique);
            // T13: the nudge FEATURE is OFF by default (matching prod claude). The
            // store-level transition logic is unchanged; the feature gate is the
            // only difference. Turn it ON for the transition assertions below.
            std::env::remove_var("LINGXI_VERIFICATION_AGENT");

            // 3-item list, none /verif/: two completed + one pending.
            let store = TodoStore::for_list(&unique);
            let id1 = store
                .create(task("Implement parser", TodoState::Completed))
                .await
                .unwrap();
            store
                .create(task("Wire it up", TodoState::Completed))
                .await
                .unwrap();
            let id3 = store
                .create(task("Write docs", TodoState::Pending))
                .await
                .unwrap();

            let tool = TaskUpdateTool::new(bctx());

            // Phase 0 — FEATURE OFF (default): even a real ->completed transition
            // that closes a 3+ all-done list must NOT fire the nudge, because the
            // VERIFICATION_AGENT/tengu_hive_evidence flags default OFF in prod.
            let res = tool
                .call(
                    json!({ "taskId": &id3, "status": "completed" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("transition update ok");
            assert_eq!(
                res.data["verificationNudgeNeeded"],
                json!(false),
                "feature OFF by default ⇒ no nudge on the common interactive path"
            );
            // Re-open #3 so the transition-on assertions below see the same shape.
            store
                .update(&id3, |t| t.status = TodoState::Pending)
                .await;

            // Enable the feature for the remaining (gate-on) assertions.
            std::env::set_var("LINGXI_VERIFICATION_AGENT", "1");

            // Phase 1 — NO-OP: re-send `completed` on the already-completed #1.
            // Raw input status == "completed" (the OLD buggy gate would fire),
            // but the COMPUTED transition is empty, so the nudge must NOT fire.
            let res = tool
                .call(
                    json!({ "taskId": &id1, "status": "completed" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("no-op update ok");
            assert_eq!(
                res.data["verificationNudgeNeeded"],
                json!(false),
                "no-op completed re-send must not trip the nudge"
            );
            assert!(res.data.get("statusChange").is_none(), "no statusChange on a no-op");

            // Phase 2 — REAL transition: #3 pending → completed closes the list
            // (all 3 completed, >= 3, none /verif/), so the nudge fires.
            let res = tool
                .call(
                    json!({ "taskId": &id3, "status": "completed" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("transition update ok");
            assert_eq!(
                res.data["verificationNudgeNeeded"],
                json!(true),
                "a real ->completed transition that closes a 3+ list fires the nudge"
            );
            assert_eq!(res.data["statusChange"]["to"], "completed");
        }
    }

    // ── Swarm-conditional TaskUpdate side-effects (batch D2b ITEM 5) ──────
    //   5a auto-owner (TaskUpdateTool.ts:188-199) + 5b owner-change mailbox
    //   notification (TaskUpdateTool.ts:277-298).
    mod swarm_side_effects {
        use super::*;
        use protocol::AgentId;
        use std::sync::{Arc, Mutex};
        use telemetry::AnalyticsBus;
        use tool_api::test_support::{ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs};
        use traits::mailbox::{
            MailboxError, MailboxMessage, MailboxRouterHandle, RouteAck,
        };

        /// Restore-on-drop guard for the swarm + store env vars; also removes the
        /// throwaway store dir. Runs even on assertion panic.
        struct Guard {
            prev_swarm: Option<std::ffi::OsString>,
            prev_config: Option<std::ffi::OsString>,
            prev_list: Option<std::ffi::OsString>,
            dir: std::path::PathBuf,
            _lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                restore("LINGXI_EXPERIMENTAL_AGENT_TEAMS", &self.prev_swarm);
                restore("LINGXI_CONFIG_DIR", &self.prev_config);
                restore("LINGXI_TASK_LIST_ID", &self.prev_list);
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }
        fn restore(key: &str, prev: &Option<std::ffi::OsString>) {
            match prev {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }

        /// Recording `MailboxRouterHandle` — captures every `route` call.
        #[derive(Default)]
        struct RecordingRouter {
            sent: Mutex<Vec<(String, String, MailboxMessage)>>,
        }
        #[async_trait]
        impl MailboxRouterHandle for RecordingRouter {
            async fn route(
                &self,
                from_agent: &str,
                to_agent: &str,
                message: MailboxMessage,
            ) -> Result<RouteAck, MailboxError> {
                self.sent
                    .lock()
                    .unwrap()
                    .push((from_agent.into(), to_agent.into(), message));
                Ok(RouteAck {
                    claimed_at: std::time::SystemTime::now(),
                    claim_window_secs: 30,
                })
            }
        }

        /// Isolate the file-backed store + set the swarm flag; returns the guard,
        /// the unique list id, and the recording router.
        fn setup(swarm_on: bool) -> (Guard, String, Arc<RecordingRouter>) {
            let lock = super::ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let unique = format!(
                "lingxi-task-swarm-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let dir = std::env::temp_dir().join(&unique);
            let guard = Guard {
                prev_swarm: std::env::var_os("LINGXI_EXPERIMENTAL_AGENT_TEAMS"),
                prev_config: std::env::var_os("LINGXI_CONFIG_DIR"),
                prev_list: std::env::var_os("LINGXI_TASK_LIST_ID"),
                dir: dir.clone(),
                _lock: lock,
            };
            if swarm_on {
                std::env::set_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS", "1");
            } else {
                std::env::remove_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS");
            }
            std::env::set_var("LINGXI_CONFIG_DIR", &dir);
            std::env::set_var("LINGXI_TASK_LIST_ID", &unique);
            (guard, unique, Arc::new(RecordingRouter::default()))
        }

        fn bctx(router: Arc<RecordingRouter>) -> BuiltinToolContext {
            let mut c = ctx_for_file_tools(
                make_dummy_fs(),
                Arc::new(AnalyticsBus::new()),
                vec![std::env::temp_dir()],
            );
            c.mailbox_router = Some(router as Arc<dyn MailboxRouterHandle>);
            c
        }

        fn ctx_with_agent(agent: Option<AgentId>) -> ToolUseContext {
            let mut c = fresh_ctx();
            c.agent_id = agent;
            c
        }

        /// Build a teammate call context with both the agent id AND the display
        /// NAME bound (claude-code `getAgentName()`), which the swarm-only
        /// auto-owner / mailbox-sender paths key on (T6).
        fn ctx_with_named_agent(agent: AgentId, name: &str) -> ToolUseContext {
            let mut c = fresh_ctx();
            c.agent_id = Some(agent);
            c.agent_name = Some(name.to_string());
            c
        }

        fn task(subject: &str, status: TodoState) -> TodoTask {
            let mut t = TodoTask::new(subject.into(), "the description".into(), None, Map::new());
            t.status = status;
            t
        }

        // ── T27 TaskList structured output omits null owner ───────────────
        #[tokio::test]
        async fn task_list_structured_output_omits_null_owner() {
            // claude-code TaskListTool.ts: `owner: z.string().optional()` sourced
            // from `task.owner` (undefined when unset) — the key is OMITTED, never
            // emitted as `owner: null`. An OWNED task keeps the `owner` key.
            let (_g, list, _router) = setup(false);
            let store = TodoStore::for_list(&list);
            store
                .create(task("Unowned task", TodoState::Pending))
                .await
                .unwrap();
            let mut owned = task("Owned task", TodoState::Pending);
            owned.owner = Some("alice".into());
            store.create(owned).await.unwrap();

            let tool = TaskListTool::new(bctx(Arc::new(RecordingRouter::default())));
            let res = tool
                .call(json!({}), fresh_ctx(), fresh_tx())
                .await
                .expect("list ok");
            let tasks = res.data["tasks"].as_array().expect("tasks array");
            assert_eq!(tasks.len(), 2);

            let unowned = tasks
                .iter()
                .find(|t| t["subject"] == "Unowned task")
                .expect("unowned row present");
            assert!(
                unowned.get("owner").is_none(),
                "an unset owner must be OMITTED, not emitted as owner:null — got {unowned}"
            );

            let owned_row = tasks
                .iter()
                .find(|t| t["subject"] == "Owned task")
                .expect("owned row present");
            assert_eq!(
                owned_row["owner"], "alice",
                "an assigned owner keeps the owner key"
            );
        }

        // ── 5a auto-owner ────────────────────────────────────────────────
        #[tokio::test]
        async fn auto_owner_sets_owner_when_swarm_in_progress_unowned() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store.create(task("Build it", TodoState::Pending)).await.unwrap();

            // T6: claude-code auto-owner = getAgentName() (the DISPLAY NAME),
            // NOT the agent:<uuid> id. getAgentStatuses matches owners by name.
            let agent = AgentId::new();
            let tool = TaskUpdateTool::new(bctx(router));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "in_progress" }),
                    ctx_with_named_agent(agent, "researcher"),
                    fresh_tx(),
                )
                .await
                .expect("update ok");

            assert_eq!(res.data["success"], true);
            let fields = res.data["updatedFields"].as_array().unwrap();
            assert!(
                fields.iter().any(|f| f == "owner"),
                "owner should be in updatedFields: {fields:?}"
            );
            // Persisted owner == the teammate DISPLAY NAME (never the uuid).
            let after = store.get(&id).await.unwrap();
            assert_eq!(after.owner.as_deref(), Some("researcher"));
            assert_ne!(
                after.owner.as_deref(),
                Some(agent.to_string().as_str()),
                "owner must NOT be the agent:<uuid> form"
            );
        }

        /// T6: when only the agent id is bound but NOT the display name (e.g. a
        /// teammate whose getAgentName() is undefined), the auto-owner is
        /// SKIPPED — claude-code does not write a uuid owner in that case.
        #[tokio::test]
        async fn auto_owner_skipped_when_name_unbound() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store.create(task("Build it", TodoState::Pending)).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router));
            // agent_id present but agent_name None ⇒ no auto-owner (no uuid owner).
            tool.call(
                json!({ "taskId": &id, "status": "in_progress" }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            assert!(
                store.get(&id).await.unwrap().owner.is_none(),
                "no auto-owner (and no uuid owner) when the display name is unbound"
            );
        }

        #[tokio::test]
        async fn auto_owner_skipped_when_swarm_off() {
            let (_g, list, router) = setup(false);
            let store = TodoStore::for_list(&list);
            let id = store.create(task("Build it", TodoState::Pending)).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router));
            tool.call(
                json!({ "taskId": &id, "status": "in_progress" }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            assert!(store.get(&id).await.unwrap().owner.is_none(), "no auto-owner when swarms off");
        }

        #[tokio::test]
        async fn auto_owner_skipped_when_not_in_progress() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store.create(task("Build it", TodoState::Pending)).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router));
            // completed (not in_progress) ⇒ no auto-owner.
            tool.call(
                json!({ "taskId": &id, "status": "completed" }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            assert!(store.get(&id).await.unwrap().owner.is_none(), "auto-owner only on in_progress");
        }

        #[tokio::test]
        async fn auto_owner_skipped_when_already_owned() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let mut seed = task("Build it", TodoState::Pending);
            seed.owner = Some("existing-owner".into());
            let id = store.create(seed).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router));
            tool.call(
                json!({ "taskId": &id, "status": "in_progress" }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            // The pre-existing owner is preserved, not overwritten by auto-owner.
            assert_eq!(store.get(&id).await.unwrap().owner.as_deref(), Some("existing-owner"));
        }

        #[tokio::test]
        async fn auto_owner_skipped_when_no_agent_id() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store.create(task("Build it", TodoState::Pending)).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router));
            // agent_id None (main thread / getAgentName() undefined) ⇒ no auto-owner.
            tool.call(
                json!({ "taskId": &id, "status": "in_progress" }),
                ctx_with_agent(None),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            assert!(store.get(&id).await.unwrap().owner.is_none(), "no auto-owner without an agent id");
        }

        // ── 5b owner-change mailbox notification ─────────────────────────
        #[tokio::test]
        async fn owner_change_emits_task_assignment_to_new_owner() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store
                .create(task("Ship the feature", TodoState::Pending))
                .await
                .unwrap();

            // Explicit owner change addressed to a teammate by NAME (claude-code
            // addresses mailboxes by name). The sender is the acting teammate's
            // DISPLAY NAME (getAgentName()), NOT its agent:<uuid> id (T6).
            let sender = AgentId::new();
            let tool = TaskUpdateTool::new(bctx(router.clone()));
            tool.call(
                json!({ "taskId": &id, "owner": "scout" }),
                ctx_with_named_agent(sender, "scribe"),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            let sent = router.sent.lock().unwrap();
            assert_eq!(sent.len(), 1, "exactly one task_assignment routed");
            let (from, to, msg) = &sent[0];
            assert_eq!(to, "scout", "routed to the new owner by NAME");
            assert_eq!(from, "scribe", "from = acting teammate display name");
            assert_ne!(from, &sender.to_string(), "sender is NOT the agent:<uuid>");

            let body: Value = serde_json::from_str(&msg.content).unwrap();
            assert_eq!(body["type"], "task_assignment");
            assert_eq!(body["taskId"], id);
            assert_eq!(body["subject"], "Ship the feature");
            assert_eq!(body["description"], "the description");
            assert_eq!(body["assignedBy"], "scribe");
            assert!(body["timestamp"].as_str().unwrap().ends_with('Z'), "ISO-8601 Z timestamp");
        }

        /// T6: when the acting agent has NO display name bound (the leader / main
        /// thread), the mailbox sender falls back to the literal `"team-lead"`
        /// (claude-code `getAgentName() || 'team-lead'`), NEVER a uuid.
        #[tokio::test]
        async fn owner_change_sender_falls_back_to_team_lead_when_unnamed() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store
                .create(task("Ship the feature", TodoState::Pending))
                .await
                .unwrap();

            // Leader: agent_id may be set but agent_name is None.
            let tool = TaskUpdateTool::new(bctx(router.clone()));
            tool.call(
                json!({ "taskId": &id, "owner": "scout" }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            let sent = router.sent.lock().unwrap();
            assert_eq!(sent.len(), 1, "exactly one task_assignment routed");
            let (from, to, msg) = &sent[0];
            assert_eq!(to, "scout");
            assert_eq!(from, "team-lead", "unnamed sender → literal team-lead label");
            let body: Value = serde_json::from_str(&msg.content).unwrap();
            assert_eq!(body["assignedBy"], "team-lead");
        }

        #[tokio::test]
        async fn no_notification_when_owner_unchanged() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let mut seed = task("Ship it", TodoState::Pending);
            seed.owner = Some("same-owner".into());
            let id = store.create(seed).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router.clone()));
            // Re-send the SAME owner ⇒ no diff ⇒ no notification.
            tool.call(
                json!({ "taskId": &id, "owner": "same-owner" }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            assert!(router.sent.lock().unwrap().is_empty(), "no route on a no-op owner write");
        }

        #[tokio::test]
        async fn no_notification_when_swarm_off() {
            let (_g, list, router) = setup(false);
            let store = TodoStore::for_list(&list);
            let id = store.create(task("Ship it", TodoState::Pending)).await.unwrap();

            let tool = TaskUpdateTool::new(bctx(router.clone()));
            tool.call(
                json!({ "taskId": &id, "owner": AgentId::new().to_string() }),
                ctx_with_agent(Some(AgentId::new())),
                fresh_tx(),
            )
            .await
            .expect("update ok");

            assert!(router.sent.lock().unwrap().is_empty(), "no route when swarms off");
        }

        // ── T5 teammate completion reminder (TaskUpdateTool.ts:386-394) ──────
        const TEAMMATE_REMINDER: &str =
            "\nTask completed. Call TaskList now to find your next available task or see if your work unblocked others.";

        #[tokio::test]
        async fn teammate_completion_reminder_present_for_swarm_completed_agent() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store
                .create(task("Build the thing", TodoState::Pending))
                .await
                .unwrap();

            // Teammate (agent_id present) closes the task → completed, swarms on.
            let tool = TaskUpdateTool::new(bctx(router));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "completed" }),
                    ctx_with_agent(Some(AgentId::new())),
                    fresh_tx(),
                )
                .await
                .expect("update ok");

            let content = res.data["content"].as_str().unwrap();
            assert!(
                content.ends_with(TEAMMATE_REMINDER),
                "reminder appended verbatim after the success line: {content:?}"
            );
            assert_eq!(res.data["statusChange"]["to"], "completed");
        }

        #[tokio::test]
        async fn teammate_completion_reminder_absent_when_swarm_off() {
            let (_g, list, router) = setup(false);
            let store = TodoStore::for_list(&list);
            let id = store
                .create(task("Build the thing", TodoState::Pending))
                .await
                .unwrap();

            let tool = TaskUpdateTool::new(bctx(router));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "completed" }),
                    ctx_with_agent(Some(AgentId::new())),
                    fresh_tx(),
                )
                .await
                .expect("update ok");

            assert!(
                !res.data["content"].as_str().unwrap().contains(TEAMMATE_REMINDER),
                "no teammate reminder when swarms are off"
            );
        }

        #[tokio::test]
        async fn teammate_completion_reminder_absent_for_main_thread() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store
                .create(task("Build the thing", TodoState::Pending))
                .await
                .unwrap();

            // Main thread (agent_id None == !getAgentId()) ⇒ no reminder.
            let tool = TaskUpdateTool::new(bctx(router));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "completed" }),
                    ctx_with_agent(None),
                    fresh_tx(),
                )
                .await
                .expect("update ok");

            assert!(
                !res.data["content"].as_str().unwrap().contains(TEAMMATE_REMINDER),
                "no teammate reminder on the main thread (no agent id)"
            );
        }

        #[tokio::test]
        async fn teammate_completion_reminder_absent_when_not_completed_transition() {
            let (_g, list, router) = setup(true);
            let store = TodoStore::for_list(&list);
            let id = store
                .create(task("Build the thing", TodoState::Pending))
                .await
                .unwrap();

            // in_progress (not a ->completed transition) ⇒ no reminder.
            let tool = TaskUpdateTool::new(bctx(router));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "in_progress" }),
                    ctx_with_agent(Some(AgentId::new())),
                    fresh_tx(),
                )
                .await
                .expect("update ok");

            assert!(
                !res.data["content"].as_str().unwrap().contains(TEAMMATE_REMINDER),
                "reminder only on a ->completed transition"
            );
        }
    }

    // ── T1 getTaskListId() 5-level precedence (utils/tasks.ts:199-210) ───────
    mod task_list_id_precedence {
        use super::*;

        /// Restore-on-drop guard for the env vars + leader-team-name global this
        /// module flips. Holds the shared ENV_LOCK so it does not race other
        /// env-mutating tests.
        struct Guard {
            prev_list: Option<std::ffi::OsString>,
            prev_team: Option<std::ffi::OsString>,
            _lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                match &self.prev_list {
                    Some(v) => std::env::set_var("LINGXI_TASK_LIST_ID", v),
                    None => std::env::remove_var("LINGXI_TASK_LIST_ID"),
                }
                match &self.prev_team {
                    Some(v) => std::env::set_var("LINGXI_TEAM_NAME", v),
                    None => std::env::remove_var("LINGXI_TEAM_NAME"),
                }
                traits::team_registry::clear_leader_team_name();
            }
        }

        fn guard() -> Guard {
            let lock = super::ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let g = Guard {
                prev_list: std::env::var_os("LINGXI_TASK_LIST_ID"),
                prev_team: std::env::var_os("LINGXI_TEAM_NAME"),
                _lock: lock,
            };
            // Start from a clean slate for every level.
            std::env::remove_var("LINGXI_TASK_LIST_ID");
            std::env::remove_var("LINGXI_TEAM_NAME");
            traits::team_registry::clear_leader_team_name();
            g
        }

        #[tokio::test]
        async fn level1_env_task_list_id_wins() {
            let _g = guard();
            std::env::set_var("LINGXI_TASK_LIST_ID", "explicit-list");
            // Even with every lower level set, the explicit env wins.
            std::env::set_var("LINGXI_TEAM_NAME", "env-team");
            traits::team_registry::set_leader_team_name("leader-team");
            let mut ctx = tool_api::test_support::fresh_ctx();
            ctx.team_name = Some("teammate-team".into());
            assert_eq!(resolve_task_list_id(&ctx).await, "explicit-list");
        }

        #[tokio::test]
        async fn level2_teammate_team_name() {
            let _g = guard();
            // No env override; teammate ctx team_name wins over env + leader.
            std::env::set_var("LINGXI_TEAM_NAME", "env-team");
            traits::team_registry::set_leader_team_name("leader-team");
            let mut ctx = tool_api::test_support::fresh_ctx();
            ctx.team_name = Some("teammate-team".into());
            assert_eq!(resolve_task_list_id(&ctx).await, "teammate-team");
        }

        #[tokio::test]
        async fn level3_env_team_name() {
            let _g = guard();
            std::env::set_var("LINGXI_TEAM_NAME", "env-team");
            traits::team_registry::set_leader_team_name("leader-team");
            // No teammate ctx team_name ⇒ LINGXI_TEAM_NAME wins over leader.
            let ctx = tool_api::test_support::fresh_ctx();
            assert_eq!(resolve_task_list_id(&ctx).await, "env-team");
        }

        #[tokio::test]
        async fn level4_leader_team_name() {
            let _g = guard();
            traits::team_registry::set_leader_team_name("leader-team");
            // No env / teammate ctx ⇒ leader team name wins over the session.
            let ctx = tool_api::test_support::fresh_ctx();
            assert_eq!(resolve_task_list_id(&ctx).await, "leader-team");
        }

        #[tokio::test]
        async fn level5_session_fallback() {
            let _g = guard();
            // Nothing set ⇒ session id (here "default" — no session wired).
            let ctx = tool_api::test_support::fresh_ctx();
            assert_eq!(resolve_task_list_id(&ctx).await, "default");
        }

        #[tokio::test]
        async fn leader_and_teammate_resolve_same_dir() {
            let _g = guard();
            // Leader (no teammate ctx) resolves to the leader team name; an
            // in-process teammate (ctx.team_name set to the SAME team) resolves
            // to the same on-disk dir — the goal of T1.
            traits::team_registry::set_leader_team_name("alpha-team");
            let leader_ctx = tool_api::test_support::fresh_ctx();
            let mut teammate_ctx = tool_api::test_support::fresh_ctx();
            teammate_ctx.team_name = Some("alpha-team".into());
            assert_eq!(
                resolve_task_list_id(&leader_ctx).await,
                resolve_task_list_id(&teammate_ctx).await
            );
            assert_eq!(resolve_task_list_id(&leader_ctx).await, "alpha-team");
        }
    }

    // ── T12 swarm-enabled prompt fragments (TaskList/TaskCreate prompt.ts) ───
    mod swarm_prompt_fragments {
        use super::*;
        use tool_api::tool_trait::PromptOptions;

        /// Restore-on-drop guard for the swarm gate env var, holding ENV_LOCK.
        struct Guard {
            prev: Option<std::ffi::OsString>,
            _lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(v) => std::env::set_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS", v),
                    None => std::env::remove_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS"),
                }
            }
        }
        fn guard(on: bool) -> Guard {
            let lock = super::ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let g = Guard {
                prev: std::env::var_os("LINGXI_EXPERIMENTAL_AGENT_TEAMS"),
                _lock: lock,
            };
            if on {
                std::env::set_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS", "1");
            } else {
                std::env::remove_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS");
            }
            g
        }

        fn bctx() -> BuiltinToolContext {
            tool_api::test_support::ctx_for_file_tools(
                tool_api::test_support::make_dummy_fs(),
                std::sync::Arc::new(telemetry::AnalyticsBus::new()),
                vec![std::env::temp_dir()],
            )
        }

        #[tokio::test]
        async fn task_list_prompt_disabled_is_base_text() {
            let _g = guard(false);
            let tool = TaskListTool::new(bctx());
            let p = tool.prompt(&PromptOptions { include_examples: false, model: None }).await;
            assert_eq!(p, TASK_LIST_PROMPT, "disabled variant is byte-identical to base");
            assert!(!p.contains("## Teammate Workflow"));
        }

        #[tokio::test]
        async fn task_list_prompt_enabled_has_teammate_workflow() {
            let _g = guard(true);
            let tool = TaskListTool::new(bctx());
            let p = tool.prompt(&PromptOptions { include_examples: false, model: None }).await;
            assert_eq!(p, TASK_LIST_PROMPT_SWARM);
            assert!(p.contains("## Teammate Workflow"));
            assert!(p.contains("- Before assigning tasks to teammates, to see what's available"));
            // The base body is preserved verbatim up to the workflow section.
            assert!(p.starts_with("Use this tool to list all tasks in the task list."));
        }

        #[tokio::test]
        async fn task_create_prompt_disabled_is_base_text() {
            let _g = guard(false);
            let tool = TaskCreateTool::new(bctx());
            let p = tool.prompt(&PromptOptions { include_examples: false, model: None }).await;
            assert_eq!(p, TASK_CREATE_PROMPT, "disabled variant is byte-identical to base");
            assert!(!p.contains("and potentially assigned to teammates"));
        }

        #[tokio::test]
        async fn task_create_prompt_enabled_has_teammate_inserts() {
            let _g = guard(true);
            let tool = TaskCreateTool::new(bctx());
            let p = tool.prompt(&PromptOptions { include_examples: false, model: None }).await;
            assert_eq!(p, TASK_CREATE_PROMPT_SWARM);
            assert!(p.contains(
                "Tasks that require careful planning or multiple operations and potentially assigned to teammates"
            ));
            assert!(p.contains(
                "- Include enough detail in the description for another agent to understand and complete the task"
            ));
            assert!(p.contains(
                "- New tasks are created with status 'pending' and no owner - use TaskUpdate with the `owner` parameter to assign them"
            ));
        }
    }

    // ── T8 / T19 Product-B TaskStop / TaskOutput tool-flag parity ────────────
    mod product_b_tool_flags {
        use super::*;
        use std::sync::Arc;
        use telemetry::AnalyticsBus;
        use tool_api::test_support::{ctx_for_file_tools, make_dummy_fs};

        fn bctx() -> BuiltinToolContext {
            ctx_for_file_tools(
                make_dummy_fs(),
                Arc::new(AnalyticsBus::new()),
                vec![std::env::temp_dir()],
            )
        }

        #[test]
        fn task_stop_should_defer_and_concurrency_safe() {
            // TaskStopTool.ts:53 `shouldDefer: true`; :54-56 isConcurrencySafe → true.
            let tool = TaskStopTool::new(bctx());
            assert!(tool.should_defer(), "TaskStop shouldDefer === true");
            assert!(
                tool.is_concurrency_safe(&Value::Null),
                "TaskStop isConcurrencySafe() === true"
            );
            // is_destructive / interrupt_behavior unchanged.
            assert!(tool.is_destructive(&Value::Null));
            assert!(matches!(
                tool.interrupt_behavior(&Value::Null),
                InterruptBehavior::Block
            ));
        }

        #[test]
        fn task_output_should_defer() {
            // TaskOutputTool.tsx:148 `shouldDefer: true`.
            let tool = TaskOutputTool::new(bctx());
            assert!(tool.should_defer(), "TaskOutput shouldDefer === true");
            assert!(
                tool.is_concurrency_safe(&Value::Null),
                "TaskOutput isConcurrencySafe stays true"
            );
        }

        #[test]
        fn task_stop_search_hint_and_user_facing_name() {
            // T21/T22: TaskStopTool.ts:41 searchHint, :46 userFacingName.
            let tool = TaskStopTool::new(bctx());
            assert_eq!(tool.search_hint(), Some("kill a running background task"));
            assert_eq!(tool.user_facing_name(), Some("Stop Task"));
        }

        #[test]
        fn task_output_search_hint_and_user_facing_name() {
            // T21/T22: TaskOutputTool.tsx:146 searchHint, :151-153 userFacingName.
            let tool = TaskOutputTool::new(bctx());
            assert_eq!(
                tool.search_hint(),
                Some("read output/logs from a background task")
            );
            assert_eq!(tool.user_facing_name(), Some("Task Output"));
        }
    }

    // ── BLOCKING TaskCreated / TaskCompleted lifecycle hooks (tool path) ──────
    //   Exercises the `BuiltinToolContext::task_lifecycle_hooks` seam with a
    //   FAKE `TaskLifecycleHookFirer` (no real hook executor needed):
    //     • a blocking TaskCreated hook → TaskCreate errors + the task is NOT
    //       persisted (rolled back from the store).
    //     • a blocking TaskCompleted hook → TaskUpdate→completed returns
    //       success:false + the status is unchanged.
    //     • no firer / a non-blocking firer → normal create/complete.
    mod lifecycle_hooks {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use telemetry::AnalyticsBus;
        use tool_api::test_support::{ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs};
        use tool_api::TaskLifecycleHookFirer;

        /// Restore-on-drop guard for the store env vars; removes the throwaway
        /// dir. Runs even on assertion panic. Holds `ENV_LOCK` so it does not
        /// race other env-mutating tests on `LINGXI_CONFIG_DIR`.
        struct Guard {
            prev_config: Option<std::ffi::OsString>,
            prev_list: Option<std::ffi::OsString>,
            dir: std::path::PathBuf,
            _lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                match &self.prev_config {
                    Some(v) => std::env::set_var("LINGXI_CONFIG_DIR", v),
                    None => std::env::remove_var("LINGXI_CONFIG_DIR"),
                }
                match &self.prev_list {
                    Some(v) => std::env::set_var("LINGXI_TASK_LIST_ID", v),
                    None => std::env::remove_var("LINGXI_TASK_LIST_ID"),
                }
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        /// Isolate the file-backed store to a unique throwaway dir + list id.
        fn setup() -> (Guard, String) {
            let lock = super::ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let unique = format!(
                "lingxi-task-lifecycle-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let dir = std::env::temp_dir().join(&unique);
            let guard = Guard {
                prev_config: std::env::var_os("LINGXI_CONFIG_DIR"),
                prev_list: std::env::var_os("LINGXI_TASK_LIST_ID"),
                dir: dir.clone(),
                _lock: lock,
            };
            std::env::set_var("LINGXI_CONFIG_DIR", &dir);
            std::env::set_var("LINGXI_TASK_LIST_ID", &unique);
            (guard, unique)
        }

        /// Fake firer: blocks (`Err`) or allows (`Ok`) on demand, and records the
        /// exact `(task_id, subject, description)` it was fired with so tests can
        /// assert the payload mapping. No real hook executor involved.
        #[derive(Default)]
        struct FakeFirer {
            block_created: Option<String>,
            block_completed: Option<String>,
            created_calls: AtomicUsize,
            completed_calls: AtomicUsize,
            #[allow(clippy::type_complexity)]
            last_created: std::sync::Mutex<
                Option<(String, String, Option<String>, Option<String>, Option<String>)>,
            >,
            last_completed: std::sync::Mutex<Option<(String, String, String, Option<String>)>>,
        }
        #[async_trait]
        impl TaskLifecycleHookFirer for FakeFirer {
            async fn fire_task_created(
                &self,
                task_id: &str,
                subject: &str,
                description: Option<&str>,
                teammate_name: Option<&str>,
                team_name: Option<&str>,
            ) -> Result<(), String> {
                self.created_calls.fetch_add(1, Ordering::SeqCst);
                *self.last_created.lock().unwrap() = Some((
                    task_id.into(),
                    subject.into(),
                    description.map(str::to_string),
                    teammate_name.map(str::to_string),
                    team_name.map(str::to_string),
                ));
                match &self.block_created {
                    Some(reason) => Err(reason.clone()),
                    None => Ok(()),
                }
            }
            async fn fire_task_completed(
                &self,
                task_id: &str,
                status: &str,
                subject: &str,
                description: Option<&str>,
            ) -> Result<(), String> {
                self.completed_calls.fetch_add(1, Ordering::SeqCst);
                *self.last_completed.lock().unwrap() = Some((
                    task_id.into(),
                    status.into(),
                    subject.into(),
                    description.map(str::to_string),
                ));
                match &self.block_completed {
                    Some(reason) => Err(reason.clone()),
                    None => Ok(()),
                }
            }
        }

        /// Build a tool ctx whose `task_lifecycle_hooks` is the given firer (or
        /// `None`).
        fn bctx(firer: Option<Arc<FakeFirer>>) -> BuiltinToolContext {
            let mut c = ctx_for_file_tools(
                make_dummy_fs(),
                Arc::new(AnalyticsBus::new()),
                vec![std::env::temp_dir()],
            );
            c.task_lifecycle_hooks =
                firer.map(|f| f as Arc<dyn TaskLifecycleHookFirer>);
            c
        }

        #[tokio::test]
        async fn blocking_task_created_hook_errors_and_does_not_persist() {
            let (_g, list) = setup();
            let firer = Arc::new(FakeFirer {
                block_created: Some("creation blocked by policy".into()),
                ..Default::default()
            });
            let tool = TaskCreateTool::new(bctx(Some(firer.clone())));
            let err = tool
                .call(
                    json!({ "subject": "Ship it", "description": "do the work" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect_err("a blocking TaskCreated hook must error the create");
            match err {
                ToolError::Internal(s) => {
                    // T24: the blocking reason is prefixed `TaskCreated hook
                    // feedback:\n` (claude-code `getTaskCreatedHookMessage`).
                    assert_eq!(
                        s, "TaskCreated hook feedback:\ncreation blocked by policy",
                        "the hook reason surfaces with the TaskCreated feedback prefix"
                    )
                }
                other => panic!("expected Internal(reason), got {other:?}"),
            }
            // The fire saw the (task_id, subject, description) payload.
            let (_id, subj, desc, _tm, _team) =
                firer.last_created.lock().unwrap().clone().unwrap();
            assert_eq!(subj, "Ship it");
            assert_eq!(desc.as_deref(), Some("do the work"));
            // CRITICAL: the just-created task was rolled back — the store is empty.
            let store = TodoStore::for_list(&list);
            assert!(
                store.list().await.is_empty(),
                "a blocked TaskCreate must NOT leave the task persisted (TS deleteTask)"
            );
        }

        #[tokio::test]
        async fn non_blocking_task_created_hook_persists_normally() {
            let (_g, list) = setup();
            let firer = Arc::new(FakeFirer::default()); // allow
            let tool = TaskCreateTool::new(bctx(Some(firer.clone())));
            let res = tool
                .call(
                    json!({ "subject": "Ship it", "description": "do the work" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("a non-blocking TaskCreated hook allows the create");
            assert_eq!(res.data["task"]["subject"], "Ship it");
            assert_eq!(firer.created_calls.load(Ordering::SeqCst), 1, "the hook fired once");
            let store = TodoStore::for_list(&list);
            assert_eq!(store.list().await.len(), 1, "the task is persisted");
        }

        #[tokio::test]
        async fn no_firer_creates_without_firing() {
            let (_g, list) = setup();
            let tool = TaskCreateTool::new(bctx(None));
            tool.call(
                json!({ "subject": "Ship it", "description": "do the work" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("no firer → normal create");
            let store = TodoStore::for_list(&list);
            assert_eq!(store.list().await.len(), 1, "the task is persisted with no firer");
        }

        /// T25: the `TaskCreated` hook fire carries the creating teammate's
        /// `teammate_name` / `team_name` (claude-code `getAgentName()` /
        /// `getTeamName()`), threaded from the call context.
        #[tokio::test]
        async fn task_created_hook_carries_teammate_and_team_name() {
            let (_g, _list) = setup();
            let firer = Arc::new(FakeFirer::default());
            let tool = TaskCreateTool::new(bctx(Some(firer.clone())));
            let mut ctx = fresh_ctx();
            ctx.agent_name = Some("researcher".into());
            ctx.team_name = Some("alpha-team".into());
            tool.call(
                json!({ "subject": "Ship it", "description": "do the work" }),
                ctx,
                fresh_tx(),
            )
            .await
            .expect("create ok");

            let (_id, _subj, _desc, teammate, team) =
                firer.last_created.lock().unwrap().clone().unwrap();
            assert_eq!(teammate.as_deref(), Some("researcher"), "teammate_name threaded");
            assert_eq!(team.as_deref(), Some("alpha-team"), "team_name threaded");
        }

        /// T25: on the main thread / leader (no teammate identity), the
        /// `TaskCreated` hook fire carries `None` for both names.
        #[tokio::test]
        async fn task_created_hook_omits_names_on_main_thread() {
            let (_g, _list) = setup();
            let firer = Arc::new(FakeFirer::default());
            let tool = TaskCreateTool::new(bctx(Some(firer.clone())));
            tool.call(
                json!({ "subject": "Ship it", "description": "do the work" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("create ok");

            let (_id, _subj, _desc, teammate, team) =
                firer.last_created.lock().unwrap().clone().unwrap();
            assert_eq!(teammate, None, "no teammate_name on the main thread");
            assert_eq!(team, None, "no team_name on the main thread");
        }

        #[tokio::test]
        async fn blocking_task_completed_hook_returns_failure_and_status_unchanged() {
            let (_g, list) = setup();
            let store = TodoStore::for_list(&list);
            let id = store
                .create(TodoTask::new(
                    "Ship it".into(),
                    "the description".into(),
                    None,
                    Map::new(),
                ))
                .await
                .unwrap();

            let firer = Arc::new(FakeFirer {
                block_completed: Some("not verified".into()),
                ..Default::default()
            });
            let tool = TaskUpdateTool::new(bctx(Some(firer.clone())));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "completed" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("a blocked completion is a benign success:false result, not an error");
            // TS shape: { success:false, taskId, updatedFields:[], error }.
            assert_eq!(res.data["success"], json!(false));
            assert_eq!(res.data["error"], "not verified");
            assert_eq!(res.data["updatedFields"], json!(Vec::<String>::new()));
            assert!(res.data.get("statusChange").is_none(), "no statusChange on a block");
            // The fire saw the EXISTING subject/description + the terminal status.
            let (_id, status, subj, desc) = firer.last_completed.lock().unwrap().clone().unwrap();
            assert_eq!(status, "completed");
            assert_eq!(subj, "Ship it");
            assert_eq!(desc.as_deref(), Some("the description"));
            // CRITICAL: the status was NOT applied — still pending.
            let after = store.get(&id).await.unwrap();
            assert_eq!(after.status, TodoState::Pending, "a blocked completion must NOT apply the status");
        }

        #[tokio::test]
        async fn non_blocking_task_completed_hook_applies_status() {
            let (_g, list) = setup();
            let store = TodoStore::for_list(&list);
            let id = store
                .create(TodoTask::new("Ship it".into(), "desc".into(), None, Map::new()))
                .await
                .unwrap();

            let firer = Arc::new(FakeFirer::default()); // allow
            let tool = TaskUpdateTool::new(bctx(Some(firer.clone())));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "completed" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("a non-blocking completion succeeds");
            assert_eq!(res.data["success"], json!(true));
            assert_eq!(res.data["statusChange"]["to"], "completed");
            assert_eq!(firer.completed_calls.load(Ordering::SeqCst), 1, "the hook fired once");
            let after = store.get(&id).await.unwrap();
            assert_eq!(after.status, TodoState::Completed, "the status is applied");
        }

        #[tokio::test]
        async fn non_terminal_update_does_not_fire_completed_hook() {
            let (_g, list) = setup();
            let store = TodoStore::for_list(&list);
            let id = store
                .create(TodoTask::new("Ship it".into(), "desc".into(), None, Map::new()))
                .await
                .unwrap();

            // Even a BLOCKING firer must be IGNORED for a non-terminal (in_progress)
            // transition — TS only fires on `status === 'completed'`.
            let firer = Arc::new(FakeFirer {
                block_completed: Some("should never fire".into()),
                ..Default::default()
            });
            let tool = TaskUpdateTool::new(bctx(Some(firer.clone())));
            let res = tool
                .call(
                    json!({ "taskId": &id, "status": "in_progress" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("a non-terminal update is unaffected by a TaskCompleted hook");
            assert_eq!(res.data["success"], json!(true));
            assert_eq!(
                firer.completed_calls.load(Ordering::SeqCst),
                0,
                "a non-terminal transition must NOT fire the TaskCompleted hook"
            );
            let after = store.get(&id).await.unwrap();
            assert_eq!(after.status, TodoState::InProgress, "the in_progress status is applied");
        }
    }

    #[test]
    fn metadata_internal_truthiness_follows_js() {
        let mut m = Map::new();
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(true));
        assert!(metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(false));
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(0));
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(""));
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!("yes"));
        assert!(metadata_internal_truthy(&m));
        m.insert("_internal".into(), Value::Null);
        assert!(!metadata_internal_truthy(&m));
    }

    // ── Product-A V2 schemas ─────────────────────────────────────────────

    #[test]
    fn task_create_schema_locked() {
        let s = &*TASK_CREATE_SCHEMA;
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"], json!(["subject", "description"]));
        assert_eq!(s["properties"]["subject"]["type"], "string");
        assert_eq!(s["properties"]["activeForm"]["type"], "string");
        assert_eq!(s["properties"]["metadata"]["type"], "object");
    }

    #[test]
    fn task_get_schema_requires_task_id() {
        let s = &*TASK_GET_SCHEMA;
        assert_eq!(s["required"], json!(["taskId"]));
        assert_eq!(s["additionalProperties"], false);
    }

    #[test]
    fn task_list_schema_takes_no_params() {
        let s = &*TASK_LIST_SCHEMA;
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["properties"], json!({}));
    }

    #[test]
    fn task_update_schema_status_enum_includes_deleted() {
        let s = &*TASK_UPDATE_SCHEMA;
        assert_eq!(s["required"], json!(["taskId"]));
        assert_eq!(
            s["properties"]["status"]["enum"],
            json!(["pending", "in_progress", "completed", "deleted"])
        );
    }

    // ── Product-B TaskStop / TaskOutput drift (batch [3]) ────────────────
    //
    // A small in-memory `TaskRegistryHandle` mock backs the `call`-level
    // tests for the two background-registry tools.

    mod product_b {
        use super::*;
        use std::collections::VecDeque;
        use std::sync::Mutex as StdMutex;
        use telemetry::AnalyticsBus;
        use tool_api::test_support::{
            ctx_for_file_tools, fresh_ctx, fresh_ctx_cancelled, fresh_tx, make_dummy_fs,
        };
        use traits::task_registry::{
            TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryHandle,
            TaskUpdatePatch,
        };

        #[derive(Default)]
        struct MockRegistry {
            /// `None` ⇒ task not found.
            record: StdMutex<Option<TaskRecord>>,
            /// Successive `output()` results; the last entry repeats once drained.
            chunks: StdMutex<VecDeque<TaskOutputChunk>>,
            kill_calls: StdMutex<u32>,
            output_calls: StdMutex<u32>,
            /// Ids passed to `mark_notified`, in call order (T9).
            notified_ids: StdMutex<Vec<String>>,
        }

        impl MockRegistry {
            fn with_record(record: Option<TaskRecord>) -> Arc<Self> {
                Arc::new(Self {
                    record: StdMutex::new(record),
                    ..Self::default()
                })
            }
            fn push_chunk(self: &Arc<Self>, c: TaskOutputChunk) {
                self.chunks.lock().unwrap().push_back(c);
            }
            fn notified_ids(self: &Arc<Self>) -> Vec<String> {
                self.notified_ids.lock().unwrap().clone()
            }
        }

        #[async_trait]
        impl TaskRegistryHandle for MockRegistry {
            async fn create(
                &self,
                _input: TaskCreateInput,
            ) -> Result<TaskRecord, TaskRegistryError> {
                Err(TaskRegistryError::Internal("unused in product_b tests".into()))
            }
            async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
                Ok(self.record.lock().unwrap().clone())
            }
            async fn list(
                &self,
                _filter: TaskListFilter,
            ) -> Result<Vec<TaskRecord>, TaskRegistryError> {
                Ok(self.record.lock().unwrap().clone().into_iter().collect())
            }
            async fn update(
                &self,
                _id: &str,
                _patch: TaskUpdatePatch,
            ) -> Result<TaskRecord, TaskRegistryError> {
                Err(TaskRegistryError::Internal("unused in product_b tests".into()))
            }
            async fn set_status(
                &self,
                _id: &str,
                _status: &str,
            ) -> Result<TaskRecord, TaskRegistryError> {
                Err(TaskRegistryError::Internal("unused in product_b tests".into()))
            }
            async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
                *self.kill_calls.lock().unwrap() += 1;
                let mut guard = self.record.lock().unwrap();
                match guard.as_mut() {
                    Some(r) => {
                        r.status = "killed".into();
                        Ok(r.clone())
                    }
                    None => Err(TaskRegistryError::NotFound(id.into())),
                }
            }
            async fn output(
                &self,
                id: &str,
                _offset: Option<u64>,
            ) -> Result<TaskOutputChunk, TaskRegistryError> {
                *self.output_calls.lock().unwrap() += 1;
                let mut q = self.chunks.lock().unwrap();
                if q.len() > 1 {
                    Ok(q.pop_front().unwrap())
                } else if let Some(front) = q.front() {
                    Ok(front.clone())
                } else {
                    Err(TaskRegistryError::NotFound(id.into()))
                }
            }
            async fn mark_notified(&self, id: &str) -> Result<(), TaskRegistryError> {
                self.notified_ids.lock().unwrap().push(id.to_string());
                Ok(())
            }
        }

        fn rec(status: &str) -> TaskRecord {
            TaskRecord {
                task_id: "b12345678".into(),
                task_type: "local_bash".into(),
                status: status.into(),
                description: "echo hi".into(),
                // A `local_bash` task carries a distinct command; TaskStop must
                // prefer this over `description` (claude-code stopTask.ts:97).
                command: Some("echo hi > out.txt".into()),
                ..Default::default()
            }
        }

        fn agent_rec(status: &str) -> TaskRecord {
            TaskRecord {
                task_id: "a12345678".into(),
                task_type: "local_agent".into(),
                status: status.into(),
                description: "run the agent".into(),
                // Non-bash tasks have no command; TaskStop falls back to description.
                command: None,
                ..Default::default()
            }
        }

        fn chunk(status: &str, done: bool, exit_code: Option<i32>, content: &str) -> TaskOutputChunk {
            TaskOutputChunk {
                task_id: "b12345678".into(),
                content: content.into(),
                total_lines: 1,
                truncated: false,
                status: Some(status.into()),
                exit_code,
                done,
                ..Default::default()
            }
        }

        /// Like [`chunk`] but also stamps the agent-specific `error` + clean
        /// `result` fields (TS `getTaskOutputData` `local_agent` branch).
        fn agent_chunk(
            status: &str,
            done: bool,
            content: &str,
            error: Option<&str>,
            result: Option<&str>,
        ) -> TaskOutputChunk {
            TaskOutputChunk {
                task_id: "a12345678".into(),
                content: content.into(),
                total_lines: 1,
                truncated: false,
                status: Some(status.into()),
                exit_code: None,
                done,
                error: error.map(str::to_string),
                prompt: Some("do the thing".into()),
                result: result.map(str::to_string),
                output_path: None,
            }
        }

        fn bctx(reg: Arc<dyn TaskRegistryHandle>) -> BuiltinToolContext {
            let bus = Arc::new(AnalyticsBus::new());
            let mut c = ctx_for_file_tools(make_dummy_fs(), bus, vec![std::env::temp_dir()]);
            c.task_registry = Some(reg);
            c
        }

        fn err_msg(e: ToolError) -> String {
            match e {
                ToolError::InvalidInput(s) | ToolError::Internal(s) => s,
                other => panic!("unexpected error variant: {other:?}"),
            }
        }

        // ── schemas ──────────────────────────────────────────────────────

        #[test]
        fn task_stop_schema_two_optional_strings_no_required() {
            let s = &*TASK_STOP_SCHEMA;
            assert_eq!(s["additionalProperties"], false);
            assert!(s.get("required").is_none(), "no required array");
            assert_eq!(s["properties"]["task_id"]["type"], "string");
            assert_eq!(
                s["properties"]["task_id"]["description"],
                "The ID of the background task to stop"
            );
            assert_eq!(s["properties"]["shell_id"]["type"], "string");
            assert_eq!(
                s["properties"]["shell_id"]["description"],
                "Deprecated: use task_id instead"
            );
        }

        #[test]
        fn task_output_schema_drops_offset_limit_adds_timeout() {
            let s = &*TASK_OUTPUT_SCHEMA;
            assert_eq!(s["additionalProperties"], false);
            assert_eq!(s["required"], json!(["task_id"]));
            assert!(s["properties"].get("offset").is_none());
            assert!(s["properties"].get("limit").is_none());
            let t = &s["properties"]["timeout"];
            assert_eq!(t["type"], "integer");
            assert_eq!(t["minimum"], 0);
            assert_eq!(t["maximum"], 600_000);
            assert_eq!(t["default"], 30_000);
            assert_eq!(s["properties"]["block"]["default"], true);
        }

        #[test]
        fn aliases_match_ts() {
            let stop = TaskStopTool::new(bctx(MockRegistry::with_record(None)));
            assert_eq!(stop.aliases(), &["KillShell", "KillBash"]);
            let out = TaskOutputTool::new(bctx(MockRegistry::with_record(None)));
            assert_eq!(
                out.aliases(),
                &["AgentOutputTool", "BashOutputTool", "AgentOutput", "BashOutput"]
            );
        }

        // ── TaskStop ─────────────────────────────────────────────────────

        #[tokio::test]
        async fn task_stop_success_result_shape() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let res = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect("stop ok");
            // T10: a `local_bash` task surfaces its COMMAND, not its description
            // (claude-code stopTask.ts:97). `rec()` has description "echo hi" but
            // command "echo hi > out.txt" — the command must win.
            assert_eq!(
                res.data["message"],
                "Successfully stopped task: b12345678 (echo hi > out.txt)"
            );
            assert_eq!(res.data["task_id"], "b12345678");
            assert_eq!(res.data["task_type"], "local_bash");
            assert_eq!(res.data["command"], "echo hi > out.txt");
            // No `content` key ⇒ orchestrator JSON-stringifies the whole data.
            assert!(res.data.get("content").is_none());
            assert_eq!(*reg.kill_calls.lock().unwrap(), 1);
        }

        #[tokio::test]
        async fn task_stop_falls_back_to_description_for_non_bash() {
            // T10: a non-bash task (no `command`) falls back to `description`
            // (claude-code stopTask.ts:97 `: task.description`).
            let reg = MockRegistry::with_record(Some(agent_rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let res = tool
                .call(json!({ "task_id": "a12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect("stop ok");
            assert_eq!(
                res.data["message"],
                "Successfully stopped task: a12345678 (run the agent)"
            );
            assert_eq!(res.data["task_type"], "local_agent");
            assert_eq!(res.data["command"], "run the agent");
        }

        #[tokio::test]
        async fn task_stop_accepts_shell_id_alias() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let res = tool
                .call(json!({ "shell_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect("shell_id resolves");
            assert_eq!(res.data["task_id"], "b12345678");
            assert_eq!(*reg.kill_calls.lock().unwrap(), 1);
        }

        #[tokio::test]
        async fn task_stop_empty_task_id_falls_through_to_missing() {
            // `"" ?? shell_id` keeps "" (not nullish) → `!id` → missing error,
            // even though shell_id is present (TS quirk, reproduced 1:1).
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let err = tool
                .call(
                    json!({ "task_id": "", "shell_id": "b12345678" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect_err("empty task_id is missing");
            assert_eq!(err_msg(err), "Missing required parameter: task_id");
            assert_eq!(*reg.kill_calls.lock().unwrap(), 0);
        }

        #[tokio::test]
        async fn task_stop_missing_param() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg));
            let err = tool
                .call(json!({}), fresh_ctx(), fresh_tx())
                .await
                .expect_err("missing");
            assert_eq!(err_msg(err), "Missing required parameter: task_id");
        }

        #[tokio::test]
        async fn task_stop_not_found() {
            let reg = MockRegistry::with_record(None);
            let tool = TaskStopTool::new(bctx(reg));
            let err = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect_err("not found");
            assert_eq!(err_msg(err), "No task found with ID: b12345678");
        }

        #[tokio::test]
        async fn task_stop_not_running() {
            let reg = MockRegistry::with_record(Some(rec("completed")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let err = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect_err("not running");
            assert_eq!(
                err_msg(err),
                "Task b12345678 is not running (status: completed)"
            );
            // Pre-validation rejects before any kill.
            assert_eq!(*reg.kill_calls.lock().unwrap(), 0);
        }

        // ── TaskOutput ───────────────────────────────────────────────────

        #[tokio::test]
        async fn task_output_nonblock_done_is_success() {
            let reg = MockRegistry::with_record(Some(rec("completed")));
            reg.push_chunk(chunk("completed", true, Some(0), "all done\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(res.data["task"]["task_id"], "b12345678");
            assert_eq!(res.data["task"]["task_type"], "local_bash");
            assert_eq!(res.data["task"]["status"], "completed");
            assert_eq!(res.data["task"]["description"], "echo hi");
            assert_eq!(res.data["task"]["output"], "all done\n");
            assert_eq!(res.data["task"]["exit_code"], 0);
            // Non-blocking ⇒ exactly one output read.
            assert_eq!(*reg.output_calls.lock().unwrap(), 1);
            let content = res.data["content"].as_str().unwrap();
            assert!(content.contains("<retrieval_status>success</retrieval_status>"));
            assert!(content.contains("<task_id>b12345678</task_id>"));
            assert!(content.contains("<task_type>local_bash</task_type>"));
            assert!(content.contains("<status>completed</status>"));
            assert!(content.contains("<exit_code>0</exit_code>"));
            assert!(content.contains("<output>\nall done\n</output>"));
            // Tags joined by a single newline.
            assert!(content.contains("</retrieval_status>\n<task_id>"));
        }

        #[tokio::test]
        async fn task_output_nonblock_running_is_not_ready() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, ""));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "not_ready");
            // Blank output ⇒ no <output> tag; running ⇒ no <exit_code>.
            let content = res.data["content"].as_str().unwrap();
            assert!(content.contains("<status>running</status>"));
            assert!(!content.contains("<output>"));
            assert!(!content.contains("<exit_code>"));
            // exit_code key omitted when the chunk carries none.
            assert!(res.data["task"].get("exit_code").is_none());
        }

        #[tokio::test]
        async fn task_output_terminal_read_marks_notified() {
            // T9: a terminal (done) read marks the task notified so a later
            // duplicate `<task-notification>` is suppressed. Mirrors claude-code
            // `TaskOutputTool`'s non-blocking terminal branch
            // `updateTaskState(task_id, t => ({ ...t, notified: true }))`.
            let reg = MockRegistry::with_record(Some(rec("completed")));
            reg.push_chunk(chunk("completed", true, Some(0), "all done\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(
                reg.notified_ids(),
                vec!["b12345678".to_string()],
                "a terminal read marks the task notified exactly once"
            );
        }

        #[tokio::test]
        async fn task_output_nonterminal_read_does_not_mark_notified() {
            // T9: a still-running (not done) read must NOT mark notified — the
            // task hasn't been consumed yet (TS only marks in the terminal
            // branches, returning `not_ready` here).
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, "partial\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "not_ready");
            assert!(
                reg.notified_ids().is_empty(),
                "a non-terminal read must not mark the task notified"
            );
        }

        #[tokio::test]
        async fn task_output_blocking_terminal_read_marks_notified() {
            // T9: the BLOCKING terminal branch also marks notified after
            // `waitForTaskCompletion` resolves to a terminal task.
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("completed", true, Some(0), "final\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": true, "timeout": 600_000 }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(
                reg.notified_ids(),
                vec!["b12345678".to_string()],
                "the blocking terminal branch marks the task notified"
            );
        }

        #[tokio::test]
        async fn task_output_block_polls_until_done() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            // running, running, then completed — the poll loop must reach it.
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("completed", true, Some(0), "final\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": true, "timeout": 600_000 }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(res.data["task"]["status"], "completed");
            assert_eq!(res.data["task"]["output"], "final\n");
            assert!(*reg.output_calls.lock().unwrap() >= 3);
        }

        #[tokio::test]
        async fn task_output_block_timeout_is_timeout() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, "still going\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            // timeout=0 ⇒ the loop breaks immediately without sleeping.
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": true, "timeout": 0 }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "timeout");
            assert_eq!(res.data["task"]["status"], "running");
            let content = res.data["content"].as_str().unwrap();
            assert!(content.contains("<retrieval_status>timeout</retrieval_status>"));
        }

        // ── TaskOutput agent-specific semantics (T3) ─────────────────────

        #[tokio::test]
        async fn task_output_agent_failed_renders_error_after_output() {
            // A failed local_agent task: the chunk carries an `error` string and
            // a clean `result`. The render must place `<error>` AFTER `<output>`
            // (TS `mapToolResultToToolResultBlockParam` lines 297-301) and the
            // model-facing `output` must be the clean result, not the raw blob.
            let reg = MockRegistry::with_record(Some(agent_rec("failed")));
            reg.push_chunk(agent_chunk(
                "failed",
                true,
                "[{\"type\":\"text\",\"text\":\"partial work\"}]",
                Some("model refused to continue"),
                Some("partial work"),
            ));
            let tool = TaskOutputTool::new(bctx(reg));
            let res = tool
                .call(
                    json!({ "task_id": "a12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["task"]["task_type"], "local_agent");
            // Clean result surfaces as the output (not the JSON blob).
            assert_eq!(res.data["task"]["output"], "partial work");
            assert_eq!(res.data["task"]["error"], "model refused to continue");
            assert_eq!(res.data["task"]["prompt"], "do the thing");
            assert_eq!(res.data["task"]["result"], "partial work");

            let content = res.data["content"].as_str().unwrap();
            assert!(
                content.contains("<error>model refused to continue</error>"),
                "rendered output carries an <error> element"
            );
            // <error> comes AFTER <output> (TS render order).
            let out_idx = content.find("<output>").expect("has <output>");
            let err_idx = content.find("<error>").expect("has <error>");
            assert!(out_idx < err_idx, "<error> renders after <output>");
        }

        #[tokio::test]
        async fn task_output_agent_success_returns_clean_text_not_json_blob() {
            // A successful local_agent task whose on-disk spool is the raw
            // pretty-JSON transcript blob, but whose chunk also carries the
            // CLEAN extracted final text. The model must see the clean text.
            let reg = MockRegistry::with_record(Some(agent_rec("completed")));
            let json_blob = "{\n  \"answer\": \"42\",\n  \"ok\": true\n}\n\
                             <usage><total_tokens>7</total_tokens></usage>\n";
            reg.push_chunk(agent_chunk(
                "completed",
                true,
                json_blob,
                None,
                Some("The answer is 42."),
            ));
            let tool = TaskOutputTool::new(bctx(reg));
            let res = tool
                .call(
                    json!({ "task_id": "a12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(res.data["task"]["output"], "The answer is 42.");
            assert_eq!(res.data["task"]["result"], "The answer is 42.");
            // No error on a successful agent ⇒ no <error> element, no error key.
            assert!(res.data["task"].get("error").is_none());

            let content = res.data["content"].as_str().unwrap();
            assert!(
                content.contains("<output>\nThe answer is 42.\n</output>"),
                "clean text renders in <output>, got: {content}"
            );
            assert!(
                !content.contains("total_tokens") && !content.contains("\"answer\""),
                "the raw JSON blob does NOT leak into the model-facing output"
            );
            assert!(!content.contains("<error>"), "no <error> on success");
        }

        #[tokio::test]
        async fn task_output_block_returns_promptly_on_cancel() {
            // A still-running task with a long timeout: a triggered cancel token
            // must break the wait loop immediately (TS `waitForTaskCompletion`
            // checks `abortController.signal.aborted` each iteration) rather than
            // blocking out the full timeout.
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, "still going\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let started = std::time::Instant::now();
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": true, "timeout": 600_000 }),
                    fresh_ctx_cancelled(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            // Returns well under the 600s timeout.
            assert!(
                started.elapsed().as_secs() < 5,
                "cancelled wait returns promptly"
            );
            assert_eq!(res.data["retrieval_status"], "timeout");
            assert_eq!(res.data["task"]["status"], "running");
            // The cancel fires BEFORE the first 100ms sleep, so the loop body
            // never issues a second poll: exactly the initial read.
            assert_eq!(*reg.output_calls.lock().unwrap(), 1);
        }

        #[tokio::test]
        async fn task_output_not_found() {
            let reg = MockRegistry::with_record(None);
            let tool = TaskOutputTool::new(bctx(reg));
            let err = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect_err("not found");
            assert_eq!(err_msg(err), "No task found with ID: b12345678");
        }

        #[tokio::test]
        async fn task_output_missing_task_id() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskOutputTool::new(bctx(reg));
            let err = tool
                .call(json!({}), fresh_ctx(), fresh_tx())
                .await
                .expect_err("missing");
            assert_eq!(err_msg(err), "Task ID is required");
        }

        #[test]
        fn render_task_output_null_task_is_status_only() {
            // The `task: null` (timeout) branch renders just the status line.
            assert_eq!(
                render_task_output("timeout", None),
                "<retrieval_status>timeout</retrieval_status>"
            );
        }

        // ── BASHOUT.3: `block` string coercion (semanticBoolean) ─────────────

        #[test]
        fn semantic_bool_coerces_string_literals() {
            assert_eq!(semantic_bool(&json!(true)), Some(true));
            assert_eq!(semantic_bool(&json!(false)), Some(false));
            assert_eq!(semantic_bool(&json!("true")), Some(true));
            assert_eq!(semantic_bool(&json!("false")), Some(false));
            // Anything else passes through to the inner schema (→ None here).
            assert_eq!(semantic_bool(&json!("FALSE")), None);
            assert_eq!(semantic_bool(&json!("maybe")), None);
            assert_eq!(semantic_bool(&json!(1)), None);
        }

        #[tokio::test]
        async fn task_output_block_string_false_is_nonblocking() {
            // Quoted `block:"false"` must be coerced to `false` (non-blocking),
            // not fall through to the `true` default. A running task therefore
            // returns `not_ready` after exactly ONE output read.
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, ""));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": "false" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "not_ready");
            assert_eq!(*reg.output_calls.lock().unwrap(), 1);
        }

        #[tokio::test]
        async fn task_output_block_string_true_blocks() {
            // Quoted `block:"true"` must be coerced to `true` (blocking): the
            // poll loop runs until the task is terminal.
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("completed", true, Some(0), "final\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": "true", "timeout": 600_000 }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(res.data["task"]["status"], "completed");
            assert!(*reg.output_calls.lock().unwrap() >= 3);
        }

        // ── BASHOUT.1: output truncation (formatTaskOutput) ──────────────────

        #[test]
        fn format_task_output_passthrough_under_limit() {
            // `output.length <= maxLen` ⇒ returned verbatim, no header.
            let out = "hello world\nsecond line\n";
            assert_eq!(format_task_output(out, "b12345678", None), out);
        }

        #[test]
        fn format_task_output_truncates_tail_over_limit() {
            let max = max_task_output_length();
            // Build an output strictly longer than the cap, with a unique marker
            // at the FRONT (must be dropped) and the END (must be kept).
            let total = max + 1000;
            let filler = "A".repeat(total - "HEADMARKER".len() - "TAILEND".len());
            let out = format!("HEADMARKER{filler}TAILEND");
            assert_eq!(out.chars().count(), total);

            // With NO resolved path the header falls back to the bare filename.
            let formatted = format_task_output(&out, "b12345678", None);
            assert!(formatted.starts_with("[Truncated. Full output: b12345678.output]\n\n"));
            // The leading marker was truncated away; the tail is preserved.
            assert!(!formatted.contains("HEADMARKER"));
            assert!(formatted.ends_with("TAILEND"));
            // header + tail exactly fills `maxLen` chars (TS slice arithmetic).
            assert_eq!(formatted.chars().count(), max);
        }

        #[test]
        fn format_task_output_header_uses_absolute_path_when_threaded() {
            // T11: when the registry threads the resolved ABSOLUTE spool path
            // (`TaskOutputChunk.output_path`), the header shows that path
            // verbatim — byte-faithful with claude-code `getTaskOutputPath`.
            let max = max_task_output_length();
            let out = format!("{}TAILEND", "C".repeat(max + 200));
            let abs = "/private/tmp/claude-501/-Users-me-proj/sess-abc/tasks/b12345678.output";
            let formatted = format_task_output(&out, "b12345678", Some(abs));
            assert!(
                formatted.starts_with(&format!("[Truncated. Full output: {abs}]\n\n")),
                "absolute path is used verbatim in the header; got {:?}",
                &formatted[..formatted.char_indices().nth(120).map_or(formatted.len(), |(i, _)| i)]
            );
            // A bare filename must NOT leak when the absolute path is present.
            assert!(!formatted.starts_with("[Truncated. Full output: b12345678.output]"));
            assert!(formatted.ends_with("TAILEND"));
            // The full byte length budget still holds with the longer header.
            assert_eq!(formatted.chars().count(), max);
        }

        #[test]
        fn render_task_output_truncates_large_output() {
            // Integration: render_task_output threads the raw output through
            // format_task_output before wrapping in `<output>`.
            let max = max_task_output_length();
            let out = format!("{}TAILEND", "B".repeat(max + 500));
            let view = TaskOutputView {
                task_id: "b12345678".into(),
                task_type: "local_bash".into(),
                status: "completed".into(),
                description: "echo hi".into(),
                output: out,
                exit_code: Some(0),
                error: None,
                output_path: None,
            };
            let rendered = render_task_output("success", Some(&view));
            assert!(rendered.contains("<output>\n[Truncated. Full output: b12345678.output]\n\n"));
            assert!(rendered.trim_end().ends_with("TAILEND\n</output>"));
        }

        #[test]
        fn render_task_output_header_shows_threaded_absolute_path() {
            // T11 integration: the absolute spool path threaded onto the view
            // reaches the `<output>` header.
            let max = max_task_output_length();
            let out = format!("{}TAILEND", "B".repeat(max + 500));
            let abs = "/private/tmp/claude-501/-Users-me-proj/sess-xyz/tasks/b12345678.output";
            let view = TaskOutputView {
                task_id: "b12345678".into(),
                task_type: "local_bash".into(),
                status: "completed".into(),
                description: "echo hi".into(),
                output: out,
                exit_code: Some(0),
                error: None,
                output_path: Some(abs.into()),
            };
            let rendered = render_task_output("success", Some(&view));
            assert!(
                rendered.contains(&format!("<output>\n[Truncated. Full output: {abs}]\n\n")),
                "the threaded absolute path reaches the rendered header"
            );
            assert!(rendered.trim_end().ends_with("TAILEND\n</output>"));
        }
    }
}
