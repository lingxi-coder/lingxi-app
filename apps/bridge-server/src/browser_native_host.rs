//! Chrome native messaging relay. Does not boot an Agent or read model credentials.
use serde::Deserialize;
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const PROTOCOL: &str = "lingxi-browser/1";
const EXTENSION_ID: &str = "nciganheacfkdbpdfdnjnjdkcabfbkmg";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    url: String,
    token: String,
}

fn endpoint(descriptor: &Descriptor) -> anyhow::Result<u16> {
    let port = descriptor
        .url
        .strip_prefix("http://127.0.0.1:")
        .ok_or_else(|| anyhow::anyhow!("native browser endpoint is not loopback"))?
        .parse::<u16>()?;
    anyhow::ensure!(
        port > 0
            && descriptor.token.len() == 64
            && descriptor
                .token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "invalid native browser descriptor"
    );
    Ok(port)
}

async fn http(
    descriptor: &Descriptor,
    method: &str,
    path: &str,
    peer: &str,
    body: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let port = endpoint(descriptor)?;
    let operation = async {
        let mut stream =
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let headers = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nX-LingXi-Browser-Peer: {peer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",descriptor.token,body.len());
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(body).await?;
        let mut response = Vec::new();
        stream
            .take((MAX_MESSAGE + 65_537) as u64)
            .read_to_end(&mut response)
            .await?;
        anyhow::ensure!(
            response.len() <= MAX_MESSAGE + 65_536,
            "native browser response exceeds limit"
        );
        let split = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .ok_or_else(|| anyhow::anyhow!("invalid native browser HTTP response"))?;
        anyhow::ensure!(
            split <= 65_536,
            "native browser response headers exceed limit"
        );
        let headers = std::str::from_utf8(&response[..split])?;
        anyhow::ensure!(
            headers
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("HTTP/1.1 200 ")),
            "native browser host rejected request"
        );
        let body = response[split + 4..].to_vec();
        anyhow::ensure!(
            body.len() <= MAX_MESSAGE,
            "native browser message exceeds limit"
        );
        Ok::<_, anyhow::Error>(body)
    };
    tokio::time::timeout(Duration::from_secs(35), operation).await?
}

async fn read_message(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_le_bytes(length) as usize;
    anyhow::ensure!(
        length > 0 && length <= MAX_MESSAGE,
        "invalid Chrome native message length"
    );
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}
async fn write_message(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    body: &[u8],
) -> anyhow::Result<()> {
    // Chrome caps messages sent by the native host to its extension at 1 MiB.
    anyhow::ensure!(
        !body.is_empty() && body.len() <= 1_048_576,
        "native-to-Chrome message exceeds limit"
    );
    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}
fn validate_message(body: &[u8], peer: Option<&str>) -> anyhow::Result<String> {
    let message: Value = serde_json::from_slice(body)?;
    anyhow::ensure!(
        message["protocol"].as_str() == Some(PROTOCOL)
            && message["extensionId"].as_str() == Some(EXTENSION_ID),
        "unsupported Chrome extension protocol"
    );
    let identity = message["peerId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Chrome peer identity is missing"))?;
    anyhow::ensure!(
        !identity.is_empty()
            && identity.len() <= 64
            && identity
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid Chrome peer identity"
    );
    anyhow::ensure!(
        peer.is_none_or(|peer| peer == identity),
        "Chrome peer identity changed"
    );
    Ok(identity.to_owned())
}

pub async fn run(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= 4096,
        "invalid browser descriptor file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "browser descriptor must be owner-only"
        );
    }
    let descriptor: Descriptor = serde_json::from_slice(&std::fs::read(path)?)?;
    endpoint(&descriptor)?;
    if let Some(origin) = std::env::args().nth(3) {
        anyhow::ensure!(
            origin == format!("chrome-extension://{EXTENSION_ID}/"),
            "unexpected browser extension origin"
        );
    }
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let first = read_message(&mut stdin)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chrome closed before handshake"))?;
    let peer = validate_message(&first, None)?;
    http(&descriptor, "POST", "/extension/message", &peer, &first).await?;
    let receive = async {
        while let Some(body) = read_message(&mut stdin).await? {
            validate_message(&body, Some(&peer))?;
            http(&descriptor, "POST", "/extension/message", &peer, &body).await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let transmit = async {
        loop {
            let body = http(&descriptor, "GET", "/extension/poll", &peer, &[]).await?;
            write_message(&mut stdout, &body).await?;
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    };
    tokio::select! { result = receive => result, result = transmit => result }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_frames_round_trip_and_reject_oversize() {
        let (mut sender, mut receiver) = tokio::io::duplex(128);
        write_message(&mut sender, b"{\"ok\":true}").await.unwrap();
        assert_eq!(
            read_message(&mut receiver).await.unwrap().unwrap(),
            b"{\"ok\":true}"
        );
        sender
            .write_all(&(MAX_MESSAGE as u32 + 1).to_le_bytes())
            .await
            .unwrap();
        assert!(read_message(&mut receiver).await.is_err());
    }
    #[test]
    fn endpoint_never_accepts_remote_or_ambiguous_hosts() {
        for url in [
            "http://localhost:1234",
            "https://127.0.0.1:1234",
            "http://127.0.0.1:1234/",
            "http://127.0.0.1:0",
            "http://127.0.0.1:1234@evil.test",
        ] {
            assert!(endpoint(&Descriptor {
                url: url.into(),
                token: "a".repeat(64)
            })
            .is_err());
        }
        assert_eq!(
            endpoint(&Descriptor {
                url: "http://127.0.0.1:1234".into(),
                token: "a".repeat(64)
            })
            .unwrap(),
            1234
        );
    }
}
