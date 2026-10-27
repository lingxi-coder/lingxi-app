//! Queuing must retain the distinction between SendPrompt and RunSlashCommand.

use super::*;
use tokio::sync::Notify;

struct CapturedPrompt {
    text: Utf16JsonProjection,
    images: Vec<ImageRefDto>,
    turn_id: Option<u64>,
}

struct PromptDriver {
    active_turn: ActiveTurnControl,
    captured: Mutex<Vec<CapturedPrompt>>,
    seed_started: Notify,
    release_seed: Notify,
}

impl PromptDriver {
    async fn capture(&self, text: Utf16JsonProjection, images: Vec<ImageRefDto>) {
        let seed = text.string_at("") == Some("seed");
        self.captured.lock().await.push(CapturedPrompt {
            text,
            images,
            turn_id: self.active_turn.turn_id(),
        });
        if seed {
            self.seed_started.notify_one();
            self.release_seed.notified().await;
        }
    }
}

#[async_trait]
impl TurnDriver for PromptDriver {
    async fn run_turn(&self, text: Utf16JsonProjection) {
        self.capture(text, Vec::new()).await;
    }

    async fn run_queued_batch(
        &self,
        mut inputs: Vec<orchestrator::QueuedPromptInput>,
        _: CancellationToken,
    ) {
        assert_eq!(inputs.len(), 1);
        self.capture(inputs.remove(0).text, Vec::new()).await;
    }

    async fn run_turn_with_images_and_cancel(
        &self,
        text: Utf16JsonProjection,
        images: Vec<ImageRefDto>,
        _: CancellationToken,
    ) {
        self.capture(text, images).await;
    }
}

#[derive(Default)]
struct SlashRouter(Mutex<Vec<Utf16JsonProjection>>);

#[async_trait]
impl crate::router::CommandRouter for SlashRouter {
    async fn route(&self, _: ClientCommand, _: Arc<dyn ClientEventSink>) {
        panic!("slash commands must use the admitted dispatcher");
    }

    async fn dispatch_slash(
        &self,
        raw: Utf16JsonProjection,
        _: CancellationToken,
    ) -> Option<crate::router::SlashDispatchOutcome> {
        self.0.lock().await.push(raw);
        Some(crate::router::SlashDispatchOutcome {
            result: lingxi_core::host::SlashDispatchResult::Handled {
                display: "slash dispatched".into(),
            },
            authority_events: Vec::new(),
        })
    }
}

#[tokio::test]
async fn queued_path_prompt_keeps_its_ingress_and_optional_attachments() {
    let text: Utf16JsonProjection = "/tmp/file.rs please inspect".into();
    for turn_id in [None, Some(42)] {
        let images = turn_id
            .map(|_| ImageRefDto {
                media_type: "image/png".into(),
                base64: "aW1hZ2U=".into(),
            })
            .into_iter()
            .collect::<Vec<_>>();
        let router = Arc::new(SlashRouter::default());
        let mut connection = BridgeConnection::new().bind_router(router.clone());
        let driver = Arc::new(PromptDriver {
            active_turn: connection.active_turn.clone(),
            captured: Mutex::new(Vec::new()),
            seed_started: Notify::new(),
            release_seed: Notify::new(),
        });
        connection.driver = Some(driver.clone());
        connection
            .dispatch(ClientCommand::SendPrompt {
                session_id: None,
                text: "seed".into(),
                prompt_mode: None,
                images: Vec::new(),
                turn_id: Some(41),
                visualization_context: None,
            })
            .await;
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            driver.seed_started.notified(),
        )
        .await
        .expect("seed turn must start before follow-up admission");

        connection
            .dispatch(ClientCommand::SendPrompt {
                session_id: None,
                text: ClientTextDto::from_projection(&text).unwrap(),
                prompt_mode: None,
                images: images.clone(),
                turn_id,
                visualization_context: None,
            })
            .await;
        connection
            .dispatch(ClientCommand::RunSlashCommand {
                session_id: None,
                raw: ClientTextDto::from_projection(&text).unwrap(),
                images: Vec::new(),
                turn_id: Some(43),
            })
            .await;
        assert_eq!(connection.queue.len().await, 2);
        assert_eq!(driver.captured.lock().await.len(), 1);
        assert!(router.0.lock().await.is_empty());

        let task = connection
            .active_turn_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("seed turn owns the queue drain");
        driver.release_seed.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(3), task)
            .await
            .expect("seed and both queued inputs must settle")
            .unwrap();

        let captured = driver.captured.lock().await;
        assert_eq!(captured.len(), 2, "SendPrompt must reach the model driver");
        assert_eq!(captured[1].text, text);
        assert_eq!(captured[1].images, images);
        assert_eq!(captured[1].turn_id, turn_id);
        assert_eq!(
            router.0.lock().await.as_slice(),
            std::slice::from_ref(&text),
            "only the explicitly submitted RunSlashCommand may dispatch as a slash"
        );
        assert!(connection.queue.is_empty().await);
        assert!(!connection.turn_running.load(Ordering::SeqCst));
    }
}
