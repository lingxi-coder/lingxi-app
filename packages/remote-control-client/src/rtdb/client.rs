//! [`RtdbClient`]: the Realtime Database over REST and Server-Sent Events.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::{header, Method, StatusCode, Url};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{
    ListenControl, ListenItem, Listener, Rtdb, RtdbError, RtdbEvent, RtdbEventKind,
    RtdbListenError, TokenProvider,
};
use crate::encoding::random_unit;
use crate::version::{SSE_BACKOFF_MAX, SSE_BACKOFF_MIN, SSE_BACKOFF_RESET, SSE_IDLE_TIMEOUT};

/// Options of [`RtdbClient`].
pub struct RtdbClientOptions {
    /// Instance URL, e.g. [`DEFAULT_RTDB_URL`](crate::version::DEFAULT_RTDB_URL), or
    /// `http://127.0.0.1:9000` for the emulator.
    pub database_url: String,
    /// Supplies the Firebase ID token. The emulator accepts an unsigned token
    /// (`alg: none`) whose `sub` is the uid.
    pub tokens: Arc<dyn TokenProvider>,
    /// The emulator's `ns` query parameter.
    pub namespace: Option<String>,
    /// A preconfigured HTTP client (proxies, extra roots); a default one is built when absent.
    pub http: Option<reqwest::Client>,
    /// Give up on a REST request (not on the stream) after this long; default 30 s.
    pub request_timeout: Duration,
    /// Jitter source in `[0, 1)`; defaults to the system random source.
    pub random: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
}

impl RtdbClientOptions {
    /// Options with the defaults for everything but the URL and token source.
    pub fn new(database_url: impl Into<String>, tokens: Arc<dyn TokenProvider>) -> Self {
        Self {
            database_url: database_url.into(),
            tokens,
            namespace: None,
            http: None,
            request_timeout: Duration::from_secs(30),
            random: None,
        }
    }
}

struct Inner {
    base: String,
    tokens: Arc<dyn TokenProvider>,
    namespace: Option<String>,
    http: reqwest::Client,
    request_timeout: Duration,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
}

/// A [`Rtdb`] over the REST API: `GET/PUT/PATCH/DELETE {databaseUrl}/{path}.json?auth={token}`
/// and, for `listen`, the same URL with `Accept: text/event-stream`.
///
/// Writes carry `print=silent`, so the server does not echo what was written
/// (tunnel frames are up to 64 KiB each, and RTDB bills download).
pub struct RtdbClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RtdbClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtdbClient")
            .field("base", &self.inner.base)
            .finish_non_exhaustive()
    }
}

/// Remove token query values from text that may quote a URL.
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.to_ascii_lowercase().find("auth=") {
        out.push_str(&rest[..index + 5]);
        let tail = &rest[index + 5..];
        let end = tail
            .find(|c: char| {
                c == '&' || c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')')
            })
            .unwrap_or(tail.len());
        out.push_str("***");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn network_error(error: reqwest::Error) -> RtdbError {
    let detail = error
        .without_url()
        .to_string()
        .chars()
        .take(300)
        .collect::<String>();
    RtdbError::new(0, redact(&format!("network error: {detail}")))
}

async fn http_error(response: reqwest::Response) -> RtdbError {
    let status = response.status();
    let mut message = status
        .canonical_reason()
        .map_or_else(|| format!("HTTP {}", status.as_u16()), str::to_owned);
    if let Ok(bytes) = response.bytes().await {
        let body: String = String::from_utf8_lossy(&bytes).chars().take(2048).collect();
        match serde_json::from_str::<Value>(&body) {
            Ok(Value::Object(object)) if object.get("error").is_some_and(Value::is_string) => {
                message = object["error"].as_str().unwrap_or_default().to_owned();
            }
            _ if !body.is_empty() => message = body,
            _ => {}
        }
    }
    RtdbError::new(
        status.as_u16(),
        redact(&message).chars().take(300).collect::<String>(),
    )
}

impl RtdbClient {
    /// Build a client.
    ///
    /// # Panics
    /// Panics if the default HTTP client cannot be built (no TLS backend).
    pub fn new(options: RtdbClientOptions) -> Self {
        let http = options.http.unwrap_or_else(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .build()
                .expect("a default HTTP client")
        });
        Self {
            inner: Arc::new(Inner {
                base: options.database_url.trim_end_matches('/').to_owned(),
                tokens: options.tokens,
                namespace: options.namespace,
                http,
                request_timeout: options.request_timeout,
                random: options.random.unwrap_or_else(|| Arc::new(random_unit)),
            }),
        }
    }
}

