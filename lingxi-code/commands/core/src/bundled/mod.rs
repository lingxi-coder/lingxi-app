//! Bundled (programmatically-registered) skills — port of the reference
//! `registerBundledSkill` / `registerBundledSkills` family
//! (`claude-code/src/skills/bundledSkills.ts`). Today the only bundled skill is
//! [`loop_skill`] (`/loop`).

use std::sync::Arc;

use command_api::{
    CommandFrontmatter, CommandRegistry, CommandSource, SlashCommand, SlashCommandKind,
};

pub mod batch_skill;
pub mod code_review_skill;
pub mod fewer_permission_prompts_skill;
pub mod loop_skill;
pub mod run_skill;
pub mod run_skill_generator_skill;
pub mod simplify_skill;
pub mod verify_skill;

/// Register all bundled skills onto `reg` (port of `registerBundledSkills`,
/// `bundledSkills.ts`).
///
/// `cron_enabled` is the host's `isKairosCronEnabled` equivalent
/// (`cron_scheduler_enabled(LINGXI_DISABLE_CRON)` on desktop). When `false`
/// the `/loop` skill is not registered — mirroring the reference
/// `isEnabled: isKairosCronEnabled` gate (loop.ts:83).
pub fn register_bundled_skills(reg: &mut CommandRegistry, cron_enabled: bool) {
    register_loop_skill(reg, cron_enabled);
    register_verify_skill(reg);
    register_run_skill(reg);
    register_simplify_skill(reg);
    register_run_skill_generator_skill(reg);
    register_fewer_permission_prompts_skill(reg);
    register_code_review_skill(reg);
    register_batch_skill(reg);
}

/// Register the `/batch` bundled skill — parallel-work orchestration
/// (`userInvocable:!0`, `disableModelInvocation:!0`, `argumentHint:"<instruction>"`).
/// The reference's `isEnabled` is an is-git-repo runtime check; we register
/// unconditionally (the prompt itself, and the `await BE()` guard it replaces,
/// have no static equivalent — see [`batch_skill`]).
fn register_batch_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "batch".into(),
        description: batch_skill::BATCH_DESCRIPTION.into(),
        menu_description: Some("Plan a large change; background agents each open a PR".into()),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(batch_skill::BatchPromptFn)),
        },
        loaded_from: Some("bundled".into()),
        user_invocable: Some(true),
        // Reference `disableModelInvocation:!0` — user-only.
        disable_model_invocation: true,
        has_user_specified_description: true,
        when_to_use: Some(batch_skill::BATCH_WHEN_TO_USE.into()),
        argument_hint: Some(batch_skill::BATCH_ARGUMENT_HINT.into()),
        ..SlashCommand::default()
    });
}

/// Register the `/code-review` bundled skill — the reference's assembler skill
/// (registrar over name-var `Lee`, `userInvocable:!0`, no `isEnabled` gate,
/// `argumentHint:"[low|medium|high|xhigh|max] [--fix] [--comment] [<target>]"`).
/// Distinct from the builtin `/review` (GitHub PRs); see [`code_review_skill`].
fn register_code_review_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "code-review".into(),
        description: code_review_skill::CODE_REVIEW_DESCRIPTION.into(),
        menu_description: Some("Review the current diff for bugs and cleanups".into()),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(code_review_skill::CodeReviewPromptFn)),
        },
        loaded_from: Some("bundled".into()),
        user_invocable: Some(true),
        has_user_specified_description: true,
        argument_hint: Some(code_review_skill::CODE_REVIEW_ARGUMENT_HINT.into()),
        ..SlashCommand::default()
    });
}

/// Register the `/fewer-permission-prompts` bundled skill — an inline-body skill
/// in the reference (`userInvocable:!0`, `requires:{workspace:!0}`, no
/// `isEnabled` gate). Its prompt appends `## Additional instructions from the
/// user` on an argument (see [`fewer_permission_prompts_skill`]).
fn register_fewer_permission_prompts_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "fewer-permission-prompts".into(),
        description: fewer_permission_prompts_skill::FEWER_PERMISSION_PROMPTS_DESCRIPTION.into(),
        menu_description: Some("Pre-approve safe read-only commands based on your usage".into()),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(
                fewer_permission_prompts_skill::FewerPermissionPromptsPromptFn,
            )),
        },
        loaded_from: Some("bundled".into()),
        user_invocable: Some(true),
        has_user_specified_description: true,
        ..SlashCommand::default()
    });
}

