//! Minimal execution-router seam (design doc Phase 6: "execution router 接入
//! `orchestrator` / composition root").
//!
//! This is a **gated entry point only** — it deliberately does NOT touch the
//! main orchestrator turn loop. The composition root calls [`decide_dispatch`]
//! once per turn with a [`crate::router::RouteInput`]; the result tells it
//! whether to keep running the ordinary single-agent loop (the default,
//! untouched path) or to hand the turn to the dual-LLM pipeline.
//!
//! Keeping this as a pure decision + a thin marker (rather than wiring the
//! dispatch directly into `orchestrator`) preserves the baseline turn loop
//! exactly: a host that never constructs the dual-LLM collaborators simply
//! never sees [`Dispatch::DualLlm`] and behaves as before.

use crate::config::MultiAgentConfig;
use crate::router::route;
use crate::router::ExecutionRoute;
use crate::router::RouteInput;

/// What the host should do with this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// Run the ordinary single-agent turn loop (unchanged baseline path).
    SingleAgent,
    /// Hand the turn to the dual-LLM competitive pipeline
    /// ([`crate::orchestrator::DualLlm`] → review → arbiter →
    /// [`crate::finalizer::Finalizer`] → [`crate::verification`]).
    DualLlm,
}

impl Dispatch {
    /// Whether this dispatch enters the dual-LLM pipeline.
    #[must_use]
    pub fn is_dual_llm(self) -> bool {
        matches!(self, Dispatch::DualLlm)
    }
}

/// Decide how to dispatch a turn (gated seam).
///
/// The dual-LLM path is taken ONLY when the routing decision is
/// [`ExecutionRoute::DualLlmCompetitive`] **and** the config is structurally
/// runnable (exactly two candidates — the MVP requirement). If routing wants
/// dual-LLM but the config cannot support it, the seam falls back to
/// single-agent rather than failing the turn: a misconfigured `multiAgent`
/// block must never break the baseline loop.
#[must_use]
pub fn decide_dispatch(input: &RouteInput<'_>) -> Dispatch {
    match route(input) {
        ExecutionRoute::SingleAgent => Dispatch::SingleAgent,
        ExecutionRoute::DualLlmCompetitive => {
            if is_runnable(input.config) {
                Dispatch::DualLlm
            } else {
                Dispatch::SingleAgent
            }
        }
    }
}

/// Whether a config can actually drive a dual-LLM run (MVP: exactly two
/// candidates). Parsing already enforces this for a fully-formed block, but the
/// seam re-checks so a partially-constructed config can never escalate a turn
/// into an unrunnable pipeline.
#[must_use]
fn is_runnable(config: &MultiAgentConfig) -> bool {
    config.candidates.len() == 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Complexity;
    use crate::router::ExplicitMultiAgentFlag;
    use crate::router::TaskAreaHint;
    use serde_json::json;

    fn config(mode: &str, enabled: bool, candidates: serde_json::Value) -> MultiAgentConfig {
        MultiAgentConfig::from_value(&json!({
            "enabled": enabled,
            "mode": mode,
            "candidates": candidates,
            "arbiter": { "model": "p/arb" }
        }))
        .unwrap()
    }

    fn two_candidates() -> serde_json::Value {
        json!([{ "id": "a", "model": "p/a" }, { "id": "b", "model": "p/b" }])
    }

    fn input<'a>(cfg: &'a MultiAgentConfig, flag: ExplicitMultiAgentFlag) -> RouteInput<'a> {
        RouteInput {
            user_prompt: "implement the thing",
            explicit_flag: flag,
            config: cfg,
            estimated_complexity: Complexity::Low,
            touched_area_hint: TaskAreaHint::default(),
            write_intent: true,
        }
    }

    #[test]
    fn disabled_config_dispatches_single_agent() {
        let cfg = config("auto", false, two_candidates());
        assert_eq!(
            decide_dispatch(&input(&cfg, ExplicitMultiAgentFlag::Unset)),
            Dispatch::SingleAgent
        );
    }

    #[test]
    fn explicit_on_force_dispatches_dual_llm() {
        let cfg = config("off", true, two_candidates());
        // Explicit --multi-agent overrides mode=off.
        assert_eq!(
            decide_dispatch(&input(&cfg, ExplicitMultiAgentFlag::On)),
            Dispatch::DualLlm
        );
    }

    #[test]
    fn force_mode_dispatches_dual_llm() {
        let cfg = config("force", true, two_candidates());
        assert!(decide_dispatch(&input(&cfg, ExplicitMultiAgentFlag::Unset)).is_dual_llm());
    }

    #[test]
    fn baseline_loop_untouched_when_unset_and_no_trigger() {
        // enabled + auto, but no escalation trigger → single agent (baseline).
        let cfg = config("auto", true, two_candidates());
        let mut inp = input(&cfg, ExplicitMultiAgentFlag::Unset);
        inp.user_prompt = "what does this function do?";
        inp.write_intent = false; // pure Q&A
        assert_eq!(decide_dispatch(&inp), Dispatch::SingleAgent);
    }
}
