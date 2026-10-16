use assert_cmd::Command;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) struct LocalMessagesStub {
    address: SocketAddr,
    stop: Option<mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<Vec<Vec<u8>>>>,
}

impl LocalMessagesStub {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stop_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let started = Instant::now();
            let mut stop_deadline = None;
            let mut requests = Vec::new();
            loop {
                if stop_deadline.is_none() && stop_rx.try_recv().is_ok() {
                    stop_deadline = Some(Instant::now() + Duration::from_millis(100));
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("accepted socket should use blocking I/O");
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let request = read_http_request(&mut stream).unwrap();
                        let (content_type, body) = response_for_request(&request);
                        write_http_response(&mut stream, content_type, body).unwrap();
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if stop_deadline.is_some_and(|deadline| Instant::now() >= deadline)
                            || started.elapsed() >= Duration::from_secs(30)
                        {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("loopback listener failed: {error}"),
                }
            }
            requests
        });
        Self {
            address,
            stop: Some(stop),
            worker: Some(worker),
        }
    }

    pub(crate) fn configure_isolated_command(
        &self,
        command: &mut Command,
        home: &Path,
        cwd: &Path,
        api_key: &str,
    ) {
        let base_url = format!("http://{}", self.address);
        command
            .env_clear()
            .current_dir(cwd)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home)
            .env("TMPDIR", home)
            .env("LANG", "C.UTF-8")
            .env("TZ", "UTC")
            .env("TERM", "dumb")
            .env("LINGXI_CONFIG_DIR", home)
            .env("CLAUDE_CONFIG_DIR", home)
            .env("LINGXI_API_BASE_URL", &base_url)
            .env("LINGXI_NATIVE_ANTHROPIC_ENDPOINT", &base_url)
            .env("ANTHROPIC_API_KEY", api_key)
            .env("ANTHROPIC_MAX_RETRIES", "0")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DO_NOT_TRACK", "1");
    }

    pub(crate) fn finish(mut self) -> Vec<Vec<u8>> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.worker
            .take()
            .expect("stub worker is present")
            .join()
            .expect("stub worker should finish")
    }
}

impl Drop for LocalMessagesStub {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn assert_unknown_fallback_request(requests: &[Vec<u8>], prompt: &str) {
    assert_eq!(
        requests.len(),
        1,
        "fallback should make one local API request"
    );
    let request = &requests[0];
    let header_end = find_bytes(request, b"\r\n\r\n").expect("request headers are complete");
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let request_line = headers.lines().next().expect("request line is present");
    let mut request_parts = request_line.split_ascii_whitespace();
    let method = request_parts.next();
    let target = request_parts.next().unwrap_or_default();
    assert!(
        method == Some("POST") && (target == "/v1/messages" || target == "/v1/messages?beta=true"),
        "fallback must use the local Anthropic Messages endpoint: {headers}"
    );
    let body: Value = serde_json::from_slice(&request[header_end + 4..])
        .expect("local Messages request body is JSON");
    let messages = body["messages"]
        .as_array()
        .expect("Messages request contains a message array");
    let user_texts = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>();
    assert!(
        user_texts.contains(&prompt),
        "request must preserve the exact unknown slash prompt: {user_texts:?}"
    );
    assert!(
        user_texts
            .iter()
            .any(|text| text.contains("The user's message starts with the slash command")),
        "request must include Native's unknown-command fallback reminder: {user_texts:?}"
    );
}

fn response_for_request(request: &[u8]) -> (&'static str, &'static [u8]) {
    let header_end = find_bytes(request, b"\r\n\r\n").expect("request headers are complete");
    let body: Value =
        serde_json::from_slice(&request[header_end + 4..]).expect("Messages request body is JSON");
    if body["stream"].as_bool().unwrap_or(false) {
        ("text/event-stream", STREAM_RESPONSE)
    } else {
        ("application/json", BATCH_RESPONSE)
    }
}

const STREAM_RESPONSE: &[u8] = br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_cli_unknown_fallback","type":"message","role":"assistant","model":"claude-sonnet-4-5","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"loopback fallback answer"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":4}}

event: message_stop
data: {"type":"message_stop"}

"#;

const BATCH_RESPONSE: &[u8] = br#"{"id":"msg_cli_unknown_fallback","type":"message","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"loopback fallback answer"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":4}}"#;

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "CLI closed before completing its request",
            ));
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = find_bytes(&request, b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let expected = header_end + 4 + content_length;
        if request.len() >= expected {
            request.truncate(expected);
            return Ok(request);
        }
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.0 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
