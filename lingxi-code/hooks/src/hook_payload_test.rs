//! Trailing tests extracted from hook_payload.rs.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pre_payload_serializes_byte_lock() {
        let p = PreToolUsePayload {
            hook_event_name: HookEventNamePre,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            tool_use_id: "tu-1".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.starts_with(r#"{"hook_event_name":"PreToolUse","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":{"command":"ls"},"tool_use_id":"tu-1"}"#));
    }

    #[test]
    fn post_payload_includes_tool_response() {
        let p = PostToolUsePayload {
            hook_event_name: HookEventNamePost,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            tool_name: "Read".into(),
            tool_input: json!({"path": "/x"}),
            tool_response: json!({"content": "data"}),
            tool_use_id: "tu-2".into(),
            duration_ms: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""hook_event_name":"PostToolUse""#));
        assert!(s.contains(r#""tool_response":{"content":"data"}"#));
    }

    #[test]
    fn round_trip_pre_payload() {
        let p = PreToolUsePayload {
            hook_event_name: HookEventNamePre,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("plan".into()),
            agent_id: Some("a-1".into()),
            agent_type: Some("general-purpose".into()),
            effort: None,
            tool_name: "Edit".into(),
            tool_input: json!({"file_path": "/f"}),
            tool_use_id: "tu".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: PreToolUsePayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tool_name, "Edit");
        assert_eq!(back.agent_type.as_deref(), Some("general-purpose"));
    }

    /// `effort` is the base-shape `effort: { level }` object (finding #44):
    /// present (as a nested object) only when populated, and OMITTED entirely
    /// when `None` — matching claude-code's conditional `effort:a` spread in
    /// `createBaseHookInput` (`effort` is `void 0` → key absent for
    /// session-lifecycle hooks and effort-incapable models).
    #[test]
    fn effort_present_serializes_as_level_object_and_omitted_when_none() {
        // PRESENT: `effort: { level: "high" }`, placed after the base
        // `agent_type` field and before the event-specific fields.
        let with_effort = PreToolUsePayload {
            hook_event_name: HookEventNamePre,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: Some(EffortLevel::new("high")),
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            tool_use_id: "tu".into(),
        };
        let s = serde_json::to_string(&with_effort).unwrap();
        // Nested `{ "level": "..." }` shape, not a bare string.
        assert!(
            s.contains(r#""effort":{"level":"high"}"#),
            "effort serializes as a {{ level }} object: {s}"
        );
        // Wire position: base block, after `cwd` (the last always-present base
        // field here) and immediately before `tool_name`.
        assert!(
            s.contains(r#""cwd":"/w","effort":{"level":"high"},"tool_name":"Bash""#),
            "effort sits in the base block before event fields: {s}"
        );
        let back: PreToolUsePayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.effort, Some(EffortLevel::new("high")));

        // ABSENT: `None` → key omitted entirely (skip_serializing_if).
        let no_effort = PreToolUsePayload {
            effort: None,
            ..with_effort.clone()
        };
        let s2 = serde_json::to_string(&no_effort).unwrap();
        assert!(
            !s2.contains("effort"),
            "effort key is omitted when None: {s2}"
        );
        let back2: PreToolUsePayload = serde_json::from_str(&s2).unwrap();
        assert_eq!(back2.effort, None);
    }

    #[test]
    fn parse_response_allow_via_permission_decision() {
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Approve));
    }

    #[test]
    fn parse_response_block_via_legacy_decision() {
        let r = parse_response(
            r#"{"decision":"block","stopReason":"because","continue":false}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Block));
        assert_eq!(r.reason.as_deref(), Some("because"));
    }

    #[test]
    fn parse_response_event_mismatch_errors() {
        let err = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse"}}"#,
            "PreToolUse",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            HookResponseParseError::EventNameMismatch { .. }
        ));
    }

    #[test]
    fn parse_response_keeps_system_message_and_additional_context_separate() {
        // Parity with claude-code: `systemMessage` and
        // `hookSpecificOutput.additionalContext` are DISTINCT fields routed to
        // SEPARATE attachments — `hook_system_message` (NOT model-facing,
        // `messages.ts:4258` → `[]`) vs `hook_additional_context` (model-facing,
        // `messages.ts:4117`). They must NEVER be merged into one field.
        let r = parse_response(
            r#"{"systemMessage":"hello","hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"world"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(
            r.system_message.as_deref(),
            Some("hello"),
            "systemMessage stays on its own field (transcript-facing, not the model)"
        );
        assert_eq!(
            r.additional_context.as_deref(),
            Some("world"),
            "additionalContext stays on its own field (model-facing)"
        );
    }

    #[test]
    fn parse_response_elicitation_accept_with_content() {
        // hookSpecificOutput.{action,content} -> elicitation_response, no block.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"Elicitation","action":"accept","content":{"token":"xyz"}}}"#,
            "Elicitation",
        )
        .unwrap();
        let er = r.elicitation_response.expect("elicitation response");
        assert_eq!(er.action, "accept");
        assert_eq!(er.content, Some(json!({"token": "xyz"})));
        assert_eq!(r.decision, None, "accept must NOT block");
    }

    #[test]
    fn parse_response_elicitation_decline_blocks() {
        // action:'decline' -> response set AND decision becomes Block (claude-code
        // `parseElicitationHookOutput` sets a blockingError on decline).
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"Elicitation","action":"decline"}}"#,
            "Elicitation",
        )
        .unwrap();
        let er = r.elicitation_response.expect("elicitation response");
        assert_eq!(er.action, "decline");
        assert_eq!(er.content, None);
        assert_eq!(r.decision, Some(HookDecision::Block));
    }

    #[test]
    fn parse_response_no_action_leaves_elicitation_none() {
        // `if (!specific.action) return {}` — no action => no elicitation response.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"Elicitation","additionalContext":"x"}}"#,
            "Elicitation",
        )
        .unwrap();
        assert!(r.elicitation_response.is_none());
    }

    // ---- PostToolUse `updatedMCPToolOutput` parse (claude-code
    //      `parseHookJSONOutput`, `utils/hooks.ts:646-649`) ------------------

    #[test]
    fn parse_response_post_extracts_updated_mcp_tool_output() {
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedMCPToolOutput":{"content":"new"}}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert_eq!(
            r.updated_mcp_tool_output,
            Some(json!({ "content": "new" }))
        );
    }

    #[test]
    fn parse_response_pre_ignores_updated_mcp_tool_output() {
        // The TS switch only reads `updatedMCPToolOutput` for the `PostToolUse`
        // case — a PreToolUse hook returning it has it dropped.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedMCPToolOutput":{"content":"new"}}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert!(r.updated_mcp_tool_output.is_none());
    }

    #[test]
    fn parse_response_post_null_updated_mcp_tool_output_is_noop() {
        // TS guards on truthiness — a JSON `null` is NOT a replacement.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedMCPToolOutput":null}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert!(r.updated_mcp_tool_output.is_none());
    }

    #[test]
    fn parse_response_post_without_updated_output_leaves_none() {
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"hi"}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert!(r.updated_mcp_tool_output.is_none());
        assert!(r.updated_tool_output.is_none());
    }

    // ---- #38 hookSpecificOutput.updatedToolOutput (all-tools) -------------

    #[test]
    fn parse_response_post_extracts_updated_tool_output_object() {
        // `!== void 0` semantics: a present non-null value IS a replacement,
        // applied for ALL tools (no isMcp gate). `Some(Some(value))`.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":{"result":"replaced"}}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert_eq!(
            r.updated_tool_output,
            Some(Some(json!({ "result": "replaced" })))
        );
    }

    #[test]
    fn parse_response_post_explicit_null_updated_tool_output_is_replacement() {
        // KEY DISTINCTION from the MCP field: `updatedToolOutput` uses
        // `!== void 0` — an explicit JSON `null` IS a replacement (the key being
        // present matters), so the outer `Some` is set with `Some(Value::Null)`.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":null}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert_eq!(r.updated_tool_output, Some(Some(serde_json::Value::Null)));
        // ... whereas the legacy MCP field treats null as a no-op:
        assert!(r.updated_mcp_tool_output.is_none());
    }

    #[test]
    fn parse_response_post_absent_updated_tool_output_is_none() {
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"hi"}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert!(r.updated_tool_output.is_none());
    }

    #[test]
    fn parse_response_pre_ignores_updated_tool_output() {
        // The TS switch only reads `updatedToolOutput` for the `PostToolUse`
        // case — a PreToolUse hook returning it has it dropped.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedToolOutput":{"x":1}}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert!(r.updated_tool_output.is_none());
    }

    #[test]
    fn updated_tool_output_wire_round_trips_camel_case() {
        // The struct field round-trips through serde with the camelCase wire key
        // and `Option<Option<Value>>` shape preserved.
        let resp = crate::response::HookResponse {
            updated_tool_output: Some(Some(json!({ "a": 1 }))),
            ..Default::default()
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains(r#""updated_tool_output":{"a":1}"#), "{s}");
        let back: crate::response::HookResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.updated_tool_output, Some(Some(json!({ "a": 1 }))));
        // Default (absent) is skip_serializing_if-omitted.
        let none = crate::response::HookResponse::default();
        let s2 = serde_json::to_string(&none).unwrap();
        assert!(!s2.contains("updated_tool_output"), "{s2}");
    }

    // ---- #37 permissionDecision "defer" (4th value) ----------------------

    #[test]
    fn parse_response_bare_top_level_permission_decision_is_ignored() {
        // R-O2a: the binary's `azn` NEVER reads a BARE top-level
        // `e.permissionDecision` — it only consults
        // `e.hookSpecificOutput.permissionDecision` (gated to PreToolUse). A
        // top-level `permissionDecision` (any value) is therefore IGNORED.
        let r = parse_response(r#"{"permissionDecision":"defer"}"#, "PreToolUse").unwrap();
        assert_eq!(r.decision, None);
        let r = parse_response(r#"{"permissionDecision":"allow"}"#, "PreToolUse").unwrap();
        assert_eq!(r.decision, None);
        // An unknown BARE top-level value must NOT throw either (the binary
        // never reaches a switch for it).
        let r = parse_response(r#"{"permissionDecision":"bogus"}"#, "PreToolUse").unwrap();
        assert_eq!(r.decision, None);
    }

    #[test]
    fn parse_response_hookspecific_defer_maps_to_defer_decision() {
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"defer"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Defer));
    }

    #[test]
    fn parse_response_hsout_permission_decision_overrides_legacy_block() {
        // R-D2: `azn`'s SECOND switch reassigns `permissionBehavior`
        // UNCONDITIONALLY — a `hookSpecificOutput.permissionDecision` ALWAYS
        // overrides a prior legacy `decision:"block"` (there is no
        // `if (decision != Block)` guard in the binary). So `block` + hsOut
        // `defer` resolves to `Defer` (the binary's `permissionBehavior="defer"`
        // wins over the earlier `"deny"`), NOT `Block`.
        let r = parse_response(
            r#"{"decision":"block","hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"defer"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Defer));
    }

    #[test]
    fn parse_response_hsout_allow_overrides_legacy_block() {
        // R-D2 target case: `{"decision":"block","hookSpecificOutput":
        // {"permissionDecision":"allow"}}` → Approve (the binary's
        // `permissionBehavior="allow"`), NOT Block. The legacy block is
        // unconditionally overridden by the hsOut allow.
        let r = parse_response(
            r#"{"decision":"block","hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Approve));
    }

    #[test]
    fn parse_response_hsout_ask_maps_to_ask_decision() {
        // R-D3: `permissionDecision:"ask"` → `HookDecision::Ask` (previously
        // silently dropped via `_ => {}`). It also unconditionally overrides a
        // legacy block.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Ask));
        let r = parse_response(
            r#"{"decision":"block","hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Ask));
    }

    #[test]
    fn parse_response_hsout_permission_decision_gated_to_pre_tool_use() {
        // R-O2b: the hsOut.permissionDecision switch is gated to
        // `hookEventName === "PreToolUse"`. For a non-PreToolUse event the
        // permissionDecision is ignored (the binary never enters the switch).
        // PostToolUse hsOut `allow` must NOT set a decision.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","permissionDecision":"allow"}}"#,
            "PostToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, None);
    }

    #[test]
    fn parse_response_unknown_decision_throws() {
        // R-O2c: an unrecognised legacy `decision` rejects the whole output
        // (`azn` `default: throw Error("Unknown hook decision type: …")`).
        let err = parse_response(r#"{"decision":"maybe"}"#, "PreToolUse").unwrap_err();
        assert!(
            matches!(&err, HookResponseParseError::UnknownDecision { value } if value == "maybe"),
            "{err:?}"
        );
        // Exact message shape mirrors the binary.
        assert_eq!(
            err.to_string(),
            "Unknown hook decision type: maybe. Valid types are: approve, block"
        );
        // An empty `decision` is falsy in the binary's `if(e.decision)` — skipped, not thrown.
        let r = parse_response(r#"{"decision":""}"#, "PreToolUse").unwrap();
        assert_eq!(r.decision, None);
    }

    #[test]
    fn parse_response_unknown_permission_decision_throws() {
        // R-O2c: an unrecognised hsOut `permissionDecision` rejects the whole
        // output (`azn` `default: throw Error("Unknown hook permissionDecision
        // type: …")`).
        let err = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"perhaps"}}"#,
            "PreToolUse",
        )
        .unwrap_err();
        assert!(
            matches!(&err, HookResponseParseError::UnknownPermissionDecision { value } if value == "perhaps"),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Unknown hook permissionDecision type: perhaps. Valid types are: allow, deny, ask, defer"
        );
        // An empty hsOut `permissionDecision` is falsy in the binary's
        // `&& e.hookSpecificOutput.permissionDecision` guard — skipped, not thrown.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":""}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, None);
    }

    #[test]
    fn defer_decision_round_trips() {
        let s = serde_json::to_string(&HookDecision::Defer).unwrap();
        assert_eq!(s, r#""Defer""#);
        let back: HookDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, HookDecision::Defer);
        // existing variants unchanged
        assert_eq!(
            serde_json::to_string(&HookDecision::Block).unwrap(),
            r#""Block""#
        );
    }

    #[test]
    fn ask_decision_round_trips() {
        // R-D3: the new `Ask` variant serialises/deserialises like the others.
        let s = serde_json::to_string(&HookDecision::Ask).unwrap();
        assert_eq!(s, r#""Ask""#);
        let back: HookDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, HookDecision::Ask);
    }

    // ---- #40 top-level terminalSequence ----------------------------------

    #[test]
    fn parse_response_reads_top_level_terminal_sequence() {
        // JSON-escaped ESC `]9;hi` BEL. Parse captures the RAW string; the
        // allowlist validation happens at apply time (the consumer).
        let raw = "{\"terminalSequence\":\"\\u001b]9;hi\\u0007\"}";
        let r = parse_response(raw, "PreToolUse").unwrap();
        assert_eq!(
            r.terminal_sequence.as_deref(),
            Some("\u{001b}]9;hi\u{0007}")
        );
    }

    #[test]
    fn parse_response_terminal_sequence_absent_is_none() {
        let r = parse_response(r#"{"systemMessage":"hi"}"#, "PreToolUse").unwrap();
        assert!(r.terminal_sequence.is_none());
    }

    #[test]
    fn terminal_sequence_round_trips_camel_case() {
        let resp = crate::response::HookResponse {
            terminal_sequence: Some("\u{0007}".into()),
            ..Default::default()
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"terminal_sequence\":\"\\u0007\""), "{s}");
        let back: crate::response::HookResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.terminal_sequence.as_deref(), Some("\u{0007}"));
        // default (None) is omitted
        let none = crate::response::HookResponse::default();
        assert!(!serde_json::to_string(&none).unwrap().contains("terminal_sequence"));
    }

    // ---- PermissionDenied wire payload (`coreSchemas.ts:461-471`) ---------

    #[test]
    fn permission_denied_payload_serializes_byte_lock() {
        let p = PermissionDeniedPayload {
            hook_event_name: HookEventNamePermissionDenied,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: Some("default".into()),
            agent_id: None,
            agent_type: None,
            effort: None,
            tool_name: "Bash".into(),
            tool_input: json!({ "command": "git push" }),
            tool_use_id: "tu-9".into(),
            reason: "policy".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""hook_event_name":"PermissionDenied""#));
        assert!(s.contains(r#""tool_name":"Bash""#));
        assert!(s.contains(r#""tool_input":{"command":"git push"}"#));
        assert!(s.contains(r#""tool_use_id":"tu-9""#));
        assert!(s.contains(r#""reason":"policy""#));
    }

    // ---- B1: lifecycle-event payload byte-lock tests --------------------

    #[test]
    fn stop_payload_serializes_byte_lock() {
        let p = StopPayload {
            hook_event_name: HookEventNameStop,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            stop_hook_active: true,
            last_assistant_message: None,
            background_tasks: None,
            session_crons: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Stop","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","stop_hook_active":true}"#
        );
    }

    #[test]
    fn stop_payload_serializes_with_last_message() {
        let p = StopPayload {
            hook_event_name: HookEventNameStop,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("default".into()),
            agent_id: None,
            agent_type: None,
            effort: None,
            stop_hook_active: false,
            last_assistant_message: Some("done".into()),
            background_tasks: None,
            session_crons: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Stop","session_id":"s","transcript_path":"/t","cwd":"/w","permission_mode":"default","stop_hook_active":false,"last_assistant_message":"done"}"#
        );
    }

    #[test]
    fn subagent_stop_payload_serializes_byte_lock() {
        let p = SubagentStopPayload {
            hook_event_name: HookEventNameSubagentStop,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            stop_hook_active: true,
            agent_id: "agent-7".into(),
            agent_transcript_path: "/tmp/agent-7.jsonl".into(),
            agent_type: "general-purpose".into(),
            effort: None,
            last_assistant_message: None,
            background_tasks: None,
            session_crons: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SubagentStop","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","stop_hook_active":true,"agent_id":"agent-7","agent_transcript_path":"/tmp/agent-7.jsonl","agent_type":"general-purpose"}"#
        );
    }

    #[test]
    fn stop_payload_background_tasks_and_crons_byte_lock() {
        // Locks the LAST-two-keys order (background_tasks then session_crons)
        // and each element's claude key order (Lic / Mic). A `local_bash`
        // task emits {id,type,status,description,command}; a cron emits
        // {id,schedule,recurring,prompt}.
        let p = StopPayload {
            hook_event_name: HookEventNameStop,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            stop_hook_active: false,
            last_assistant_message: None,
            background_tasks: Some(vec![HookBackgroundTask {
                id: "bt1".into(),
                r#type: "shell".into(),
                status: "running".into(),
                description: "d".into(),
                command: Some("ls".into()),
                agent_type: None,
                server: None,
                tool: None,
                name: None,
            }]),
            session_crons: Some(vec![HookSessionCron {
                id: "c1".into(),
                schedule: "* * * * *".into(),
                recurring: false,
                prompt: "p".into(),
            }]),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Stop","session_id":"s","transcript_path":"/t","cwd":"/w","stop_hook_active":false,"background_tasks":[{"id":"bt1","type":"shell","status":"running","description":"d","command":"ls"}],"session_crons":[{"id":"c1","schedule":"* * * * *","recurring":false,"prompt":"p"}]}"#
        );
    }

    #[test]
    fn stop_payload_empty_background_arrays_emit_brackets() {
        // claude: when tool-use context IS present but registries are empty,
        // both keys are emitted as `[]` (Some(vec![])), never omitted.
        let p = StopPayload {
            hook_event_name: HookEventNameStop,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            stop_hook_active: false,
            last_assistant_message: None,
            background_tasks: Some(vec![]),
            session_crons: Some(vec![]),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Stop","session_id":"s","transcript_path":"/t","cwd":"/w","stop_hook_active":false,"background_tasks":[],"session_crons":[]}"#
        );
    }

    #[test]
    fn task_completed_payload_serializes_byte_lock() {
        let p = TaskCompletedPayload {
            hook_event_name: HookEventNameTaskCompleted,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            task_id: "task-42".into(),
            task_subject: "Build the thing".into(),
            task_description: None,
            teammate_name: None,
            team_name: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"TaskCompleted","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","task_id":"task-42","task_subject":"Build the thing"}"#
        );
    }

    #[test]
    fn task_completed_payload_serializes_with_optionals() {
        let p = TaskCompletedPayload {
            hook_event_name: HookEventNameTaskCompleted,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            task_id: "t1".into(),
            task_subject: "subj".into(),
            task_description: Some("desc".into()),
            teammate_name: Some("alice".into()),
            team_name: Some("core".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"TaskCompleted","session_id":"s","transcript_path":"/t","cwd":"/w","task_id":"t1","task_subject":"subj","task_description":"desc","teammate_name":"alice","team_name":"core"}"#
        );
    }

    #[test]
    fn task_created_payload_serializes_byte_lock() {
        let p = TaskCreatedPayload {
            hook_event_name: HookEventNameTaskCreated,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            task_id: "task-42".into(),
            task_subject: "LocalBash".into(),
            task_description: Some("do the work".into()),
            teammate_name: None,
            team_name: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"TaskCreated","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","task_id":"task-42","task_subject":"LocalBash","task_description":"do the work"}"#
        );
    }

    /// T25: when the creating teammate's identity is bound, the `TaskCreated`
    /// wire payload carries `teammate_name` / `team_name` (claude-code
    /// `getAgentName()` / `getTeamName()`, `utils/hooks.ts:3756-3764`).
    #[test]
    fn task_created_payload_serializes_teammate_and_team() {
        let p = TaskCreatedPayload {
            hook_event_name: HookEventNameTaskCreated,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            task_id: "t1".into(),
            task_subject: "subj".into(),
            task_description: Some("desc".into()),
            teammate_name: Some("researcher".into()),
            team_name: Some("alpha".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"TaskCreated","session_id":"s","transcript_path":"/t","cwd":"/w","task_id":"t1","task_subject":"subj","task_description":"desc","teammate_name":"researcher","team_name":"alpha"}"#
        );
    }

    #[test]
    fn user_prompt_submit_payload_serializes_byte_lock() {
        let p = UserPromptSubmitPayload {
            hook_event_name: HookEventNameUserPromptSubmit,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            prompt: "fix the bug".into(),
            session_title: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","prompt":"fix the bug"}"#
        );
    }

    #[test]
    fn session_start_payload_serializes_byte_lock() {
        let p = SessionStartPayload {
            hook_event_name: HookEventNameSessionStart,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            source: "startup".into(),
            agent_type: None,
            effort: None,
            model: None,
            session_title: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SessionStart","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","source":"startup"}"#
        );
    }

    #[test]
    fn session_start_payload_serializes_with_model() {
        let p = SessionStartPayload {
            hook_event_name: HookEventNameSessionStart,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            source: "resume".into(),
            agent_type: Some("code-reviewer".into()),
            effort: None,
            model: Some("claude-opus".into()),
            session_title: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SessionStart","session_id":"s","transcript_path":"/t","cwd":"/w","source":"resume","agent_type":"code-reviewer","model":"claude-opus"}"#
        );
    }

    #[test]
    fn stop_failure_payload_serializes_byte_lock() {
        let p = StopFailurePayload {
            hook_event_name: HookEventNameStopFailure,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            error: "rate_limit".into(),
            error_details: None,
            last_assistant_message: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"StopFailure","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","error":"rate_limit"}"#
        );
    }

    #[test]
    fn stop_failure_payload_serializes_with_details() {
        let p = StopFailurePayload {
            hook_event_name: HookEventNameStopFailure,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            error: "server_error".into(),
            error_details: Some("upstream 500".into()),
            last_assistant_message: Some("partial".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"StopFailure","session_id":"s","transcript_path":"/t","cwd":"/w","error":"server_error","error_details":"upstream 500","last_assistant_message":"partial"}"#
        );
    }

    // ---- B1: expected_event marker round-trips --------------------------

    #[test]
    fn lifecycle_event_name_markers_round_trip() {
        // Each marker serializes to exactly its literal and deserializes back.
        assert_eq!(
            serde_json::to_string(&HookEventNameStop).unwrap(),
            r#""Stop""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameSubagentStop).unwrap(),
            r#""SubagentStop""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameTaskCompleted).unwrap(),
            r#""TaskCompleted""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameUserPromptSubmit).unwrap(),
            r#""UserPromptSubmit""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameSessionStart).unwrap(),
            r#""SessionStart""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameStopFailure).unwrap(),
            r#""StopFailure""#
        );

        let _: HookEventNameStop = serde_json::from_str(r#""Stop""#).unwrap();
        let _: HookEventNameSubagentStop = serde_json::from_str(r#""SubagentStop""#).unwrap();
        let _: HookEventNameTaskCompleted = serde_json::from_str(r#""TaskCompleted""#).unwrap();
        let _: HookEventNameUserPromptSubmit =
            serde_json::from_str(r#""UserPromptSubmit""#).unwrap();
        let _: HookEventNameSessionStart = serde_json::from_str(r#""SessionStart""#).unwrap();
        let _: HookEventNameStopFailure = serde_json::from_str(r#""StopFailure""#).unwrap();
    }

    #[test]
    fn lifecycle_event_name_marker_rejects_wrong_literal() {
        assert!(serde_json::from_str::<HookEventNameStop>(r#""SubagentStop""#).is_err());
        assert!(serde_json::from_str::<HookEventNameStopFailure>(r#""Stop""#).is_err());
    }

    #[test]
    fn lifecycle_payloads_round_trip() {
        let p = SubagentStopPayload {
            hook_event_name: HookEventNameSubagentStop,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("plan".into()),
            stop_hook_active: true,
            agent_id: "a-1".into(),
            agent_transcript_path: "/t/a-1.jsonl".into(),
            agent_type: "general-purpose".into(),
            effort: None,
            last_assistant_message: Some("hi".into()),
            background_tasks: None,
            session_crons: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: SubagentStopPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.agent_id, "a-1");
        assert!(back.stop_hook_active);
        assert_eq!(back.last_assistant_message.as_deref(), Some("hi"));
    }

    // ---- B1: parse_response validates the new hookEventNames ------------

    #[test]
    fn parse_response_validates_new_event_names() {
        for name in [
            "Stop",
            "SubagentStop",
            "TaskCompleted",
            "UserPromptSubmit",
            "SessionStart",
            "StopFailure",
        ] {
            // Matching event name parses fine.
            let raw = format!(
                r#"{{"hookSpecificOutput":{{"hookEventName":"{name}","additionalContext":"x"}}}}"#
            );
            let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
            let r = parse_response(&raw, leaked).unwrap();
            // `additionalContext` now lands on its own field (NOT system_message).
            assert_eq!(r.additional_context.as_deref(), Some("x"));

            // A mismatched name is rejected.
            let err = parse_response(
                r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse"}}"#,
                leaked,
            )
            .unwrap_err();
            assert!(matches!(
                err,
                HookResponseParseError::EventNameMismatch { .. }
            ));
        }
    }

    // ---- B6: additional-event payload byte-lock tests -------------------

    #[test]
    fn post_tool_use_failure_payload_serializes_byte_lock() {
        let p = PostToolUseFailurePayload {
            hook_event_name: HookEventNamePostToolUseFailure,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            tool_name: "Bash".into(),
            tool_input: Value::Null,
            tool_use_id: "tu-1".into(),
            error: "boom".into(),
            is_interrupt: None,
            duration_ms: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PostToolUseFailure","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":null,"tool_use_id":"tu-1","error":"boom"}"#
        );
    }

    #[test]
    fn post_tool_use_failure_payload_serializes_with_interrupt() {
        let p = PostToolUseFailurePayload {
            hook_event_name: HookEventNamePostToolUseFailure,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            tool_name: "Edit".into(),
            tool_input: json!({"file_path": "/f"}),
            tool_use_id: "tu".into(),
            error: "cancelled".into(),
            is_interrupt: Some(true),
            duration_ms: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""tool_input":{"file_path":"/f"}"#));
        assert!(s.contains(r#""error":"cancelled","is_interrupt":true"#));
    }

    #[test]
    fn session_end_payload_serializes_byte_lock() {
        let p = SessionEndPayload {
            hook_event_name: HookEventNameSessionEnd,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            reason: "logout".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SessionEnd","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","reason":"logout"}"#
        );
    }

    #[test]
    fn pre_compact_payload_serializes_null_custom_instructions() {
        // `custom_instructions` is `.nullable()` (not optional) — must always
        // be present, serialized as `null` when absent.
        let p = PreCompactPayload {
            hook_event_name: HookEventNamePreCompact,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            trigger: "manual".into(),
            custom_instructions: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PreCompact","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","trigger":"manual","custom_instructions":null}"#
        );
    }

    #[test]
    fn pre_compact_payload_serializes_with_custom_instructions() {
        let p = PreCompactPayload {
            hook_event_name: HookEventNamePreCompact,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            trigger: "auto".into(),
            custom_instructions: Some("keep the API surface".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""trigger":"auto","custom_instructions":"keep the API surface""#));
    }

    #[test]
    fn post_compact_payload_serializes_byte_lock() {
        let p = PostCompactPayload {
            hook_event_name: HookEventNamePostCompact,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            trigger: String::new(),
            compact_summary: "did the thing".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PostCompact","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","trigger":"","compact_summary":"did the thing"}"#
        );
    }

    #[test]
    fn notification_payload_serializes_byte_lock() {
        let p = NotificationPayload {
            hook_event_name: HookEventNameNotification,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            message: "build done".into(),
            title: None,
            notification_type: "info".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Notification","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","message":"build done","notification_type":"info"}"#
        );
    }

    #[test]
    fn notification_payload_serializes_with_title() {
        let p = NotificationPayload {
            hook_event_name: HookEventNameNotification,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            message: "msg".into(),
            title: Some("Heads up".into()),
            notification_type: "warn".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""message":"msg","title":"Heads up","notification_type":"warn""#));
    }

    #[test]
    fn permission_request_payload_serializes_byte_lock() {
        let p = PermissionRequestPayload {
            hook_event_name: HookEventNamePermissionRequest,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            tool_name: "Bash".into(),
            tool_input: json!({"command": "rm -rf /"}),
            permission_suggestions: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PermissionRequest","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":{"command":"rm -rf /"}}"#
        );
    }

    #[test]
    fn setup_payload_serializes_byte_lock() {
        let p = SetupPayload {
            hook_event_name: HookEventNameSetup,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            trigger: String::new(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Setup","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","trigger":""}"#
        );
    }

    #[test]
    fn subagent_start_payload_serializes_byte_lock() {
        let p = SubagentStartPayload {
            hook_event_name: HookEventNameSubagentStart,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: "agent-7".into(),
            agent_type: "general-purpose".into(),
            effort: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SubagentStart","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","agent_id":"agent-7","agent_type":"general-purpose"}"#
        );
    }

    #[test]
    fn cwd_changed_payload_serializes_byte_lock() {
        let p = CwdChangedPayload {
            hook_event_name: HookEventNameCwdChanged,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            old_cwd: "/old".into(),
            new_cwd: "/new".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"CwdChanged","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","old_cwd":"/old","new_cwd":"/new"}"#
        );
    }

    #[test]
    fn file_changed_payload_serializes_byte_lock() {
        let p = FileChangedPayload {
            hook_event_name: HookEventNameFileChanged,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            file_path: "/work/src/main.rs".into(),
            event: "change".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"FileChanged","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","file_path":"/work/src/main.rs","event":"change"}"#
        );
    }

    #[test]
    fn worktree_remove_payload_serializes_byte_lock() {
        let p = WorktreeRemovePayload {
            hook_event_name: HookEventNameWorktreeRemove,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            worktree_path: "/work/.worktrees/feat".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"WorktreeRemove","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","worktree_path":"/work/.worktrees/feat"}"#
        );
    }

    #[test]
    fn b6_event_name_markers_round_trip() {
        for (got, want) in [
            (serde_json::to_string(&HookEventNamePostToolUseFailure).unwrap(), r#""PostToolUseFailure""#),
            (serde_json::to_string(&HookEventNameSessionEnd).unwrap(), r#""SessionEnd""#),
            (serde_json::to_string(&HookEventNamePreCompact).unwrap(), r#""PreCompact""#),
            (serde_json::to_string(&HookEventNamePostCompact).unwrap(), r#""PostCompact""#),
            (serde_json::to_string(&HookEventNameNotification).unwrap(), r#""Notification""#),
            (serde_json::to_string(&HookEventNamePermissionRequest).unwrap(), r#""PermissionRequest""#),
            (serde_json::to_string(&HookEventNameSetup).unwrap(), r#""Setup""#),
            (serde_json::to_string(&HookEventNameSubagentStart).unwrap(), r#""SubagentStart""#),
            (serde_json::to_string(&HookEventNameCwdChanged).unwrap(), r#""CwdChanged""#),
            (serde_json::to_string(&HookEventNameFileChanged).unwrap(), r#""FileChanged""#),
            (serde_json::to_string(&HookEventNameWorktreeRemove).unwrap(), r#""WorktreeRemove""#),
        ] {
            assert_eq!(got, want);
        }
        let _: HookEventNamePostToolUseFailure =
            serde_json::from_str(r#""PostToolUseFailure""#).unwrap();
        let _: HookEventNameSessionEnd = serde_json::from_str(r#""SessionEnd""#).unwrap();
        let _: HookEventNameWorktreeRemove = serde_json::from_str(r#""WorktreeRemove""#).unwrap();
        assert!(serde_json::from_str::<HookEventNameSetup>(r#""Notification""#).is_err());
    }

    // ---- deferred-completion batch: final-four payload byte-lock tests ------

    #[test]
    fn config_change_payload_serializes_byte_lock() {
        let p = ConfigChangePayload {
            hook_event_name: HookEventNameConfigChange,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            source: crate::events::ConfigChangeSource::LocalSettings,
            file_path: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"ConfigChange","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","source":"local_settings"}"#
        );
    }

    #[test]
    fn config_change_payload_serializes_with_file_path() {
        let p = ConfigChangePayload {
            hook_event_name: HookEventNameConfigChange,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            source: crate::events::ConfigChangeSource::PolicySettings,
            file_path: Some("/etc/claude/policy.json".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"ConfigChange","session_id":"s","transcript_path":"/t","cwd":"/w","source":"policy_settings","file_path":"/etc/claude/policy.json"}"#
        );
    }

    #[test]
    fn instructions_loaded_payload_serializes_byte_lock() {
        let p = InstructionsLoadedPayload {
            hook_event_name: HookEventNameInstructionsLoaded,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            file_path: "/work/LINGXI.md".into(),
            memory_type: crate::events::InstructionsMemoryType::Project,
            load_reason: crate::events::InstructionsLoadReason::SessionStart,
            globs: None,
            trigger_file_path: None,
            parent_file_path: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"InstructionsLoaded","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","file_path":"/work/LINGXI.md","memory_type":"Project","load_reason":"session_start"}"#
        );
    }

    #[test]
    fn instructions_loaded_payload_serializes_with_optionals() {
        let p = InstructionsLoadedPayload {
            hook_event_name: HookEventNameInstructionsLoaded,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            file_path: "/w/rules/api.md".into(),
            memory_type: crate::events::InstructionsMemoryType::Managed,
            load_reason: crate::events::InstructionsLoadReason::Compact,
            globs: Some(vec!["src/**/*.rs".into()]),
            trigger_file_path: Some("/w/src/main.rs".into()),
            parent_file_path: Some("/w/LINGXI.md".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"InstructionsLoaded","session_id":"s","transcript_path":"/t","cwd":"/w","file_path":"/w/rules/api.md","memory_type":"Managed","load_reason":"compact","globs":["src/**/*.rs"],"trigger_file_path":"/w/src/main.rs","parent_file_path":"/w/LINGXI.md"}"#
        );
    }

    #[test]
    fn elicitation_payload_serializes_byte_lock() {
        let p = ElicitationPayload {
            hook_event_name: HookEventNameElicitation,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            mcp_server_name: "github".into(),
            message: "Authorize?".into(),
            mode: None,
            url: None,
            elicitation_id: None,
            requested_schema: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Elicitation","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","mcp_server_name":"github","message":"Authorize?"}"#
        );
    }

    #[test]
    fn elicitation_payload_serializes_with_optionals_and_permission_mode() {
        // Elicitation alone threads permission_mode (createBaseHookInput(permissionMode)).
        let p = ElicitationPayload {
            hook_event_name: HookEventNameElicitation,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("default".into()),
            agent_id: None,
            agent_type: None,
            effort: None,
            mcp_server_name: "linear".into(),
            message: "Pick".into(),
            mode: Some(crate::events::ElicitationMode::Url),
            url: Some("https://example.test".into()),
            elicitation_id: Some("e-1".into()),
            requested_schema: Some(json!({"type": "object"})),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Elicitation","session_id":"s","transcript_path":"/t","cwd":"/w","permission_mode":"default","mcp_server_name":"linear","message":"Pick","mode":"url","url":"https://example.test","elicitation_id":"e-1","requested_schema":{"type":"object"}}"#
        );
    }

    #[test]
    fn worktree_create_payload_serializes_byte_lock() {
        let p = WorktreeCreatePayload {
            hook_event_name: HookEventNameWorktreeCreate,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            name: "feature-x".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"WorktreeCreate","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","name":"feature-x"}"#
        );
    }

    #[test]
    fn deferred_batch_event_name_markers_round_trip() {
        for (got, want) in [
            (serde_json::to_string(&HookEventNameConfigChange).unwrap(), r#""ConfigChange""#),
            (serde_json::to_string(&HookEventNameInstructionsLoaded).unwrap(), r#""InstructionsLoaded""#),
            (serde_json::to_string(&HookEventNameElicitation).unwrap(), r#""Elicitation""#),
            (serde_json::to_string(&HookEventNameWorktreeCreate).unwrap(), r#""WorktreeCreate""#),
        ] {
            assert_eq!(got, want);
        }
        let _: HookEventNameConfigChange = serde_json::from_str(r#""ConfigChange""#).unwrap();
        let _: HookEventNameInstructionsLoaded =
            serde_json::from_str(r#""InstructionsLoaded""#).unwrap();
        let _: HookEventNameElicitation = serde_json::from_str(r#""Elicitation""#).unwrap();
        let _: HookEventNameWorktreeCreate = serde_json::from_str(r#""WorktreeCreate""#).unwrap();
        assert!(serde_json::from_str::<HookEventNameConfigChange>(r#""Elicitation""#).is_err());
    }

    // ---- [P0] ElicitationResult payload (parity fix) ---------------------

    #[test]
    fn elicitation_result_payload_serializes_required_fields() {
        // Binary-confirmed schema: {hook_event_name:"ElicitationResult",
        // mcp_server_name:string, …, action:enum(["accept","decline","cancel"]),
        // content?:record}. Required wire shape with minimal fields.
        let p = ElicitationResultPayload {
            hook_event_name: HookEventNameElicitationResult,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            mcp_server_name: "my-server".into(),
            elicitation_id: None,
            mode: None,
            action: "accept".into(),
            content: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""hook_event_name":"ElicitationResult""#), "{s}");
        assert!(s.contains(r#""mcp_server_name":"my-server""#), "{s}");
        assert!(s.contains(r#""action":"accept""#), "{s}");
        // Optional fields absent when None.
        assert!(!s.contains("elicitation_id"), "{s}");
        assert!(!s.contains("content"), "{s}");
        assert!(!s.contains("mode"), "{s}");
    }

    #[test]
    fn elicitation_result_payload_serializes_with_content() {
        // `content` is a record(string, unknown) — passes through as-is.
        let p = ElicitationResultPayload {
            hook_event_name: HookEventNameElicitationResult,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            mcp_server_name: "srv".into(),
            elicitation_id: Some("eid-42".into()),
            mode: Some(crate::events::ElicitationMode::Form),
            action: "decline".into(),
            content: Some(json!({"reason": "no thanks"})),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""action":"decline""#), "{s}");
        assert!(s.contains(r#""elicitation_id":"eid-42""#), "{s}");
        assert!(s.contains(r#""mode":"form""#), "{s}");
        assert!(s.contains(r#""content":{"reason":"no thanks"}"#), "{s}");
    }

    #[test]
    fn elicitation_result_marker_serializes_correct_literal() {
        assert_eq!(
            serde_json::to_string(&HookEventNameElicitationResult).unwrap(),
            r#""ElicitationResult""#
        );
        let _: HookEventNameElicitationResult =
            serde_json::from_str(r#""ElicitationResult""#).unwrap();
        assert!(
            serde_json::from_str::<HookEventNameElicitationResult>(r#""Elicitation""#).is_err()
        );
    }

    // ---- [P1] session_title in input payloads ----------------------------

    #[test]
    fn user_prompt_submit_payload_with_session_title() {
        // Binary-confirmed: `session_title` is optional on `UserPromptSubmit`
        // (BIN off 201745825). When `Some`, it appears AFTER `prompt`.
        let p = UserPromptSubmitPayload {
            hook_event_name: HookEventNameUserPromptSubmit,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            effort: None,
            prompt: "hello".into(),
            session_title: Some("My Project".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""session_title":"My Project""#), "{s}");
        // When None, key is omitted.
        let p2 = UserPromptSubmitPayload { session_title: None, ..p };
        let s2 = serde_json::to_string(&p2).unwrap();
        assert!(!s2.contains("session_title"), "{s2}");
    }

    #[test]
    fn session_start_payload_with_session_title() {
        // Binary-confirmed: `session_title` is optional on `SessionStart`
        // (BIN off ~201746000).
        let p = SessionStartPayload {
            hook_event_name: HookEventNameSessionStart,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            source: "startup".into(),
            agent_type: None,
            effort: None,
            model: None,
            session_title: Some("New Chat".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""session_title":"New Chat""#), "{s}");
        // When None, key is omitted.
        let p2 = SessionStartPayload { session_title: None, ..p };
        let s2 = serde_json::to_string(&p2).unwrap();
        assert!(!s2.contains("session_title"), "{s2}");
    }

    // ---- [P1] UserPromptSubmit hookSpecificOutput: sessionTitle + suppressOriginalPrompt

    #[test]
    fn parse_response_user_prompt_submit_session_title() {
        // Binary-confirmed: `sessionTitle` in hookSpecificOutput for `UserPromptSubmit`
        // (BIN off 201754804). Scoped to `UserPromptSubmit` only.
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","sessionTitle":"Renamed"}}"#,
            "UserPromptSubmit",
        )
        .unwrap();
        assert_eq!(r.session_title.as_deref(), Some("Renamed"));
        // Not parsed for other events.
        let r2 = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"Stop","sessionTitle":"x"}}"#,
            "Stop",
        )
        .unwrap();
        assert!(r2.session_title.is_none(), "sessionTitle ignored for non-UserPromptSubmit");
    }

    #[test]
    fn parse_response_user_prompt_submit_suppress_original_prompt() {
        // Binary-confirmed: `suppressOriginalPrompt` in hookSpecificOutput for
        // `UserPromptSubmit` (BIN off 201754804; description: "When decision is
        // 'block', omit the original prompt from the block message").
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","suppressOriginalPrompt":true}}"#,
            "UserPromptSubmit",
        )
        .unwrap();
        assert!(r.suppress_original_prompt);
        // `false` is passed through correctly.
        let r2 = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","suppressOriginalPrompt":false}}"#,
            "UserPromptSubmit",
        )
        .unwrap();
        assert!(!r2.suppress_original_prompt);
        // Not parsed for other events.
        let r3 = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"Stop","suppressOriginalPrompt":true}}"#,
            "Stop",
        )
        .unwrap();
        assert!(!r3.suppress_original_prompt, "suppressOriginalPrompt ignored for non-UserPromptSubmit");
        // Default is false.
        let r4 = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit"}}"#,
            "UserPromptSubmit",
        )
        .unwrap();
        assert!(!r4.suppress_original_prompt);
    }

    // ---- [P1] MessageDisplay hookSpecificOutput: displayContent -----------

    #[test]
    fn parse_response_message_display_display_content() {
        // Binary-confirmed: `displayContent` in hookSpecificOutput for
        // `MessageDisplay` (BIN off 201757586; description: "Text displayed in
        // place of the delta.").
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"MessageDisplay","displayContent":"overridden text"}}"#,
            "MessageDisplay",
        )
        .unwrap();
        assert_eq!(r.display_content.as_deref(), Some("overridden text"));
        // Absent when not set.
        let r2 = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"MessageDisplay"}}"#,
            "MessageDisplay",
        )
        .unwrap();
        assert!(r2.display_content.is_none());
        // Not parsed for other events.
        let r3 = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"Stop","displayContent":"x"}}"#,
            "Stop",
        )
        .unwrap();
        assert!(r3.display_content.is_none(), "displayContent ignored for non-MessageDisplay");
    }
}
