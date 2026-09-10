//! The `OO()` todo/task-tool model gate.
//!
//! claude-code 2.1.263 stopped advertising the five todo/task-tracking tools
//! (`TodoWrite`, `TaskCreate`, `TaskGet`, `TaskUpdate`, `TaskList`) on the
//! current model generation. Its CHANGELOG entry (chunk `src_168628323.js`
//! @113894) reads:
//!
//! > Todo/task-tracking tools (TaskCreate/Get/Update/List, TodoWrite) are no
//! > longer available on Opus 4.8, Sonnet 5, Fable 5, Mythos 5, and newer
//! > models; set `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` to bring them back
//!
//! The gate itself, verbatim from `src_160988549.js` @1995502:
//!
//! ```js
//! var Zao=[["opus",[4,8]],["sonnet",[5]],["fable",[5]],["mythos",[5]]],
//!     ERe=[XS,UE,mG,WE,kT],
//!     elo="tengu_rosy_wren";
//! function tlo(e){return!s$e(e,Zao)}
//! function OO(){
//!   if(Ja()||QDn())return!0;
//!   let e=J$e();
//!   if(e===void 0||tlo(e))return!0;
//!   if(a.CLAUDE_CODE_ENABLE_TODO_TOOLS===!0)return!0;
//!   return H(elo,!1)===!0
//! }
//! function h3(){return X_()&&OO()}
//! ```
//!
//! Resolved terms:
//! - `Ja()` = `_t()||Jh()!==null` (`src_158095655.js` @674937) — a background
//!   session (`E6()==="bg"`, itself `CLAUDE_CODE_SESSION_KIND==="bg"` @674824)
//!   or an active bg takeover.
//! - `QDn()` = `host.launchOptions.todoToolsOptIn()` (`src_156627426.js`
//!   @78682), set at `src_160988549.js` @3434970 by `ZDn(ERe.some(ue))` — the
//!   user named one of the five tools in `--tools`/`--allowedTools`.
//! - `J$e()` = `m1().mainLoopCanonical?.()` (`src_158095655.js` @260166),
//!   registered as `Dx(()=>HR(rt()))` @349424 — the **session** main-loop model
//!   id, canonicalised, read live (so `/model` moves the gate mid-session).
//!   Note this is the main-loop model, never the caller's: a subagent on an
//!   older model does not get the tools back.
//! - `CLAUDE_CODE_ENABLE_TODO_TOOLS` is declared `I.bool()`
//!   (`src_156866368.js`) — plain env-truthiness, compared `=== true`. This is
//!   deliberately NOT the `I.triBool()` shape that `CLAUDE_CODE_ENABLE_TASKS`
//!   uses, so it does not share `is_env_defined_falsy` with
//!   `is_todo_v2_enabled`.
//! - `H("tengu_rosy_wren", false)` is a GrowthBook gate defaulting to **false**,
//!   ported through `telemetry::flag_bool`. No fetcher is wired in production,
//!   so the snapshot is empty and the read returns its default — byte-identical
//!   to a GrowthBook-absent oracle, and the same reasoning
//!   `telemetry::feature_flags` already documents for its other callers. Going
//!   through the real helper rather than a hardcoded `false` is what lets
//!   `telemetry::test_set_flag` exercise the arm.
//!
//! `h3()` (`X_() && OO()`) is not spelled here because `X_()` — the
//! `LINGXI_ENABLE_TASKS` half — lives with the tools in `tool-task`. Callers
//! compose the two.

use platform_api::env::is_env_truthy;

use crate::tool_trait::ToolStaticContext;

/// `Zao` — the per-family version thresholds at or above which the todo/task
/// tools are withdrawn. A family absent from this table is never gated.
const MODEL_THRESHOLDS: &[(&str, &[u64])] = &[
    ("opus", &[4, 8]),
    ("sonnet", &[5]),
    ("fable", &[5]),
    ("mythos", &[5]),
];

/// The env escape hatch, port-renamed from `CLAUDE_CODE_ENABLE_TODO_TOOLS`.
pub const ENABLE_TODO_TOOLS_ENV: &str = "LINGXI_ENABLE_TODO_TOOLS";

/// `elo` — the gate key of the last `OO()` term.
const ROSY_WREN_FLAG: &str = "tengu_rosy_wren";

