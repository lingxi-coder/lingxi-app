//! A fake desktop, built from the crate's own primitives, standing on the host side
//! of a `MemoryRtdb`: it verifies offers, signs answers, derives tunnel keys, speaks
//! the handshake and lets a test play the user at the pairing dialog.

use async_trait::async_trait;
use remote_control_client::clock::Clock;
use remote_control_client::encoding::{b64u_decode, b64u_encode, random_bytes};
use remote_control_client::keys::{EphemeralKey, IdentityKey};
use remote_control_client::messages::{methods, InitializeResult, RemoteHostInfo};
use remote_control_client::pairing::pairing_code_for;
use remote_control_client::rpc::{RemoteErrorCode, RequestHandler, RpcError, RpcOptions, RpcPeer};
use remote_control_client::rtdb::{ChangeKind, ListenItem, Rtdb, RtdbMirror};
use remote_control_client::signaling::{
    answer_path, down_path, host_path, parse_frame_key, sign_answer, to_host_path, up_path,
    verify_offer, AnswerDraft, AnswerHost, AnswerRejectReason, AnswerStatus, OfferRecord,
    SigningContext, TransportKind,
};
use remote_control_client::testing::FakePeerNetwork;
use remote_control_client::transport::Link;
use remote_control_client::tunnel::{TunnelLink, TunnelLinkOptions};
use remote_control_client::tunnel_crypto::{derive_tunnel_keys, TunnelDir};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// What the fake desktop does.
#[derive(Clone)]
pub struct HostConfig {
    /// Write answers at all (off: the viewer times out).
    pub answer: bool,
    /// Refuse every offer with this reason.
    pub reject: Option<AnswerRejectReason>,
    /// Answer a `webrtc` offer with a `tunnel` answer on the same connection (the downgrade).
    pub downgrade: bool,
    /// Compute a different pairing code for `pending_approval`.
    pub wrong_pairing_code: bool,
    /// Sign answers with this key instead of the host key.
    pub sign_with: Option<Arc<IdentityKey>>,
    /// Fail `initialize` with this error.
    pub initialize_error: Option<RemoteErrorCode>,
    /// Never answer `initialize`.
    pub initialize_hangs: bool,
    /// Simulate approval arriving before the caller consumes the initialize reply.
    pub approve_before_initialize_reply: bool,
    /// Answers for other methods; `None` falls back to `{}`.
    pub respond: Option<Arc<dyn Fn(&str, &Value) -> Option<Result<Value, RpcError>> + Send + Sync>>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            answer: true,
            reject: None,
            downgrade: false,
            wrong_pairing_code: false,
            sign_with: None,
            initialize_error: None,
            initialize_hangs: false,
            approve_before_initialize_reply: false,
            respond: None,
        }
    }
}

/// One connection as the host sees it.
#[derive(Clone)]
pub struct HostConn {
    pub conn_id: String,
    pub transport: TransportKind,
    pub device_id: String,
    pub rpc: Option<RpcPeer>,
    pub pending: bool,
    pub initialized: bool,
}

struct Shared {
    config: Mutex<HostConfig>,
    known: Mutex<HashSet<String>>,
    conns: Mutex<Vec<HostConn>>,
    requests: Mutex<Vec<(String, String, Value)>>,
    tunnels: Mutex<HashMap<String, TunnelLink>>,
    offers: Mutex<Vec<String>>,
}

