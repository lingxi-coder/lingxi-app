use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::{mpsc, Arc, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _};
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{
    Compression, Protocol as OtlpWireProtocol, WithExportConfig, WithHttpConfig,
};
use opentelemetry_prometheus::exporter as prometheus_exporter;
use opentelemetry_sdk::logs::{
    BatchConfigBuilder as LogBatchConfigBuilder, BatchLogProcessor, SdkLoggerProvider,
};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{
    BatchConfigBuilder as TraceBatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider,
};
use opentelemetry_sdk::Resource;
use prometheus::{Encoder, Registry, TextEncoder};

use super::config::{ExporterKind, OtelConfig, OtlpExporterConfig, OtlpProtocol};
use super::metrics;
use super::record::{truncate_content, AttrValue, Attributes};
use crate::sink::{AnalyticsValue, LogEventMetadata};

static ACTIVE_RUNTIME: OnceLock<RwLock<Option<Arc<OtelRuntime>>>> = OnceLock::new();

fn runtime_slot() -> &'static RwLock<Option<Arc<OtelRuntime>>> {
    ACTIVE_RUNTIME.get_or_init(|| RwLock::new(None))
}

#[derive(Debug, Clone)]
/// Drop guard for a process-level OTEL runtime installed via [`install_process`].
pub struct TelemetryGuard {
    runtime: Option<Arc<OtelRuntime>>,
}

impl TelemetryGuard {
    #[must_use]
    fn disabled() -> Self {
        TelemetryGuard { runtime: None }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        if let Ok(mut slot) = runtime_slot().write() {
            let clear = slot
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &runtime));
            if clear {
                slot.take();
            }
        }
        runtime.shutdown();
    }
}

#[derive(Debug)]
struct OtelRuntime {
    config: OtelConfig,
    entrypoint: &'static str,
    started_at: Instant,
    resource: Resource,
    metrics: Option<MetricsRuntime>,
    logs: Option<SdkLoggerProvider>,
    traces: Option<SdkTracerProvider>,
    shutdown: AtomicBool,
    #[cfg(test)]
    debug: Mutex<DebugMirrorState>,
}

#[derive(Debug)]
struct MetricsRuntime {
    provider: SdkMeterProvider,
    prometheus_registry: Option<Registry>,
}

#[cfg(test)]
#[derive(Debug, Default, Clone)]
struct DebugMirrorState {
    counters: Vec<DebugMetricSample>,
    histograms: Vec<DebugMetricSample>,
    logs: Vec<DebugLogSample>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct DebugMetricSample {
    instrument: String,
    value: f64,
    attributes: Attributes,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct DebugLogSample {
    event_name: String,
    attributes: Attributes,
}

#[derive(Debug, Clone)]
struct MetricUpdate {
    instrument: &'static str,
    value: f64,
    attributes: Attributes,
    kind: MetricUpdateKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetricUpdateKind {
    Counter,
    Histogram,
}

impl OtelRuntime {
    fn new(
        config: OtelConfig,
        entrypoint: &'static str,
        record_session_metric: bool,
    ) -> Result<Self, String> {
        let resource = build_resource(&config);
        let metrics = build_metrics_runtime(&config, resource.clone())?;
        let logs = build_logger_provider(&config, resource.clone())?;
        let traces = build_tracer_provider(&config, resource.clone())?;

        let runtime = OtelRuntime {
            config,
            entrypoint,
            started_at: Instant::now(),
            resource,
            metrics,
            logs,
            traces,
            shutdown: AtomicBool::new(false),
            #[cfg(test)]
            debug: Mutex::new(DebugMirrorState::default()),
        };

        runtime.emit_span("lingxi.telemetry.start", runtime.process_attributes());
        runtime.emit_log_event(
            "telemetry_started",
            &attrs_from_pairs(&[
                ("entrypoint", AttrValue::from(entrypoint)),
                (
                    "metrics_exporter",
                    AttrValue::from(exporter_kind_name(&runtime.config.metrics.kind)),
                ),
                (
                    "logs_exporter",
                    AttrValue::from(exporter_kind_name(&runtime.config.logs.kind)),
                ),
                (
                    "traces_exporter",
                    AttrValue::from(exporter_kind_name(&runtime.config.traces.kind)),
                ),
            ]),
        );
        if record_session_metric {
            runtime.record_counter(metrics::SESSION_COUNT, 1.0, &Attributes::new());
        }
        Ok(runtime)
    }

    fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }

        let uptime_ms = self.started_at.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let shutdown_attrs = attrs_from_pairs(&[
            ("entrypoint", AttrValue::from(self.entrypoint)),
            ("uptime_ms", AttrValue::from(uptime_ms)),
        ]);
        self.emit_log_event("telemetry_stopped", &shutdown_attrs);
        self.emit_span(
            "lingxi.telemetry.stop",
            self.process_attributes_with_extra([KeyValue::new("uptime_ms", uptime_ms)]),
        );

        let flush_timeout = duration_ms(self.config.timeouts.flush_timeout_ms);
        let shutdown_timeout = duration_ms(self.config.timeouts.shutdown_timeout_ms);

        if let Some(metrics) = &self.metrics {
            let provider = metrics.provider.clone();
            let _ = run_with_timeout("metrics.force_flush", flush_timeout, move || {
                provider.force_flush().map_err(|err| err.to_string())
            });
        }
        if let Some(logs) = &self.logs {
            let provider = logs.clone();
            let _ = run_with_timeout("logs.force_flush", flush_timeout, move || {
                provider.force_flush().map_err(|err| err.to_string())
            });
        }
        if let Some(traces) = &self.traces {
            let provider = traces.clone();
            let _ = run_with_timeout("traces.force_flush", flush_timeout, move || {
                provider.force_flush().map_err(|err| err.to_string())
            });
        }

        if let Some(metrics) = &self.metrics {
            let provider = metrics.provider.clone();
            let _ = run_with_timeout("metrics.shutdown", shutdown_timeout, move || {
                provider.shutdown().map_err(|err| err.to_string())
            });
        }
        if let Some(logs) = &self.logs {
            let provider = logs.clone();
            let _ = run_with_timeout("logs.shutdown", shutdown_timeout, move || {
                provider.shutdown().map_err(|err| err.to_string())
            });
        }
        if let Some(traces) = &self.traces {
            let provider = traces.clone();
            let _ = run_with_timeout("traces.shutdown", shutdown_timeout, move || {
                provider.shutdown().map_err(|err| err.to_string())
            });
        }
    }

    fn process_attributes(&self) -> Vec<KeyValue> {
        self.process_attributes_with_extra(std::iter::empty())
    }

    fn process_attributes_with_extra<I>(&self, extra: I) -> Vec<KeyValue>
    where
        I: IntoIterator<Item = KeyValue>,
    {
        let mut out = vec![KeyValue::new("entrypoint", self.entrypoint)];
        if self.config.metrics_include.version {
            out.push(KeyValue::new("version", env!("CARGO_PKG_VERSION")));
        }
        if self.config.metrics_include.resource_attributes {
            out.extend(resource_kvs(&self.resource));
        }
        out.extend(extra);
        out
    }

    fn record_counter(&self, instrument: &str, value: f64, attrs: &Attributes) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let meter = metrics.provider.meter(metrics::METER_NAME);
        let counter = meter.f64_counter(instrument.to_string()).build();
        let key_values = self.metric_attributes(attrs);
        counter.add(value, &key_values);
        #[cfg(test)]
        self.debug
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .counters
            .push(DebugMetricSample {
                instrument: instrument.to_string(),
                value,
                attributes: attrs.clone(),
            });
    }

