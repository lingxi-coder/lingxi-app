//! Mobile composition root (M8-P11).
//!
//! The mobile sibling of `engine-desktop`: the single declarative place that
//! decides the mobile capability set. It links a *different* subset of crates
//! via `Cargo.toml` — the cross-platform tools (file/task/web/plan/meta/cron/
//! ui/skill) plus the mobile-exclusive tools (camera/voice/share), the mobile
//! skill set, and the core + mobile command sets — while deliberately omitting
//! the desktop-only tools (shell/agent/mcp/lsp/team/worktree) and the
//! device-control tools. Same core agent logic, different assembly: no
//! `#[cfg(target_os)]` switching in any library crate.
//!
//! As with `engine-desktop`, the host binary owns the runtime wiring
//! (constructing the `BuiltinToolContext` from an `Arc<dyn Platform>` and
//! threading the orchestrator handle); this crate provides the pure
//! registry-assembly functions.

#![forbid(unsafe_code)]

use command_api::{
    BundledPromptFn, CommandFrontmatter, CommandRegistry, CommandSource, SlashCommand,
    SlashCommandKind,
};
use command_core::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2,
    register_core_batch_4, register_core_batch_5,
};
use skill_api::SkillRegistry;
use std::sync::Arc;
use tool_api::{BuiltinToolContext, ToolRegistry};
use tool_ui::ask_user_question::AskUserQuestionResolver;
use traits::{AuthHandle, OrchestratorHandle};

// F3-03: the shared mobile session-host module — `MobileConfig` +
// `build_mobile(MobileConfig, Platform, listener, sink) -> MobileRuntime`. It
// lives under the `uniffi` feature because it wires the `client-adapter` sinks
// (`AdapterOutputStream` / `AdapterPermissionGate`) + the `ClientEventListener`,
// which are pulled ONLY under that feature (the FFI surface). Both FFI packager
// crates (`ios-framework` / `android-aar`) re-export this shared host (F3-04) so
// iOS and Android cannot drift.
#[cfg(feature = "uniffi")]
mod host;

// Audit fix (#14): the disk-backed Skill loader the FFI host wires so the mobile
// Skill tool resolves on-disk `.lingxi/commands` / `.lingxi/skills` under the
// app-private root. uniffi-gated — its `SkillLoader` impl uses `async-trait`
// (an FFI-only optional dep) and only the FFI host constructs a real loader.
#[cfg(feature = "uniffi")]
mod skill_loader;
// v3 Phase 1: workflow-on-mobile composition pieces (launcher + deferred
// invoker), consumed by the `host` build path.
#[cfg(feature = "uniffi")]
mod workflow_support;

// LOCAL-APPS (phase 1): the domain ⇄ protocol bridge for the engine-owned
// `local_apps::AppService` — the observer that lowers domain events onto the
// client event sink plus the DTO lowering/raising helpers the `submit` command
// arms use. uniffi-gated like `host` (it names the client-protocol DTO surface,
// which is pulled only under that feature).
#[cfg(feature = "uniffi")]
mod local_apps_bridge;

#[cfg(feature = "uniffi")]
mod local_apps_build;
#[cfg(feature = "uniffi")]
mod local_apps_host;
// Live per-connection device handles (camera / voice / location /
// notifications) behind a SharedLlm-style swap cell — see the module doc for
// why a bare OnceLock would pin a torn-down engine's Swift objects.
#[cfg(feature = "uniffi")]
mod local_apps_device;
// LOCAL-APPS (v3): the app-facing LLM seam — `LocalAppsModel` + the
// `ApiService`-backed `chat` used by the `llm.chat` bridge operation. The
// designer/generation pipeline that used to live behind this seam is gone.
#[cfg(feature = "uniffi")]
mod local_apps_llm;
#[cfg(feature = "uniffi")]
mod local_apps_mcp;
#[cfg(feature = "uniffi")]
mod local_apps_profile;

