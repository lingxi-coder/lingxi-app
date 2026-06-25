//! Execution routing: decide whether a turn runs single-agent or via the
//! dual-LLM competitive path (design doc §路由策略).
//!
//! Precedence (highest first):
//! 1. explicit `--no-multi-agent`            -> SingleAgent
//! 2. explicit `--multi-agent` / `--dual-llm` -> DualLlmCompetitive
//! 3. env / settings `enabled=false` or `mode=off` -> SingleAgent
//! 4. `mode=force`                            -> DualLlmCompetitive
//! 5. `mode=auto` + WRITE-INTENT gate + any escalation trigger -> DualLlmCompetitive
//! 6. otherwise                               -> SingleAgent

use crate::config::Complexity;
use crate::config::MultiAgentConfig;
use crate::config::MultiAgentMode;

/// The route chosen for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionRoute {
    /// Ordinary single-agent turn loop.
    SingleAgent,
    /// Dual-LLM competitive multi-agent path.
    DualLlmCompetitive,
}

/// Explicit user override (CLI flag / env), independent of settings `mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplicitMultiAgentFlag {
    /// No explicit flag; fall through to env/settings.
    #[default]
    Unset,
    /// `--multi-agent` / `--dual-llm`.
    On,
    /// `--no-multi-agent`.
    Off,
}

/// Coarse hint about what area the task touches, used by the auto-mode
/// escalation conditions (§路由策略 / `estimated_complexity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskAreaHint {
    /// Touches security / permission / auth / payment / data-deletion code.
    pub security_sensitive: bool,
    /// Touches architecture / shared abstractions.
    pub architecture: bool,
    /// Predicted to span multiple crates or produce a large diff.
    pub large_or_cross_crate: bool,
    /// The previous single-agent attempt failed verification.
    pub previous_single_agent_failed: bool,
}

/// Inputs to the routing decision.
#[derive(Debug, Clone, Copy)]
pub struct RouteInput<'a> {
    /// The raw user prompt for this turn.
    pub user_prompt: &'a str,
    /// Explicit CLI/env override.
    pub explicit_flag: ExplicitMultiAgentFlag,
    /// Parsed multi-agent config.
    pub config: &'a MultiAgentConfig,
    /// Heuristic complexity estimate for this turn.
    pub estimated_complexity: Complexity,
    /// Touched-area hints.
    pub touched_area_hint: TaskAreaHint,
    /// Whether this turn actually intends to write/edit files (the auto-mode
    /// pre-gate). A pure Q&A / explain / review turn sets this `false` so it
    /// never enters the expensive dual-LLM path.
    pub write_intent: bool,
}

/// Words/phrases that, when present in a prompt, indicate write intent. Kept
/// small and heuristic (Phase 1, no model judgement).
const WRITE_INTENT_HINTS: &[&str] = &[
    "implement", "fix", "add", "refactor", "rewrite", "create", "edit", "change",
    "update", "build", "write", "remove", "delete", "rename", "migrate",
    "实现", "修复", "重构", "新增", "修改", "删除", "添加", "编写",
];

/// Heuristic: does the prompt look like it intends to modify files?
///
/// This is a convenience for callers that do not compute write-intent
/// upstream; callers with better signal should set [`RouteInput::write_intent`]
/// directly.
pub fn heuristic_write_intent(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    WRITE_INTENT_HINTS.iter().any(|h| {
        // ASCII hints matched case-insensitively; CJK hints matched as-is.
        if h.is_ascii() {
            lower.contains(h)
        } else {
            prompt.contains(h)
        }
    })
}

/// Heuristic complexity estimate from the prompt text alone (§路由策略).
pub fn heuristic_complexity(prompt: &str) -> Complexity {
    let lower = prompt.to_lowercase();
    const HIGH_SIGNALS: &[&str] = &[
        "architecture", "refactor", "security", "auth", "permission",
        "架构", "重构", "安全", "最优", "准确率",
    ];
    if HIGH_SIGNALS.iter().any(|s| if s.is_ascii() { lower.contains(s) } else { prompt.contains(s) }) {
        Complexity::High
    } else {
        Complexity::Low
    }
}

fn prompt_hits_keyword(prompt: &str, keywords: &[String]) -> bool {
    let lower = prompt.to_lowercase();
    keywords.iter().any(|k| {
        if k.is_ascii() {
            lower.contains(&k.to_lowercase())
        } else {
            prompt.contains(k)
        }
    })
}

