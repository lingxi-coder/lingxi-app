//! Mobile data-only WebRTC adapter. One task owns the session and drains its
//! receive queue; control calls never hold a mutex across network awaits.

use crate::peer::{
    PeerAnswer, PeerConfig, PeerConnectionState, PeerConnector, PeerError, PeerEvent, PeerHandle,
    PeerOfferer,
};
use async_trait::async_trait;
use lingxi_webrtc_core::{AnswerOutcome, IceConfig, IceServer, RtcSession, SessionConfig};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use webrtc::peer_connection::RTCPeerConnectionState;

/// Uses the Harness transport without adding an audio/video track.
pub struct NativePeerConnector;

enum Command {
    Offer(oneshot::Sender<Result<String, PeerError>>),
    Answer(String, oneshot::Sender<Result<PeerAnswer, PeerError>>),
    Send(Vec<u8>, oneshot::Sender<Result<(), PeerError>>),
}

struct NativePeer {
    commands: mpsc::Sender<Command>,
    closed: CancellationToken,
}

impl Drop for NativePeer {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}

fn failure(message: &str) -> PeerError {
    PeerError(message.to_owned())
}

#[async_trait]
impl PeerConnector for NativePeerConnector {
    async fn create_offerer(&self, config: PeerConfig) -> Result<PeerOfferer, PeerError> {
        if config.channel_label != "lingxi-agent" {
            return Err(failure("invalid channel label"));
        }
        let mut session_config = SessionConfig::data_only(IceConfig {
            servers: config
                .stun_urls
                .into_iter()
                .map(|url| IceServer {
                    urls: vec![url],
                    ..Default::default()
                })
                .collect(),
            relay_only: false,
        });
        session_config.gather_timeout = Some(config.gather_timeout);
        let session = RtcSession::with_config(session_config)
            .await
            .map_err(failure)?;
        let (commands, requests) = mpsc::channel(32);
        let (events, receiver) = mpsc::unbounded_channel();
        let closed = CancellationToken::new();
        tokio::spawn(run(session, requests, events, closed.clone()));
        Ok(PeerOfferer {
            peer: Arc::new(NativePeer { commands, closed }),
            events: receiver,
        })
    }
}

impl NativePeer {
    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, PeerError>>) -> Command,
    ) -> Result<T, PeerError> {
        let (sender, receiver) = oneshot::channel();
        tokio::select! {
            biased;
            _ = self.closed.cancelled() => Err(failure("peer closed")),
            result = async {
                self.commands.send(make(sender)).await.map_err(|_| failure("peer closed"))?;
                receiver.await.map_err(|_| failure("peer closed"))?
            } => result,
        }
    }
}

#[async_trait]
impl PeerHandle for NativePeer {
    async fn offer_sdp(&self) -> Result<String, PeerError> {
        self.request(Command::Offer).await
    }
    async fn apply_answer(&self, sdp: &str) -> Result<PeerAnswer, PeerError> {
        self.request(|reply| Command::Answer(sdp.to_owned(), reply))
            .await
    }
    async fn send(&self, data: Vec<u8>) -> Result<(), PeerError> {
        if data.len() > 16 * 1024 {
            return Err(failure("oversized data fragment"));
        }
        self.request(|reply| Command::Send(data, reply)).await
    }
    fn close(&self) {
        self.closed.cancel();
    }
}

fn state(value: RTCPeerConnectionState) -> PeerConnectionState {
    match value {
        RTCPeerConnectionState::Unspecified | RTCPeerConnectionState::New => {
            PeerConnectionState::New
        }
        RTCPeerConnectionState::Connecting => PeerConnectionState::Connecting,
        RTCPeerConnectionState::Connected => PeerConnectionState::Connected,
        RTCPeerConnectionState::Disconnected => PeerConnectionState::Disconnected,
        RTCPeerConnectionState::Failed => PeerConnectionState::Failed,
        RTCPeerConnectionState::Closed => PeerConnectionState::Closed,
    }
}

