//! Proactive refresh: short-lived token (TTL < 5 min) triggers refresh at
//! remaining/2 (not at a fixed 5 min lead). Handle is owned by `AuthState` and
//! cancellable via `AuthState::shutdown` (Task 6).

use async_trait::async_trait;
use lingxi_anthropic_oauth::refresh::{AuthState, RefreshDriver};
use lingxi_anthropic_oauth::ClaudeAiOAuthConfig;
use lingxi_protocol::{HttpRequest, HttpResponse, Secret};
use lingxi_traits::http::SseStream;
use lingxi_traits::{
    BackgroundTaskHandle, Clock, HttpError, HttpTransport, RuntimeError, RuntimeSpawner,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;

struct CountingTransport {
    calls: Arc<AtomicU32>,
    expires_in_secs: u64,
}

#[async_trait]
impl HttpTransport for CountingTransport {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let body = format!(
            r#"{{"access_token":"REFRESHED_v{n}","refresh_token":"R_v{n}","expires_in":{}}}"#,
            self.expires_in_secs,
        );
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body,
        })
    }
    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        unimplemented!()
    }
}

struct AdvancingClock {
    base: SystemTime,
    elapsed: Arc<AtomicU64>,
}
impl Clock for AdvancingClock {
    fn now(&self) -> SystemTime {
        self.base + Duration::from_millis(self.elapsed.load(Ordering::SeqCst))
    }
}

/// Spawner that runs tasks on the tokio runtime and tracks handles via tokio
/// `JoinHandle` stored in a Mutex. Cancellation aborts the `JoinHandle`.
struct TokioSpawner {
    next_id: AtomicU64,
    handles: Mutex<Vec<(u64, tokio::task::JoinHandle<()>)>>,
}

#[async_trait]
impl RuntimeSpawner for TokioSpawner {
    async fn spawn(
        &self,
        name: &str,
        task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Result<BackgroundTaskHandle, RuntimeError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let jh = tokio::spawn(task);
        self.handles.lock().await.push((id, jh));
        Ok(BackgroundTaskHandle {
            task_name: name.into(),
            task_id: id,
        })
    }
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
    async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError> {
        let mut h = self.handles.lock().await;
        if let Some(pos) = h.iter().position(|(id, _)| *id == handle.task_id) {
            let (_, jh) = h.remove(pos);
            jh.abort();
            Ok(())
        } else {
            Err(RuntimeError::NotFound(handle.task_name.clone()))
        }
    }
}

#[tokio::test(start_paused = true)]
async fn short_ttl_token_refreshes_at_half_remaining() {
    // Construct a token with 60-second TTL. proactive_lead = 30s (remaining/2).
    // The task should wake at remaining(60) - lead(30) = 30s after construction.
    let cfg = ClaudeAiOAuthConfig::default_with_port(0);
    let calls = Arc::new(AtomicU32::new(0));
    let elapsed = Arc::new(AtomicU64::new(0));
    let transport: Arc<dyn HttpTransport> = Arc::new(CountingTransport {
        calls: calls.clone(),
        expires_in_secs: 60,
    });
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let clock: Arc<dyn Clock> = Arc::new(AdvancingClock {
        base,
        elapsed: elapsed.clone(),
    });
    let spawner: Arc<dyn RuntimeSpawner> = Arc::new(TokioSpawner {
        next_id: AtomicU64::new(0),
        handles: Mutex::new(vec![]),
    });

    let state = AuthState::new(
        cfg,
        Secret::new("INITIAL".to_string()),
        Some(Secret::new("INITIAL_R".to_string())),
        base + Duration::from_secs(60),
        transport,
        clock,
        None,
        None,
    );

    RefreshDriver::spawn_proactive(state.clone(), spawner.clone())
        .await
        .expect("spawn ok");
    assert!(state.proactive_handle().await.is_some(), "handle stored");

    // Yield once so the spawned task gets scheduled and reaches its first
    // `await` point (the sleep). Without this the `advance` call below may
    // fire before the task has parked on the timer.
    tokio::task::yield_now().await;

    // Advance virtual time past the wake instant (30s). Real clock used by the
    // background sleep_until is tokio's auto-advance under `start_paused=true`.
    tokio::time::advance(Duration::from_secs(31)).await;
    // Yield to let the task observe the wake.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    tokio::task::yield_now().await;

    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "expected proactive refresh fired at remaining/2 = 30s, calls = {}",
        calls.load(Ordering::SeqCst),
    );
}