pub struct FakeHost {
    pub uid: String,
    pub host_id: String,
    pub key: Arc<IdentityKey>,
    pub network: FakePeerNetwork,
    rtdb: Arc<dyn Rtdb>,
    clock: Arc<dyn Clock>,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct HostHandler {
    shared: Arc<Shared>,
    conn_id: String,
    host_key: String,
    offer: OfferRecord,
    answer_nonce: String,
}

#[async_trait]
impl RequestHandler for HostHandler {
    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.shared.requests.lock().unwrap().push((
            self.conn_id.clone(),
            method.to_owned(),
            params.clone(),
        ));
        let config = self.shared.config.lock().unwrap().clone();
        match method {
            methods::INITIALIZE => {
                if config.initialize_hangs {
                    std::future::pending::<()>().await;
                }
                if let Some(code) = config.initialize_error {
                    return Err(RpcError::code(code));
                }
                if params.get("protocol").and_then(Value::as_u64) != Some(1) {
                    return Err(RpcError::code(RemoteErrorCode::UpgradeRequired));
                }
                let device_id = self.offer.draft.device.id.clone();
                let known = self.shared.known.lock().unwrap().contains(&device_id);
                let result = if known {
                    InitializeResult::Approved { host: host_info() }
                } else {
                    let mut code = pairing_code_for(
                        &self.offer.draft.device.key,
                        &self.offer.draft.nonce,
                        &self.host_key,
                        &self.answer_nonce,
                    )
                    .unwrap();
                    if config.wrong_pairing_code {
                        code = if code == "000000" {
                            "000001".into()
                        } else {
                            "000000".into()
                        };
                    }
                    InitializeResult::PendingApproval { pairing_code: code }
                };
                if !known && config.approve_before_initialize_reply {
                    let rpc = self
                        .shared
                        .conns
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|conn| conn.conn_id == self.conn_id)
                        .and_then(|conn| conn.rpc.clone())
                        .unwrap();
                    self.shared.known.lock().unwrap().insert(device_id);
                    rpc.notify(methods::AUTH_APPROVED, Some(json!({ "host": host_info() })))
                        .await
                        .unwrap();
                }
                let mut conns = self.shared.conns.lock().unwrap();
                if let Some(conn) = conns.iter_mut().find(|c| c.conn_id == self.conn_id) {
                    conn.initialized = true;
                    conn.pending = !known;
                }
                Ok(serde_json::to_value(result).unwrap())
            }
            _ => {
                let pending = self
                    .shared
                    .conns
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|c| c.conn_id == self.conn_id && c.pending);
                if pending {
                    return Err(RpcError::code(RemoteErrorCode::NotApproved));
                }
                if let Some(respond) = &config.respond {
                    if let Some(answer) = respond(method, &params) {
                        return answer;
                    }
                }
                Ok(json!({}))
            }
        }
    }
}

pub fn host_info() -> RemoteHostInfo {
    RemoteHostInfo {
        name: "Studio Mac".into(),
        platform: "macos".into(),
        app_version: "1.2.3".into(),
    }
}

impl FakeHost {
    pub fn start(
        rtdb: Arc<dyn Rtdb>,
        clock: Arc<dyn Clock>,
        network: FakePeerNetwork,
        uid: &str,
        key: IdentityKey,
    ) -> Arc<Self> {
        let key = Arc::new(key);
        let host_id = key.id();
        let shared = Arc::new(Shared {
            config: Mutex::new(HostConfig::default()),
            known: Mutex::default(),
            conns: Mutex::default(),
            requests: Mutex::default(),
            tunnels: Mutex::default(),
            offers: Mutex::default(),
        });
        let mut listener = rtdb.listen(&to_host_path(uid, &host_id).unwrap());
        let host = Arc::new_cyclic(|weak: &std::sync::Weak<FakeHost>| {
            let weak = weak.clone();
            let shared_for_task = Arc::clone(&shared);
            let task = tokio::spawn(async move {
                let mut mirror = RtdbMirror::new();
                while let Some(item) = listener.recv().await {
                    let ListenItem::Event(event) = item else {
                        return;
                    };
                    let changes = mirror.apply(&event);
                    for change in changes.at(2) {
                        if change.path[1] == "offer" && change.kind != ChangeKind::Removed {
                            let Some(host) = weak.upgrade() else { return };
                            let conn_id = change.path[0].clone();
                            let seen = {
                                let mut offers = host.shared.offers.lock().unwrap();
                                if offers.contains(&conn_id) {
                                    true
                                } else {
                                    offers.push(conn_id.clone());
                                    false
                                }
                            };
                            if !seen {
                                let value = change.value.clone();
                                tokio::spawn(
                                    async move { host.handle_offer(&conn_id, &value).await },
                                );
                            }
                        }
                    }
                    for change in changes.at(3) {
                        let [conn, side, frame] = change.path.as_slice() else {
                            continue;
                        };
                        if side != "up" || change.kind == ChangeKind::Removed {
                            continue;
                        }
                        let link = shared_for_task.tunnels.lock().unwrap().get(conn).cloned();
                        if let (Some(seq), Some(link)) = (parse_frame_key(frame), link) {
                            link.deliver_frame(seq, &change.value);
                        }
                    }
                }
            });
            FakeHost {
                uid: uid.to_owned(),
                host_id,
                key,
                network,
                rtdb,
                clock,
                shared,
                task,
            }
        });
        host
    }

