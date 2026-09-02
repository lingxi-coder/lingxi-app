//! Progress fan-out helper.

use platform_api::{FusionProgress, FusionStage};
use tokio::sync::mpsc::Sender;

/// Best-effort progress send. A closed OR FULL channel is ignored (F005):
/// progress is a lossy side-channel, never a pipeline the orchestrator's own
/// stage transitions should back-pressure on. `try_send` (not `.send().await`)
/// is what makes that true — a slow/stalled consumer must not delay the
/// panel/analyst/synth stage it is merely observing.
pub async fn emit(
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
        });
    }
}