async fn run(
    mut session: RtcSession,
    mut requests: mpsc::Receiver<Command>,
    events: mpsc::UnboundedSender<PeerEvent>,
    closed: CancellationToken,
) {
    let mut ready = session.subscribe_ready();
    let mut states = session.subscribe_connection_state();
    let mut opened = false;
    let mut messages = session
        .take_data_receiver()
        .expect("fresh session receiver");
    // Serial control operations preserve fragment order while the select loop
    // continues draining ingress during gathering, readiness and backpressure.
    let mut operation: Option<Pin<Box<dyn Future<Output = ()> + Send + '_>>> = None;
    let reason = loop {
        // Publish Open before any received bytes, including those queued while
        // apply_answer was waiting for the DataChannel.
        if !opened && *ready.borrow_and_update() {
            opened = true;
            if events.send(PeerEvent::Open).is_err() {
                break "closed";
            }
        }
        tokio::select! {
            biased;
            _ = closed.cancelled() => break "closed",
            _ = events.closed() => break "closed",
            result = ready.changed() => {
                if result.is_err() || (opened && !*ready.borrow()) {
                    break if session.receive_overflowed() { "receive_overflow" } else { "closed" };
                }
            },
            result = states.changed() => {
                if result.is_err() { break "closed"; }
                let current = state(*states.borrow_and_update());
                let terminal = matches!(current, PeerConnectionState::Failed | PeerConnectionState::Closed);
                if events.send(PeerEvent::State(current)).is_err() { break "closed"; }
                if terminal { break "connection_failed"; }
            },
            message = messages.recv() => {
                let Some(message) = message else { break "closed"; };
                if message.is_string { break "protocol_error"; }
                if message.data.len() > 16 * 1024 { break "protocol_error"; }
                if events.send(PeerEvent::Message(message.data.to_vec())).is_err() { break "closed"; }
            },
            _ = async { operation.as_mut().expect("active operation").await }, if operation.is_some() => {
                operation = None;
            },
            command = requests.recv(), if operation.is_none() => {
                let Some(command) = command else { break "closed"; };
                operation = Some(Box::pin(execute(&session, command)));
            },
        }
    };
    drop(operation);
    closed.cancel();
    requests.close();
    let _ = session.close().await;
    let _ = events.send(PeerEvent::Closed(reason.to_owned()));
}

async fn execute(session: &RtcSession, command: Command) {
    match command {
        Command::Offer(reply) => {
            let _ = reply.send(session.offer().await.map_err(failure));
        }
        Command::Answer(sdp, reply) => {
            let result = session
                .apply_answer(sdp)
                .await
                .map(|answer| match answer {
                    AnswerOutcome::Ready => PeerAnswer::Ready,
                    AnswerOutcome::TimedOut => PeerAnswer::TimedOut,
                })
                .map_err(failure);
            let _ = reply.send(result);
        }
        Command::Send(data, reply) => {
            let _ = reply.send(session.send_bytes(&data).await.map_err(failure));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn native_adapter_negotiates_data_only_and_transfers_binary() {
        tokio::time::timeout(Duration::from_secs(25), async {
            let mut viewer = NativePeerConnector
                .create_offerer(PeerConfig {
                    stun_urls: vec![],
                    channel_label: "lingxi-agent".into(),
                    gather_timeout: Duration::from_millis(100),
                })
                .await
                .unwrap();
            let offer = viewer.peer.offer_sdp().await.unwrap();
            assert!(offer.contains("m=application"));
            assert!(!offer.contains("m=audio"));
            assert!(!offer.contains("m=video"));
            let mut config = SessionConfig::data_only(IceConfig::default());
            config.answerer = true;
            config.gather_timeout = Some(Duration::from_millis(100));
            let mut host = RtcSession::with_config(config).await.unwrap();
            let answer = host.answer(offer).await.unwrap();
            assert!(matches!(
                viewer.peer.apply_answer(&answer).await.unwrap(),
                PeerAnswer::Ready
            ));
            let payload = vec![0, 255, 1, 0, 127];
            viewer.peer.send(payload.clone()).await.unwrap();
            assert_eq!(
                host.receive().await.unwrap().data.as_ref(),
                payload.as_slice()
            );
            host.send_bytes(&payload).await.unwrap();
            let mut opened = false;
            loop {
                match viewer.events.recv().await.unwrap() {
                    PeerEvent::Open => opened = true,
                    PeerEvent::Message(data) => {
                        assert!(opened);
                        assert_eq!(data, payload);
                        break;
                    }
                    PeerEvent::Closed(reason) => panic!("unexpected close: {reason}"),
                    _ => {}
                }
            }
            viewer.peer.close();
            viewer.peer.close();
            assert!(viewer.peer.send(vec![1]).await.is_err());
            host.close().await.unwrap();
        })
        .await
        .unwrap();
    }
}