/// Port of `s$e(e, r)` (`src_159167389.js` @1061):
///
/// ```js
/// function s$e(e,r){
///   let t=/^claude-([a-z]+)-(\d+(?:-\d+)*)$/.exec(e),i=t?.[1],l=t?.[2];
///   if(!i||!l)return!1;
///   let o=r.find(([s])=>s===i)?.[1];
///   if(!o)return!1;
///   let c=l.split("-").map(Number);
///   for(let s=0;s<Math.max(c.length,o.length);s++){
///     let f=(c[s]??0)-(o[s]??0);
///     if(f!==0)return f>0
///   }
///   return!0
/// }
/// ```
///
/// "Is this model id at or above its family's threshold?" — a component-wise
/// `>=`, with missing components read as `0` and an exact tie counting as at
/// threshold.
///
/// **The two `false` exits are load-bearing and must stay.** An id that does
/// not match the regex (`gpt-5`, `anthropic/claude-opus-4-8`,
/// `claude-3-5-sonnet`, `claude-opus-4-1-eap`) and an id whose family is not in
/// the table (`claude-haiku-4-5`) both answer `false`, which the caller reads as
/// "not gated — keep the tools". That is what holds LingXi's third-party
/// provider ids out of the gate; a looser "is this current-gen" heuristic would
/// silently strip the task tools from every non-Anthropic model.
#[must_use]
fn model_at_or_above_threshold(model: &str, thresholds: &[(&str, &[u64])]) -> bool {
    let Some((family, version)) = split_claude_model_id(model) else {
        return false;
    };
    let Some((_, threshold)) = thresholds.iter().find(|(name, _)| *name == family) else {
        return false;
    };
    for i in 0..version.len().max(threshold.len()) {
        let lhs = i128::from(version.get(i).copied().unwrap_or(0));
        let rhs = i128::from(threshold.get(i).copied().unwrap_or(0));
        if lhs != rhs {
            return lhs > rhs;
        }
    }
    true
}

/// `/^claude-([a-z]+)-(\d+(?:-\d+)*)$/`, hand-rolled so the anchors and the
/// character classes stay literal. Returns the family and the parsed version
/// components, or `None` when the id does not match the whole pattern.
fn split_claude_model_id(model: &str) -> Option<(&str, Vec<u64>)> {
    let rest = model.strip_prefix("claude-")?;
    // `([a-z]+)` — the maximal run of lowercase ASCII. `[a-z]` matches neither
    // a digit nor `-`, so the split point is unambiguous and greediness cannot
    // backtrack into the version.
    let family_len = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    if family_len == 0 {
        return None;
    }
    let (family, tail) = rest.split_at(family_len);
    // The literal `-` between the two capture groups.
    let version = tail.strip_prefix('-')?;
    // `(\d+(?:-\d+)*)$` — one or more `-`-joined runs of ASCII digits, and
    // nothing else through end of string.
    let mut components = Vec::new();
    for part in version.split('-') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // The regex admits arbitrarily many digits; JS would widen to f64. A
        // component past `u64` is astronomically above any threshold, so
        // saturating preserves the comparison's answer.
        components.push(part.parse::<u64>().unwrap_or(u64::MAX));
    }
    Some((family, components))
}

/// Pure core of [`todo_tools_enabled`] — the `OO()` body with its four
/// ambient reads (`Ja()`, `QDn()`, the env var, the gate flag) lifted into
/// parameters.
///
/// `main_loop_model` is `None` for the JS `e === void 0` path, which is an
/// *enable*: an unknown model keeps the tools.
#[must_use]
pub fn todo_tools_enabled_inner(
    main_loop_model: Option<&str>,
    background_session: bool,
    opt_in: bool,
    env_enable: bool,
    gate_flag: bool,
) -> bool {
    // `if(Ja()||QDn())return!0`
    if background_session || opt_in {
        return true;
    }
    // `let e=J$e(); if(e===void 0||tlo(e))return!0`
    match main_loop_model {
        None => return true,
        Some(model) if !model_at_or_above_threshold(model, MODEL_THRESHOLDS) => return true,
        Some(_) => {}
    }
    // `if(a.CLAUDE_CODE_ENABLE_TODO_TOOLS===!0)return!0`
    if env_enable {
        return true;
    }
    // `return H("tengu_rosy_wren",!1)===!0`
    gate_flag
}

/// `OO()` — whether the todo/task tool family is advertised at all.
///
/// The model comes from [`ToolStaticContext::main_loop_model`], which
/// `ToolRegistry::available_tools` fills from the registry's session-scoped
/// main-loop model. That is deliberately the **session** model (the port's
/// `J$e()`) and never the calling agent's: a subagent running an older model
/// does not get the tools back, matching the oracle.
#[must_use]
pub fn todo_tools_enabled(ctx: &ToolStaticContext) -> bool {
    todo_tools_enabled_for_model(ctx.main_loop_model.as_deref())
}

