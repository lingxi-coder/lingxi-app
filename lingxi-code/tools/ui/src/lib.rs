//! UI / interaction tools: AskUserQuestion, Brief, SendMessage, Sleep,
//! StructuredOutput (the `SyntheticOutputTool` struct, wire name
//! `StructuredOutput`). Extracted in M8-P7. Cross-platform.
#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::match_wildcard_for_single_variants,
    clippy::single_match_else,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::format_collect,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::manual_let_else
)]
pub mod artifact;
pub mod ask_user_question;
pub mod brief;
pub mod push_notification;
pub mod send_message;
pub mod sleep;
pub mod synthetic_output;
pub use artifact::ArtifactTool;
pub use ask_user_question::AskUserQuestionTool;
pub use brief::BriefTool;
pub use push_notification::PushNotificationTool;
pub use send_message::SendMessageTool;
pub use sleep::SleepTool;
pub use synthetic_output::SyntheticOutputTool;
/// Register the UI tools against `reg` (the full set, including the builtin
/// `SendMessage`). This is the default-session path.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    register_with_options(reg, ctx, true);
}

/// Register the UI tools EXCEPT the builtin `SendMessage`.
///
/// A coordinator-capable session registers the richer
/// `coordinator::tool_send_message::SendMessageTool` IN PLACE OF this builtin
/// (it carries the swarm routing surface — broadcast / name-resolution /
/// shutdown + plan-approval handshake). Because the registry's `find_by_name`
/// is builtin-first, both must not be present or the earlier one would
/// silently shadow the later — so the engine skips this builtin in coordinator
/// mode, mirroring how `tool_team::register_all` is skipped for `TeamCreate` /
/// `TeamDelete` (see `engine-desktop::register_desktop_tools`).
pub fn register_all_except_send_message(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
) {
    register_with_options(reg, ctx, false);
}

fn register_with_options(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    include_send_message: bool,
) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(SleepTool::new(ctx.clone())));
    if include_send_message {
        reg.register_builtin(Arc::new(SendMessageTool::new(ctx.clone())));
    }
    // AskUserQuestion must NOT auto-continue by default (oracle 2.1.201): the
    // production resolver honors `askUserQuestionTimeout` (default `never` ⇒
    // block on the user), while a non-interactive (`--print`) session still
    // auto-picks the first option so batch runs never hang. This replaces the
    // old `FirstOptionResolver` default, which silently auto-selected option #1
    // in every session. See `ask_user_question::DefaultTimeoutResolver`.
    //
    // (M-15) The live `askUserQuestionTimeout` settings value rides on
    // `ctx.ask_user_question_timeout` (populated at the composition roots from
    // the merged settings.json). Parse it via `parse_or_default` — mirroring the
    // zod `.catch(void 0)` + `?? "never"` chain, so an absent / unparsable value
    // falls back to `Never` (block). The non-interactive `--print` auto-first
    // behavior is unchanged: `DefaultTimeoutResolver::resolve` short-circuits to
    // first-option whenever `non_interactive` is set, regardless of the window.
    let ask_timeout = ask_user_question::AskUserQuestionTimeout::parse_or_default(
        ctx.ask_user_question_timeout.as_deref(),
    );
    reg.register_builtin(Arc::new(AskUserQuestionTool::with_resolver(
        ctx.clone(),
        Arc::new(ask_user_question::DefaultTimeoutResolver::new(ask_timeout)),
    )));
    reg.register_builtin(Arc::new(BriefTool::new(ctx.clone())));
    // PARITY: the `PushNotification` tool (binary `Wzp`). Registered always; its
    // `is_enabled` (flag `tengu_kairos_push_notifications`, default off) gates
    // exposure — so by default it is invisible to the model, exactly like the
    // shipped binary.
    reg.register_builtin(Arc::new(PushNotificationTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(SyntheticOutputTool::new(ctx)));
    // NOTE: `ArtifactTool` lives in this crate but is registered by the DESKTOP
    // composition root (`engine_desktop::register_desktop_tools`), not here — it
    // is a first-party/claude.ai feature gated on `tengu_cobalt_plinth` +
    // first-party auth, so it is desktop-only (and stays out of the mobile tool
    // set). Its `is_enabled` (CC `dY()`) keeps it invisible to the model until
    // the Statsig gate is available.
}

// =============================================================================
// Tests — M-15: `askUserQuestionTimeout` threaded from `BuiltinToolContext`
// into the registered `AskUserQuestion` tool's resolver.
// =============================================================================
#[cfg(test)]
mod ask_timeout_wiring_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    fn one_question() -> serde_json::Value {
        json!({
            "questions": [{
                "question": "Pick one?",
                "header": "Choice",
                "options": [
                    { "label": "Alpha", "description": "the a" },
                    { "label": "Beta", "description": "the b" }
                ]
            }]
        })
    }

    /// Register the UI tools with the given `askUserQuestionTimeout` carrier and
    /// return the registered `AskUserQuestion` tool.
    fn register_and_get_ask(timeout: Option<&str>) -> Arc<dyn tool_api::tool_trait::Tool> {
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.ask_user_question_timeout = timeout.map(str::to_string);
        let mut reg = tool_api::ToolRegistry::new();
        register_all(&mut reg, ctx);
        reg.find_by_name("AskUserQuestion")
            .expect("AskUserQuestion must be registered")
    }

    #[tokio::test]
    async fn none_carrier_blocks_interactive_as_never() {
        // No setting ⇒ `never` ⇒ an interactive call blocks (does NOT auto-pick).
        let tool = register_and_get_ask(None);
        let err = tool
            .call(one_question(), fresh_ctx(), fresh_tx())
            .await
            .expect_err("never must block an interactive call");
        assert!(format!("{err}").contains("askUserQuestionTimeout=never"));
    }

    #[tokio::test]
    async fn bogus_carrier_falls_back_to_never() {
        // Unparsable value ⇒ zod `.catch` fallback to `never` (blocks).
        let tool = register_and_get_ask(Some("bogus"));
        let err = tool
            .call(one_question(), fresh_ctx(), fresh_tx())
            .await
            .expect_err("bogus must fall back to never and block");
        assert!(format!("{err}").contains("askUserQuestionTimeout=never"));
    }

    #[tokio::test(start_paused = true)]
    async fn duration_carrier_auto_advances_interactive() {
        // A valid duration ⇒ the resolver waits the idle window then auto-continues
        // with the first option, even in an interactive session. Paused time makes
        // the `tokio::time::sleep(60s)` complete instantly.
        let tool = register_and_get_ask(Some("60s"));
        let res = tool
            .call(one_question(), fresh_ctx(), fresh_tx())
            .await
            .expect("a duration must auto-advance, not block");
        // The auto-continued answer echoes the first option's label.
        assert_eq!(
            res.data["answers"]["Pick one?"].as_str(),
            Some("Alpha"),
            "auto-advance must select the first option: {:?}",
            res.data
        );
    }

    #[tokio::test]
    async fn duration_carrier_headless_auto_picks_first_option() {
        // Non-interactive (`--print`) is unchanged: first-option regardless of the
        // window (never blocks a batch run).
        let tool = register_and_get_ask(Some("5m"));
        let mut ctx = fresh_ctx();
        ctx.options.is_non_interactive_session = true;
        let res = tool
            .call(one_question(), ctx, fresh_tx())
            .await
            .expect("headless must not block");
        assert_eq!(
            res.data["answers"]["Pick one?"].as_str(),
            Some("Alpha"),
            "data: {:?}",
            res.data
        );
    }
}
