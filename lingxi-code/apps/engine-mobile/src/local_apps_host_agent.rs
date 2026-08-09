//! The `agent.post` operation of the `window.lingxi.v1` bridge.
//!
//! An app hands the conversation a small structured event; the assistant
//! collects it later through the MCP `read_app_events` tool. The client-side
//! event this raises deliberately carries NO body — a badge is all a client
//! needs, and keeping the payload on one path (the MCP tool, where the
//! untrusted framing lives) means there is exactly one place that has to get
//! that framing right.

use super::{BridgeFailure, LocalAppsHostBroker};
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{AppCapabilityKindDto, AppEventDto};
use local_apps::mailbox::{load_mailbox, save_mailbox};
use local_apps::AppCapability;
use serde_json::{json, Value};

const REASON_AGENT_NOTIFY: &str = "应用请求向你的对话助手发送事件与数据。";

impl LocalAppsHostBroker {
    pub(super) async fn agent_post_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        let topic = payload
            .get("topic")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeFailure::coded("invalid_request", "topic is required"))?
            .to_string();
        let body = payload.get("body").cloned().unwrap_or(json!({}));
        self.authorize_declared_capability(
            app_id,
            AppCapability::AgentNotify,
            AppCapabilityKindDto::AgentNotify,
            REASON_AGENT_NOTIFY,
        )
        .await?;

        let layout = self.layout(app_id)?;
        // Same wall-clock read `mutate_data_value` uses: the broker holds no
        // injected clock, and a mailbox timestamp is display metadata rather
        // than anything the service's ordering depends on.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        // The mailbox lock is held across the read-modify-write and NOTHING
        // else. Emitting under it would put a client callback inside a lock
        // the same app's next post needs — the exact shape `AppEmissionQueue`
        // exists to keep out of this subsystem.
        let (seq, dropped) = {
            let _guard = self.mailbox_writes.lock().await;
            let mut mailbox = load_mailbox(&layout).map_err(|error| error.to_string())?;
            let seq = mailbox
                .append(&topic, body, now_ms)
                .map_err(|error| BridgeFailure::coded("invalid_request", error.to_string()))?;
            save_mailbox(&layout, &mailbox).map_err(|error| error.to_string())?;
            (seq, mailbox.dropped_count)
        };

        self.event_sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppAgentEventPosted {
                    app_id: app_id.to_string(),
                    seq,
                    topic: topic.clone(),
                    created_at_ms: now_ms,
                },
            })
            .await;
        Ok(json!({ "seq": seq, "droppedCount": dropped }))
    }
}

#[cfg(test)]
mod tests {
    use crate::local_apps_host::LocalAppsHostBroker;
    use client_adapter::{ClientEventSink, MockSink};
    use client_protocol::events::ClientEvent;
    use client_protocol::local_apps::{
        AppBridgeOperationDto, AppBridgeRequestDto, AppEventDto, AppUiActionKindDto,
        AppUiRequestDto,
    };
    use local_apps::mailbox::{load_mailbox, MAX_MAILBOX_EVENTS};
    use local_apps::test_support::FixedClock;
    use local_apps::{
        load_manifest, load_permissions, save_manifest, save_permissions, AppCapability, AppLayout,
        AppService, NoopAppEventObserver, NoopContinuationSink,
    };
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time::timeout;

    struct Harness {
        _root: TempDir,
        broker: Arc<LocalAppsHostBroker>,
        sink: Arc<MockSink>,
        app_id: String,
        layout: AppLayout,
    }