    fn record_histogram(&self, instrument: &str, value: f64, attrs: &Attributes) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let meter = metrics.provider.meter(metrics::METER_NAME);
        let histogram = meter.f64_histogram(instrument.to_string()).build();
        let key_values = self.metric_attributes(attrs);
        histogram.record(value, &key_values);
        #[cfg(test)]
        self.debug
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .histograms
            .push(DebugMetricSample {
                instrument: instrument.to_string(),
                value,
                attributes: attrs.clone(),
            });
    }

    fn metric_attributes(&self, attrs: &Attributes) -> Vec<KeyValue> {
        let mut out = Vec::new();
        if self.config.metrics_include.entrypoint {
            out.push(KeyValue::new("entrypoint", self.entrypoint));
        }
        if self.config.metrics_include.version {
            out.push(KeyValue::new("version", env!("CARGO_PKG_VERSION")));
        }
        if self.config.metrics_include.resource_attributes {
            out.extend(resource_kvs(&self.resource));
        }
        out.extend(attrs_to_kvs(attrs));
        out
    }

    fn emit_log_event(&self, event_name: &'static str, attrs: &Attributes) {
        let Some(provider) = &self.logs else {
            #[cfg(test)]
            self.debug
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .logs
                .push(DebugLogSample {
                    event_name: event_name.to_string(),
                    attributes: attrs.clone(),
                });
            return;
        };
        let logger = provider.logger(super::logs::EVENTS_SIGNAL);
        let mut record = logger.create_log_record();
        record.set_event_name(event_name);
        record.set_body(AnyValue::String(event_name.into()));
        record.add_attributes(log_attrs(attrs));
        logger.emit(record);
        #[cfg(test)]
        self.debug
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .logs
            .push(DebugLogSample {
                event_name: event_name.to_string(),
                attributes: attrs.clone(),
            });
    }

    fn emit_dynamic_log_event(&self, body_name: &str, attrs: &Attributes) {
        let mut dynamic_attrs = attrs.clone();
        dynamic_attrs.insert(
            "analytics_event".into(),
            AttrValue::from(body_name.to_string()),
        );
        let Some(provider) = &self.logs else {
            #[cfg(test)]
            self.debug
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .logs
                .push(DebugLogSample {
                    event_name: body_name.to_string(),
                    attributes: dynamic_attrs,
                });
            return;
        };
        let logger = provider.logger(super::logs::EVENTS_SIGNAL);
        let mut record = logger.create_log_record();
        record.set_event_name("analytics_bus_event");
        record.set_body(AnyValue::String(body_name.to_string().into()));
        record.add_attributes(log_attrs(&dynamic_attrs));
        logger.emit(record);
        #[cfg(test)]
        self.debug
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .logs
            .push(DebugLogSample {
                event_name: body_name.to_string(),
                attributes: dynamic_attrs,
            });
    }

    fn emit_span(&self, name: &'static str, attrs: Vec<KeyValue>) {
        let Some(provider) = &self.traces else {
            return;
        };
        let tracer = provider.tracer(super::logs::TRACING_SIGNAL);
        let mut span = tracer
            .span_builder(name)
            .with_attributes(attrs)
            .start(&tracer);
        span.end();
    }
}

/// Install the process-scoped OTEL runtime from the current environment.
///
/// This is default-off and fail-open: when the master gate is off, or exporter
/// setup fails, the returned guard is inert and the process continues without
/// telemetry side effects.
pub fn install_process(entrypoint: &'static str, record_session_metric: bool) -> TelemetryGuard {
    let Some(config) = super::init_from_env() else {
        return TelemetryGuard::disabled();
    };

    match OtelRuntime::new(config, entrypoint, record_session_metric) {
        Ok(runtime) => {
            let runtime = Arc::new(runtime);
            if let Ok(mut slot) = runtime_slot().write() {
                if let Some(previous) = slot.replace(runtime.clone()) {
                    previous.shutdown();
                }
            }
            TelemetryGuard {
                runtime: Some(runtime),
            }
        }
        Err(err) => {
            emit_diag(
                true,
                &format!("OpenTelemetry initialization failed for {entrypoint}: {err}"),
                false,
            );
            TelemetryGuard::disabled()
        }
    }
}

/// Record one floating-point counter update against the active OTEL runtime.
pub fn record_counter(instrument: &str, value: f64, attrs: &Attributes) {
    with_runtime(|runtime| runtime.record_counter(instrument, value, attrs));
}

/// Record one floating-point histogram sample against the active OTEL runtime.
pub fn record_histogram(instrument: &str, value: f64, attrs: &Attributes) {
    with_runtime(|runtime| runtime.record_histogram(instrument, value, attrs));
}

/// Mirror one post-privacy-gate analytics-bus event into the active OTEL
/// runtime's metric and log signals.
pub fn mirror_analytics_event(name: &str, metadata: &LogEventMetadata) {
    with_runtime(|runtime| runtime.mirror_analytics_event(name, metadata));
}

/// Record added/removed line counts from a concrete code-edit operation.
pub fn record_lines_of_code_change(tool_name: &str, added: u64, removed: u64) {
    with_runtime(|runtime| {
        if added > 0 {
            runtime.record_counter(
                metrics::LINES_OF_CODE_COUNT,
                added as f64,
                &attrs_from_optional_pairs([
                    ("type", Some(AttrValue::from("add"))),
                    ("tool_name", Some(AttrValue::from(tool_name.to_string()))),
                ]),
            );
        }
        if removed > 0 {
            runtime.record_counter(
                metrics::LINES_OF_CODE_COUNT,
                removed as f64,
                &attrs_from_optional_pairs([
                    ("type", Some(AttrValue::from("remove"))),
                    ("tool_name", Some(AttrValue::from(tool_name.to_string()))),
                ]),
            );
        }
    });
}

/// Record one hook lifecycle event directly into OTEL.
pub fn emit_hook_lifecycle(
    phase: &'static str,
    status: &'static str,
    tool_name: &str,
    duration_ms: Option<u64>,
) {
    with_runtime(|runtime| {
        let attrs = attrs_from_optional_pairs([
            ("phase", Some(AttrValue::from(phase))),
            ("status", Some(AttrValue::from(status))),
            ("tool_name", Some(AttrValue::from(tool_name.to_string()))),
        ]);
        runtime.record_counter(metrics::HOOK, 1.0, &attrs);
        if let Some(duration_ms) = duration_ms {
            runtime.record_histogram(metrics::HOOK, duration_ms as f64, &attrs);
        }
        runtime.emit_dynamic_log_event(
            match (phase, status) {
                ("pre", "started") => "hook_pre_started",
                ("pre", "completed") => "hook_pre_completed",
                ("post", "started") => "hook_post_started",
                ("post", "completed") => "hook_post_completed",
                _ => "hook_event",
            },
            &attrs,
        );
    });
}

/// Emit the opt-in `assistant_response` OTEL log event with truncation applied.
pub fn emit_assistant_response_log(
    request_id: &str,
    model: &str,
    stop_reason: &str,
    input_tokens: u64,
    output_tokens: u64,
    body: &str,
) {
    with_runtime(|runtime| {
        if !runtime.config.log_include.assistant_responses {
            return;
        }
        let truncated = truncate_content(body, runtime.config.content_max_length);
        let attrs = attrs_from_pairs(&[
            ("request_id", AttrValue::from(request_id.to_string())),
            ("model", AttrValue::from(model.to_string())),
            ("stop_reason", AttrValue::from(stop_reason.to_string())),
            ("input_tokens", AttrValue::from(input_tokens as i64)),
            ("output_tokens", AttrValue::from(output_tokens as i64)),
            ("body", AttrValue::from(truncated.content)),
            ("body_truncated", AttrValue::from(truncated.truncated)),
        ]);
        runtime.emit_log_event("assistant_response", &attrs);
    });
}

