//! Progress fan-out helper.

use platform_api::{FusionProgress, FusionStage};
use tokio::sync::mpsc::Sender;

/// Best-effort progress send. A closed OR FULL channel is ignored (F005):
/// progress is a lossy side-channel, never a pipeline the orchestrator's own
/// stage transitions should back-pressure on. `try_send` (not `.send().await`)
/// is what makes that true — a slow/stalled consumer must not delay the
/// panel/analyst/synth stage it is merely observing.
pub fn emit(
    progress: &Option<Sender<FusionProgress>>,
    stage: FusionStage,
    panel_id: Option<String>,
    message: impl Into<String>,
) {
    if let Some(tx) = progress {
        let _ = tx.try_send(FusionProgress {
            stage,
            panel_id,
            message: message.into(),
            realized_output_tokens: None,
        });
    }
}

/// [Finding 12] Same as [`emit`], but carries this run's realized
/// output-token spend so far — for the one caller (`local_workflow`'s
/// `fusion()` bridge arm) that needs to charge already-billed spend even
/// when the overall call ends in `Err`.
pub fn emit_with_realized_tokens(
    progress: &Option<Sender<FusionProgress>>,
    stage: FusionStage,
    message: impl Into<String>,
    realized_output_tokens: u64,
) {
    if let Some(tx) = progress {
        let _ = tx.try_send(FusionProgress {
            stage,
            panel_id: None,
            message: message.into(),
            realized_output_tokens: Some(realized_output_tokens),
        });
    }
}