/// Register the `/run-skill-generator` bundled skill — a file-based skill in the
/// reference (registrar `hab`), USER-only (`disableModelInvocation:!0`, no
/// `isEnabled` gate). Prompt shape matches [`verify_skill`].
fn register_run_skill_generator_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "run-skill-generator".into(),
        description: run_skill_generator_skill::RUN_SKILL_GENERATOR_DESCRIPTION.into(),
        menu_description: Some("Create a skill that knows how to run this project’s app".into()),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(
                run_skill_generator_skill::RunSkillGeneratorPromptFn,
            )),
        },
        loaded_from: Some("bundled".into()),
        user_invocable: Some(true),
        // Reference `disableModelInvocation:!0` — a user-only command.
        disable_model_invocation: true,
        has_user_specified_description: true,
        ..SlashCommand::default()
    });
}

/// Register the `/simplify` bundled skill — an inline-body skill in the
/// reference (registrar `eVp`, `userInvocable:!0`, `argumentHint:"[<target>]"`,
/// no `isEnabled` gate). Its `getPromptForCommand` PREPENDS a `Review target:`
/// line (see [`simplify_skill`]).
fn register_simplify_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "simplify".into(),
        description: simplify_skill::SIMPLIFY_DESCRIPTION.into(),
        menu_description: Some("Clean up the changed code without changing behavior".into()),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(simplify_skill::SimplifyPromptFn)),
        },
        loaded_from: Some("bundled".into()),
        user_invocable: Some(true),
        has_user_specified_description: true,
        // Reference `argumentHint:"[<target>]"`.
        argument_hint: Some(simplify_skill::SIMPLIFY_ARGUMENT_HINT.into()),
        ..SlashCommand::default()
    });
}

/// Register the `/run` bundled skill — a file-based bundled skill in the
/// reference (static SKILL.md, `userInvocable:!0`, no `isEnabled` gate).
fn register_run_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "run".into(),
        description: run_skill::RUN_DESCRIPTION.into(),
        menu_description: Some("Launch this project’s app to see your change working".into()),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(run_skill::RunPromptFn)),
        },
        loaded_from: Some("bundled".into()),
        user_invocable: Some(true),
        has_user_specified_description: true,
        ..SlashCommand::default()
    });
}

/// Register the `/verify` bundled skill — a file-based bundled skill in the
/// reference (`Lu({name:Mee, description:Pob, userInvocable:!0, files:…,
/// getPromptForCommand})`, name-var `Mee="verify"`). Unconditionally registered
/// (no `isEnabled` gate, unlike `/loop`).
fn register_verify_skill(reg: &mut CommandRegistry) {
    reg.register_command(SlashCommand {
        name: "verify".into(),
        description: verify_skill::VERIFY_DESCRIPTION.into(),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(verify_skill::VerifyPromptFn)),
        },
        // Sets `is_bundled` in the skill listing + satisfies the listing's
        // loadedFrom ∈ {bundled,…} filter.
        loaded_from: Some("bundled".into()),
        // Reference `userInvocable:!0`.
        user_invocable: Some(true),
        // The frontmatter carries an explicit `description` ⇒ listing-eligible.
        has_user_specified_description: true,
        ..SlashCommand::default()
    });
}