    pub fn host_key_b64u(&self) -> String {
        self.key.public_key_b64u()
    }

    /// Write `hosts/{host_id}` the way a running desktop does.
    pub async fn publish_presence(&self) {
        let now = self.clock.now_ms();
        let record = json!({
            "v": 1,
            "name": "Studio Mac",
            "platform": "macos",
            "app_version": "1.2.3",
            "client_protocol_version": remote_control_client::version::client_protocol_major(),
            "host_key": self.host_key_b64u(),
            "online": true,
            "last_seen": now,
            "started_at": now,
        });
        self.rtdb
            .put(&host_path(&self.uid, &self.host_id).unwrap(), &record)
            .await
            .unwrap();
    }

    pub fn configure(&self, change: impl FnOnce(&mut HostConfig)) {
        change(&mut self.shared.config.lock().unwrap());
    }

    /// The user already approved this device (it is "paired").
    pub fn know_device(&self, device_id: &str) {
        self.shared
            .known
            .lock()
            .unwrap()
            .insert(device_id.to_owned());
    }

    pub fn forget_device(&self, device_id: &str) {
        self.shared.known.lock().unwrap().remove(device_id);
    }

    pub fn conns(&self) -> Vec<HostConn> {
        self.shared.conns.lock().unwrap().clone()
    }

    pub fn offers_seen(&self) -> Vec<String> {
        self.shared.offers.lock().unwrap().clone()
    }

    /// Every request the host received: (conn_id, method, params).
    pub fn requests(&self) -> Vec<(String, String, Value)> {
        self.shared.requests.lock().unwrap().clone()
    }

    fn rpc_of(&self, conn_id: &str) -> RpcPeer {
        self.conns()
            .into_iter()
            .find(|c| c.conn_id == conn_id)
            .and_then(|c| c.rpc)
            .expect("connection with an rpc peer")
    }

    /// The user taps "Allow" in the desktop dialog.
    pub async fn approve(&self, conn_id: &str) {
        let device_id = {
            let mut conns = self.shared.conns.lock().unwrap();
            let conn = conns
                .iter_mut()
                .find(|c| c.conn_id == conn_id)
                .expect("conn");
            conn.pending = false;
            conn.device_id.clone()
        };
        self.know_device(&device_id);
        self.rpc_of(conn_id)
            .notify(methods::AUTH_APPROVED, Some(json!({ "host": host_info() })))
            .await
            .unwrap();
    }

    /// The user taps "Deny": `auth.denied`, then the host closes the connection.
    pub async fn deny(&self, conn_id: &str, reason: &str) {
        let rpc = self.rpc_of(conn_id);
        rpc.notify(methods::AUTH_DENIED, Some(json!({ "reason": reason })))
            .await
            .unwrap();
        rpc.close("denied");
    }

    pub async fn notify(&self, conn_id: &str, method: &str, params: Value) {
        self.rpc_of(conn_id)
            .notify(method, Some(params))
            .await
            .unwrap();
    }

    pub fn drop_connection(&self, conn_id: &str, reason: &str) {
        self.rpc_of(conn_id).close(reason);
    }

