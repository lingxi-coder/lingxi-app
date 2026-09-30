//! Pure workflow-size warning helper for the `/workflows` TUI surfaces.

/// Claude Code's default workflow-size warning agent cap.
pub const DEFAULT_WORKFLOW_WARNING_AGENT_CAP: f64 = 25.0;
/// Claude Code's default workflow-size warning token cap.
pub const DEFAULT_WORKFLOW_WARNING_TOKEN_CAP: f64 = 1_500_000.0;
/// Claude Code's fallback average tokens per started agent when none have
/// started yet.
pub const DEFAULT_WORKFLOW_WARNING_FALLBACK_AVG_TOKENS: f64 = 70_000.0;

const WORKFLOW_WARNING_AGENT_CAP_ENV: &str = "CLAUDE_CODE_WORKFLOW_SIZE_WARNING_AGENTS";
const WORKFLOW_WARNING_TOKEN_CAP_ENV: &str = "CLAUDE_CODE_WORKFLOW_SIZE_WARNING_TOKENS";
const FOOTER_COPY: &str = "Large workflow \u{00b7} /workflows to stop";
const COMPACT_COPY: &str = "Large workflow";

/// Inputs required to reproduce Claude Code's workflow-size warning decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkflowSizeWarningInput {
    /// Total agents currently scheduled by the workflow.
    pub scheduled_agents: u64,
    /// Scheduled agents that have actually acquired a runtime slot.
    pub started_agents: u64,
    /// Tokens reported by the workflow so far.
    pub total_tokens: u64,
    /// Whether Ultracode is active, which suppresses this warning.
    pub ultracode_active: bool,
    /// Explicit 5/15/50 `/config` cap; `None` for the built-in default.
    pub guideline_agent_cap: Option<u32>,
    /// Remote `tengu_ochre_gantry.enabled`; explicit `false` suppresses.
    pub remote_enabled: Option<bool>,
    /// Positive finite remote `agents` threshold override.
    pub remote_agent_cap: Option<f64>,
    /// Positive finite remote `tokens` threshold override.
    pub remote_token_cap: Option<f64>,
}

/// Which threshold(s) triggered the warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSizeWarningAxis {
    /// Only the agent-count threshold was exceeded.
    Agents,
    /// Only the current/projected-token threshold was exceeded.
    Tokens,
    /// Both thresholds were exceeded.
    Both,
}

/// Fully resolved warning payload for presentation + telemetry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkflowSizeWarning {
    /// Threshold dimension(s) that triggered the warning.
    pub axis: WorkflowSizeWarningAxis,
    /// Total scheduled agents used in the decision.
    pub scheduled_agents: u64,
    /// Started agents used to compute the observed average.
    pub started_agents: u64,
    /// Current total tokens.
    pub total_tokens: u64,
    /// Projected total tokens after all scheduled agents run.
    pub projected_tokens: u64,
    /// Effective agent warning cap.
    pub agent_cap: f64,
    /// Effective token warning cap.
    pub token_cap: f64,
    /// Whether an explicit workflow-size guideline caused the agent warning.
    pub cap_from_guideline: bool,
}

impl WorkflowSizeWarning {
    /// Full footer copy used while the workflow is selected.
    #[must_use]
    pub const fn footer_copy(self) -> &'static str {
        FOOTER_COPY
    }

    /// Compact label used alongside the workflow row/detail metadata.
    #[must_use]
    pub const fn compact_copy(self) -> &'static str {
        COMPACT_COPY
    }
}