impl Inner {
    fn url(&self, path: &str, token: &str, silent: bool) -> Result<Url, RtdbError> {
        let mut url = Url::parse(&self.base)
            .map_err(|e| RtdbError::new(0, format!("invalid database URL: {e}")))?;
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        {
            let mut parts = url
                .path_segments_mut()
                .map_err(|()| RtdbError::new(0, "database URL cannot be a base"))?;
            parts.pop_if_empty();
            match segments.split_last() {
                None => {
                    parts.push(".json");
                }
                Some((last, init)) => {
                    parts.extend(init);
                    parts.push(&format!("{last}.json"));
                }
            }
        }
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("auth", token);
            if let Some(namespace) = &self.namespace {
                query.append_pair("ns", namespace);
            }
            if silent {
                query.append_pair("print", "silent");
            }
        }
        Ok(url)
    }

    /// One REST request, bounded by the request timeout. A 401 is retried once with
    /// a freshly minted token; other failures surface as [`RtdbError`].
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        silent: bool,
    ) -> Result<reqwest::Response, RtdbError> {
        for attempt in 0..2 {
            let token = self
                .tokens
                .id_token(attempt > 0)
                .await
                .map_err(|e| RtdbError::new(0, redact(&e.to_string())))?;
            let url = self.url(path, &token, silent)?;
            let mut request = self
                .http
                .request(method.clone(), url)
                .timeout(self.request_timeout);
            if let Some(body) = &body {
                request = request.body(body.clone());
            }
            let response = request.send().await.map_err(network_error)?;
            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                continue;
            }
            if !response.status().is_success() {
                return Err(http_error(response).await);
            }
            return Ok(response);
        }
        unreachable!("the second attempt returns")
    }
}

#[async_trait]
impl Rtdb for RtdbClient {
    async fn get(&self, path: &str) -> Result<Value, RtdbError> {
        let response = self.inner.request(Method::GET, path, None, false).await?;
        let bytes = response.bytes().await.map_err(network_error)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| RtdbError::new(0, format!("network error: bad JSON: {e}")))
    }

    async fn put(&self, path: &str, value: &Value) -> Result<(), RtdbError> {
        self.inner
            .request(Method::PUT, path, Some(value.to_string()), true)
            .await
            .map(drop)
    }

    async fn patch(&self, path: &str, value: &Map<String, Value>) -> Result<(), RtdbError> {
        self.inner
            .request(
                Method::PATCH,
                path,
                Some(Value::Object(value.clone()).to_string()),
                true,
            )
            .await
            .map(drop)
    }

    async fn remove(&self, path: &str) -> Result<(), RtdbError> {
        self.inner
            .request(Method::DELETE, path, None, true)
            .await
            .map(drop)
    }

    fn listen(&self, path: &str) -> Listener {
        let (sender, receiver) = mpsc::unbounded_channel();
        let control = Arc::new(SseControl {
            cancel: CancellationToken::new(),
            reconnect: Notify::new(),
        });
        tokio::spawn(sse_loop(
            Arc::clone(&self.inner),
            path.to_owned(),
            sender,
            Arc::clone(&control),
        ));
        Listener::new(receiver, control)
    }
}

// ---------------------------------------------------------------------------
// Streaming subscription
// ---------------------------------------------------------------------------

struct SseControl {
    cancel: CancellationToken,
    reconnect: Notify,
}

impl ListenControl for SseControl {
    fn close(&self) {
        self.cancel.cancel();
    }

    fn reconnect(&self) {
        self.reconnect.notify_one();
    }
}

enum Next {
    Backoff,
    Now,
    Stop,
}

/// An `auth_revoked` sooner than this after connecting is retried with backoff, not
/// at once, so a server that revokes every new stream cannot make us spin.
const AUTH_REVOKED_MIN_UPTIME: Duration = Duration::from_secs(5);

/// Statuses a stream cannot recover from by retrying.
fn fatal_status(status: u16) -> bool {
    matches!(status, 400 | 401 | 403 | 404)
}

struct SseState {
    attempt: u32,
    force_refresh: bool,
    /// A request failed before any response; one immediate second try is made (the
    /// server may have closed a pooled connection, which says nothing about its health).
    quick_retried: bool,
    /// The token of the failed request, reused by the quick retry.
    retry_token: Option<(String, bool)>,
}

fn backoff_delay(inner: &Inner, attempt: u32) -> Duration {
    let base = SSE_BACKOFF_MIN
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(SSE_BACKOFF_MAX);
    base.mul_f64(0.5 + 0.5 * (inner.random)())
}