/// Emit a named OTEL log event directly against the active runtime.
pub fn emit_named_log_event(name: &'static str, attrs: &Attributes) {
    with_runtime(|runtime| runtime.emit_log_event(name, attrs));
}

/// Render the active Prometheus registry, when the metrics exporter is
/// configured for `prometheus`.
#[must_use]
pub fn prometheus_text() -> Option<String> {
    let slot = runtime_slot().read().ok()?;
    let runtime = slot.as_ref()?;
    let metrics = runtime.metrics.as_ref()?;
    let registry = metrics.prometheus_registry.as_ref()?;
    let encoder = TextEncoder::new();
    let families = registry.gather();
    let mut bytes = Vec::new();
    encoder.encode(&families, &mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn with_runtime(f: impl FnOnce(&OtelRuntime)) {
    if let Ok(slot) = runtime_slot().read() {
        if let Some(runtime) = slot.as_ref() {
            if !runtime.shutdown.load(Ordering::Relaxed) {
                f(runtime);
            }
        }
    }
}

impl OtelRuntime {
    fn mirror_analytics_event(&self, name: &str, metadata: &LogEventMetadata) {
        for update in metric_updates_for_event(name, metadata) {
            match update.kind {
                MetricUpdateKind::Counter => {
                    self.record_counter(update.instrument, update.value, &update.attributes);
                }
                MetricUpdateKind::Histogram => {
                    self.record_histogram(update.instrument, update.value, &update.attributes);
                }
            }
        }

        let attrs = analytics_log_attrs(&self.config, metadata);
        self.emit_dynamic_log_event(name, &attrs);
    }
}

fn metric_updates_for_event(name: &str, metadata: &LogEventMetadata) -> Vec<MetricUpdate> {
    let mut out = Vec::new();
    match name {
        "tengu_api_success" => {
            let shared = attrs_from_optional_pairs([
                ("model", stable_string(metadata, "model")),
                ("provider", stable_string(metadata, "provider")),
                ("status", Some(AttrValue::from("success"))),
                ("query_source", stable_string(metadata, "querySource")),
            ]);
            out.push(counter_update(metrics::LLM_REQUEST, 1.0, shared.clone()));
            if let Some(duration_ms) = int_value(metadata, "durationMs") {
                out.push(histogram_update(
                    metrics::LLM_REQUEST,
                    duration_ms as f64,
                    shared.clone(),
                ));
            }
            for (key, token_type) in [
                ("inputTokens", "input"),
                ("outputTokens", "output"),
                ("cachedInputTokens", "cache_read"),
                ("uncachedInputTokens", "cache_create"),
            ] {
                if let Some(value) = int_value(metadata, key) {
                    out.push(counter_update(
                        metrics::TOKEN_USAGE,
                        value as f64,
                        attrs_from_optional_pairs([
                            ("model", stable_string(metadata, "model")),
                            ("provider", stable_string(metadata, "provider")),
                            ("token_type", Some(AttrValue::from(token_type))),
                        ]),
                    ));
                }
            }
            if let Some(cost_usd) = float_value(metadata, "costUSD") {
                out.push(counter_update(
                    metrics::COST_USAGE,
                    cost_usd,
                    attrs_from_optional_pairs([
                        ("model", stable_string(metadata, "model")),
                        ("provider", stable_string(metadata, "provider")),
                    ]),
                ));
            }
        }
        "tengu_api_request_failed" => {
            out.push(counter_update(
                metrics::LLM_REQUEST,
                1.0,
                attrs_from_optional_pairs([
                    ("model", stable_string(metadata, "model")),
                    ("status", Some(AttrValue::from("failed"))),
                    ("error_kind", stable_string(metadata, "error_kind")),
                ]),
            ));
        }
        "tengu_api_rate_limited" => {
            out.push(counter_update(
                metrics::LLM_REQUEST,
                1.0,
                attrs_from_optional_pairs([
                    ("model", stable_string(metadata, "model")),
                    ("status", Some(AttrValue::from("rate_limited"))),
                ]),
            ));
        }
        "tengu_api_request_cancelled" => {
            out.push(counter_update(
                metrics::LLM_REQUEST,
                1.0,
                attrs_from_optional_pairs([
                    ("model", stable_string(metadata, "model")),
                    ("status", Some(AttrValue::from("cancelled"))),
                ]),
            ));
        }
        "tengu_tool_permission_requested" => {
            out.push(counter_update(
                metrics::TOOL_BLOCKED_ON_USER,
                1.0,
                attrs_from_optional_pairs([("tool_name", tool_name_for_event(name, metadata))]),
            ));
        }
        "tengu_tool_bash_completed" | "tengu_tool_bash_timeout" | "tengu_tool_bash_failed" => {
            out.extend(tool_execution_updates("Bash", name, metadata));
            out.push(counter_update(
                metrics::BASH_SUBPROCESS,
                1.0,
                attrs_from_optional_pairs([("status", Some(AttrValue::from(tool_status(name))))]),
            ));
            if let Some(duration_ms) = int_value(metadata, "duration_ms") {
                out.push(histogram_update(
                    metrics::BASH_SUBPROCESS,
                    duration_ms as f64,
                    attrs_from_optional_pairs([(
                        "status",
                        Some(AttrValue::from(tool_status(name))),
                    )]),
                ));
            }
        }
        "tengu_tool_mcp_invoked" | "tengu_tool_mcp_completed" | "tengu_tool_mcp_failed" => {
            out.extend(tool_execution_updates("Mcp", name, metadata));
            if name != "tengu_tool_mcp_invoked" {
                let status = tool_status(name);
                out.push(counter_update(
                    metrics::MCP_RPC,
                    1.0,
                    attrs_from_optional_pairs([("status", Some(AttrValue::from(status)))]),
                ));
                if let Some(duration_ms) = int_value(metadata, "duration_ms") {
                    out.push(histogram_update(
                        metrics::MCP_RPC,
                        duration_ms as f64,
                        attrs_from_optional_pairs([("status", Some(AttrValue::from(status)))]),
                    ));
                }
            }
        }
        "tengu_tool_task_dispatched" | "tengu_tool_agent_started" => {
            out.push(counter_update(
                metrics::SUBAGENT_SPAWN,
                1.0,
                attrs_from_optional_pairs([("tool_name", tool_name_for_event(name, metadata))]),
            ));
        }
        "tengu_tool_task_completed" | "tengu_tool_agent_completed" | "tengu_tool_agent_failed" => {
            out.extend(tool_execution_updates(
                tool_name_for_event(name, metadata)
                    .as_ref()
                    .and_then(attr_str)
                    .unwrap_or("Task"),
                name,
                metadata,
            ));
            if let Some(duration_ms) = int_value(metadata, "duration_ms") {
                out.push(histogram_update(
                    metrics::SUBAGENT_SPAWN,
                    duration_ms as f64,
                    attrs_from_optional_pairs([(
                        "status",
                        Some(AttrValue::from(tool_status(name))),
                    )]),
                ));
            }
        }
        "tengu_auto_compact_prefix_overflow"
        | "tengu_auto_compact_rapid_refill_breaker"
        | "tengu_command_compact_completed"
        | "tengu_command_compact_failed" => {
            let status = if name.ends_with("_failed") {
                "failed"
            } else {
                "completed"
            };
            out.push(counter_update(
                metrics::COMPACTION,
                1.0,
                attrs_from_optional_pairs([("status", Some(AttrValue::from(status)))]),
            ));
            if let Some(duration_ms) = int_value(metadata, "duration_ms") {
                out.push(histogram_update(
                    metrics::COMPACTION,
                    duration_ms as f64,
                    attrs_from_optional_pairs([("status", Some(AttrValue::from(status)))]),
                ));
            }
        }
        _ => {
            if let Some(tool_name) = tool_name_for_event(name, metadata) {
                if name.contains("_completed")
                    || name.contains("_failed")
                    || name.contains("_timeout")
                {
                    let tool_name_str = attr_str(&tool_name).unwrap_or("tool");
                    out.extend(tool_execution_updates(tool_name_str, name, metadata));
                }
            }
        }
    }
    out
}

fn tool_execution_updates(
    tool_name: &str,
    event_name: &str,
    metadata: &LogEventMetadata,
) -> Vec<MetricUpdate> {
    let mut out = vec![counter_update(
        metrics::TOOL_EXECUTION,
        1.0,
        attrs_from_optional_pairs([
            ("tool_name", Some(AttrValue::from(tool_name.to_string()))),
            ("status", Some(AttrValue::from(tool_status(event_name)))),
        ]),
    )];
    if tool_name == "Mcp" {
        if let Some(duration_ms) = int_value(metadata, "duration_ms") {
            out.push(histogram_update(
                metrics::TOOL_EXECUTION,
                duration_ms as f64,
                attrs_from_optional_pairs([
                    ("tool_name", Some(AttrValue::from(tool_name.to_string()))),
                    ("status", Some(AttrValue::from(tool_status(event_name)))),
                ]),
            ));
        }
    }
    out
}

fn tool_status(event_name: &str) -> &'static str {
    if event_name.ends_with("_failed") {
        "failed"
    } else if event_name.ends_with("_timeout") {
        "timeout"
    } else {
        "completed"
    }
}

fn tool_name_for_event(event_name: &str, metadata: &LogEventMetadata) -> Option<AttrValue> {
    if let Some(name) = stable_string(metadata, "tool_name") {
        return Some(name);
    }
    let name = if event_name.contains("_bash_") {
        "Bash"
    } else if event_name.contains("_mcp_") {
        "Mcp"
    } else if event_name.contains("_task_") {
        "Task"
    } else if event_name.contains("_agent_") {
        "Agent"
    } else if event_name.contains("_edit_") {
        "Edit"
    } else if event_name.contains("_write_") {
        "Write"
    } else if event_name.contains("_read_") {
        "Read"
    } else if event_name.contains("_web_fetch_") {
        "WebFetch"
    } else if event_name.contains("_web_search_") {
        "WebSearch"
    } else if event_name.contains("_enter_worktree_") {
        "EnterWorktree"
    } else if event_name.contains("_exit_worktree_") {
        "ExitWorktree"
    } else if event_name.contains("_schedule_cron_") {
        "ScheduleCron"
    } else if event_name.contains("_cron_delete_") {
        "CronDelete"
    } else if event_name.contains("_cron_list_") {
        "CronList"
    } else if event_name.contains("_tool_search_") {
        "ToolSearch"
    } else if event_name.contains("_config_") {
        "Config"
    } else {
        return None;
    };
    Some(AttrValue::from(name))
}

fn analytics_log_attrs(config: &OtelConfig, metadata: &LogEventMetadata) -> Attributes {
    let mut attrs = Attributes::new();
    for (key, value) in metadata {
        if key.starts_with("_PROTO_") {
            if let Some((log_key, body)) = gated_proto_attr(config, key, value) {
                attrs.insert(log_key, body);
            }
            continue;
        }
        if let Some(attr) = analytics_value_to_attr(key, value) {
            attrs.insert(key.clone(), attr);
        }
    }
    attrs
}

fn gated_proto_attr(
    config: &OtelConfig,
    key: &str,
    value: &AnalyticsValue,
) -> Option<(String, AttrValue)> {
    let gate = if key.contains("prompt") || key.contains("query") {
        config.log_include.user_prompts
    } else if key.contains("command")
        || key.contains("url")
        || key.contains("slug")
        || key.contains("server_name")
        || key.contains("tool_name")
        || key.contains("branch_name")
    {
        config.log_include.tool_details
    } else if key.contains("reason") {
        config.log_include.user_prompts || config.log_include.tool_content
    } else {
        false
    };
    if !gate {
        return None;
    }
    let AnalyticsValue::String(raw) = value else {
        return analytics_value_to_attr(key, value)
            .map(|attr| (key.trim_start_matches("_PROTO_").to_ascii_lowercase(), attr));
    };
    let sanitized = redact_and_cap(key, raw, config.content_max_length);
    Some((
        key.trim_start_matches("_PROTO_").to_ascii_lowercase(),
        AttrValue::from(sanitized),
    ))
}

fn analytics_value_to_attr(key: &str, value: &AnalyticsValue) -> Option<AttrValue> {
    match value {
        AnalyticsValue::Bool(v) => Some(AttrValue::from(*v)),
        AnalyticsValue::Int(v) => Some(AttrValue::from(*v)),
        AnalyticsValue::Float(v) => Some(AttrValue::from(*v)),
        AnalyticsValue::String(v) => Some(AttrValue::from(redact_secret_value(key, v))),
        AnalyticsValue::None => None,
    }
}

fn redact_and_cap(key: &str, value: &str, max_len: i64) -> String {
    let redacted = redact_secret_value(key, value);
    truncate_content(&redacted, max_len).content
}

fn redact_secret_value(key: &str, value: &str) -> String {
    let lowered = key.to_ascii_lowercase();
    // Analytics producers use snake_case, HTTP-style hyphenated names and
    // occasionally dotted keys. Normalize those separators before matching so
    // `x-api-key` cannot bypass the same policy as `api_key`.
    let normalized = lowered.replace(['-', '.'], "_");
    if lowered.contains("secret")
        || lowered.contains("token")
        || lowered.contains("authorization")
        || normalized.contains("api_key")
        || lowered.contains("password")
    {
        return "[REDACTED]".to_string();
    }
    let mut out = value.to_string();
    for marker in ["Bearer ", "bearer ", "sk-", "ctx7sk-", "ghp_"] {
        if let Some(index) = out.find(marker) {
            let suffix_end = out[index + marker.len()..]
                .find(char::is_whitespace)
                .map(|delta| index + marker.len() + delta)
                .unwrap_or(out.len());
            out.replace_range(index..suffix_end, "[REDACTED]");
        }
    }
    out
}

fn counter_update(instrument: &'static str, value: f64, attributes: Attributes) -> MetricUpdate {
    MetricUpdate {
        instrument,
        value,
        attributes,
        kind: MetricUpdateKind::Counter,
    }
}

fn histogram_update(instrument: &'static str, value: f64, attributes: Attributes) -> MetricUpdate {
    MetricUpdate {
        instrument,
        value,
        attributes,
        kind: MetricUpdateKind::Histogram,
    }
}

fn stable_string(metadata: &LogEventMetadata, key: &str) -> Option<AttrValue> {
    let AnalyticsValue::String(value) = metadata.get(key)? else {
        return None;
    };
    Some(AttrValue::from(redact_secret_value(key, value)))
}

fn int_value(metadata: &LogEventMetadata, key: &str) -> Option<i64> {
    match metadata.get(key)? {
        AnalyticsValue::Int(value) => Some(*value),
        AnalyticsValue::Float(value) => Some(*value as i64),
        _ => None,
    }
}

fn float_value(metadata: &LogEventMetadata, key: &str) -> Option<f64> {
    match metadata.get(key)? {
        AnalyticsValue::Float(value) => Some(*value),
        AnalyticsValue::Int(value) => Some(*value as f64),
        _ => None,
    }
}

fn attrs_from_optional_pairs<const N: usize>(pairs: [(&str, Option<AttrValue>); N]) -> Attributes {
    pairs
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key.to_string(), value)))
        .collect()
}