#[cfg(feature = "uniffi")]
pub use host::{
    build_mobile, build_mobile_engine, build_mobile_engine_inner, build_mobile_inner,
    parse_mobile_provider_config_json, CronDueOccurrenceDto, CronFireStatusDto, CronTaskDto,
    FiredCronJobDto, MobileBuildError, MobileConfig, MobileCronStoreHandle, MobileEngineError,
    MobileEngineHandle, MobileRuntime, ProviderConnectionTestDto,
    MobileOAuthSessionDto, MobileOAuthStateDto,
};

// F3-06: the host-only walking-skeleton support — a portable fake `Platform`
// shim (fs/http/clock stubs over a temp root), a recording `ClientEventListener`,
// a collecting `PermissionRequestSink`, and a streaming-injecting engine
// constructor. Lives behind the `uniffi` feature (it names the FFI-surface
// types) and is exposed so both the in-crate F3-03/F3-05 unit tests AND the
// `tests/skeleton_test.rs` integration test build the SAME off-device host. The
// real device `Platform` is `cfg(target_os)`-gated, so this shim is what proves
// the skeleton on CI — exactly the spec §8 "prove from a Swift/Kotlin unit test"
// smoke path, runnable on the host.
#[cfg(feature = "uniffi")]
pub mod test_support;

// F3-04: re-export the FFI-visible adapter types both packager crates name when
// they call `build_mobile_engine` (the foreign `ClientEventListener` they
// register and the `PermissionRequestSink` the gate emits to). Re-exporting them
// from the shared host crate keeps the FFI crates free of a direct
// `client-adapter` import for these types — the shared host is the single seam.
#[cfg(feature = "uniffi")]
pub use client_adapter::{ClientEventListener, ListenerSink, PermissionRequestSink};

// F3-04: this crate now DEFINES UniFFI-exported types (`MobileEngineHandle` as a
// `uniffi::Object`, `MobileEngineError` as a `uniffi::Error` — see `host`), so it
// must register their FFI metadata via the scaffolding macro. The aggregating
// cdylib crates (`ios-framework` / `android-aar`) re-export this scaffolding so
// the symbols land in the final library (the same pattern `client-adapter` uses
// for the `ClientEventListener` callback interface). Compiles ONLY under the
// `uniffi` feature; the default host build never includes it.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

/// Mobile engine knobs.
#[derive(Clone, Debug)]
pub struct MobileEngineConfig {
    /// Model id the mobile build defaults to.
    pub default_model: String,
}

impl Default for MobileEngineConfig {
    fn default() -> Self {
        Self {
            // The host's boot default for the Anthropic route. Must stay a
            // CURATED id (`traits::is_curated_model`), otherwise a client with
            // no configured provider boots on a model the picker only shows
            // because `curated_model_refs` pins the active one — the dated
            // `claude-sonnet-4-20250514` rendered as a stray "Claude Sonnet 4
            // 20250514" row above the real Anthropic shortlist.
            default_model: traits::provider_default_model("anthropic")
                .unwrap_or("claude-sonnet-5")
                .to_string(),
        }
    }
}

/// Assemble the mobile builtin **tool** registry from a freshly-built
/// [`BuiltinToolContext`] (whose `camera`/`voice`/`share` handles come from the
/// mobile `Platform`).
#[must_use]
pub fn mobile_tool_registry(ctx: BuiltinToolContext) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_tools(&mut reg, ctx);
    reg
}

/// Register the mobile tool set into an existing registry, with the `Skill` tool
/// INERT (the hermetic `EmptySkillLoader`). Used by tests and the non-FFI host
/// build. The FFI host instead calls [`register_mobile_tools_with_skill_loader`]
/// to wire a disk-backed loader (audit fix #14).
pub fn register_mobile_tools(reg: &mut ToolRegistry, ctx: BuiltinToolContext) {
    register_mobile_non_skill_tools_with_ask_resolver(reg, ctx.clone(), None);
    // Skill tool with the hermetic `EmptySkillLoader` (no on-disk discovery).
    tool_skill::register_all(reg, ctx);
}