/// Decide the execution route for a turn per the documented precedence.
pub fn route(input: &RouteInput<'_>) -> ExecutionRoute {
    // 1. Explicit off always wins.
    match input.explicit_flag {
        ExplicitMultiAgentFlag::Off => return ExecutionRoute::SingleAgent,
        ExplicitMultiAgentFlag::On => return ExecutionRoute::DualLlmCompetitive,
        ExplicitMultiAgentFlag::Unset => {}
    }

    let cfg = input.config;

    // 3. Disabled or off.
    if !cfg.enabled || cfg.mode == MultiAgentMode::Off {
        return ExecutionRoute::SingleAgent;
    }

    // 4. Force.
    if cfg.mode == MultiAgentMode::Force {
        return ExecutionRoute::DualLlmCompetitive;
    }

    // 5. Auto: write-intent pre-gate, then any escalation condition.
    debug_assert_eq!(cfg.mode, MultiAgentMode::Auto);
    if !input.write_intent {
        return ExecutionRoute::SingleAgent;
    }

    let triggers = &cfg.triggers;
    let hint = &input.touched_area_hint;

    let keyword_hit = prompt_hits_keyword(input.user_prompt, &triggers.keywords);
    let complexity_hit = triggers
        .min_complexity
        .is_some_and(|min| input.estimated_complexity >= min);
    let security_hit = triggers.security && hint.security_sensitive;
    let architecture_hit = triggers.architecture && hint.architecture;
    let large_diff_hit = triggers.large_diff && hint.large_or_cross_crate;
    // Previous single-agent verification failure is always an escalation
    // signal once write-intent is established.
    let retry_hit = hint.previous_single_agent_failed;

    if keyword_hit
        || complexity_hit
        || security_hit
        || architecture_hit
        || large_diff_hit
        || retry_hit
    {
        ExecutionRoute::DualLlmCompetitive
    } else {
        ExecutionRoute::SingleAgent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(mode: &str, enabled: bool) -> MultiAgentConfig {
        let v = json!({
            "enabled": enabled,
            "mode": mode,
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" },
            "triggers": {
                "keywords": ["最优解"],
                "minComplexity": "high",
                "security": true,
                "architecture": true,
                "largeDiff": true
            }
        });
        MultiAgentConfig::from_value(&v).unwrap()
    }

    fn base_input<'a>(prompt: &'a str, cfg: &'a MultiAgentConfig) -> RouteInput<'a> {
        RouteInput {
            user_prompt: prompt,
            explicit_flag: ExplicitMultiAgentFlag::Unset,
            config: cfg,
            estimated_complexity: Complexity::Low,
            touched_area_hint: TaskAreaHint::default(),
            write_intent: true,
        }
    }

    #[test]
    fn explicit_off_beats_force() {
        let cfg = config("force", true);
        let mut input = base_input("anything", &cfg);
        input.explicit_flag = ExplicitMultiAgentFlag::Off;
        assert_eq!(route(&input), ExecutionRoute::SingleAgent);
    }

    #[test]
    fn explicit_on_beats_disabled() {
        let cfg = config("off", false);
        let mut input = base_input("anything", &cfg);
        input.explicit_flag = ExplicitMultiAgentFlag::On;
        assert_eq!(route(&input), ExecutionRoute::DualLlmCompetitive);
    }

    #[test]
    fn disabled_is_single_even_in_auto() {
        let cfg = config("auto", false);
        let input = base_input("最优解 architecture", &cfg);
        assert_eq!(route(&input), ExecutionRoute::SingleAgent);
    }

    #[test]
    fn mode_off_is_single() {
        let cfg = config("off", true);
        let input = base_input("最优解", &cfg);
        assert_eq!(route(&input), ExecutionRoute::SingleAgent);
    }

    #[test]
    fn force_is_dual() {
        let cfg = config("force", true);
        let input = base_input("hello", &cfg);
        assert_eq!(route(&input), ExecutionRoute::DualLlmCompetitive);
    }

    #[test]
    fn auto_without_write_intent_is_single() {
        let cfg = config("auto", true);
        let mut input = base_input("最优解 architecture security", &cfg);
        input.write_intent = false; // pure discussion
        input.estimated_complexity = Complexity::High;
        input.touched_area_hint.security_sensitive = true;
        assert_eq!(route(&input), ExecutionRoute::SingleAgent);
    }

    #[test]
    fn auto_write_intent_keyword_trigger() {
        let cfg = config("auto", true);
        let input = base_input("请给出最优解", &cfg);
        assert_eq!(route(&input), ExecutionRoute::DualLlmCompetitive);
    }

    #[test]
    fn auto_write_intent_no_trigger_is_single() {
        let cfg = config("auto", true);
        let input = base_input("tweak a comment", &cfg);
        assert_eq!(route(&input), ExecutionRoute::SingleAgent);
    }

    #[test]
    fn auto_complexity_trigger() {
        let cfg = config("auto", true);
        let mut input = base_input("do the thing", &cfg);
        input.estimated_complexity = Complexity::High;
        assert_eq!(route(&input), ExecutionRoute::DualLlmCompetitive);

        input.estimated_complexity = Complexity::Medium; // below min=high
        assert_eq!(route(&input), ExecutionRoute::SingleAgent);
    }

    #[test]
    fn auto_security_hint_trigger() {
        let cfg = config("auto", true);
        let mut input = base_input("do the thing", &cfg);
        input.touched_area_hint.security_sensitive = true;
        assert_eq!(route(&input), ExecutionRoute::DualLlmCompetitive);
    }

    #[test]
    fn auto_previous_failure_triggers() {
        let cfg = config("auto", true);
        let mut input = base_input("do the thing", &cfg);
        input.touched_area_hint.previous_single_agent_failed = true;
        assert_eq!(route(&input), ExecutionRoute::DualLlmCompetitive);
    }

    #[test]
    fn write_intent_heuristic() {
        assert!(heuristic_write_intent("Please implement the parser"));
        assert!(heuristic_write_intent("重构这个模块"));
        assert!(!heuristic_write_intent("What does this function do?"));
    }

    #[test]
    fn complexity_heuristic() {
        assert_eq!(heuristic_complexity("refactor the architecture"), Complexity::High);
        assert_eq!(heuristic_complexity("安全相关改动"), Complexity::High);
        assert_eq!(heuristic_complexity("print hello"), Complexity::Low);
    }
}