    async fn harness() -> Harness {
        let root = TempDir::new().expect("tempdir");
        let service = Arc::new(
            AppService::load(
                root.path(),
                Arc::new(FixedClock::new(1_700_000_000_000)),
                Arc::new(NoopContinuationSink),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .expect("load app service"),
        );
        let sink = MockSink::arc();
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            sink.clone() as Arc<dyn ClientEventSink>,
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        let record = service
            .create_app(Some("Poster"), "a mailbox test app", None)
            .await
            .expect("create app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).expect("layout");
        Harness {
            _root: root,
            broker,
            sink,
            app_id: record.id,
            layout,
        }
    }

    fn declare_and_grant(h: &Harness) {
        let mut manifest = load_manifest(&h.layout).expect("manifest");
        manifest.capabilities.push(AppCapability::AgentNotify);
        save_manifest(&h.layout, &manifest).expect("declare");
        let mut permissions = load_permissions(&h.layout).expect("permissions");
        permissions.grant(AppCapability::AgentNotify);
        save_permissions(&h.layout, &permissions).expect("grant");
    }

    async fn post(h: &Harness, payload: Value) -> (bool, Value, Option<String>, Option<String>) {
        post_as(h, "req-1", payload).await
    }

    async fn post_as(
        h: &Harness,
        request_id: &str,
        payload: Value,
    ) -> (bool, Value, Option<String>, Option<String>) {
        h.broker
            .execute_bridge(AppBridgeRequestDto {
                request_id: request_id.to_string(),
                app_id: h.app_id.clone(),
                operation: AppBridgeOperationDto::AgentPost,
                payload_json: Some(payload.to_string()),
            })
            .await;
        let response = h
            .sink
            .events()
            .await
            .into_iter()
            .rev()
            .find_map(|event| match event {
                ClientEvent::AppEvent {
                    event: AppEventDto::AppBridgeResponse { response },
                } if response.request_id == request_id => Some(response),
                _ => None,
            })
            .expect("a bridge response");
        let result = response
            .result_json
            .as_deref()
            .map(|body| serde_json::from_str(body).expect("result json"))
            .unwrap_or(Value::Null);
        (response.ok, result, response.error, response.error_code)
    }

    #[tokio::test]
    async fn a_posted_event_lands_in_the_mailbox_and_raises_a_bodyless_badge_event() {
        let h = harness().await;
        declare_and_grant(&h);

        let (ok, result, error, code) = post(
            &h,
            json!({"topic": "timer.done", "body": {"minutes": 25}}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["seq"], 1);

        let mailbox = load_mailbox(&h.layout).expect("mailbox");
        assert_eq!(mailbox.events.len(), 1);
        assert_eq!(mailbox.events[0].topic, "timer.done");
        assert_eq!(mailbox.events[0].body["minutes"], 25);

        let posted = h
            .sink
            .events()
            .await
            .into_iter()
            .find_map(|event| match event {
                ClientEvent::AppEvent {
                    event: AppEventDto::AppAgentEventPosted { topic, seq, .. },
                } => Some((topic, seq)),
                _ => None,
            })
            .expect("a badge event");
        assert_eq!(posted, ("timer.done".to_string(), 1));
    }

    #[tokio::test]
    async fn an_undeclared_agent_notify_is_refused_without_prompting() {
        let h = harness().await;

        let (ok, _, _, code) = timeout(
            Duration::from_secs(2),
            post(&h, json!({"topic": "timer.done", "body": {}})),
        )
        .await
        .expect("the refusal must not wait on a prompt");
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("capability_not_declared"));
        assert!(load_mailbox(&h.layout).expect("mailbox").events.is_empty());
    }

    #[tokio::test]
    async fn a_malformed_topic_is_refused_and_nothing_lands() {
        let h = harness().await;
        declare_and_grant(&h);

        let (ok, _, _, code) = post(&h, json!({"topic": "../escape", "body": {}})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("invalid_request"));
        assert!(load_mailbox(&h.layout).expect("mailbox").events.is_empty());
    }

    /// The cycle `AppEmissionQueue`'s docs warn about, in one test: the app
    /// is mid-`act_on_ui` (the broker is holding a pending-UI slot waiting on
    /// the page) and the SAME app posts. If the mailbox write emitted under
    /// its own lock — or shared one with the UI path — this would wedge.
    #[tokio::test]
    async fn posting_while_the_agent_drives_the_same_app_does_not_wedge() {
        let h = harness().await;
        declare_and_grant(&h);

        // Park a UI request: it registers a pending slot and waits for the
        // page to answer, which nobody will do here.
        let driving = {
            let broker = h.broker.clone();
            let app_id = h.app_id.clone();
            tokio::spawn(async move {
                broker
                    .request_ui(AppUiRequestDto {
                        request_id: "ui-1".into(),
                        app_id,
                        action: AppUiActionKindDto::Inspect,
                        target: None,
                        value: None,
                    })
                    .await
            })
        };
        // Wait until the UI request is actually parked.
        loop {
            let parked = h.sink.events().await.into_iter().any(|event| {
                matches!(
                    event,
                    ClientEvent::AppEvent {
                        event: AppEventDto::AppUiRequest { .. }
                    }
                )
            });
            if parked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let (ok, _, error, code) = timeout(
            Duration::from_secs(3),
            post_as(&h, "req-2", json!({"topic": "note.added", "body": {}})),
        )
        .await
        .expect("a post must not block on an in-flight UI automation request");
        assert!(ok, "{error:?} {code:?}");
        driving.abort();
    }

    #[tokio::test]
    async fn the_mailbox_stays_bounded_across_many_posts() {
        let h = harness().await;
        declare_and_grant(&h);

        for i in 0..(MAX_MAILBOX_EVENTS + 3) {
            let (ok, _, error, _) = post_as(
                &h,
                &format!("req-{i}"),
                json!({"topic": "tick", "body": {"i": i}}),
            )
            .await;
            assert!(ok, "post {i}: {error:?}");
        }
        let mailbox = load_mailbox(&h.layout).expect("mailbox");
        assert_eq!(mailbox.events.len(), MAX_MAILBOX_EVENTS);
        assert_eq!(mailbox.dropped_count, 3);
    }
}
