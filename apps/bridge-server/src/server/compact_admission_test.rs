//! Compact admission must not observe the transient idle flag during a drain handoff.

use super::*;
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct AdmissionRouter {
    calls: AtomicUsize,
    run_as_turn: bool,
}

#[async_trait]
impl crate::router::CommandRouter for AdmissionRouter {
    async fn route(&self, _: ClientCommand, _: Arc<dyn ClientEventSink>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    async fn dispatch_slash(
        &self,
        _: Utf16JsonProjection,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> Option<crate::router::SlashDispatchOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Some(crate::router::SlashDispatchOutcome {
            result: if self.run_as_turn {
                lingxi_core::host::SlashDispatchResult::RunAsTurn {
                    prompt: "custom compact prompt".into(),
                }
            } else {
                lingxi_core::host::SlashDispatchResult::Handled {
                    display: "idle compact".into(),
                }
            },
            authority_events: Vec::new(),
        })
    }

    async fn requires_idle_turn_for_slash(&self, raw: &Utf16JsonProjection) -> bool {
        command_api::parser::parse_slash_command(raw)
            .is_some_and(|parsed| parsed.name.string_at("").as_deref() == Some("compact"))
    }
}

struct AdmissionDriver(tokio::sync::Notify);

#[async_trait]
impl TurnDriver for AdmissionDriver {
    async fn run_turn(&self, prompt: Utf16JsonProjection) {
        assert_eq!(prompt.string_at(""), Some("custom compact prompt"));
        self.0.notify_one();
    }
}

struct IdleTurnDriver;

#[async_trait]
impl TurnDriver for IdleTurnDriver {
    async fn run_turn(&self, _: Utf16JsonProjection) {}
}

#[tokio::test]
async fn custom_compact_turn_releases_admission_before_starting_turn() {
    let router = Arc::new(AdmissionRouter {
        run_as_turn: true,
        ..AdmissionRouter::default()
    });
    let driver = Arc::new(AdmissionDriver(tokio::sync::Notify::new()));
    let mut connection = BridgeConnection::new().bind_router(router.clone());
    // This driver performs no tools/interactions, so it needs no permission gate.
    connection.driver = Some(driver.clone());
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connection.dispatch(ClientCommand::RunSlashCommand {
            raw: "/compact custom prompt".into(),
            images: Vec::new(),
            turn_id: Some(18),
        }),
    )
    .await
    .expect("RunAsTurn must not recursively acquire compact admission");
    tokio::time::timeout(std::time::Duration::from_secs(1), driver.0.notified())
        .await
        .expect("the expanded custom compact prompt must actually run");
    assert_eq!(router.calls.load(Ordering::SeqCst), 1);
    let task = connection
        .active_turn_task
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .expect("the custom command owns a spawned turn");
    tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("turn cleanup must release admission")
        .unwrap();
}

#[tokio::test]
async fn compact_cannot_enter_during_transient_idle_handoff() {
    for slash in [false, true] {
        let router = Arc::new(AdmissionRouter::default());
        let mut connection = BridgeConnection::new().bind_router(router.clone());
        // Both slash admission and session transitions require a real bound
        // turn driver. Without one, RunSlashCommand is rejected before it
        // reaches the handoff gate this regression exercises.
        connection.driver = Some(Arc::new(IdleTurnDriver));
        let mut captured = if slash {
            Some(super::connection_regression_test::captured_sink().await)
        } else {
            None
        };
        if let Some((_, _, sink)) = captured.as_ref() {
            assert!(connection.claim_outbound(sink).await);
        }
        let command = || {
            if slash {
                ClientCommand::RunSlashCommand {
                    raw: "/compact preserve decisions".into(),
                    images: Vec::new(),
                    turn_id: Some(17),
                }
            } else {
                ClientCommand::ForceCompact
            }
        };
        connection.turn_running.store(true, Ordering::SeqCst);
        // Model the real drain owner's critical interval: the flag is false
        // while queued follow-ups are inspected, before ownership is reclaimed.
        let handoff = connection.turn_handoff.lock().await;
        connection.turn_running.store(false, Ordering::SeqCst);
        let dispatch = connection.dispatch(command());
        tokio::pin!(dispatch);
        assert!(
            futures_util::poll!(dispatch.as_mut()).is_pending(),
            "compact must wait for a settled handoff, not trust transient idle"
        );
        assert_eq!(router.calls.load(Ordering::SeqCst), 0);
        connection.turn_running.store(true, Ordering::SeqCst);
        drop(handoff);
        tokio::time::timeout(std::time::Duration::from_secs(1), dispatch)
            .await
            .expect("settled active ownership must reject without entering the router");
        assert_eq!(router.calls.load(Ordering::SeqCst), 0);
        assert!(
            connection.queue.is_empty().await,
            "busy compact must be rejected, not queued for a later turn"
        );
        if let Some((_, socket, _)) = captured.as_mut() {
            assert!(matches!(
                super::connection_regression_test::next_frame(socket).await,
                Frame::Event(ClientEvent::SlashCommandResult {
                    turn_id: Some(17),
                    is_error: true,
                    display,
                }) if display.code_units
                    == "this command requires an idle session".encode_utf16().collect::<Vec<_>>()
            ));
        }

        // Once the drain owner really becomes idle, both command surfaces must
        // dispatch normally; the admission lock must not remain held on refusal.
        {
            let _handoff = connection.turn_handoff.lock().await;
            connection.turn_running.store(false, Ordering::SeqCst);
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            connection.dispatch(command()),
        )
        .await
        .expect("idle compact must still dispatch");
        if slash {
            let task = connection
                .active_turn_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("idle slash compact owns its dispatched task");
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .expect("idle slash compact dispatch must finish")
                .unwrap();
        }
        assert_eq!(router.calls.load(Ordering::SeqCst), 1);
        if let Some((endpoint, mut socket, _)) = captured {
            assert!(matches!(
                super::connection_regression_test::next_frame(&mut socket).await,
                Frame::Event(ClientEvent::SlashCommandResult {
                    turn_id: Some(17),
                    is_error: false,
                    ..
                })
            ));
            socket.close(None).await.unwrap();
            endpoint.shutdown().await;
        }
    }
}