/// Audit fix (#14): register the mobile tool set with a FUNCTIONAL `Skill` tool
/// backed by `skill_loader` (the disk-backed `MobileDiskSkillLoader`) instead of
/// the inert `EmptySkillLoader`, so model-invoked skills resolve against the
/// device's on-disk `.lingxi/commands` / `.lingxi/skills`. uniffi-gated because
/// the loader impl needs `async-trait` (an FFI-only optional dep) and only the
/// FFI host wires a real loader.
#[cfg(feature = "uniffi")]
pub fn register_mobile_tools_with_skill_loader(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
) {
    register_mobile_non_skill_tools_with_ask_resolver(reg, ctx.clone(), None);
    reg.register_builtin(Arc::new(tool_skill::SkillTool::with_loader(
        ctx,
        skill_loader,
    )));
}

/// Every mobile tool EXCEPT `Skill` (whose loader differs by build). Builtin
/// wire order is locale-sorted at enumeration time, so registration order is
/// immaterial. Hosts that surface a live questionnaire bridge inject a custom
/// `AskUserQuestion` resolver here so the registry never contains duplicate
/// same-name builtins.
fn register_mobile_non_skill_tools_with_ask_resolver(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    ask_resolver: Option<Arc<dyn AskUserQuestionResolver>>,
) {
    // ----- cross-platform subset (also linked by engine-desktop) -----------
    tool_file::register_all(reg, ctx.clone());
    tool_task::register_all(reg, ctx.clone());
    tool_web::register_all(reg, ctx.clone(), None);
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    // Audit fix (#7): the cron tools (Create/List/Delete/RemoteTrigger) are
    // registered, but mobile starts NO `cron::CronScheduler` (the desktop root is
    // the only place one runs) and wires no `task_registry` for it to fire into —
    // a backgrounded app has no long-running daemon. So a created cron job is
    // saved/listed/deletable but does NOT auto-fire on this platform; CronCreate's
    // result text says so (see schedule_cron.rs `scheduler_active`). RemoteTrigger
    // is independent of the local scheduler (it triggers a cloud-side run).
    tool_cron::register_all(reg, ctx.clone());
    match ask_resolver {
        Some(resolver) => tool_ui::register_all_with_ask_resolver(reg, ctx.clone(), resolver),
        None => tool_ui::register_all(reg, ctx.clone()),
    }
    // ----- mobile-exclusive tools ------------------------------------------
    // camera / voice / speech / notification / clipboard / share, folded into
    // the single `tool-mobile` crate.
    tool_mobile::register_all(reg, ctx.clone());
    // P3/P4: mobile shell tool. The composition root pre-gates it so a selected
    // but unavailable mobile-linux runtime never silently falls back to legacy.
    tool_shell_mobile::register_all(reg, ctx.clone());
    // P4: mobile structured git tool. Same pre-gate rule as shell.
    tool_git_mobile::register_all(reg, ctx);
}

/// Audit fix (#14): the FFI sibling of [`mobile_tool_registry`] that wires a
/// disk-backed `Skill` loader (the mobile composition root passes the
/// `MobileDiskSkillLoader` it built from the device's app-private root).
#[cfg(feature = "uniffi")]
#[must_use]
pub fn mobile_tool_registry_with_skill_loader(
    ctx: BuiltinToolContext,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_tools_with_skill_loader(&mut reg, ctx, skill_loader);
    reg
}

#[cfg(feature = "uniffi")]
#[must_use]
/// FFI host variant of [`mobile_tool_registry_with_skill_loader`] that installs
/// a live `AskUserQuestion` resolver exactly once, preserving builtin lookup
/// order while keeping the disk-backed `Skill` loader.
pub fn mobile_tool_registry_with_skill_loader_and_ask_resolver(
    ctx: BuiltinToolContext,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
    ask_resolver: Arc<dyn AskUserQuestionResolver>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_non_skill_tools_with_ask_resolver(&mut reg, ctx.clone(), Some(ask_resolver));
    reg.register_builtin(Arc::new(tool_skill::SkillTool::with_loader(
        ctx,
        skill_loader,
    )));
    reg
}

/// Register Android Computer Use only when both the Direct-build Cargo feature
/// and a live native host are present. Play builds pass no host and compile
/// without the feature. Direct foreground and headless engines intentionally
/// share the host; its in-memory active-session grants remain the security gate.
pub fn register_android_ui_automation(
    reg: &mut ToolRegistry,
    automation: Option<Arc<dyn traits::AndroidUiAutomation>>,
) {
    #[cfg(feature = "android-computer-use")]
    if let Some(automation) = automation {
        tool_android_use::register_all(reg, automation);
    }
    #[cfg(not(feature = "android-computer-use"))]
    let _ = (reg, automation);
}