/// Register the `/loop` bundled skill (loop.ts:74-92). Metadata is byte-faithful
/// to `registerBundledSkill({ … })` (loop.ts:75-82).
fn register_loop_skill(reg: &mut CommandRegistry, cron_enabled: bool) {
    // loop.ts:83 — `isEnabled: isKairosCronEnabled`.
    if !cron_enabled {
        return;
    }
    reg.register_command(SlashCommand {
        // PARITY: binary `_Zm` (cc_all.txt:521920) `name:"loop"`.
        name: "loop".into(),
        // PARITY: binary `_Zm` `get description(){if(q_e())return"…self-pace.";
        // return"…defaults to 10m)"}` (cc_all.txt:521920). `q_e()` =
        // `tengu_kairos_loop_dynamic` defaults FALSE and is ABSENT from the
        // port's `features` crate (no flag backend) → the cron variant, which is
        // the shipped binary default.
        description:
            "Run a prompt or slash command on a recurring interval (e.g. /loop 5m /foo, defaults to 10m)"
                .into(),
        source: CommandSource::Bundled,
        kind: SlashCommandKind::Bundled {
            frontmatter: CommandFrontmatter::default(),
            prompt_fn: Some(Arc::new(loop_skill::LoopPromptFn)),
        },
        // Sets `is_bundled` in the skill listing + satisfies the listing's
        // loadedFrom ∈ {bundled,…} filter.
        loaded_from: Some("bundled".into()),
        // PARITY: binary `_Zm` `aliases:["proactive"]` (cc_all.txt:521920).
        aliases: vec!["proactive".into()],
        // PARITY: binary `_Zm` `whenToUse` (cc_all.txt:521920).
        when_to_use: Some(
            "When the user wants to set up a recurring task, poll for status, or run something repeatedly on an interval (e.g. \"check the deploy every 5 minutes\", \"keep running /babysit-prs\"). Do NOT invoke for one-off tasks."
                .into(),
        ),
        // PARITY: binary `_Zm` `get argumentHint(){if(isLoopDefaultPromptEnabled())
        // return"[interval] [prompt]";return"[interval] <prompt>"}`
        // (cc_all.txt:521920). `isLoopDefaultPromptEnabled()` =
        // `tengu_kairos_loop_prompt` defaults FALSE / absent → the cron variant.
        argument_hint: Some("[interval] <prompt>".into()),
        // PARITY: binary `_Zm` `userInvocable:!0` (cc_all.txt:521920).
        user_invocable: Some(true),
        // Explicit description ⇒ listing-eligible.
        has_user_specified_description: true,
        // PARITY: binary `_Zm` `menuDescription:"Repeat a prompt or command on an
        // interval (e.g. /loop 5m /foo)"` (cc_all.txt:521920) — the compact
        // `/`-menu label, now wired via `menu_description` + the completion popup
        // (`menuDescription ?? description`).
        menu_description: Some(
            "Repeat a prompt or command on an interval (e.g. /loop 5m /foo)".into(),
        ),
        ..SlashCommand::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_loop_when_cron_enabled() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, true);
        let cmd = reg.resolve("loop").expect("loop registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.loaded_from.as_deref(), Some("bundled"));
        assert_eq!(cmd.user_invocable, Some(true));
        assert_eq!(cmd.argument_hint.as_deref(), Some("[interval] <prompt>"));
        assert!(cmd.has_user_specified_description);
        assert!(matches!(cmd.kind, SlashCommandKind::Bundled { .. }));
        assert_eq!(
            cmd.description,
            "Run a prompt or slash command on a recurring interval (e.g. /loop 5m /foo, defaults to 10m)"
        );
    }

    #[test]
    fn skips_loop_when_cron_disabled() {
        // loop.ts:83 — `isEnabled: isKairosCronEnabled`. Disabled ⇒ not registered.
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        assert!(reg.resolve("loop").is_none());
    }

    #[test]
    fn verify_is_registered_unconditionally() {
        // `/verify` has no `isEnabled` gate — present even when cron is off.
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("verify").expect("verify registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.loaded_from.as_deref(), Some("bundled"));
        assert_eq!(cmd.user_invocable, Some(true));
        assert!(cmd.has_user_specified_description);
        assert!(cmd
            .description
            .starts_with("Verify that a code change actually does"));
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                assert!(f
                    .build("")
                    .starts_with("**Verification is runtime observation.**"));
                assert!(f
                    .build("check X")
                    .contains("\n\n## User Request\n\ncheck X"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }

    #[test]
    fn run_is_registered_unconditionally() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("run").expect("run registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.loaded_from.as_deref(), Some("bundled"));
        assert_eq!(cmd.user_invocable, Some(true));
        assert!(cmd
            .description
            .starts_with("Launch and drive this project's app"));
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                assert!(f
                    .build("")
                    .starts_with("**Running means launching the actual app"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }

    #[test]
    fn simplify_is_registered_with_argument_hint() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("simplify").expect("simplify registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.loaded_from.as_deref(), Some("bundled"));
        assert_eq!(cmd.user_invocable, Some(true));
        assert_eq!(cmd.argument_hint.as_deref(), Some("[<target>]"));
        assert!(cmd
            .description
            .contains("reuse, simplification, efficiency, and altitude"));
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                // Inline skill PREPENDS a target line (contrast verify/run).
                assert!(f.build("").starts_with("`/simplify → 4 cleanup agents"));
                assert!(f.build("pull/9").starts_with("Review target: `pull/9`\n\n"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }

    #[test]
    fn run_skill_generator_is_user_only() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("run-skill-generator").expect("registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.user_invocable, Some(true));
        // Reference `disableModelInvocation:!0` — the model may not invoke it.
        assert!(cmd.disable_model_invocation);
        assert!(cmd
            .description
            .starts_with("Author or improve the run-<unit> skill"));
        // Excluded from the model-invocable listing.
        assert!(!reg
            .model_invocable_commands()
            .iter()
            .any(|c| c.name == "run-skill-generator"));
    }

    #[test]
    fn fewer_permission_prompts_is_registered() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("fewer-permission-prompts").expect("registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.user_invocable, Some(true));
        assert!(cmd.description.contains(".lingxi/settings.json"));
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                assert!(f.build("").starts_with("# Fewer Permission Prompts"));
                assert!(f
                    .build("x")
                    .contains("\n\n## Additional instructions from the user\n\nx"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }

    #[test]
    fn code_review_is_registered_with_argument_hint() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("code-review").expect("registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.user_invocable, Some(true));
        assert_eq!(
            cmd.argument_hint.as_deref(),
            Some("[low|medium|high|xhigh|max] [--fix] [--comment] [<target>]")
        );
        assert!(cmd
            .description
            .starts_with("Review the current diff for correctness bugs"));
        // Distinct command from the builtin `/review`.
        assert!(reg.resolve("review").is_none());
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                // Default = the medium effort body; explicit level selects another.
                assert!(f.build("").starts_with("`medium effort"));
                assert!(f.build("high").starts_with("`high effort"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }

    #[test]
    fn batch_is_user_only_with_when_to_use() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, false);
        let cmd = reg.resolve("batch").expect("registered");
        assert_eq!(cmd.source, CommandSource::Bundled);
        assert_eq!(cmd.user_invocable, Some(true));
        assert!(cmd.disable_model_invocation);
        assert_eq!(cmd.argument_hint.as_deref(), Some("<instruction>"));
        assert!(cmd
            .when_to_use
            .as_deref()
            .unwrap()
            .starts_with("Use when the user wants"));
        assert!(cmd.description.contains("5–30 isolated worktree agents"));
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                assert!(f.build("").starts_with("Provide an instruction"));
                assert!(f.build("do X").contains("## User Instruction\n\ndo X"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }

    #[test]
    fn registered_loop_carries_dynamic_prompt_fn() {
        let mut reg = CommandRegistry::new();
        register_bundled_skills(&mut reg, true);
        let cmd = reg.resolve("loop").expect("loop registered");
        match &cmd.kind {
            SlashCommandKind::Bundled { prompt_fn, .. } => {
                let f = prompt_fn.as_ref().expect("prompt_fn set");
                // Empty args → usage; non-empty → buildPrompt.
                assert!(f.build("").starts_with("Usage: /loop"));
                assert!(f.build("5m /x").starts_with("# /loop"));
            }
            other => panic!("expected Bundled kind, got {other:?}"),
        }
    }
}