fn attr_str(value: &AttrValue) -> Option<&str> {
    match value {
        AttrValue::Str(value) => Some(value.as_str()),
        _ => None,
    }
}

fn build_metrics_runtime(
    config: &OtelConfig,
    resource: Resource,
) -> Result<Option<MetricsRuntime>, String> {
    let provider = match &config.metrics.kind {
        ExporterKind::None => return Ok(None),
        ExporterKind::Prometheus => {
            let registry = Registry::new();
            let reader = prometheus_exporter()
                .with_registry(registry.clone())
                .build()
                .map_err(|err| err.to_string())?;
            let provider = SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build();
            return Ok(Some(MetricsRuntime {
                provider,
                prometheus_registry: Some(registry),
            }));
        }
        ExporterKind::Console => {
            let reader = PeriodicReader::builder(opentelemetry_stdout::MetricExporter::default())
                .with_interval(duration_ms_opt(config.metrics.export_interval_ms))
                .build();
            SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build()
        }
        ExporterKind::Otlp => {
            let exporter = build_metric_exporter(&config.metrics)?;
            let reader = PeriodicReader::builder(exporter)
                .with_interval(duration_ms_opt(config.metrics.export_interval_ms))
                .build();
            SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build()
        }
        ExporterKind::Other(other) => {
            emit_diag(
                false,
                &format!("unsupported OTEL metrics exporter `{other}`; metrics disabled"),
                config.timeouts.diag_stderr,
            );
            return Ok(None);
        }
    };

    Ok(Some(MetricsRuntime {
        provider,
        prometheus_registry: None,
    }))
}

