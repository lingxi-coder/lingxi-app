//! Tests for `skill.rs`, extracted from inline `#[cfg(test)]` blocks. Included via `#[path] mod skill_test;`.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx, StubProcess};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// A loader that returns a fixed descriptor for any name.
    struct FixedLoader(Option<SkillDescriptor>);
    #[async_trait]
    impl SkillLoader for FixedLoader {
        async fn load(&self, _name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            Ok(self.0.clone())
        }
    }

    /// A loader that captures the (normalized) name it was asked to load.
    struct CapturingLoader {
        seen: std::sync::Mutex<Option<String>>,
        desc: Option<SkillDescriptor>,
    }
    #[async_trait]
    impl SkillLoader for CapturingLoader {
        async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
            *self.seen.lock().unwrap() = Some(name.to_string());
            Ok(self.desc.clone())
        }
    }

    fn prompt_desc(name: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.into(),
            description: "a skill".into(),
            body: "body here".into(),
            ..SkillDescriptor::default()
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SKILL_TOOL_NAME, "Skill");
        assert_eq!(MAX_SKILL_DESCRIPTOR_LEN, 1024);
    }

    #[test]
    fn normalize_strips_single_leading_slash_and_trims() {
        assert_eq!(normalize_skill_name("/commit"), "commit");
        assert_eq!(normalize_skill_name("  /commit  "), "commit");
        assert_eq!(normalize_skill_name("commit"), "commit");
        // Only a single leading slash is stripped.
        assert_eq!(normalize_skill_name("//commit"), "/commit");
    }

    #[test]
    fn skill_name_dimension_keeps_builtin_and_sanitizes_custom() {
        // A known builtin command name passes through unchanged...
        let a_builtin = command_api::builtin_support::names::BUILTIN_COMMAND_NAMES[0];
        assert_eq!(skill_name_dimension(a_builtin), a_builtin);
        // ...while any non-builtin (custom) skill collapses to "custom" — the
        // Verified/whitelisted SKILL_INVOKED dimension never leaks a raw PII name.
        assert_eq!(
            skill_name_dimension("definitely-not-a-builtin-skill-xyz"),
            "custom"
        );
    }

    #[test]
    fn schema_uses_skill_and_optional_args() {
        let props = &SCHEMA["properties"];
        assert_eq!(props["skill"]["type"], json!("string"));
        // TS `inputSchema` is `skill: z.string().describe(...)` with NO `.min(1)`
        // (SkillTool.ts:291-298) — the delivered schema must NOT carry a
        // `minLength` constraint. The runtime "Invalid skill format" check
        // (validate_input/call) is what enforces non-blankness, not the schema.
        assert!(
            props["skill"].get("minLength").is_none(),
            "skill schema must not constrain minLength (TS has no .min(1))"
        );
        assert_eq!(props["args"]["type"], json!("string"));
        assert_eq!(SCHEMA["required"], json!(["skill"]));
        // No legacy `name` field remains.
        assert!(props.get("name").is_none());
    }

    #[tokio::test]
    async fn slash_prefixed_skill_is_normalized_before_lookup() {
        let loader = Arc::new(CapturingLoader {
            seen: std::sync::Mutex::new(None),
            desc: Some(prompt_desc("commit")),
        });
        let tool = SkillTool::with_loader(shell_test_ctx(dummy_out()), loader.clone());
        let out = tool
            .call(json!({"skill": "/commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Loader saw the slash-stripped name.
        assert_eq!(loader.seen.lock().unwrap().as_deref(), Some("commit"));
        // commandName is the normalized name + inline status.
        assert_eq!(out.data["commandName"], json!("commit"));
        assert_eq!(out.data["status"], json!("inline"));
        assert_eq!(out.data["success"], json!(true));
    }

    #[tokio::test]
    async fn unknown_skill_locked_error() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"skill": "absent"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("unknown");
        assert!(format!("{err}").contains("Unknown skill: absent"));
    }

    #[tokio::test]
    async fn disable_model_invocation_locked_error() {
        let desc = SkillDescriptor {
            disable_model_invocation: true,
            ..prompt_desc("locked")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let err = tool
            .call(json!({"skill": "locked"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("disabled");
        assert!(format!("{err}").contains(
            "Skill locked cannot be used with Skill tool due to disable-model-invocation"
        ));
    }

    #[tokio::test]
    async fn non_prompt_skill_locked_error() {
        let desc = SkillDescriptor {
            command_type: SkillCommandType::Other,
            ..prompt_desc("local-cmd")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let err = tool
            .call(json!({"skill": "local-cmd"}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("non-prompt");
        assert!(format!("{err}").contains(
            "local-cmd is a built-in CLI command, not a skill. Ask the user to run /local-cmd themselves — it cannot be invoked via the Skill tool."
        ));
    }

    #[tokio::test]
    async fn result_surfaces_model_and_allowed_tools_and_status_inline() {
        let desc = SkillDescriptor {
            model: Some("opus".into()),
            allowed_tools: vec!["Bash".into(), "Read".into()],
            ..prompt_desc("rich")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "rich"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["status"], json!("inline"));
        assert_eq!(out.data["model"], json!("opus"));
        assert_eq!(out.data["allowedTools"], json!(["Bash", "Read"]));
        assert_eq!(out.data["commandName"], json!("rich"));
    }

    #[tokio::test]
    async fn model_and_allowed_tools_omitted_when_absent() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Optional fields are omitted (matches TS `.optional()` surfacing).
        assert!(out.data.get("model").is_none());
        assert!(out.data.get("allowedTools").is_none());
        assert_eq!(out.data["body"], json!("body here"));
    }

    #[tokio::test]
    async fn model_frontmatter_returns_context_modifier_switching_main_loop_model() {
        // SKILLEXEC.3 (model scope): a skill with `model:` returns a
        // `context_modifier` that switches the turn's main-loop model. Here the
        // seed `ctx` (fresh_ctx → main_loop_model "test", no `[1m]`) has no 1M
        // suffix, so the resolved model is the bare override.
        let desc = SkillDescriptor {
            model: Some("claude-opus-4-6".into()),
            ..prompt_desc("switcher")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "switcher"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let modifier = out
            .context_modifier
            .expect("a skill with model: returns a context_modifier");
        let modified = modifier(fresh_ctx());
        assert_eq!(modified.options.main_loop_model, "claude-opus-4-6");
    }

    #[tokio::test]
    async fn model_frontmatter_modifier_preserves_1m_suffix() {
        // When the session is on `[1m]` and the skill's family supports 1M, the
        // suffix is carried over (TS resolveSkillModelOverride).
        let desc = SkillDescriptor {
            model: Some("claude-sonnet-4-6".into()),
            ..prompt_desc("switcher")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "switcher"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let modifier = out.context_modifier.expect("context_modifier present");
        let mut seed = fresh_ctx();
        seed.options.main_loop_model = "claude-opus-4-6[1m]".into();
        let modified = modifier(seed);
        assert_eq!(modified.options.main_loop_model, "claude-sonnet-4-6[1m]");
    }

    #[tokio::test]
    async fn no_model_frontmatter_returns_no_context_modifier() {
        // Byte-identical guard: a skill WITHOUT a `model:` frontmatter returns
        // `context_modifier: None`, so the turn loop's no-override path is
        // untouched (session.model never changes).
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(
            out.context_modifier.is_none(),
            "no model: frontmatter → no context_modifier (byte-identical)"
        );
    }

    #[tokio::test]
    async fn model_content_is_inline_launching_line_and_omits_body() {
        // TS inline path (SkillTool.ts:856-861): the model's tool_result content
        // is exactly `Launching skill: ${commandName}` — never the JSON dump of
        // the output object (which would leak the full skill `body`).
        let desc = SkillDescriptor {
            body: "SECRET FULL SKILL BODY that must not reach the model".into(),
            ..prompt_desc("commit")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        // Slash-prefixed input: model_content uses the normalized name.
        let out = tool
            .call(json!({"skill": "/commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let mc = out.data["model_content"]
            .as_str()
            .expect("model_content is a string");
        assert_eq!(mc, "Launching skill: commit");
        // The body still rides along for non-model consumers, but is NOT in the
        // model-facing string.
        assert!(!mc.contains("SECRET FULL SKILL BODY"));
        assert_eq!(
            out.data["body"],
            json!("SECRET FULL SKILL BODY that must not reach the model")
        );
    }

    #[tokio::test]
    async fn args_accepted_optionally_and_echoed() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("commit")))),
        );
        // With args.
        let out = tool
            .call(
                json!({"skill": "commit", "args": "--amend"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["args"], json!("--amend"));
        // Without args — no `args` key.
        let out2 = tool
            .call(json!({"skill": "commit"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(out2.data.get("args").is_none());
    }

    /// SKILLEXEC.3: a model-invocable Prompt skill expands its body's
    /// `$ARGUMENTS` into a `new_messages` user message the turn loop injects, so
    /// the model acts on the expanded skill prompt.
    #[tokio::test]
    async fn expands_arguments_into_new_messages() {
        let desc = SkillDescriptor {
            body: "Review PR $ARGUMENTS now".into(),
            ..prompt_desc("review-pr")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "review-pr", "args": "123"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.new_messages.len(), 1, "expanded prompt injected");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    assert_eq!(text, "Review PR 123 now");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
        // The inline model-facing string is still the launch line — the body is
        // NOT leaked into the tool_result content (SKILLEXEC.1 invariant holds).
        assert_eq!(
            out.data["model_content"],
            json!("Launching skill: review-pr")
        );
    }

    /// Named frontmatter arguments (`$name`) resolve via the descriptor's
    /// `argument_names` (TS `processPromptSlashCommand` passes the command's
    /// `argNames`).
    #[tokio::test]
    async fn expands_named_argument_into_new_messages() {
        let desc = SkillDescriptor {
            body: "Hello $name".into(),
            argument_names: vec!["name".into()],
            ..prompt_desc("greet")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "greet", "args": "world"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => assert_eq!(text, "Hello world"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// SKILLEXEC.3 (bundled): when a descriptor carries `dynamic_body`, the Skill
    /// tool calls the builder with the raw args INSTEAD of static templating, so
    /// the bundled-skill two-branch behavior (empty→usage, else→buildPrompt) is
    /// honored end-to-end (port of `getPromptForCommand`, loop.ts:84). The
    /// descriptor `body` is ignored on this path.
    #[tokio::test]
    async fn dynamic_body_replaces_static_templating() {
        // Test builder mirroring loop.ts's empty→usage vs non-empty branch.
        struct TestBuilder;
        impl command_api::BundledPromptFn for TestBuilder {
            fn build(&self, args: &str) -> String {
                let t = args.trim();
                if t.is_empty() {
                    "USAGE".into()
                } else {
                    format!("BUILT[{t}]")
                }
            }
        }
        let desc = SkillDescriptor {
            // `body` carries a `$ARGUMENTS` placeholder that MUST be ignored.
            body: "STATIC $ARGUMENTS".into(),
            dynamic_body: Some(Arc::new(TestBuilder)),
            ..prompt_desc("loop")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc.clone()))),
        );

        // Empty args → usage branch.
        let out = tool
            .call(json!({"skill": "loop"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => assert_eq!(text, "USAGE"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }

        // Non-empty args → buildPrompt branch; raw args reach the builder.
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "loop", "args": "  check the deploy  "}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    assert_eq!(text, "BUILT[check the deploy]");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// A skill with no placeholders and empty args injects the body verbatim
    /// (no `$ARGUMENTS` tail append — TS appends only when args is non-empty).
    #[tokio::test]
    async fn no_placeholder_no_args_injects_body_verbatim() {
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(prompt_desc("plain")))),
        );
        let out = tool
            .call(json!({"skill": "plain"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => assert_eq!(text, "body here"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    fn out_with_stdout(stdout: &str) -> ProcessOutput {
        ProcessOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// SKILLEXEC.6: a skill body with an embedded inline `!`…`` block runs the
    /// command through the injected shell runner and splices its stdout into the
    /// expanded prompt (the fake `ProcessRunner` returns "hi" for any command).
    #[tokio::test]
    async fn expands_inline_shell_command_into_new_messages() {
        let desc = SkillDescriptor {
            body: "before !`echo hi` after".into(),
            ..prompt_desc("sh")
        };
        // Snapshot creation consumes the first process result; because the stub
        // does not materialize the requested file, execution falls back to the
        // login shell and consumes the second result.
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = Arc::new(StubProcess::with(vec![
            dummy_out(),
            out_with_stdout("hi\n"),
        ]));
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "sh"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    // format_bash_output trims the stdout -> "hi".
                    assert_eq!(text, "before hi after");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
        // The model-facing line is still the launch string (body not leaked).
        assert_eq!(out.data["model_content"], json!("Launching skill: sh"));
    }

    /// SKILLEXEC #26: the shell snapshot is SESSION-scoped — its creation shell
    /// runs at most once per `SkillTool`, not once per `call()`. Two skill calls
    /// with embedded commands therefore consume 3 process runs (1 snapshot + 2
    /// commands), not 4 (which is what re-creating the provider per call did,
    /// re-sourcing the user's rc file every invocation).
    #[tokio::test]
    async fn shell_snapshot_is_created_once_per_session_not_per_call() {
        let desc = SkillDescriptor {
            body: "before !`echo hi` after".into(),
            ..prompt_desc("sh")
        };
        let stub = std::sync::Arc::new(StubProcess::with(vec![
            dummy_out(),             // snapshot creation — expected exactly ONCE
            out_with_stdout("hi\n"), // call 1's embedded command
            out_with_stdout("hi\n"), // call 2's embedded command
            dummy_out(),             // slack: consumed only if the snapshot re-runs
        ]));
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = stub.clone();
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));

        for _ in 0..2 {
            let out = tool
                .call(json!({"skill": "sh"}), fresh_ctx(), fresh_tx())
                .await
                .expect("ok");
            match &out.new_messages[0] {
                protocol::ConversationMessage::User { content, .. } => match content.first() {
                    Some(protocol::ContentBlock::Text { text }) => {
                        assert_eq!(text, "before hi after");
                    }
                    other => panic!("expected leading Text block, got {other:?}"),
                },
                other => panic!("expected injected User message, got {other:?}"),
            }
        }

        // 4 queued − 3 consumed (1 snapshot + 2 commands) = 1 left. Re-creating
        // the provider per call would have consumed all 4.
        assert_eq!(
            stub.remaining(),
            1,
            "the snapshot shell must run once per session, not once per skill call"
        );
    }

    /// SAFETY: a skill body with NO `!command` is byte-identical to the
    /// argument-substituted text — the shell-expansion engine returns the input
    /// unchanged and the fake process is NEVER invoked (it would error if it were:
    /// the stub is exhausted after one call, but no call happens).
    #[tokio::test]
    async fn no_shell_command_is_byte_identical_to_arg_substituted() {
        let body = "Review PR $ARGUMENTS now (no shell here)";
        let desc = SkillDescriptor {
            body: body.into(),
            ..prompt_desc("nb")
        };
        // Independently compute the pure arg-substitution result.
        let expected =
            command_api::substitute_arguments_faithful(body, Some("123"), true, &[]).unwrap();
        // dummy_out() has empty stdout; if the runner were ever called and then
        // called AGAIN, the stub would error — proving the no-op path.
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "nb", "args": "123"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    assert_eq!(text, &expected);
                    assert_eq!(text, "Review PR 123 now (no shell here)");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// SAFETY: an MCP-sourced skill (`skip_shell_expansion = true`) is
    /// byte-identical to the arg-substituted text even when its body LOOKS like it
    /// has an embedded `!command` — the expansion call is skipped entirely (TS
    /// `loadedFrom !== 'mcp'` gate).
    #[tokio::test]
    async fn mcp_skip_shell_expansion_is_byte_identical() {
        let desc = SkillDescriptor {
            body: "before !`echo hi` after".into(),
            skip_shell_expansion: true,
            ..prompt_desc("mcpish")
        };
        // dummy_out(): the runner must NOT be called, so its (empty) stdout never
        // matters; a call would not change the body, but skip proves no execution.
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "mcpish"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => {
                    // Body is verbatim — the `!`echo hi`` block is NOT expanded.
                    assert_eq!(text, "before !`echo hi` after");
                }
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn descriptor_truncated_at_1024() {
        let desc = SkillDescriptor {
            description: "x".repeat(MAX_SKILL_DESCRIPTOR_LEN + 100),
            ..prompt_desc("huge")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "huge"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert!(out.data["body"].as_str().is_some(), "body present");
        assert_eq!(out.data["descriptor_truncated"], json!(true));
    }

    #[tokio::test]
    async fn blank_skill_rejected() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"skill": "   "}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("blank");
        assert!(format!("{err}").contains("Invalid skill format"));
    }

    #[tokio::test]
    async fn rejects_missing_skill() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string skill"));
    }

    #[tokio::test]
    async fn validate_input_applies_locked_rejections() {
        // Unknown via empty loader.
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(&json!({"skill": "/absent"}), &fresh_ctx())
            .await
            .expect_err("unknown");
        assert!(err.0.contains("Unknown skill: absent"));

        // disable-model-invocation.
        let desc = SkillDescriptor {
            disable_model_invocation: true,
            ..prompt_desc("x")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let err = tool
            .validate_input(&json!({"skill": "x"}), &fresh_ctx())
            .await
            .expect_err("disabled");
        assert!(err
            .0
            .contains("cannot be used with Skill tool due to disable-model-invocation"));
    }

    // ========================================================================
    // ${LINGXI_SKILL_DIR} / ${LINGXI_SESSION_ID} token substitution
    // (TS getPromptForCommand steps 2-3, loadSkillsDir.ts:359-369). The tokens
    // are substituted AFTER argument substitution and BEFORE shell expansion.
    // ========================================================================

    /// Extract the leading Text block of the single injected user message.
    fn injected_text(out: &ToolCallResult) -> String {
        match &out.new_messages[0] {
            protocol::ConversationMessage::User { content, .. } => match content.first() {
                Some(protocol::ContentBlock::Text { text }) => text.clone(),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// Step 2: `${LINGXI_SKILL_DIR}` is replaced with `skill_root` when present.
    #[tokio::test]
    async fn skill_dir_token_replaced_when_skill_root_present() {
        let desc = SkillDescriptor {
            body: "scripts live in ${LINGXI_SKILL_DIR}/bin".into(),
            skill_root: Some(std::path::PathBuf::from("/skills/foo")),
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "Base directory for this skill: /skills/foo\n\nscripts live in /skills/foo/bin"
        );
    }

    #[tokio::test]
    async fn file_backed_skill_prompt_includes_base_directory_prefix() {
        let desc = SkillDescriptor {
            body: "Use the local scripts".into(),
            skill_root: Some(std::path::PathBuf::from("/skills/foo")),
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "Base directory for this skill: /skills/foo\n\nUse the local scripts"
        );
    }

    /// Step 2 (gate): with NO `skill_root` (e.g. MCP / non-file skills), the
    /// `${LINGXI_SKILL_DIR}` token is left untouched (TS gates on `baseDir`).
    #[tokio::test]
    async fn skill_dir_token_left_as_is_when_skill_root_absent() {
        let desc = SkillDescriptor {
            body: "scripts live in ${LINGXI_SKILL_DIR}/bin".into(),
            skill_root: None,
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "scripts live in ${LINGXI_SKILL_DIR}/bin"
        );
    }

    /// Step 2: ALL occurrences of `${LINGXI_SKILL_DIR}` are replaced (global,
    /// matching the TS `/…/g` regex).
    #[tokio::test]
    async fn skill_dir_token_replaced_globally() {
        let desc = SkillDescriptor {
            body: "${LINGXI_SKILL_DIR}/a and ${LINGXI_SKILL_DIR}/b".into(),
            skill_root: Some(std::path::PathBuf::from("/r")),
            ..prompt_desc("dir")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "dir"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            "Base directory for this skill: /r\n\n/r/a and /r/b"
        );
    }

    /// Step 3: `${LINGXI_SESSION_ID}` is replaced with the session id (always,
    /// when one is wired) — including every occurrence.
    #[tokio::test]
    async fn session_id_token_replaced() {
        let desc = SkillDescriptor {
            body: "session ${LINGXI_SESSION_ID} = ${LINGXI_SESSION_ID}".into(),
            session_id: Some("sess:abc-123".into()),
            ..prompt_desc("sid")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "sid"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(injected_text(&out), "session sess:abc-123 = sess:abc-123");
    }

    /// Step 3 (gate): with NO `session_id` wired (hermetic loader), the token is
    /// left untouched rather than substituting an empty string.
    #[tokio::test]
    async fn session_id_token_left_as_is_when_unset() {
        let desc = SkillDescriptor {
            body: "session ${LINGXI_SESSION_ID}".into(),
            session_id: None,
            ..prompt_desc("sid")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(json!({"skill": "sid"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(injected_text(&out), "session ${LINGXI_SESSION_ID}");
    }

    /// A `ProcessRunner` that records the embedded shell command it was handed
    /// (the last `args` element, which `SkillShellRunner` fills with the expanded
    /// `-c` command string) so a test can assert WHAT the shell saw.
    struct CapturingProcess {
        seen: std::sync::Mutex<Vec<String>>,
        stdout: String,
    }
    #[async_trait]
    impl traits::process::ProcessRunner for CapturingProcess {
        async fn run(
            &self,
            cmd: &traits::sandbox::SandboxedCommand,
        ) -> Result<ProcessOutput, traits::process::ProcessError> {
            if let Some(last) = cmd.inner().args.last() {
                self.seen.lock().unwrap().push(last.clone());
            }
            Ok(ProcessOutput {
                stdout: self.stdout.clone(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
            })
        }
        async fn spawn_background(
            &self,
            _cmd: &traits::sandbox::SandboxedCommand,
        ) -> Result<traits::process::ProcessHandle, traits::process::ProcessError> {
            Err(traits::process::ProcessError::Unsupported)
        }
        async fn kill(
            &self,
            _handle: &traits::process::ProcessHandle,
        ) -> Result<(), traits::process::ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    /// ORDERING: a `!command` block that references `${LINGXI_SESSION_ID}` sees
    /// the SUBSTITUTED value — token replacement (step 3) runs BEFORE the embedded
    /// `!command` shell expansion (step 4). We capture the command string the
    /// shell runner is handed and assert the token is already substituted there.
    #[tokio::test]
    async fn token_substitution_precedes_shell_expansion() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        let desc = SkillDescriptor {
            body: "pre !`echo ${LINGXI_SESSION_ID}` post".into(),
            session_id: Some("sess:zzz".into()),
            ..prompt_desc("ord")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "ord"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // The expanded prompt splices the (trimmed) stdout.
        assert_eq!(injected_text(&out), "pre OUT post");
        // The command the shell actually ran already had the token substituted.
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "snapshot plus embedded command ran");
        let command = seen.last().expect("embedded command");
        assert!(
            command.contains("echo sess:zzz"),
            "shell saw substituted session id, got: {}",
            command
        );
        assert!(
            !command.contains("${LINGXI_SESSION_ID}"),
            "token must be substituted BEFORE shell expansion, got: {}",
            command
        );
    }

    /// Records `wrap`/`cleanup_after_command` calls so a test can prove the
    /// skill `!command` expansion routes through `ctx.sandbox_runner`.
    struct WrapCall {
        command: String,
        bin_shell: Option<String>,
        cwd: Option<std::path::PathBuf>,
    }

    #[derive(Default)]
    struct RecordingSandboxRunner {
        wrap_calls: std::sync::Mutex<Vec<WrapCall>>,
        cleanups: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl tool_api::SandboxRunner for RecordingSandboxRunner {
        async fn wrap(
            &self,
            command: &str,
            _cfg: &sandbox::runtime_config::SandboxRuntimeConfig,
            _platform: sandbox::runtime_config::Platform,
            bin_shell: Option<&str>,
            cwd: Option<&std::path::Path>,
        ) -> Result<String, sandbox::wrap::SandboxWrapError> {
            self.wrap_calls.lock().unwrap().push(WrapCall {
                command: command.to_string(),
                bin_shell: bin_shell.map(ToString::to_string),
                cwd: cwd.map(std::path::Path::to_path_buf),
            });
            Ok(format!("WRAPPED::{command}"))
        }

        async fn cleanup_after_command(&self) {
            self.cleanups
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The embedded `!command` shell expansion routes through the injected
    /// `ctx.sandbox_runner` (not the sync free fn): the runner's wrapped output
    /// is what gets spawned, it sees the resolved shell + workspace cwd, and
    /// `cleanup_after_command` runs after the command finishes.
    #[tokio::test]
    async fn shell_expansion_routes_through_injected_runner_and_cleans_up() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        let runner = Arc::new(RecordingSandboxRunner::default());
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        // Force the Sandbox branch: a non-empty, non-excluded command runs
        // through the wrap whenever the host has a working sandbox backend.
        ctx.sandbox_available = true;
        ctx.session_cwd
            .swap(std::path::PathBuf::from("/tmp"), ctx.trusted_dirs());
        ctx.sandbox_runner = runner.clone();

        let desc = SkillDescriptor {
            body: "pre !`echo hi` post".into(),
            ..prompt_desc("wrap")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        tool.call(json!({"skill": "wrap"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");

        let calls = runner.wrap_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "wrap should be called once");
        let call = &calls[0];
        assert!(
            call.command.contains("echo hi"),
            "runner received the embedded command, got: {}",
            call.command
        );
        assert_eq!(
            call.bin_shell.as_deref(),
            Some(crate::prompt_shell::resolve_shell_path())
        );
        assert_eq!(call.cwd.as_deref(), Some(std::path::Path::new("/tmp")));

        // The runner's wrapped output (sentinel) is what actually got spawned.
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "snapshot plus embedded command ran");
        let command = seen.last().expect("embedded command");
        assert!(
            command.contains("WRAPPED::"),
            "the runner's wrapped command must be spawned, got: {}",
            command
        );
        drop(seen);

        assert_eq!(
            runner.cleanups.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "cleanup_after_command must be invoked once"
        );
    }

    #[tokio::test]
    async fn file_backed_body_without_tokens_still_gets_base_directory_prefix() {
        let body = "Plain body $ARGUMENTS, no tokens here at all.";
        let desc = SkillDescriptor {
            body: body.into(),
            skill_root: Some(std::path::PathBuf::from("/r")),
            session_id: Some("sess:abc".into()),
            ..prompt_desc("plain")
        };
        let expected =
            command_api::substitute_arguments_faithful(body, Some("X"), true, &[]).unwrap();
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        let out = tool
            .call(
                json!({"skill": "plain", "args": "X"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(
            injected_text(&out),
            format!("Base directory for this skill: /r\n\n{expected}")
        );
    }

    // ========================================================================
    // SKILLEXEC.6 sandbox parity: the embedded `!command` runner mirrors
    // `BashTool::call`'s `should_use_sandbox` + `wrap_with_sandbox` decision.
    // ========================================================================

    /// The platform wrapper prefix `wrap_with_sandbox` emits, so the assertions
    /// below stay host-agnostic: `sandbox-exec -f` on macOS, `bwrap ` elsewhere.
    fn sandbox_wrap_prefix() -> &'static str {
        if cfg!(target_os = "macos") {
            "sandbox-exec -f"
        } else {
            "bwrap "
        }
    }

    /// With a config that WOULD sandbox (`sandbox_available = true`, default
    /// permission mode, classifier `None` so the trusted-safe shortcut never
    /// fires), the embedded command is run through `wrap_with_sandbox` — the
    /// spawned command string is the WRAPPED form, exactly as `BashTool::call`
    /// would produce. We capture the spawned `-c -l` payload and assert it is the
    /// platform sandbox wrapper, with the original command nested inside.
    #[tokio::test]
    async fn embedded_command_is_sandbox_wrapped_when_decision_says_sandbox() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        // Flip the one input that moves the decision from NoSandbox -> Sandbox:
        // a working sandbox backend. (Default mode + classifier `None` means the
        // trusted-safe shortcut is skipped, so the decision lands on Sandbox.)
        ctx.sandbox_available = true;
        let desc = SkillDescriptor {
            body: "pre !`echo hi` post".into(),
            ..prompt_desc("sbx")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "sbx"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        // The splice still works (the stub stdout is "OUT").
        assert_eq!(injected_text(&out), "pre OUT post");
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "snapshot plus embedded command ran");
        let command = seen.last().expect("embedded command");
        // The spawned payload is the platform sandbox wrapper — NOT the raw
        // command — proving the should_use_sandbox + wrap_with_sandbox path ran.
        assert!(
            command.starts_with(sandbox_wrap_prefix()),
            "embedded command must be sandbox-wrapped, got: {}",
            command
        );
        // The original command (plus the BASH.1 extglob guard) is nested inside
        // the wrapper's `/bin/sh -c '…'` payload.
        assert!(
            command.contains("echo hi"),
            "wrapped command should still carry the original command, got: {}",
            command
        );
    }

    /// With the DEFAULT config (`sandbox_available = false`), the decision is
    /// `NoSandbox`, so the embedded command is run UNWRAPPED — byte-identical to
    /// the pre-sandbox-parity behavior. The spawned payload is the bare
    /// extglob-guarded command, never the platform wrapper.
    #[tokio::test]
    async fn embedded_command_is_unwrapped_when_decision_says_no_sandbox() {
        let capture = Arc::new(CapturingProcess {
            seen: std::sync::Mutex::new(Vec::new()),
            stdout: "OUT\n".into(),
        });
        // shell_test_ctx defaults: sandbox_available = false -> NoSandbox.
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.process = capture.clone();
        let desc = SkillDescriptor {
            body: "pre !`echo hi` post".into(),
            ..prompt_desc("nosbx")
        };
        let tool = SkillTool::with_loader(ctx, Arc::new(FixedLoader(Some(desc))));
        let out = tool
            .call(json!({"skill": "nosbx"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(injected_text(&out), "pre OUT post");
        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "snapshot plus embedded command ran");
        let command = seen.last().expect("embedded command");
        // No sandbox wrapper — the raw command runs directly.
        assert!(
            !command.starts_with(sandbox_wrap_prefix()),
            "NoSandbox path must run the command unwrapped, got: {}",
            command
        );
        assert!(
            command.contains("echo hi"),
            "unwrapped command should be the raw command, got: {}",
            command
        );
    }

    // ========================================================================
    // Parity: prompt() byte-exact comparison (binary bytes 198002225+)
    // ========================================================================

    /// The `prompt()` string must match the binary's Skill tool description
    /// exactly (byte-for-byte). This test locks the full text so any future
    /// change requires an explicit binary re-audit.
    #[tokio::test]
    async fn prompt_is_binary_faithful() {
        let tool = SkillTool::new(shell_test_ctx(dummy_out()));
        let p = tool.prompt(&PromptOptions::default()).await;
        // Key phrases that must be present (guards against accidental truncation
        // or whitespace normalization).
        assert!(
            p.starts_with("Execute a skill within the main conversation"),
            "prompt must start with binary-faithful opening line"
        );
        assert!(
            p.contains("available skills"),
            "prompt must mention available skills list"
        );
        assert!(
            p.contains("BLOCKING REQUIREMENT"),
            "prompt must contain BLOCKING REQUIREMENT clause"
        );
        assert!(
            p.contains("plugin:skill"),
            "prompt must mention plugin:skill qualified form"
        );
        assert!(
            p.contains("<command-name>"),
            "prompt must contain <command-name> re-entry guard"
        );
        assert!(
            p.contains("apps/web:deploy"),
            "prompt must contain scoped-skill directory example"
        );
        assert!(
            p.contains("NEVER mention a skill without actually calling this tool"),
            "prompt must contain NEVER-mention clause"
        );
        assert!(
            p.contains("Do not invoke a skill that is already running"),
            "prompt must contain already-running guard"
        );
        assert!(
            p.contains("/help, /clear"),
            "prompt must contain built-in CLI commands example"
        );
    }

    // P2-12 / `zSr`: a successful skill invocation records the skill in the
    // process-global invoked-skill registry (main thread → key `":{name}"`), so
    // its content can be re-injected after a compaction (`rRg`).
    #[tokio::test]
    async fn invocation_registers_skill_in_invoked_registry() {
        let _g = compaction::invoked_skills::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        compaction::invoked_skills::reset_for_test();

        let desc = SkillDescriptor {
            skill_root: Some(std::path::PathBuf::from("/skills/regtest")),
            ..prompt_desc("regtest")
        };
        let tool = SkillTool::with_loader(
            shell_test_ctx(dummy_out()),
            Arc::new(FixedLoader(Some(desc))),
        );
        tool.call(json!({"skill": "regtest"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");

        // The registry now carries the main-thread row keyed `":regtest"` with
        // the expanded skill content the model received (`body here`).
        let content = compaction::invoked_skills::content_for_test(":regtest")
            .expect("skill registered under main-thread key");
        assert!(
            content.contains("body here"),
            "registered content must carry the expanded skill body; got: {content}"
        );
        // A skill NOT invoked leaves no row.
        assert!(compaction::invoked_skills::content_for_test(":other").is_none());

        compaction::invoked_skills::reset_for_test();
    }
}
