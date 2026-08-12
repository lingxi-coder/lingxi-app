//! Connection-scoped `AskUserQuestion` request/response broker.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use client_protocol::ask_user_question::{AskOptionDto, AskQuestionDto, AskUserQuestionRequestDto};
use client_protocol::events::ClientEvent;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tui_core::ask_user_question_bridge::{AskOption, AskQuestion, AskUserQuestionExchange};

use crate::ClientEventSink;

const MAX_PENDING_ASK_USER_QUESTIONS: usize = 64;

struct PendingAskUserQuestion {
    resp_tx: oneshot::Sender<HashMap<String, String>>,
    timeout_task: Option<JoinHandle<()>>,
    /// The emitted request, retained verbatim so a reconnecting client can
    /// have every still-parked question replayed with its ORIGINAL (stable)
    /// `request_id` — the tool side keeps waiting on the same oneshot across
    /// the disconnect, so the correlator must not change either.
    request: AskUserQuestionRequestDto,
}

fn lower_option(option: AskOption) -> AskOptionDto {
    AskOptionDto {
        label: option.label,
        description: option.description,
        preview: option.preview,
    }
}

fn lower_question(question: AskQuestion) -> AskQuestionDto {
    AskQuestionDto {
        question: question.question,
        header: question.header,
        options: question.options.into_iter().map(lower_option).collect(),
        multi_select: question.multi_select,
    }
}

/// Bridges the tool's session-scoped questionnaire channel to a client event
/// stream and correlates inbound answers back to the parked tool call.
///
/// Ids are connection-scoped `u64`s, but STABLE for the lifetime of the
/// parked tool call — [`Self::replay_pending`] re-announces a still-parked
/// request with its original id after a reconnect, and the tool side keeps
/// waiting on the same oneshot across the disconnect. Nothing here needs to
/// be durable: the parked turn dies with the process.
pub struct BridgeAskUserQuestionBroker {
    sink: Arc<dyn ClientEventSink>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, PendingAskUserQuestion>>>,
}

impl BridgeAskUserQuestionBroker {
    /// Construct a connection-scoped broker.
    #[must_use]
    pub fn new(sink: Arc<dyn ClientEventSink>) -> Self {
        Self {
            sink,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Drain the tool-side exchange channel until its sender is dropped.
    pub async fn run(&self, mut rx: mpsc::Receiver<AskUserQuestionExchange>) {
        while let Some(exchange) = rx.recv().await {
            let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let AskUserQuestionExchange {
                questions,
                timeout_secs,
                resp_tx,
            } = exchange;
            let request = AskUserQuestionRequestDto {
                request_id,
                questions: questions.into_iter().map(lower_question).collect(),
                timeout_secs,
            };
            {
                let mut pending = self.pending.lock().await;
                if pending.len() >= MAX_PENDING_ASK_USER_QUESTIONS {
                    tracing::warn!(
                        limit = MAX_PENDING_ASK_USER_QUESTIONS,
                        "dropping AskUserQuestion request because the pending limit was reached"
                    );
                    continue;
                }
                pending.insert(
                    request_id,
                    PendingAskUserQuestion {
                        resp_tx,
                        timeout_task: None,
                        request: request.clone(),
                    },
                );
            }
            self.sink
                .emit(ClientEvent::AskUserQuestion { request })
                .await;
            if let Some(timeout_secs) = timeout_secs {
                let pending = Arc::clone(&self.pending);
                let sink = Arc::clone(&self.sink);
                let timeout_task = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
                    let Some(entry) = pending.lock().await.remove(&request_id) else {
                        return;
                    };
                    // The client supplied no confirmed answers. Preserve that
                    // fact instead of converting the highlighted first row into
                    // a user decision.
                    let _ = entry.resp_tx.send(HashMap::new());
                    sink.emit(ClientEvent::AskUserQuestionResolved { request_id })
                        .await;
                });
                let mut pending = self.pending.lock().await;
                if let Some(entry) = pending.get_mut(&request_id) {
                    entry.timeout_task = Some(timeout_task);
                } else {
                    timeout_task.abort();
                }
            }
        }
    }

    /// Resolve one parked questionnaire. Unknown/already-resolved ids are safe
    /// no-ops and return `false`.
    pub async fn resolve(&self, request_id: u64, answers: HashMap<String, String>) -> bool {
        let Some(entry) = self.pending.lock().await.remove(&request_id) else {
            return false;
        };
        if let Some(task) = entry.timeout_task {
            task.abort();
        }
        let resolved = entry.resp_tx.send(answers).is_ok();
        self.sink
            .emit(ClientEvent::AskUserQuestionResolved { request_id })
            .await;
        resolved
    }

    /// Cancel one parked questionnaire by dropping its response sender.
    pub async fn cancel(&self, request_id: u64) -> bool {
        let Some(entry) = self.pending.lock().await.remove(&request_id) else {
            return false;
        };
        if let Some(task) = entry.timeout_task {
            task.abort();
        }
        drop(entry.resp_tx);
        self.sink
            .emit(ClientEvent::AskUserQuestionResolved { request_id })
            .await;
        true
    }

    /// Re-emit every still-parked request with its ORIGINAL id — the
    /// reconnect path. A mobile client that backgrounded (dropping the event
    /// stream while the engine process stayed alive) re-renders its pending
    /// question cards from this replay, the same shape as the app side's
    /// `resync_pending_gates`. Safe to call repeatedly; order is unspecified
    /// (at most `MAX_PENDING_ASK_USER_QUESTIONS` live at once, and each id
    /// appears at most once).
    pub async fn replay_pending(&self) -> usize {
        let requests: Vec<AskUserQuestionRequestDto> = {
            let pending = self.pending.lock().await;
            pending
                .values()
                .map(|entry| entry.request.clone())
                .collect()
        };
        let count = requests.len();
        for request in requests {
            self.sink
                .emit(ClientEvent::AskUserQuestion { request })
                .await;
        }
        count
    }

