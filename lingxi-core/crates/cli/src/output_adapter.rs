//! Adapter that bridges [`lingxi_traits::OutputStream`] emissions from
//! the orchestrator into the CLI's [`crate::output::OutputSink`].
//!
//! Wired in M5-12 Task 9: when the orchestrator emits a `Text` content
//! block, a tool-call, or an end-of-turn cost snapshot, the adapter
//! forwards the event through the configured sink. In JSON mode that
//! produces an NDJSON line; in plain mode it writes to stdout.

use crate::output::OutputSink;
use async_trait::async_trait;
use lingxi_traits::{CostSnapshot, OutputStream};
use std::sync::Arc;

/// Concrete adapter — owns an `Arc<dyn OutputSink>` and projects every
/// `OutputStream` method through it.
pub struct SinkAdapter {
    sink: Arc<dyn OutputSink>,
}

impl SinkAdapter {
    /// Construct an adapter forwarding to the given sink.
    #[must_use]
    pub fn new(sink: Arc<dyn OutputSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl OutputStream for SinkAdapter {
    async fn emit_text(&self, text: &str) {
        self.sink.text(text).await;
    }
    async fn emit_tool_call(
        &self,
        _id: &lingxi_protocol::ToolUseId,
        tool: &str,
        input: &serde_json::Value,
    ) {
        // CLI sinks (plain stdout + NDJSON) do not surface the tool_use_id
        // today — they're orientated on the wire-level event stream where
        // the id is implicit in dispatch order. M6-04 keeps the parameter
        // for forward compatibility; M6-09 may wire it into NDJSON.
        self.sink.tool_call(tool, input).await;
    }
    async fn emit_tool_result(
        &self,
        _id: &lingxi_protocol::ToolUseId,
        tool: &str,
        result: &serde_json::Value,
    ) {
        self.sink.tool_result(tool, result).await;
    }
    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
        self.sink
            .turn_end(
                stop_reason,
                cost.total_usd,
                cost.input_tokens,
                cost.output_tokens,
            )
            .await;
    }
}
