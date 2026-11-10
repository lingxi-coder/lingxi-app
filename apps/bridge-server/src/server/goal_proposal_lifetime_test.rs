//! Goal proposals remain answerable across the actual bridge turn boundary.
use super::connection_regression_test::{captured_sink, next_frame};
use super::*;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tool_api::ask_user_question::{AskOption, AskQuestion, AskUserQuestionLifetime};

fn exchange(
    lifetime: AskUserQuestionLifetime,
    resp_tx: oneshot::Sender<Vec<(Utf16JsonProjection, Utf16JsonProjection)>>,
) -> AskUserQuestionExchange {
    AskUserQuestionExchange {
        lifetime,
        questions: vec![AskQuestion {
            question: "tests pass".into(),
            header: "Goal".into(),
            options: vec![AskOption::new("Approve", "Set this session goal")],
            multi_select: false,
        }],
        timeout_secs: None,
        resp_tx,
    }
}

#[tokio::test]
async fn goal_proposal_survives_real_turn_cleanup_and_accepts_an_idle_answer() {
    let (_endpoint, mut socket, sink) = captured_sink().await;
    let connection = BridgeConnection::new();
    let broker = Arc::new(AskUserQuestionBroker::new(connection.event_sink()));
    let (tx, rx) = mpsc::channel(2);
    let connection = connection.bind_ask_user_question(broker.clone(), rx);
    assert!(connection.claim_outbound(&sink).await);
    connection.handshaken.store(true, Ordering::SeqCst);
    let (generation, _) = connection.active_turn.begin(Some(100));
    let (goal_tx, mut goal_rx) = oneshot::channel();
    tx.send(exchange(
        AskUserQuestionLifetime::SessionGoalProposal(CancellationToken::new()),
        goal_tx,
    ))
    .await
    .unwrap();
    let Frame::Event(ClientEvent::AskUserQuestion { request }) = next_frame(&mut socket).await
    else {
        panic!("Goal approval must reach the client");
    };
    let request_id = request.request_id;
    let (turn_tx, turn_rx) = oneshot::channel();
    tx.send(exchange(AskUserQuestionLifetime::Turn, turn_tx))
        .await
        .unwrap();
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Event(ClientEvent::AskUserQuestion { .. })
    ));
    finish_turn(
        &connection.active_turn,
        generation,
        &TurnInteractions {
            ask_user_question_broker: Some(broker.clone()),
            ..Default::default()
        },
        &Arc::new(Mutex::new(())),
    )
    .await;
    assert!(turn_rx.await.is_err(), "ordinary turn question must retire");
    assert!(matches!(
        goal_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(broker.pending_count().await, 1);
    assert!(!connection.active_turn.accepts_interactions());
    let answers = serde_json::from_str(r#"{"tests pass":"Approve"}"#).unwrap();
    connection
        .resolve_ask_user_question(request_id, answers)
        .await;
    let answers = goal_rx.await.unwrap();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].1.string_at(""), Some("Approve"));
    assert_eq!(broker.pending_count().await, 0);
}

#[tokio::test]
async fn goal_proposal_can_first_publish_after_turn_end_and_session_cancel_retires_it() {
    let (_endpoint, mut socket, sink) = captured_sink().await;
    let connection = BridgeConnection::new();
    let broker = Arc::new(AskUserQuestionBroker::new(connection.event_sink()));
    let (tx, rx) = mpsc::channel(1);
    let connection = connection.bind_ask_user_question(broker.clone(), rx);
    assert!(connection.claim_outbound(&sink).await);
    connection.handshaken.store(true, Ordering::SeqCst);
    let (generation, _) = connection.active_turn.begin(Some(101));
    finish_turn(
        &connection.active_turn,
        generation,
        &TurnInteractions {
            ask_user_question_broker: Some(broker.clone()),
            ..Default::default()
        },
        &Arc::new(Mutex::new(())),
    )
    .await;
    let cancel = CancellationToken::new();
    let (resp_tx, resp_rx) = oneshot::channel();
    tx.send(exchange(
        AskUserQuestionLifetime::SessionGoalProposal(cancel.clone()),
        resp_tx,
    ))
    .await
    .unwrap();
    let Frame::Event(ClientEvent::AskUserQuestion { request }) = next_frame(&mut socket).await
    else {
        panic!("session Goal dialog must publish while idle");
    };
    assert!(broker.is_live_goal_proposal(request.request_id).await);
    cancel.cancel();
    assert!(tokio::time::timeout(Duration::from_secs(1), resp_rx)
        .await
        .unwrap()
        .is_err());
    assert!(matches!(next_frame(&mut socket).await,
        Frame::Event(ClientEvent::AskUserQuestionResolved { request_id }) if request_id == request.request_id));
    let answers = serde_json::from_str(r#"{"tests pass":"Approve"}"#).unwrap();
    connection
        .resolve_ask_user_question(request.request_id, answers)
        .await;
    assert_eq!(broker.pending_count().await, 0);
}
