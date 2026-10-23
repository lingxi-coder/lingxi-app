//! A tiny hand-written HTTP/1.1 server for exercising the REST and SSE client.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Req {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap()
    }
    pub fn query(&self) -> &str {
        self.target.split_once('?').map_or("", |(_, q)| q)
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub enum Resp {
    /// A normal response with a body.
    Body(u16, String),
    /// Event-stream chunks; afterwards the connection is closed, or held open.
    Sse(Vec<String>, bool),
}

pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Req>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    /// `handler(request, index)` decides every response; `index` counts requests.
    pub async fn start(handler: impl Fn(&Req, usize) -> Resp + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Req>>> = Arc::default();
        let log = Arc::clone(&requests);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let log = Arc::clone(&log);
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    let index = {
                        let mut log = log.lock().unwrap();
                        log.push(request.clone());
                        log.len() - 1
                    };
                    match handler(&request, index) {
                        Resp::Body(status, body) => {
                            let text = format!(
                                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = socket.write_all(text.as_bytes()).await;
                            let _ = socket.shutdown().await;
                        }
                        Resp::Sse(chunks, hold) => {
                            let _ = socket
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
                                .await;
                            for chunk in chunks {
                                let _ = socket.write_all(chunk.as_bytes()).await;
                                let _ = socket.flush().await;
                            }
                            if hold {
                                let mut sink = [0u8; 64];
                                while matches!(socket.read(&mut sink).await, Ok(n) if n > 0) {}
                            } else {
                                let _ = socket.shutdown().await;
                            }
                        }
                    }
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Req> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        let n = socket.read(&mut buffer).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..n]);
        if let Some(position) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break position;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = data[header_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut buffer).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..n]);
    }
    Some(Req {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