#[cfg(test)]
mod android_ui_registration_tests {
    use super::register_android_ui_automation;
    use async_trait::async_trait;
    use std::sync::Arc;
    use traits::{
        AndroidAccessRequest, AndroidAction, AndroidActionResult, AndroidAppInfo,
        AndroidAutomationError, AndroidAutomationStatus, AndroidNodeQuery, AndroidScreenshot,
        AndroidUiAutomation, AndroidUiNode, AndroidUiSnapshot, AndroidWaitCondition,
    };

    struct StubAutomation;

    fn unsupported<T>() -> Result<T, AndroidAutomationError> {
        Err(AndroidAutomationError::Unsupported("test stub".into()))
    }

    #[async_trait]
    impl AndroidUiAutomation for StubAutomation {
        async fn status(&self) -> Result<AndroidAutomationStatus, AndroidAutomationError> {
            unsupported()
        }

        async fn request_access(
            &self,
            _request: AndroidAccessRequest,
        ) -> Result<Vec<AndroidAppInfo>, AndroidAutomationError> {
            unsupported()
        }

        async fn list_granted_apps(&self) -> Result<Vec<AndroidAppInfo>, AndroidAutomationError> {
            unsupported()
        }

        async fn screenshot(&self) -> Result<AndroidScreenshot, AndroidAutomationError> {
            unsupported()
        }

        async fn ui_tree(&self) -> Result<AndroidUiSnapshot, AndroidAutomationError> {
            unsupported()
        }

        async fn find_nodes(
            &self,
            _query: AndroidNodeQuery,
        ) -> Result<Vec<AndroidUiNode>, AndroidAutomationError> {
            unsupported()
        }

        async fn inspect_node(
            &self,
            _node_id: String,
        ) -> Result<AndroidUiNode, AndroidAutomationError> {
            unsupported()
        }

        async fn perform(
            &self,
            _action: AndroidAction,
        ) -> Result<AndroidActionResult, AndroidAutomationError> {
            unsupported()
        }

        async fn wait_for(
            &self,
            _condition: AndroidWaitCondition,
            _timeout_ms: u64,
        ) -> Result<AndroidActionResult, AndroidAutomationError> {
            unsupported()
        }

        async fn stop(&self) -> Result<(), AndroidAutomationError> {
            Ok(())
        }
    }

    #[test]
    fn registration_follows_the_direct_feature_gate() {
        let mut registry = tool_api::ToolRegistry::new();
        register_android_ui_automation(&mut registry, Some(Arc::new(StubAutomation)));
        #[cfg(feature = "android-computer-use")]
        assert!(registry.find_by_name("android_use").is_some());
        #[cfg(not(feature = "android-computer-use"))]
        assert!(registry.find_by_name("android_use").is_none());
    }
}

/// Assemble the mobile builtin **skill** registry.
#[must_use]
pub fn mobile_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_api::register_mobile(&mut reg);
    reg
}

/// Assemble the mobile slash-command registry: the core handlers plus the
/// mobile-only handlers (`/mobile` `/voice` `/share` `/camera`).
#[must_use]
pub fn mobile_command_registry(
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    // Bundled programmatic skills (`/loop`), mirroring desktop. Gated on the cron
    // kill-switch (loop.ts:83); mobile starts no cron scheduler so a scheduled
    // job is inert, but the skill's listing/usage path is harmless and faithful.
    let cron_enabled =
        !traits::env::is_env_truthy(std::env::var("LINGXI_DISABLE_CRON").ok().as_deref());
    command_core::register_bundled_skills(&mut reg, cron_enabled);
    register_mobile_skill_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    // Batch 8 (`/fork`, `/goal`, `/recap`, `/reload-skills`, `/skill-doctor`,
    // `/stop`) is wired by the uniffi composition root (`host::build_mobile`)
    // right after this returns, because it needs the shared
    // `Arc<tokio::sync::RwLock<CommandRegistry>>` slot (tokio is a
    // `uniffi`-gated optional dependency here, unavailable in this default-lean
    // lib build).
    //
    // Mobile-only command handlers: currently none — the mobile command names
    // (/mobile, /voice, /share, /camera) are served as command-core
    // unimplemented stubs. Register real mobile handlers on `reg` directly here
    // when implemented.
    reg
}