fn build_logger_provider(
    config: &OtelConfig,
    resource: Resource,
) -> Result<Option<SdkLoggerProvider>, String> {
    let provider = match &config.logs.kind {
        ExporterKind::None => return Ok(None),
        ExporterKind::Prometheus => {
            emit_diag(
                false,
                "OTEL logs exporter `prometheus` is unsupported; logs disabled",
                config.timeouts.diag_stderr,
            );
            return Ok(None);
        }
        ExporterKind::Console => {
            let batch = BatchLogProcessor::builder(opentelemetry_stdout::LogExporter::default())
                .with_batch_config(log_batch_config(config))
                .build();
            SdkLoggerProvider::builder()
                .with_resource(resource)
                .with_log_processor(batch)
                .build()
        }
        ExporterKind::Otlp => {
            let exporter = build_log_exporter(&config.logs)?;
            let batch = BatchLogProcessor::builder(exporter)
                .with_batch_config(log_batch_config(config))
                .build();
            SdkLoggerProvider::builder()
                .with_resource(resource)
                .with_log_processor(batch)
                .build()
        }
        ExporterKind::Other(other) => {
            emit_diag(
                false,
                &format!("unsupported OTEL logs exporter `{other}`; logs disabled"),
                config.timeouts.diag_stderr,
            );
            return Ok(None);
        }
    };

    Ok(Some(provider))
}

fn build_tracer_provider(
    config: &OtelConfig,
    resource: Resource,
) -> Result<Option<SdkTracerProvider>, String> {
    let provider = match &config.traces.kind {
        ExporterKind::None => return Ok(None),
        ExporterKind::Prometheus => {
            emit_diag(
                false,
                "OTEL traces exporter `prometheus` is unsupported; traces disabled",
                config.timeouts.diag_stderr,
            );
            return Ok(None);
        }
        ExporterKind::Console => {
            let batch = BatchSpanProcessor::builder(opentelemetry_stdout::SpanExporter::default())
                .with_batch_config(trace_batch_config(config))
                .build();
            SdkTracerProvider::builder()
                .with_resource(resource)
                .with_span_processor(batch)
                .with_sampler(trace_sampler(config))
                .build()
        }
        ExporterKind::Otlp => {
            let exporter = build_span_exporter(&config.traces)?;
            let batch = BatchSpanProcessor::builder(exporter)
                .with_batch_config(trace_batch_config(config))
                .build();
            SdkTracerProvider::builder()
                .with_resource(resource)
                .with_span_processor(batch)
                .with_sampler(trace_sampler(config))
                .build()
        }
        ExporterKind::Other(other) => {
            emit_diag(
                false,
                &format!("unsupported OTEL traces exporter `{other}`; traces disabled"),
                config.timeouts.diag_stderr,
            );
            return Ok(None);
        }
    };

    Ok(Some(provider))
}

fn build_metric_exporter(
    config: &OtlpExporterConfig,
) -> Result<opentelemetry_otlp::MetricExporter, String> {
    match config.protocol {
        OtlpProtocol::Grpc => {
            let mut builder = opentelemetry_otlp::MetricExporter::builder().with_tonic();
            builder = apply_grpc_export_config(builder, config)?;
            builder.build().map_err(|err| err.to_string())
        }
        OtlpProtocol::HttpProtobuf | OtlpProtocol::HttpJson => {
            let mut builder = opentelemetry_otlp::MetricExporter::builder().with_http();
            builder = apply_http_export_config(builder, config)?;
            builder.build().map_err(|err| err.to_string())
        }
    }
}

fn build_log_exporter(
    config: &OtlpExporterConfig,
) -> Result<opentelemetry_otlp::LogExporter, String> {
    match config.protocol {
        OtlpProtocol::Grpc => {
            let mut builder = opentelemetry_otlp::LogExporter::builder().with_tonic();
            builder = apply_grpc_export_config(builder, config)?;
            builder.build().map_err(|err| err.to_string())
        }
        OtlpProtocol::HttpProtobuf | OtlpProtocol::HttpJson => {
            let mut builder = opentelemetry_otlp::LogExporter::builder().with_http();
            builder = apply_http_export_config(builder, config)?;
            builder.build().map_err(|err| err.to_string())
        }
    }
}

fn build_span_exporter(
    config: &OtlpExporterConfig,
) -> Result<opentelemetry_otlp::SpanExporter, String> {
    match config.protocol {
        OtlpProtocol::Grpc => {
            let mut builder = opentelemetry_otlp::SpanExporter::builder().with_tonic();
            builder = apply_grpc_export_config(builder, config)?;
            builder.build().map_err(|err| err.to_string())
        }
        OtlpProtocol::HttpProtobuf | OtlpProtocol::HttpJson => {
            let mut builder = opentelemetry_otlp::SpanExporter::builder().with_http();
            builder = apply_http_export_config(builder, config)?;
            builder.build().map_err(|err| err.to_string())
        }
    }
}

fn apply_http_export_config<T>(mut builder: T, config: &OtlpExporterConfig) -> Result<T, String>
where
    T: WithExportConfig + WithHttpConfig,
{
    if let Some(endpoint) = normalize_http_endpoint(config)? {
        builder = builder.with_endpoint(endpoint);
    }
    builder = builder.with_protocol(protocol_for(config.protocol));
    if let Some(timeout_ms) = config.timeout_ms {
        builder = builder.with_timeout(duration_ms(timeout_ms));
    }
    if !config.headers.is_empty() {
        builder = builder.with_headers(
            config
                .headers
                .clone()
                .into_iter()
                .collect::<HashMap<_, _>>(),
        );
    }
    if compression_from_config(config.compression.as_deref())?.is_some() {
        return Err(
            "OTLP HTTP compression is not supported on the Rust-1.82-compatible OpenTelemetry line"
                .to_string(),
        );
    }
    builder = builder.with_http_client(build_http_client(config)?);
    Ok(builder)
}