/// One SSE subscription with its reconnect loop. A stream that ends, errors or
/// stays silent for 90 s is reopened after 1 s doubling to 30 s (each delay scaled
/// by a random 0.5-1.0); the delay restarts at 1 s once a stream has stayed up for
/// 60 s. `auth_revoked` and a first 401 reopen at once with a fresh token.
async fn sse_loop(
    inner: Arc<Inner>,
    path: String,
    sender: mpsc::UnboundedSender<ListenItem>,
    control: Arc<SseControl>,
) {
    let mut state = SseState {
        attempt: 0,
        force_refresh: false,
        quick_retried: false,
        retry_token: None,
    };
    loop {
        if control.cancel.is_cancelled() {
            return;
        }
        match connect_once(&inner, &path, &sender, &control, &mut state).await {
            Next::Stop => return,
            Next::Now => {}
            Next::Backoff => {
                let delay = backoff_delay(&inner, state.attempt);
                state.attempt = state.attempt.saturating_add(1);
                tokio::select! {
                    () = control.cancel.cancelled() => return,
                    () = control.reconnect.notified() => state.attempt = 0,
                    () = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}

fn fail(sender: &mpsc::UnboundedSender<ListenItem>, error: RtdbListenError) -> Next {
    let _ = sender.send(ListenItem::Error(error));
    Next::Stop
}

async fn connect_once(
    inner: &Arc<Inner>,
    path: &str,
    sender: &mpsc::UnboundedSender<ListenItem>,
    control: &SseControl,
    state: &mut SseState,
) -> Next {
    let (token, refreshed) = match state.retry_token.take() {
        Some(kept) => kept,
        None => {
            let refreshed = std::mem::take(&mut state.force_refresh);
            let Ok(token) = inner.tokens.id_token(refreshed).await else {
                return Next::Backoff;
            };
            (token, refreshed)
        }
    };
    let Ok(url) = inner.url(path, &token, false) else {
        return Next::Backoff;
    };
    let request = inner
        .http
        .get(url)
        .header(header::ACCEPT, "text/event-stream")
        .send();
    let response = tokio::select! {
        () = control.cancel.cancelled() => return Next::Stop,
        () = control.reconnect.notified() => { state.attempt = 0; return Next::Now; }
        result = request => match result {
            Ok(response) => response,
            Err(_) if !state.quick_retried => {
                state.quick_retried = true;
                state.retry_token = Some((token, refreshed));
                return Next::Now;
            }
            Err(_) => {
                state.force_refresh = refreshed;
                return Next::Backoff;
            }
        },
    };
    state.quick_retried = false;
    if response.status() == StatusCode::UNAUTHORIZED && !refreshed {
        state.force_refresh = true;
        return Next::Now;
    }
    if !response.status().is_success() {
        let error = http_error(response).await;
        if fatal_status(error.status) {
            return fail(
                sender,
                RtdbListenError::Http {
                    status: error.status,
                    message: error.message,
                },
            );
        }
        return Next::Backoff;
    }
    let connected_at = Instant::now();
    let next = pump(response, sender, control, state, connected_at).await;
    if connected_at.elapsed() >= SSE_BACKOFF_RESET {
        state.attempt = 0;
    }
    next
}

async fn pump(
    response: reqwest::Response,
    sender: &mpsc::UnboundedSender<ListenItem>,
    control: &SseControl,
    state: &mut SseState,
    connected_at: Instant,
) -> Next {
    let mut stream = response.bytes_stream().eventsource();
    loop {
        let item = tokio::select! {
            () = control.cancel.cancelled() => return Next::Stop,
            () = control.reconnect.notified() => { state.attempt = 0; return Next::Now; }
            item = tokio::time::timeout(SSE_IDLE_TIMEOUT, stream.next()) => item,
        };
        let event = match item {
            Err(_elapsed) => return Next::Backoff,
            Ok(None | Some(Err(_))) => return Next::Backoff,
            Ok(Some(Ok(event))) => event,
        };
        match event.event.as_str() {
            "put" | "patch" => {
                let Some(parsed) = parse_event(&event.event, &event.data) else {
                    // A stream that sends garbage is not trusted further.
                    return Next::Backoff;
                };
                if sender.send(ListenItem::Event(parsed)).is_err() {
                    return Next::Stop;
                }
            }
            "cancel" => return fail(sender, RtdbListenError::Cancel),
            "auth_revoked" => {
                state.force_refresh = true;
                return if connected_at.elapsed() < AUTH_REVOKED_MIN_UPTIME {
                    Next::Backoff
                } else {
                    Next::Now
                };
            }
            // keep-alive and anything unknown only prove liveness
            _ => {}
        }
    }
}

fn parse_event(kind: &str, data: &str) -> Option<RtdbEvent> {
    let parsed: Value = serde_json::from_str(data).ok()?;
    let object = parsed.as_object()?;
    let path = object.get("path")?.as_str()?;
    if !path.starts_with('/') {
        return None;
    }
    Some(RtdbEvent {
        kind: if kind == "patch" {
            RtdbEventKind::Patch
        } else {
            RtdbEventKind::Put
        },
        path: path.to_owned(),
        data: object.get("data").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redacts_tokens_in_urls() {
        assert_eq!(
            redact("GET https://x/y.json?auth=abc.def&ns=z failed"),
            "GET https://x/y.json?auth=***&ns=z failed"
        );
        assert_eq!(redact("no secrets"), "no secrets");
        assert_eq!(redact("AUTH=secret"), "AUTH=***");
    }
}