    /// Fail closed on disconnect/session teardown.
    pub async fn drain(&self) -> usize {
        let drained = {
            let mut pending = self.pending.lock().await;
            pending.drain().collect::<Vec<_>>()
        };
        let count = drained.len();
        for (request_id, entry) in drained {
            if let Some(task) = entry.timeout_task {
                task.abort();
            }
            // Drop the response sender before notifying the client so the
            // blocked tool is already guaranteed to unwind when the UI clears.
            drop(entry.resp_tx);
            self.sink
                .emit(ClientEvent::AskUserQuestionResolved { request_id })
                .await;
        }
        count
    }

    /// Number of currently parked requests.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockSink;
    use tui_core::ask_user_question_bridge::{AskOption, AskQuestion};

    fn an_exchange(resp_tx: oneshot::Sender<HashMap<String, String>>) -> AskUserQuestionExchange {
        AskUserQuestionExchange {
            questions: vec![AskQuestion {
                question: "Choose?".into(),
                header: "Choice".into(),
                options: vec![AskOption::new("A", "first")],
                multi_select: false,
            }],
            timeout_secs: None,
            resp_tx,
        }
    }

    async fn first_request_id(sink: &Arc<MockSink>) -> u64 {
        loop {
            if let Some(ClientEvent::AskUserQuestion { request }) =
                sink.events().await.into_iter().next()
            {
                break request.request_id;
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn emits_and_resolves_correlated_questionnaire() {
        let sink = MockSink::arc();
        let broker = Arc::new(BridgeAskUserQuestionBroker::new(sink.clone()));
        let (tx, rx) = mpsc::channel(1);
        let runner = broker.clone();
        let task = tokio::spawn(async move { runner.run(rx).await });
        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(an_exchange(resp_tx)).await.unwrap();

        let request_id = first_request_id(&sink).await;
        assert!(
            broker
                .resolve(request_id, HashMap::from([("Choose?".into(), "A".into())]))
                .await
        );
        assert_eq!(resp_rx.await.unwrap()["Choose?"], "A");
        assert!(sink.events().await.into_iter().any(|event| matches!(
            event,
            ClientEvent::AskUserQuestionResolved { request_id: id } if id == request_id
        )));
        drop(tx);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn replay_re_emits_parked_requests_with_the_same_id() {
        let sink = MockSink::arc();
        let broker = Arc::new(BridgeAskUserQuestionBroker::new(sink.clone()));
        let (tx, rx) = mpsc::channel(1);
        let runner = broker.clone();
        let task = tokio::spawn(async move { runner.run(rx).await });
        let (resp_tx, _resp_rx) = oneshot::channel();
        tx.send(an_exchange(resp_tx)).await.unwrap();

        let original_id = first_request_id(&sink).await;
        assert_eq!(broker.replay_pending().await, 1);
        let replayed: Vec<u64> = sink
            .events()
            .await
            .into_iter()
            .filter_map(|event| match event {
                ClientEvent::AskUserQuestion { request } => Some(request.request_id),
                _ => None,
            })
            .collect();
        assert_eq!(
            replayed,
            vec![original_id, original_id],
            "the replay must reuse the original correlator, not mint a new one"
        );
        // The replayed request is still resolvable exactly once.
        assert!(broker.resolve(original_id, HashMap::new()).await);
        assert!(!broker.resolve(original_id, HashMap::new()).await);
        drop(tx);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn drain_drops_parked_reply() {
        let sink = MockSink::arc();
        let broker = Arc::new(BridgeAskUserQuestionBroker::new(sink.clone()));
        let (tx, rx) = mpsc::channel(1);
        let runner = broker.clone();
        tokio::spawn(async move { runner.run(rx).await });
        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(AskUserQuestionExchange {
            questions: Vec::new(),
            timeout_secs: None,
            resp_tx,
        })
        .await
        .unwrap();
        while broker.pending_count().await == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(broker.drain().await, 1);
        assert!(resp_rx.await.is_err());
        assert!(sink.events().await.into_iter().any(|event| matches!(
            event,
            ClientEvent::AskUserQuestionResolved { request_id: 1 }
        )));
    }

    #[tokio::test]
    async fn timeout_resolves_with_no_fabricated_client_answer() {
        let sink = MockSink::arc();
        let broker = Arc::new(BridgeAskUserQuestionBroker::new(sink.clone()));
        let (tx, rx) = mpsc::channel(1);
        let runner = broker.clone();
        tokio::spawn(async move { runner.run(rx).await });
        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(AskUserQuestionExchange {
            questions: vec![AskQuestion {
                question: "Choose?".into(),
                header: "Choice".into(),
                options: vec![AskOption::new("A", "first"), AskOption::new("B", "second")],
                multi_select: false,
            }],
            timeout_secs: Some(0),
            resp_tx,
        })
        .await
        .unwrap();

        let answers = resp_rx.await.unwrap();
        assert!(answers.is_empty());
        assert_eq!(broker.pending_count().await, 0);
        let events = sink.events().await;
        assert!(matches!(
            events.as_slice(),
            [
                ClientEvent::AskUserQuestion { .. },
                ClientEvent::AskUserQuestionResolved { .. }
            ]
        ));
    }

    #[tokio::test]
    async fn resolve_of_unknown_id_is_a_noop() {
        let sink = MockSink::arc();
        let broker = BridgeAskUserQuestionBroker::new(sink.clone());
        assert!(!broker.resolve(999, HashMap::new()).await);
        assert!(!broker.cancel(999).await);
        assert_eq!(broker.replay_pending().await, 0);
    }
}