fn apply_grpc_export_config<T>(mut builder: T, config: &OtlpExporterConfig) -> Result<T, String>
where
    T: WithExportConfig + opentelemetry_otlp::WithTonicConfig,
{
    if let Some(endpoint) = normalize_grpc_endpoint(config) {
        builder = builder.with_endpoint(endpoint);
    }
    builder = builder.with_protocol(OtlpWireProtocol::Grpc);
    if let Some(timeout_ms) = config.timeout_ms {
        builder = builder.with_timeout(duration_ms(timeout_ms));
    }
    if !config.headers.is_empty() {
        builder = builder.with_metadata(metadata_from_headers(&config.headers)?);
    }
    if let Some(compression) = compression_from_config(config.compression.as_deref())? {
        builder = builder.with_compression(compression);
    }
    if let Some(tls) = build_tonic_tls_config(config)? {
        builder = builder.with_tls_config(tls);
    }
    Ok(builder)
}

fn build_http_client(config: &OtlpExporterConfig) -> Result<reqwest::blocking::Client, String> {
    let mut builder = reqwest::blocking::Client::builder();
    if let Some(timeout_ms) = config.timeout_ms {
        builder = builder.timeout(duration_ms(timeout_ms));
    }
    if config.insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(path) = &config.certificate {
        let cert = fs::read(path)
            .map_err(|err| format!("failed to read OTLP CA certificate `{path}`: {err}"))?;
        let cert = reqwest::Certificate::from_pem(&cert).map_err(|err| err.to_string())?;
        builder = builder.add_root_certificate(cert);
    }
    match (&config.client_certificate, &config.client_key) {
        (Some(cert_path), Some(key_path)) => {
            let cert = fs::read(cert_path).map_err(|err| {
                format!("failed to read OTLP client certificate `{cert_path}`: {err}")
            })?;
            let key = fs::read(key_path)
                .map_err(|err| format!("failed to read OTLP client key `{key_path}`: {err}"))?;
            let mut pem = cert;
            pem.push(b'\n');
            pem.extend(key);
            let identity = reqwest::Identity::from_pem(&pem).map_err(|err| err.to_string())?;
            builder = builder.identity(identity);
        }
        (None, None) => {}
        _ => {
            return Err(
                "OTLP HTTP mTLS requires both client certificate and client key paths".to_string(),
            );
        }
    }
    builder.build().map_err(|err| err.to_string())
}

fn build_tonic_tls_config(
    config: &OtlpExporterConfig,
) -> Result<Option<tonic::transport::ClientTlsConfig>, String> {
    let needs_tls = config
        .endpoint
        .as_deref()
        .is_some_and(|endpoint| endpoint.starts_with("https://"));
    if !needs_tls && config.certificate.is_none() && config.client_certificate.is_none() {
        return Ok(None);
    }

    let mut tls = tonic::transport::ClientTlsConfig::new();
    if let Some(endpoint) = &config.endpoint {
        if let Ok(url) = reqwest::Url::parse(endpoint) {
            if let Some(host) = url.host_str() {
                tls = tls.domain_name(host.to_string());
            }
        }
    }
    if let Some(path) = &config.certificate {
        let cert = fs::read(path)
            .map_err(|err| format!("failed to read OTLP CA certificate `{path}`: {err}"))?;
        tls = tls.ca_certificate(tonic::transport::Certificate::from_pem(cert));
    }
    match (&config.client_certificate, &config.client_key) {
        (Some(cert_path), Some(key_path)) => {
            let cert = fs::read(cert_path).map_err(|err| {
                format!("failed to read OTLP client certificate `{cert_path}`: {err}")
            })?;
            let key = fs::read(key_path)
                .map_err(|err| format!("failed to read OTLP client key `{key_path}`: {err}"))?;
            tls = tls.identity(tonic::transport::Identity::from_pem(cert, key));
        }
        (None, None) => {}
        _ => {
            return Err(
                "OTLP gRPC mTLS requires both client certificate and client key paths".to_string(),
            );
        }
    }
    Ok(Some(tls))
}

fn metadata_from_headers(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<tonic::metadata::MetadataMap, String> {
    let mut metadata = tonic::metadata::MetadataMap::new();
    for (key, value) in headers {
        let metadata_key: tonic::metadata::AsciiMetadataKey = key
            .parse()
            .map_err(|err| format!("invalid OTLP gRPC metadata key `{key}`: {err}"))?;
        let parsed = value
            .parse()
            .map_err(|err| format!("invalid OTLP gRPC metadata value for `{key}`: {err}"))?;
        metadata.insert(metadata_key, parsed);
    }
    Ok(metadata)
}

fn protocol_for(protocol: OtlpProtocol) -> OtlpWireProtocol {
    match protocol {
        OtlpProtocol::Grpc => OtlpWireProtocol::Grpc,
        OtlpProtocol::HttpProtobuf => OtlpWireProtocol::HttpBinary,
        OtlpProtocol::HttpJson => OtlpWireProtocol::HttpJson,
    }
}

fn compression_from_config(raw: Option<&str>) -> Result<Option<Compression>, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some("none") => Ok(None),
        Some("gzip") => Ok(Some(Compression::Gzip)),
        Some(other) => Err(format!("unsupported OTLP compression `{other}`")),
    }
}

fn normalize_http_endpoint(config: &OtlpExporterConfig) -> Result<Option<String>, String> {
    let Some(endpoint) = config.endpoint.clone() else {
        return Ok(None);
    };
    let mut url = reqwest::Url::parse(&endpoint).map_err(|err| err.to_string())?;
    if url.path().is_empty() || url.path() == "/" {
        url.set_path(match config.signal {
            super::config::Signal::Metrics => "/v1/metrics",
            super::config::Signal::Logs => "/v1/logs",
            super::config::Signal::Traces => "/v1/traces",
        });
    }
    Ok(Some(url.to_string()))
}

fn normalize_grpc_endpoint(config: &OtlpExporterConfig) -> Option<String> {
    let Some(endpoint) = config.endpoint.clone() else {
        return None;
    };
    if config.insecure && endpoint.starts_with("https://") {
        Some(format!("http://{}", &endpoint["https://".len()..]))
    } else {
        Some(endpoint)
    }
}

fn build_resource(config: &OtelConfig) -> Resource {
    let mut builder = Resource::builder_empty().with_service_name(config.service_name.clone());
    let attrs = config
        .resource_attributes
        .as_deref()
        .map(parse_resource_attributes)
        .unwrap_or_default();
    if !attrs.is_empty() {
        builder = builder.with_attributes(attrs);
    }
    builder.build()
}

fn parse_resource_attributes(raw: &str) -> Vec<KeyValue> {
    raw.split(',')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(KeyValue::new(key.to_string(), value.trim().to_string()))
        })
        .collect()
}

fn resource_kvs(resource: &Resource) -> Vec<KeyValue> {
    resource
        .iter()
        .map(|(key, value)| KeyValue::new(key.to_string(), value.clone()))
        .collect()
}

fn attrs_to_kvs(attrs: &Attributes) -> Vec<KeyValue> {
    attrs
        .iter()
        .map(|(key, value)| attr_to_key_value(key, value))
        .collect()
}