/// Compute the workflow-size warning, matching Claude Code 2.1.245's
/// scheduling/token projection rules.
#[must_use]
pub fn workflow_size_warning(input: WorkflowSizeWarningInput) -> Option<WorkflowSizeWarning> {
    if input.ultracode_active || input.remote_enabled == Some(false) {
        return None;
    }

    let env_agent_cap = positive_finite_env(WORKFLOW_WARNING_AGENT_CAP_ENV);
    let agent_cap = env_agent_cap
        .or_else(|| input.guideline_agent_cap.map(f64::from))
        .or_else(|| positive_finite(input.remote_agent_cap))
        .unwrap_or(DEFAULT_WORKFLOW_WARNING_AGENT_CAP);
    let token_cap = positive_finite_env(WORKFLOW_WARNING_TOKEN_CAP_ENV)
        .or_else(|| positive_finite(input.remote_token_cap))
        .unwrap_or(DEFAULT_WORKFLOW_WARNING_TOKEN_CAP);
    let scheduled_agents = input.scheduled_agents as f64;
    let started_agents = input.started_agents as f64;
    let total_tokens = input.total_tokens as f64;
    let avg_tokens = if input.started_agents > 0 {
        total_tokens / started_agents
    } else {
        DEFAULT_WORKFLOW_WARNING_FALLBACK_AVG_TOKENS
    };
    let projected_tokens = input
        .total_tokens
        .max((avg_tokens * scheduled_agents).round().max(0.0) as u64);
    let agents_warning = scheduled_agents > agent_cap;
    let tokens_warning = total_tokens > token_cap || (projected_tokens as f64) > token_cap;

    let axis = match (agents_warning, tokens_warning) {
        (true, true) => WorkflowSizeWarningAxis::Both,
        (true, false) => WorkflowSizeWarningAxis::Agents,
        (false, true) => WorkflowSizeWarningAxis::Tokens,
        (false, false) => return None,
    };

    Some(WorkflowSizeWarning {
        axis,
        scheduled_agents: input.scheduled_agents,
        started_agents: input.started_agents,
        total_tokens: input.total_tokens,
        projected_tokens,
        agent_cap,
        token_cap,
        cap_from_guideline: agents_warning
            && env_agent_cap.is_none()
            && input.guideline_agent_cap.is_some(),
    })
}

/// Resolve the agent cap from env, then an explicit guideline, then 25.
#[must_use]
pub fn workflow_warning_agent_cap(guideline_agent_cap: Option<u32>) -> f64 {
    positive_finite_env(WORKFLOW_WARNING_AGENT_CAP_ENV)
        .or_else(|| guideline_agent_cap.map(f64::from))
        .unwrap_or(DEFAULT_WORKFLOW_WARNING_AGENT_CAP)
}

/// Resolve the token cap from env, falling back to 1,500,000.
#[must_use]
pub fn workflow_warning_token_cap() -> f64 {
    positive_finite_env(WORKFLOW_WARNING_TOKEN_CAP_ENV)
        .unwrap_or(DEFAULT_WORKFLOW_WARNING_TOKEN_CAP)
}

fn positive_finite_env(name: &str) -> Option<f64> {
    std::env::var(name)
        .ok()
        // Both 2.1.245 env entries use `t.int({min:1})`; decimal overrides are
        // invalid even though the downstream `gN` helper accepts any number.
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|value| value as f64)
}

