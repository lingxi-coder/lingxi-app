//! OpenTelemetry enterprise **Monitoring** subsystem (CC 2.1.207 parity,
//! finding H-BIN-06).
//!
//! CC ships a first-party, default-off OpenTelemetry monitoring stack distinct
//! from its `tengu_*` Statsig product telemetry: when
//! `CLAUDE_CODE_ENABLE_TELEMETRY` is truthy it stands up metric/log/trace
//! exporters (OTLP/console/prometheus) driven entirely by the standard `OTEL_*`
//! env surface plus a small `CLAUDE_CODE_OTEL_*` gate family and an
//! `otelHeadersHelper` settings hook for dynamic export headers.
//!
//! ## What this module ports (the load-bearing parity)
//!
//! - [`config`]: the **full env surface** — the master gate, per-signal
//!   exporter selection, OTLP endpoint/headers/protocol (incl. per-signal
//!   override precedence) + mTLS material, export intervals, the
//!   `OTEL_METRICS_INCLUDE_*` attribute toggles, the `OTEL_LOG_*` opt-ins, and
//!   the gate-family timeouts — with byte-faithful boolean/numeric parse
//!   semantics and defaults lifted from the binary.
//! - [`headers_helper`]: the `otelHeadersHelper` validation + debounce + failure
//!   caching state machine, with the byte-exact CC error strings.
//! - [`metrics`]: the `claude_code.*` instrument-name schema + meter identity.
//! - [`logs`]: the `claude_code.events` log signal names + `OTEL_LOG_*` gate
//!   helpers (the orchestrator's opt-in `assistant_response` log line reads
//!   [`logs::assistant_responses_enabled`]).
//!
//! ## Rebrand policy
//!
//! `CLAUDE_CODE_ENABLE_TELEMETRY` → `LINGXI_ENABLE_TELEMETRY` and
//! `CLAUDE_CODE_OTEL_*` → `LINGXI_OTEL_*` (matching the existing
//! `LINGXI_OTEL_DIAG_STDERR` sibling in the hook-env denylist). Every standard
//! `OTEL_*` spec var and every `claude_code.*` / `com.anthropic.claude_code`
//! schema identifier is kept verbatim (`OTel` wire contract; collectors key on
//! them). See [`config`] for the full rationale.
//!
//! ## Remainder (partial — see H-BIN-06 return notes)
//!
//! The SDK exporters/readers that consume [`config::OtelConfig`] (real OTLP
//! egress over grpc/http, the metric readers, the `LoggerProvider`, the trace
//! provider) and the ~20 instrument recording sites (session/token/cost/tool/
//! git/subagent/mcp/hook/compaction) are **not** wired here; this commit lands
//! the config/gate/env/validation core that all of that will read.

pub mod config;
pub mod headers_helper;
pub mod logs;
pub mod metrics;

pub use config::{
    bool_env, env_truthy, int_env, ExporterKind, GateTimeouts, LogIncludeFlags, MetricsInclude,
    OtelConfig, OtlpExporterConfig, OtlpProtocol, Signal, ENV_ENABLE_TELEMETRY,
};
pub use headers_helper::{
    ExecOutcome, HeadersHelperState, ResolveOutcome, validate_helper_output,
};

/// Whether the OpenTelemetry monitoring stack is enabled for this process
/// (binary `o7u()`: `ct(process.env.CLAUDE_CODE_ENABLE_TELEMETRY)`, rebranded
/// to [`config::ENV_ENABLE_TELEMETRY`]). When `false` the whole subsystem is a
/// byte-noop.
#[must_use]
pub fn telemetry_enabled() -> bool {
    match std::env::var(config::ENV_ENABLE_TELEMETRY) {
        Ok(v) => env_truthy(&v),
        Err(_) => false,
    }
}

/// Resolve the full monitoring config from the process env, returning `None`
/// when the master gate is off (so callers can early-out without constructing
/// any exporter state — the default byte-noop path).
///
/// This is the intended boot seam: a future egress-wiring commit calls this at
/// startup and, on `Some`, constructs the OTLP exporters/readers described by
/// the returned [`config::OtelConfig`].
#[must_use]
pub fn init_from_env() -> Option<OtelConfig> {
    let cfg = OtelConfig::from_env();
    if cfg.enabled {
        Some(cfg)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_returns_none_when_gate_off() {
        // Gate parsed from an explicit lookup (never touches process env).
        let off = OtelConfig::from_lookup(|_| None);
        assert!(!off.enabled);
        let on = OtelConfig::from_lookup(|k| {
            (k == config::ENV_ENABLE_TELEMETRY).then(|| "true".to_string())
        });
        assert!(on.enabled);
    }
}