fn log_attrs(attrs: &Attributes) -> Vec<(String, AnyValue)> {
    attrs
        .iter()
        .map(|(key, value)| (key.clone(), attr_to_log_value(value)))
        .collect()
}

fn attr_to_key_value(key: &str, value: &AttrValue) -> KeyValue {
    match value {
        AttrValue::Str(v) => KeyValue::new(key.to_string(), v.clone()),
        AttrValue::Int(v) => KeyValue::new(key.to_string(), *v),
        AttrValue::Float(v) => KeyValue::new(key.to_string(), *v),
        AttrValue::Bool(v) => KeyValue::new(key.to_string(), *v),
    }
}

fn attr_to_log_value(value: &AttrValue) -> AnyValue {
    match value {
        AttrValue::Str(v) => AnyValue::String(v.clone().into()),
        AttrValue::Int(v) => AnyValue::Int(*v),
        AttrValue::Float(v) => AnyValue::Double(*v),
        AttrValue::Bool(v) => AnyValue::Boolean(*v),
    }
}

fn attrs_from_pairs(pairs: &[(&str, AttrValue)]) -> Attributes {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn log_batch_config(config: &OtelConfig) -> opentelemetry_sdk::logs::BatchConfig {
    LogBatchConfigBuilder::default()
        .with_scheduled_delay(duration_ms_opt(config.logs.export_interval_ms))
        .build()
}

fn trace_batch_config(_config: &OtelConfig) -> opentelemetry_sdk::trace::BatchConfig {
    TraceBatchConfigBuilder::default().build()
}

fn trace_sampler(config: &OtelConfig) -> opentelemetry_sdk::trace::Sampler {
    use opentelemetry_sdk::trace::Sampler;

    match config.traces_sampler.as_deref() {
        Some("always_on") => Sampler::AlwaysOn,
        Some("always_off") => Sampler::AlwaysOff,
        Some("traceidratio") => {
            Sampler::TraceIdRatioBased(parse_ratio(config.traces_sampler_arg.as_deref()))
        }
        Some("parentbased_always_off") => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
        Some("parentbased_traceidratio") => Sampler::ParentBased(Box::new(
            Sampler::TraceIdRatioBased(parse_ratio(config.traces_sampler_arg.as_deref())),
        )),
        _ => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
    }
}

fn parse_ratio(raw: Option<&str>) -> f64 {
    raw.and_then(|value| value.parse::<f64>().ok())
        .map(|value| value.clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

fn exporter_kind_name(kind: &ExporterKind) -> &'static str {
    match kind {
        ExporterKind::Otlp => "otlp",
        ExporterKind::Console => "console",
        ExporterKind::Prometheus => "prometheus",
        ExporterKind::None => "none",
        ExporterKind::Other(_) => "other",
    }
}

fn duration_ms(ms: i64) -> Duration {
    Duration::from_millis(ms.max(0) as u64)
}

fn duration_ms_opt(ms: Option<i64>) -> Duration {
    duration_ms(ms.unwrap_or(0))
}

fn run_with_timeout(
    label: &'static str,
    timeout: Duration,
    f: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "{label} timed out after {} ms",
            timeout.as_millis()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!("{label} worker disconnected")),
    }
}

