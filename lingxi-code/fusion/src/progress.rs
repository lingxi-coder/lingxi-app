//! Progress fan-out helper.

use platform_api::{FusionProgress, FusionStage};
use tokio::sync::mpsc::Sender;

/// Best-effort progress send. A closed or full channel is ignored.
pub async fn emit(
    progress: &Option<Sender<FusionProgress>>,
    stage: FusionStage,
    panel_id: Option<String>,
    message: impl Into<String>,
) {
    if let Some(tx) = progress {
        let _ = tx
            .send(FusionProgress {
                stage,
                panel_id,
                message: message.into(),
            })
            .await;
    }
}