fn positive_finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_env() {
        std::env::remove_var(WORKFLOW_WARNING_AGENT_CAP_ENV);
        std::env::remove_var(WORKFLOW_WARNING_TOKEN_CAP_ENV);
    }

    fn base() -> WorkflowSizeWarningInput {
        WorkflowSizeWarningInput {
            scheduled_agents: 0,
            started_agents: 0,
            total_tokens: 0,
            ultracode_active: false,
            guideline_agent_cap: None,
            remote_enabled: None,
            remote_agent_cap: None,
            remote_token_cap: None,
        }
    }

    #[test]
    fn ultracode_suppresses_warning() {
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            ultracode_active: true,
            scheduled_agents: 40,
            ..base()
        });
        assert!(warning.is_none());
    }

    #[test]
    fn remote_disabled_suppresses_warning() {
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            remote_enabled: Some(false),
            scheduled_agents: 40,
            ..base()
        });
        assert!(warning.is_none());
    }

    #[test]
    fn remote_thresholds_override_builtins_but_not_env_or_guideline() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 11,
            remote_agent_cap: Some(10.5),
            remote_token_cap: Some(9_000_000.0),
            ..base()
        })
        .expect("remote agent threshold warning");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Agents);
        assert_eq!(warning.agent_cap, 10.5);
        assert_eq!(warning.token_cap, 9_000_000.0);

        let guideline = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 6,
            guideline_agent_cap: Some(5),
            remote_agent_cap: Some(3.0),
            remote_token_cap: Some(9_000_000.0),
            ..base()
        })
        .expect("guideline has precedence over remote agent cap");
        assert_eq!(guideline.agent_cap, 5.0);
        assert!(guideline.cap_from_guideline);
        clear_env();
    }

    #[test]
    fn guideline_cap_applies_when_env_override_is_absent() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 6,
            guideline_agent_cap: Some(5),
            ..base()
        })
        .expect("guideline cap warning");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Agents);
        assert_eq!(warning.agent_cap, 5.0);
        assert!(warning.cap_from_guideline);
    }

    #[test]
    fn builtin_medium_default_uses_25_but_explicit_medium_uses_15() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        assert!(workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 20,
            guideline_agent_cap: None,
            ..base()
        })
        .is_none());
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 20,
            guideline_agent_cap: Some(15),
            ..base()
        })
        .expect("explicit medium guideline warning");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Agents);
        assert!(warning.cap_from_guideline);
        clear_env();
    }

    #[test]
    fn env_agent_cap_override_beats_guideline() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        std::env::set_var(WORKFLOW_WARNING_AGENT_CAP_ENV, "8");

        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 9,
            guideline_agent_cap: Some(5),
            ..base()
        })
        .expect("env cap warning");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Agents);
        assert_eq!(warning.agent_cap, 8.0);
        assert!(!warning.cap_from_guideline);

        clear_env();
    }

    #[test]
    fn only_positive_finite_env_values_are_accepted() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        std::env::set_var(WORKFLOW_WARNING_AGENT_CAP_ENV, "inf");
        std::env::set_var(WORKFLOW_WARNING_TOKEN_CAP_ENV, "1.5");

        assert_eq!(
            workflow_warning_agent_cap(Some(15)),
            15.0,
            "invalid env falls back to guideline"
        );
        assert_eq!(
            workflow_warning_token_cap(),
            DEFAULT_WORKFLOW_WARNING_TOKEN_CAP,
            "invalid env falls back to default"
        );

        clear_env();
    }

    #[test]
    fn projection_uses_fallback_average_before_any_agent_starts() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 22,
            ..base()
        })
        .expect("token warning from fallback projection");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Tokens);
        assert_eq!(warning.projected_tokens, 1_540_000);
        assert_eq!(warning.footer_copy(), FOOTER_COPY);
        assert_eq!(warning.compact_copy(), COMPACT_COPY);
        clear_env();
    }

    #[test]
    fn projection_uses_started_agent_average_and_rounds() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 50,
            started_agents: 3,
            total_tokens: 100_000,
            ..base()
        })
        .expect("projected token warning");
        assert_eq!(warning.projected_tokens, 1_666_667);
        clear_env();
    }

    #[test]
    fn both_axes_can_trigger_together() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 30,
            started_agents: 2,
            total_tokens: 2_000_000,
            ..base()
        })
        .expect("both-axis warning");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Both);
        clear_env();
    }

    #[test]
    fn no_warning_under_caps() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        assert!(workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 10,
            started_agents: 2,
            total_tokens: 100_000,
            ..base()
        })
        .is_none());
        clear_env();
    }

    #[test]
    fn token_only_warning_does_not_claim_the_guideline_triggered_it() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        clear_env();
        let warning = workflow_size_warning(WorkflowSizeWarningInput {
            scheduled_agents: 5,
            guideline_agent_cap: Some(15),
            total_tokens: 1_600_000,
            ..base()
        })
        .expect("token warning");
        assert_eq!(warning.axis, WorkflowSizeWarningAxis::Tokens);
        assert!(!warning.cap_from_guideline);
        clear_env();
    }
}