/// `OO()` for a caller that already holds the canonical main-loop model and has
/// no [`ToolStaticContext`] — the reminder producer, which runs off the session
/// rather than off the tool registry.
#[must_use]
pub fn todo_tools_enabled_for_model(main_loop_model: Option<&str>) -> bool {
    todo_tools_enabled_inner(
        main_loop_model,
        platform_api::env::is_bg_session(),
        platform_api::session_flags::todo_tools_opt_in(),
        is_env_truthy(std::env::var(ENABLE_TODO_TOOLS_ENV).ok().as_deref()),
        telemetry::flag_bool(ROSY_WREN_FLAG, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four ids the CHANGELOG names, plus the newer ones the `>=`
    /// comparison is there to catch.
    #[test]
    fn changelog_named_models_are_gated() {
        for model in [
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-mythos-5",
            // "and newer models"
            "claude-opus-4-9",
            "claude-opus-5",
            "claude-opus-6-0",
            "claude-fable-5-1",
            "claude-mythos-5-1",
            "claude-sonnet-5-2",
        ] {
            assert!(
                model_at_or_above_threshold(model, MODEL_THRESHOLDS),
                "{model} must be at or above its threshold"
            );
            assert!(
                !todo_tools_enabled_inner(Some(model), false, false, false, false),
                "{model} must lose the todo tools"
            );
        }
    }

    /// Below threshold in the same family — the tools stay.
    #[test]
    fn older_models_in_a_gated_family_keep_the_tools() {
        for model in [
            "claude-opus-4-7",
            "claude-opus-4-0",
            "claude-opus-3",
            "claude-sonnet-4-5",
            "claude-sonnet-4-6",
            "claude-fable-4-9",
            "claude-mythos-4",
        ] {
            assert!(
                !model_at_or_above_threshold(model, MODEL_THRESHOLDS),
                "{model} must be below its threshold"
            );
            assert!(
                todo_tools_enabled_inner(Some(model), false, false, false, false),
                "{model} must keep the todo tools"
            );
        }
    }

    /// A family with no row in `Zao` is never gated, however new it is.
    #[test]
    fn families_absent_from_the_table_are_never_gated() {
        for model in ["claude-haiku-4-5", "claude-haiku-9-9", "claude-instant-1"] {
            assert!(!model_at_or_above_threshold(model, MODEL_THRESHOLDS));
            assert!(todo_tools_enabled_inner(
                Some(model),
                false,
                false,
                false,
                false
            ));
        }
    }

    /// The regex's `false` exit is what protects LingXi's multi-provider ids —
    /// a deliberate, user-confirmed divergence. If this test goes red the gate
    /// has started eating third-party models.
    #[test]
    fn ids_that_fail_the_regex_keep_the_tools() {
        for model in [
            // Non-Anthropic providers.
            "gpt-5",
            "gemini-3-pro",
            "deepseek-chat",
            "grok-4",
            // Provider-qualified refs — the `^claude-` anchor rejects these.
            "anthropic/claude-opus-4-8",
            "openrouter/anthropic/claude-fable-5.1",
            // Dotted rather than dashed version.
            "claude-fable-5.1",
            // Old-style ids where the family does not lead.
            "claude-3-5-sonnet",
            "claude-3-opus",
            // Early-access and bracketed suffixes — `$` rejects the trailing
            // non-digits.
            "claude-opus-5-eap",
            "claude-opus-5[1m]",
            // Degenerate shapes.
            "",
            "claude-",
            "claude-opus",
            "claude-opus-",
            "claude--4-8",
            "claude-opus-4-",
            "claude-Opus-4-8",
            "claude-opus-4-8 ",
            " claude-opus-4-8",
        ] {
            assert!(
                !model_at_or_above_threshold(model, MODEL_THRESHOLDS),
                "{model:?} must not match the oracle regex"
            );
            assert!(
                todo_tools_enabled_inner(Some(model), false, false, false, false),
                "{model:?} must keep the todo tools"
            );
        }
    }

    /// `(c[s]??0)-(o[s]??0)` — a component the other side does not have reads
    /// as zero, so a bare `claude-sonnet-5` ties `[5]` and a `claude-opus-4`
    /// falls short of `[4,8]`.
    #[test]
    fn missing_version_components_read_as_zero() {
        assert!(model_at_or_above_threshold(
            "claude-sonnet-5",
            MODEL_THRESHOLDS
        ));
        assert!(model_at_or_above_threshold(
            "claude-sonnet-5-0",
            MODEL_THRESHOLDS
        ));
        assert!(!model_at_or_above_threshold(
            "claude-opus-4",
            MODEL_THRESHOLDS
        ));
        assert!(!model_at_or_above_threshold(
            "claude-opus-4-0-0",
            MODEL_THRESHOLDS
        ));
        assert!(model_at_or_above_threshold(
            "claude-opus-4-8-0",
            MODEL_THRESHOLDS
        ));
        assert!(model_at_or_above_threshold(
            "claude-opus-4-8-1",
            MODEL_THRESHOLDS
        ));
    }

    /// The comparison is numeric, not lexicographic: `10 > 9`.
    #[test]
    fn version_components_compare_numerically() {
        assert!(model_at_or_above_threshold(
            "claude-opus-4-10",
            MODEL_THRESHOLDS
        ));
        assert!(!model_at_or_above_threshold(
            "claude-opus-4-9",
            &[("opus", &[4, 10])]
        ));
        // Leading zeros parse as the number, matching JS `Number("08")`.
        assert!(model_at_or_above_threshold(
            "claude-opus-4-08",
            MODEL_THRESHOLDS
        ));
    }

    /// `if(e===void 0||…)return!0` — no known model means no gate.
    #[test]
    fn unknown_model_keeps_the_tools() {
        assert!(todo_tools_enabled_inner(None, false, false, false, false));
    }

    /// The three escape hatches, each on its own.
    #[test]
    fn each_escape_hatch_restores_a_gated_model() {
        let gated = Some("claude-opus-4-8");
        assert!(!todo_tools_enabled_inner(gated, false, false, false, false));
        // `Ja()` — background session / bg takeover.
        assert!(todo_tools_enabled_inner(gated, true, false, false, false));
        // `QDn()` — the user named one of the five tools in --allowedTools.
        assert!(todo_tools_enabled_inner(gated, false, true, false, false));
        // `CLAUDE_CODE_ENABLE_TODO_TOOLS` / `LINGXI_ENABLE_TODO_TOOLS`.
        assert!(todo_tools_enabled_inner(gated, false, false, true, false));
    }

    /// The public wrapper reads the model off the context. Guarded on the live
    /// process state so a stray `LINGXI_SESSION_KIND=bg` or
    /// `LINGXI_ENABLE_TODO_TOOLS` in the environment cannot make this flaky.
    #[test]
    fn the_wrapper_reads_the_model_off_the_context() {
        let hatch_open = platform_api::env::is_bg_session()
            || platform_api::session_flags::todo_tools_opt_in()
            || is_env_truthy(std::env::var(ENABLE_TODO_TOOLS_ENV).ok().as_deref());
        let gated = ToolStaticContext {
            main_loop_model: Some("claude-opus-4-8".to_string()),
            ..Default::default()
        };
        assert_eq!(todo_tools_enabled(&gated), hatch_open);
        // Below threshold, unknown, and non-Claude all stay enabled regardless.
        for model in [None, Some("claude-opus-4-7"), Some("gpt-5")] {
            let ctx = ToolStaticContext {
                main_loop_model: model.map(str::to_string),
                ..Default::default()
            };
            assert!(todo_tools_enabled(&ctx), "{model:?}");
            assert_eq!(
                todo_tools_enabled(&ctx),
                todo_tools_enabled_for_model(model)
            );
        }
    }

    /// `H("tengu_rosy_wren", false)` is the one `OO()` term that is a real gate
    /// read rather than a constant, so it is exercised through the gate helper
    /// the rest of the workspace uses.
    #[test]
    fn the_rosy_wren_gate_reopens_the_gate() {
        let gated = Some("claude-opus-4-8");
        assert!(!todo_tools_enabled_inner(gated, false, false, false, false));
        assert!(todo_tools_enabled_inner(gated, false, false, false, true));
        assert!(
            !telemetry::flag_bool(ROSY_WREN_FLAG, false),
            "default is off"
        );
    }

    /// `Ja()` and `QDn()` short-circuit ahead of the model read, so they hold
    /// even for a model that is otherwise gated and even with no model at all.
    #[test]
    fn session_and_opt_in_hatches_precede_the_model_read() {
        assert!(todo_tools_enabled_inner(None, true, false, false, false));
        assert!(todo_tools_enabled_inner(None, false, true, false, false));
        assert!(todo_tools_enabled_inner(
            Some("claude-mythos-5-1"),
            true,
            true,
            false,
            false
        ));
    }
}