fn emit_diag(error: bool, message: &str, stderr_gate: bool) {
    if error {
        tracing::warn!(target: "telemetry::otel", "{message}");
    } else {
        tracing::debug!(target: "telemetry::otel", "{message}");
    }
    if stderr_gate {
        eprintln!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AnalyticsBus;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use tokio::runtime::Runtime;

    static RUNTIME_SLOT_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, Clone)]
    struct CapturedRequest {
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    fn enabled_lookup(port: u16) -> OtelConfig {
        OtelConfig::from_lookup(|key| match key {
            super::super::config::ENV_ENABLE_TELEMETRY => Some("1".to_string()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(format!("http://127.0.0.1:{port}")),
            "OTEL_EXPORTER_OTLP_HEADERS" => Some("authorization=Bearer test".to_string()),
            "OTEL_METRICS_EXPORTER" => Some("otlp".to_string()),
            "OTEL_LOGS_EXPORTER" => Some("otlp".to_string()),
            "OTEL_TRACES_EXPORTER" => Some("otlp".to_string()),
            "OTEL_LOG_ASSISTANT_RESPONSES" => Some("1".to_string()),
            "OTEL_SERVICE_NAME" => Some("telemetry-test".to_string()),
            _ => None,
        })
    }

    fn local_runtime_config() -> OtelConfig {
        OtelConfig::from_lookup(|key| match key {
            super::super::config::ENV_ENABLE_TELEMETRY => Some("1".to_string()),
            "OTEL_METRICS_EXPORTER" => Some("prometheus".to_string()),
            "OTEL_LOGS_EXPORTER" => Some("none".to_string()),
            "OTEL_TRACES_EXPORTER" => Some("none".to_string()),
            _ => None,
        })
    }

    fn local_runtime_with_logs(log_user_prompts: bool) -> OtelConfig {
        OtelConfig::from_lookup(|key| match key {
            super::super::config::ENV_ENABLE_TELEMETRY => Some("1".to_string()),
            "OTEL_METRICS_EXPORTER" => Some("none".to_string()),
            "OTEL_LOGS_EXPORTER" => Some("none".to_string()),
            "OTEL_TRACES_EXPORTER" => Some("none".to_string()),
            "OTEL_LOG_USER_PROMPTS" if log_user_prompts => Some("1".to_string()),
            _ => None,
        })
    }

    fn install_test_runtime(config: OtelConfig) -> Arc<OtelRuntime> {
        let runtime = Arc::new(OtelRuntime::new(config, "test-entry", false).expect("runtime"));
        *runtime_slot().write().unwrap() = Some(runtime.clone());
        runtime
    }

    fn clear_runtime() {
        if let Some(runtime) = runtime_slot().write().unwrap().take() {
            runtime.shutdown();
        }
    }

    fn snapshot(runtime: &Arc<OtelRuntime>) -> DebugMirrorState {
        runtime
            .debug
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn spawn_http_collector() -> (
        u16,
        Arc<Mutex<Vec<CapturedRequest>>>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test collector");
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let shared = captured.clone();
        let handle = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut header_buf = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read_exact(&mut byte).is_ok() {
                    header_buf.push(byte[0]);
                    if header_buf.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let header_text = String::from_utf8(header_buf.clone()).expect("header utf8");
                let mut lines = header_text.split("\r\n");
                let first = lines.next().unwrap();
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut headers = HashMap::new();
                let mut content_length = 0usize;
                for line in lines {
                    if line.is_empty() {
                        continue;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        let value = value.trim().to_string();
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.parse().unwrap_or(0);
                        }
                        headers.insert(name.to_ascii_lowercase(), value);
                    }
                }
                let mut body = vec![0; content_length];
                stream.read_exact(&mut body).expect("body");
                shared.lock().unwrap().push(CapturedRequest {
                    path,
                    headers,
                    body,
                });
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .expect("response");
            }
        });
        (port, captured, handle)
    }

    #[test]
    fn http_runtime_exports_all_three_signals() {
        let (port, captured, handle) = spawn_http_collector();
        let runtime = OtelRuntime::new(enabled_lookup(port), "test-entry", true).expect("runtime");

        runtime.record_counter(metrics::SESSION_COUNT, 1.0, &Attributes::new());
        runtime.emit_log_event(
            "assistant_response",
            &attrs_from_pairs(&[("body", AttrValue::from("ok"))]),
        );
        runtime.emit_span("test.span", vec![KeyValue::new("entrypoint", "test-entry")]);
        runtime.shutdown();

        handle.join().expect("collector join");

        let requests = captured.lock().unwrap().clone();
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().any(|request| request.path == "/v1/metrics"));
        assert!(requests.iter().any(|request| request.path == "/v1/logs"));
        assert!(requests.iter().any(|request| request.path == "/v1/traces"));
        for request in requests {
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                Some("Bearer test")
            );
            assert!(!request.body.is_empty());
        }
    }

    #[test]
    fn prometheus_exporter_renders_metrics_text() {
        let config = OtelConfig::from_lookup(|key| match key {
            super::super::config::ENV_ENABLE_TELEMETRY => Some("1".to_string()),
            "OTEL_METRICS_EXPORTER" => Some("prometheus".to_string()),
            _ => None,
        });
        let runtime = OtelRuntime::new(config, "prom", true).expect("runtime");
        runtime.record_counter(metrics::SESSION_COUNT, 1.0, &Attributes::new());
        let text = {
            let registry = runtime
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.prometheus_registry.as_ref())
                .expect("registry");
            let encoder = TextEncoder::new();
            let mut bytes = Vec::new();
            encoder.encode(&registry.gather(), &mut bytes).unwrap();
            String::from_utf8(bytes).unwrap()
        };
        assert!(text.contains("claude_code_session_count_total"));
        runtime.shutdown();
    }

    #[test]
    fn bus_api_success_maps_metrics_and_log() {
        let _lock = RUNTIME_SLOT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_runtime();
        let runtime = install_test_runtime(local_runtime_config());
        let bus = AnalyticsBus::new();
        let task = async {
            let mut md = LogEventMetadata::new();
            md.insert(
                "model".into(),
                AnalyticsValue::String("claude-opus-4-6".into()),
            );
            md.insert(
                "provider".into(),
                AnalyticsValue::String("anthropic".into()),
            );
            md.insert("querySource".into(), AnalyticsValue::String("user".into()));
            md.insert("inputTokens".into(), AnalyticsValue::Int(1000));
            md.insert("outputTokens".into(), AnalyticsValue::Int(500));
            md.insert("cachedInputTokens".into(), AnalyticsValue::Int(128));
            md.insert("uncachedInputTokens".into(), AnalyticsValue::Int(64));
            md.insert("costUSD".into(), AnalyticsValue::Float(0.0175));
            md.insert("durationMs".into(), AnalyticsValue::Int(250));
            bus.log_event("tengu_api_success", md).await;
        };
        Runtime::new().unwrap().block_on(task);

        let debug = snapshot(&runtime);
        assert!(debug.counters.iter().any(|sample| {
            sample.instrument == metrics::TOKEN_USAGE
                && sample.attributes.get("token_type") == Some(&AttrValue::from("input"))
                && (sample.value - 1000.0).abs() < f64::EPSILON
        }));
        assert!(debug.counters.iter().any(|sample| {
            sample.instrument == metrics::COST_USAGE && (sample.value - 0.0175).abs() < 1e-12
        }));
        assert!(debug.counters.iter().any(|sample| {
            sample.instrument == metrics::LLM_REQUEST
                && sample.attributes.get("status") == Some(&AttrValue::from("success"))
        }));
        assert!(debug.histograms.iter().any(|sample| {
            sample.instrument == metrics::LLM_REQUEST && (sample.value - 250.0).abs() < f64::EPSILON
        }));
        assert!(debug
            .logs
            .iter()
            .any(|sample| sample.event_name == "tengu_api_success"));
        clear_runtime();
    }

    #[test]
    fn bus_tool_and_blocked_mappings_emit_metrics() {
        let _lock = RUNTIME_SLOT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_runtime();
        let runtime = install_test_runtime(local_runtime_config());
        let bus = AnalyticsBus::new();
        let task = async {
            let mut blocked = LogEventMetadata::new();
            blocked.insert("tool_name".into(), AnalyticsValue::String("Bash".into()));
            bus.log_event("tengu_tool_permission_requested", blocked)
                .await;

            let mut bash = LogEventMetadata::new();
            bash.insert("duration_ms".into(), AnalyticsValue::Int(42));
            bus.log_event("tengu_tool_bash_completed", bash).await;

            let mut mcp = LogEventMetadata::new();
            mcp.insert("duration_ms".into(), AnalyticsValue::Int(77));
            bus.log_event("tengu_tool_mcp_completed", mcp).await;

            bus.log_event("tengu_tool_task_dispatched", LogEventMetadata::new())
                .await;
        };
        Runtime::new().unwrap().block_on(task);

        let debug = snapshot(&runtime);
        assert!(debug
            .counters
            .iter()
            .any(|sample| { sample.instrument == metrics::TOOL_BLOCKED_ON_USER }));
        assert!(debug.counters.iter().any(|sample| {
            sample.instrument == metrics::BASH_SUBPROCESS
                && sample.attributes.get("status") == Some(&AttrValue::from("completed"))
        }));
        assert!(debug.histograms.iter().any(|sample| {
            sample.instrument == metrics::MCP_RPC && (sample.value - 77.0).abs() < f64::EPSILON
        }));
        assert!(debug
            .counters
            .iter()
            .any(|sample| sample.instrument == metrics::SUBAGENT_SPAWN));
        clear_runtime();
    }

    #[test]
    fn proto_prompt_logging_is_opt_in_and_redacted() {
        let _lock = RUNTIME_SLOT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_runtime();
        let bus = AnalyticsBus::new();

        let runtime_off = install_test_runtime(local_runtime_with_logs(false));
        let task = async {
            let mut md = LogEventMetadata::new();
            md.insert(
                "_PROTO_prompt".into(),
                AnalyticsValue::String("Bearer secret sk-live-token".into()),
            );
            bus.log_event("tengu_tool_schedule_cron_completed", md)
                .await;
        };
        Runtime::new().unwrap().block_on(task);
        let debug_off = snapshot(&runtime_off);
        assert!(debug_off
            .logs
            .iter()
            .all(|sample| !sample.attributes.contains_key("prompt")));
        clear_runtime();

        let runtime_on = install_test_runtime(local_runtime_with_logs(true));
        Runtime::new().unwrap().block_on(async {
            let mut md = LogEventMetadata::new();
            md.insert(
                "_PROTO_prompt".into(),
                AnalyticsValue::String("Bearer secret sk-live-token".into()),
            );
            bus.log_event("tengu_tool_schedule_cron_completed", md)
                .await;
        });
        let debug_on = snapshot(&runtime_on);
        let prompt = debug_on
            .logs
            .iter()
            .find(|sample| sample.event_name == "tengu_tool_schedule_cron_completed")
            .and_then(|sample| sample.attributes.get("prompt"))
            .and_then(attr_str)
            .expect("prompt log");
        assert!(prompt.contains("[REDACTED]"));
        clear_runtime();
    }

    #[test]
    fn redaction_covers_hyphenated_and_dotted_api_key_names() {
        for key in ["x-api-key", "provider.api.key", "API_KEY"] {
            assert_eq!(
                redact_secret_value(key, "plain-secret-value"),
                "[REDACTED]",
                "secret-bearing key {key} must be redacted by name"
            );
        }
    }

    #[test]
    fn bus_without_runtime_records_nothing() {
        let _lock = RUNTIME_SLOT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_runtime();
        let bus = AnalyticsBus::new();
        Runtime::new().unwrap().block_on(async {
            bus.log_event("tengu_api_success", LogEventMetadata::new())
                .await;
        });
        assert!(runtime_slot().read().unwrap().is_none());
    }
}
