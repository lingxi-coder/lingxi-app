use super::*;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Sink(mpsc::UnboundedSender<ClientEvent>);
#[async_trait]
impl AudioRequestSink for Sink {
    async fn emit_request(&self, event: ClientEvent) -> bool {
        self.0.send(event).is_ok()
    }
}
fn fixture() -> (
    Arc<DecisionBridge>,
    DecisionResponder,
    mpsc::UnboundedReceiver<ClientEvent>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (service, responder) = new_decision_bridge(Arc::new(Sink(tx)));
    (service, responder, rx)
}
fn input() -> Input {
    serde_json::from_value(json!({"state":"evidence","questions":{"ok":{"type":"binary","instructions":"Is it valid?"}}})).unwrap()
}
fn context(cancellation: CancellationToken) -> InvocationContext {
    InvocationContext {
        timeout: Duration::from_secs(30),
        cancellation,
    }
}
fn reply() -> String {
    json!({"result":{"state_revision":0,"answers":{"ok":{"type":"binary","probability":0.9}},"usage":{"input_tokens":null,"output_tokens":null},"actual_model":null,"elapsed_ms":1}}).to_string()
}

#[tokio::test]
async fn unconfigured_calls_do_not_emit_requests_and_configuration_is_live() {
    let (service, responder, mut events) = fixture();
    assert!(!service.is_configured());
    assert_eq!(
        service
            .evaluate(input(), context(CancellationToken::new()))
            .await
            .unwrap_err(),
        Error::Unavailable
    );
    assert!(events.try_recv().is_err());
    responder.configure(true);
    assert!(service.is_configured());
    responder.configure(false);
    assert!(!service.is_configured());
}
#[tokio::test]
async fn only_the_matching_connection_response_resolves_the_foreground_call() {
    let (service, responder, mut events) = fixture();
    responder.configure(true);
    let operation = tokio::spawn(async move {
        service
            .evaluate(input(), context(CancellationToken::new()))
            .await
    });
    let ClientEvent::DecisionRequest {
        request_id,
        input_json,
        ..
    } = events.recv().await.unwrap()
    else {
        panic!("request")
    };
    let submitted: serde_json::Value = serde_json::from_str(&input_json).unwrap();
    assert!(submitted.get("model").is_none());
    assert!(submitted.get("credential").is_none());
    let (_, other, _) = fixture();
    assert!(!other.resolve(&request_id, &reply()));
    assert!(responder.resolve(&request_id, &reply()));
    assert!(!responder.resolve(&request_id, &reply()));
    assert!(operation.await.unwrap().is_ok());
}
#[tokio::test]
async fn dropping_the_tool_future_emits_targeted_cancel_and_fences_late_reply() {
    let (service, responder, mut events) = fixture();
    responder.configure(true);
    let operation = tokio::spawn(async move {
        service
            .evaluate(input(), context(CancellationToken::new()))
            .await
    });
    let ClientEvent::DecisionRequest { request_id, .. } = events.recv().await.unwrap() else {
        panic!("request")
    };
    operation.abort();
    let _ = operation.await;
    let ClientEvent::DecisionCancel {
        request_id: cancelled,
    } = events.recv().await.unwrap()
    else {
        panic!("cancel")
    };
    assert_eq!(request_id, cancelled);
    assert!(!responder.resolve(&request_id, &reply()));
}
#[tokio::test]
async fn cancellation_and_disconnect_stop_waiters_without_replay() {
    let (service, responder, mut events) = fixture();
    responder.configure(true);
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let caller = service.clone();
    let operation = tokio::spawn(async move { caller.evaluate(input(), context(token)).await });
    let ClientEvent::DecisionRequest { request_id, .. } = events.recv().await.unwrap() else {
        panic!("request")
    };
    cancellation.cancel();
    assert_eq!(operation.await.unwrap().unwrap_err(), Error::Cancelled);
    assert!(
        matches!(events.recv().await,Some(ClientEvent::DecisionCancel{request_id:id}) if id==request_id)
    );
    let operation = tokio::spawn(async move {
        service
            .evaluate(input(), context(CancellationToken::new()))
            .await
    });
    assert!(matches!(
        events.recv().await,
        Some(ClientEvent::DecisionRequest { .. })
    ));
    responder.drain();
    assert_eq!(operation.await.unwrap().unwrap_err(), Error::Unavailable);
}
