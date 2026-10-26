//! Trace context follows input ownership, including spawned and queued turns.
use super::connection_regression_test::{captured_sink, next_frame};
use super::*;
use std::time::Duration;
use telemetry::otel::{capture_current_trace_context, SerializedTraceContext};
use tokio::sync::{mpsc, Notify};

struct TraceDriver {
    observations: mpsc::UnboundedSender<(String, Option<SerializedTraceContext>)>,
    release_first: Arc<Notify>,
}

#[async_trait]
impl TurnDriver for TraceDriver {
    async fn run_turn(&self, prompt: Utf16JsonProjection) {
        tokio::task::yield_now().await;
        let text = prompt.string_at("").unwrap_or_default().to_string();
        self.observations
            .send((text.clone(), capture_current_trace_context()))
            .unwrap();
        if text == "first" {
            self.release_first.notified().await;
        }
    }

    async fn run_queued_batch(
        &self,
        inputs: Vec<orchestrator::QueuedPromptInput>,
        _cancel: CancellationToken,
    ) {
        let prompt = join_source_text(
            &inputs
                .into_iter()
                .map(|input| input.text)
                .collect::<Vec<_>>(),
        );
        self.run_turn(prompt).await;
    }
}

fn context(byte: char) -> SerializedTraceContext {
    SerializedTraceContext {
        traceparent: format!(
            "00-{}-{}-01",
            byte.to_string().repeat(32),
            byte.to_string().repeat(16)
        ),
        tracestate: None,
    }
}

fn prompt(id: u64, text: &str, trace_context: Option<SerializedTraceContext>) -> Frame {
    Frame::Request(BridgeRequest {
        id,
        method: "command".into(),
        params: serialize_source(&ClientCommand::SendPrompt {
            text: text.into(),
            images: Vec::new(),
            turn_id: Some(id),
            prompt_mode: None,
            visualization_context: None,
        })
        .unwrap(),
        trace_context,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telemetry_parent_survives_spawn_and_queued_turns_do_not_inherit_seed() {
    let (endpoint, _socket, sink) = captured_sink().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let mut connection = BridgeConnection::new();
    connection.driver = Some(Arc::new(TraceDriver {
        observations: tx,
        release_first: release.clone(),
    }));
    connection.handshaken.store(true, Ordering::SeqCst);
    let first = context('1');
    let second = context('2');
    connection
        .on_frame(prompt(1, "first", Some(first.clone())), sink.clone())
        .await;
    let observed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed, ("first".into(), Some(first)));
    assert!(
        capture_current_trace_context().is_none(),
        "request parent is scoped to polling"
    );

    connection
        .on_frame(prompt(2, "second", Some(second.clone())), sink.clone())
        .await;
    connection.on_frame(prompt(3, "third", None), sink).await;
    release.notify_one();
    let observed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed, ("second".into(), Some(second)));
    let observed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed, ("third".into(), None));
    let task = connection.active_turn_task.lock().unwrap().take().unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(connection.queued_prompt_payloads.lock().await.is_empty());
    connection.close_connection(None).await;
    endpoint.shutdown().await;
}

#[tokio::test]
async fn telemetry_metadata_preserves_text_batching_and_is_retired_with_input() {
    let queue = Arc::new(MessageQueueManager::new());
    let first = prompt_command("one".into());
    let second = prompt_command("two".into());
    let original_priority = first.priority;
    let payloads = Arc::new(Mutex::new(HashMap::from([
        (
            first.uuid.clone(),
            QueuedPromptPayload {
                images: Vec::new(),
                turn_id: None,
                prompt_inputs: None,
                trace_context: Some(context('3')),
            },
        ),
        (
            "already-consumed".into(),
            QueuedPromptPayload {
                images: Vec::new(),
                turn_id: None,
                prompt_inputs: None,
                trace_context: Some(context('4')),
            },
        ),
    ])));
    queue.enqueue(first).await;
    queue.enqueue(second).await;
    let handoff = Arc::new(Mutex::new(()));
    let batch = batchable_prompt_snapshot(&queue, &payloads, &handoff).await;
    assert_eq!(
        batch.len(),
        2,
        "trace-only metadata must not split a text batch"
    );
    assert_eq!(batch[0].priority, original_priority);
    assert!(!payloads.lock().await.contains_key("already-consumed"));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let driver: Arc<dyn TurnDriver> = Arc::new(TraceDriver {
        observations: tx,
        release_first: Arc::new(Notify::new()),
    });
    drain_main_thread(
        &driver,
        &queue,
        &Arc::new(tool_cron::LoopRuntime::default()),
        &ActiveTurnControl::default(),
        &TurnInteractions::default(),
        &payloads,
        &handoff,
        None,
    )
    .await;
    let (_, observed) = rx.recv().await.unwrap();
    assert_eq!(observed, Some(context('3')));
    assert!(payloads.lock().await.is_empty());
    assert!(capture_current_trace_context().is_none());
}

#[tokio::test]
async fn telemetry_disable_requires_handshake_and_does_not_cancel_chat() {
    let (endpoint, mut socket, sink) = captured_sink().await;
    let connection = BridgeConnection::new();
    let request = |id, params| {
        Frame::Request(BridgeRequest {
            id,
            method: "telemetry_disable".into(),
            params: Utf16JsonProjection::parse(params).unwrap(),
            trace_context: None,
        })
    };
    assert!(!connection.is_priority_frame(&request(1, "{}")));
    connection.on_frame(request(1, "{}"), sink.clone()).await;
    let Frame::Response(response) = next_frame(&mut socket).await else {
        panic!("expected refusal")
    };
    assert!(response.error.is_some());

    connection.handshaken.store(true, Ordering::SeqCst);
    assert!(connection.is_priority_frame(&request(2, "{}")));
    connection
        .on_frame(request(2, r#"{"enabled":true}"#), sink.clone())
        .await;
    let Frame::Response(response) = next_frame(&mut socket).await else {
        panic!("expected refusal")
    };
    assert!(response.error.is_some());

    let (_, cancel) = connection.active_turn.begin(Some(77));
    connection.on_frame(request(3, "{}"), sink).await;
    let Frame::Response(response) = next_frame(&mut socket).await else {
        panic!("expected disable acknowledgement")
    };
    assert_eq!(response.result.unwrap().string_at("/status"), Some("ok"));
    assert!(!cancel.is_cancelled());
    connection.close_connection(None).await;
    endpoint.shutdown().await;
}