/// Mirror the compiled-in mobile Skill registry into the slash-command
/// catalog. The Skill tool remains the canonical invocation path, but a
/// settings screen and `/` palette must see the same five shipped skills — a
/// second hard-coded client list would drift again. The prompt body is the
/// exact bundled markdown, so a slash invocation and a Skill-tool invocation
/// receive identical guidance.
fn register_mobile_skill_commands(reg: &mut CommandRegistry) {
    let skills = mobile_skill_registry();
    for name in [
        "create-local-app",
        "frontend-design",
        "frontend-qa",
        "accessibility",
        "react-best-practices",
    ] {
        let Some(skill) = skills.get(name) else {
            continue;
        };
        reg.register_command(SlashCommand {
            name: skill.name.clone(),
            description: skill.description.clone(),
            source: CommandSource::Bundled,
            kind: SlashCommandKind::Bundled {
                frontmatter: CommandFrontmatter::default(),
                prompt_fn: Some(Arc::new(MobileSkillPrompt {
                    body: skill.content.clone(),
                })),
            },
            loaded_from: Some("bundled".into()),
            user_invocable: Some(true),
            has_user_specified_description: true,
            ..SlashCommand::default()
        });
    }
}

struct MobileSkillPrompt {
    body: String,
}

impl BundledPromptFn for MobileSkillPrompt {
    fn build(&self, args: &str) -> String {
        if args.trim().is_empty() {
            self.body.clone()
        } else {
            format!("{}\n\nUser-supplied focus:\n{}", self.body, args.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use tool_api::tool_trait::{ToolError, ToolStaticContext};
    use traits::process::ProcessOutput;

    struct NoopSkillLoader;

    #[async_trait]
    impl tool_skill::skill::SkillLoader for NoopSkillLoader {
        async fn load(
            &self,
            _name: &str,
        ) -> Result<Option<tool_skill::skill::SkillDescriptor>, ToolError> {
            Ok(None)
        }
    }

    struct FirstAnswerResolver;

    #[async_trait]
    impl AskUserQuestionResolver for FirstAnswerResolver {
        async fn resolve(
            &self,
            questions: &[tool_ui::ask_user_question::Question],
            _non_interactive: bool,
        ) -> Result<HashMap<String, String>, ToolError> {
            Ok(questions
                .iter()
                .filter_map(|question| {
                    question
                        .options
                        .first()
                        .map(|option| (question.question.clone(), option.label.clone()))
                })
                .collect())
        }
    }

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[cfg(feature = "uniffi")]
    #[tokio::test]
    async fn custom_mobile_ask_resolver_does_not_duplicate_builtin() {
        let registry = mobile_tool_registry_with_skill_loader_and_ask_resolver(
            shell_test_ctx(dummy_out()),
            Arc::new(NoopSkillLoader),
            Arc::new(FirstAnswerResolver),
        );
        let tools = registry.available_tools(&ToolStaticContext::default());
        let ask_count = tools
            .iter()
            .filter(|tool| tool.name() == tool_ui::ask_user_question::ASK_USER_QUESTION_TOOL_NAME)
            .count();
        assert_eq!(ask_count, 1);
        let ask = registry
            .find_by_name(tool_ui::ask_user_question::ASK_USER_QUESTION_TOOL_NAME)
            .expect("AskUserQuestion registered");
        let result = ask
            .call(
                serde_json::json!({
                    "questions": [{
                        "question": "Pick one?",
                        "header": "Choice",
                        "options": [
                            { "label": "Alpha", "description": "first" },
                            { "label": "Beta", "description": "second" }
                        ]
                    }]
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("custom resolver should answer");
        assert_eq!(result.data["answers"]["Pick one?"].as_str(), Some("Alpha"));
    }
}