    async fn handle_offer(self: Arc<Self>, conn_id: &str, value: &Value) {
        let config = self.shared.config.lock().unwrap().clone();
        let ctx = SigningContext {
            uid: self.uid.clone(),
            host_id: self.host_id.clone(),
            conn_id: conn_id.to_owned(),
        };
        let Ok(offer) = verify_offer(&ctx, value, self.clock.now_ms(), None) else {
            return;
        };
        if !config.answer {
            return;
        }
        let answer_nonce = b64u_encode(&random_bytes::<16>());
        let host = AnswerHost {
            id: self.host_id.clone(),
            key: self.host_key_b64u(),
        };
        let signer: Arc<IdentityKey> = config
            .sign_with
            .clone()
            .unwrap_or_else(|| Arc::clone(&self.key));
        let mut link: Option<Link> = None;
        let mut tunnel: Option<(TunnelLink, tokio::sync::mpsc::UnboundedReceiver<_>)> = None;
        let draft = if let Some(reason) = config.reject {
            AnswerDraft {
                v: 1,
                status: AnswerStatus::Rejected,
                reason: Some(reason),
                transport: None,
                sdp: None,
                host,
                eph: None,
                nonce: answer_nonce.clone(),
            }
        } else if offer.draft.transport == TransportKind::Webrtc && !config.downgrade {
            let (sdp, host_link) = self
                .network
                .accept_offer(offer.draft.sdp.as_deref().unwrap())
                .expect("fake peer");
            link = Some(host_link);
            AnswerDraft {
                v: 1,
                status: AnswerStatus::Accepted,
                reason: None,
                transport: Some(TransportKind::Webrtc),
                sdp: Some(sdp),
                host,
                eph: None,
                nonce: answer_nonce.clone(),
            }
        } else {
            let eph = EphemeralKey::generate();
            let public = b64u_encode(&eph.public_key());
            let shared_secret = eph.agree(&b64u_decode(&offer.draft.eph).unwrap()).unwrap();
            let keys = derive_tunnel_keys(shared_secret.as_bytes(), conn_id).unwrap();
            let (tl, events) = TunnelLink::new(TunnelLinkOptions {
                rtdb: Arc::clone(&self.rtdb),
                conn_id: conn_id.to_owned(),
                send_path: down_path(&self.uid, &self.host_id, conn_id).unwrap(),
                send_dir: TunnelDir::Down,
                send_key: keys.down,
                recv_path: up_path(&self.uid, &self.host_id, conn_id).unwrap(),
                recv_dir: TunnelDir::Up,
                recv_key: keys.up,
                subscribe: false,
            });
            self.shared
                .tunnels
                .lock()
                .unwrap()
                .insert(conn_id.to_owned(), tl.clone());
            tunnel = Some((tl, events));
            AnswerDraft {
                v: 1,
                status: AnswerStatus::Accepted,
                reason: None,
                transport: Some(TransportKind::Tunnel),
                sdp: None,
                host,
                eph: Some(public),
                nonce: answer_nonce.clone(),
            }
        };

        let accepted = draft.status == AnswerStatus::Accepted;
        let chosen = draft.transport;
        if accepted {
            let host_link = match (link, tunnel) {
                (Some(link), _) => link,
                (None, Some((tl, events))) => tl.into_link(events),
                _ => unreachable!(),
            };
            let handler = Arc::new(HostHandler {
                shared: Arc::clone(&self.shared),
                conn_id: conn_id.to_owned(),
                host_key: self.host_key_b64u(),
                offer: offer.clone(),
                answer_nonce: answer_nonce.clone(),
            });
            let rpc = RpcPeer::start(
                host_link,
                RpcOptions {
                    timeout: None,
                    requests: Some(handler),
                    notifications: None,
                },
            );
            self.shared.conns.lock().unwrap().push(HostConn {
                conn_id: conn_id.to_owned(),
                transport: chosen.unwrap(),
                device_id: offer.draft.device.id.clone(),
                rpc: Some(rpc),
                pending: false,
                initialized: false,
            });
        }
        let signed = sign_answer(&ctx, signer.as_ref(), &offer.draft.nonce, draft)
            .await
            .unwrap();
        self.rtdb
            .put(
                &answer_path(&self.uid, &self.host_id, conn_id).unwrap(),
                &signed.to_value(),
            )
            .await
            .unwrap();
    }
}
