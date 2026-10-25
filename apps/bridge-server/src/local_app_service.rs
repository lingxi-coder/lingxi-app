//! Private stdio application-library host, independent of chat and credentials.
//!
//! The parent owns this process and supplies the canonical configuration root.
//! No network listener or discovery file exposes storage operations to pages.

use local_app::v2::AppLibrary;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: String,
    request: Value,
}

fn config_home(args: impl IntoIterator<Item = String>) -> anyhow::Result<PathBuf> {
    let args: Vec<_> = args.into_iter().collect();
    anyhow::ensure!(
        args.len() == 2 && args[0] == "--config-home",
        "expected --config-home <absolute directory>"
    );
    let path = PathBuf::from(&args[1]);
    anyhow::ensure!(
        path.is_absolute(),
        "configuration directory must be absolute"
    );
    Ok(path)
}

/// Enter the bounded request loop without starting an Agent or reading credentials.
pub async fn run() -> anyhow::Result<()> {
    let home = config_home(std::env::args().skip(2))?;
    tokio::task::spawn_blocking(move || {
        let library = AppLibrary::open(home)?;
        // A reader reconnect must never recover records while another runtime
        // owner is alive. This guard is held for the entire private channel.
        let _runtime_owner = library.acquire_runtime_lease()?;
        serve(&library, std::io::stdin().lock(), std::io::stdout().lock())
    })
    .await??;
    Ok(())
}

/// Read one bounded line at a time. Oversized input closes the private channel,
/// rather than allocating an unbounded line or treating a suffix as a request.
fn serve(
    library: &AppLibrary,
    mut input: impl BufRead,
    mut output: impl Write,
) -> anyhow::Result<()> {
    loop {
        let mut line = Vec::new();
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let take = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |n| n + 1);
            anyhow::ensure!(
                line.len() + take <= MAX_REQUEST_BYTES,
                "application request exceeds size limit"
            );
            line.extend_from_slice(&available[..take]);
            input.consume(take);
            if line.last() == Some(&b'\n') {
                break;
            }
        }
        if line.is_empty() {
            return Ok(());
        }
        let response = match serde_json::from_slice::<Envelope>(&line) {
            Ok(envelope) if !envelope.id.is_empty() && envelope.id.len() <= 128 => {
                match library.request_json(&envelope.request.to_string()) {
                    Ok(value) => json!({"id":envelope.id,"ok":true,"value":value}),
                    Err(error) => {
                        json!({"id":envelope.id,"ok":false,"error":{"code":error.code(),"message":error.to_string()}})
                    }
                }
            }
            _ => {
                json!({"id":null,"ok":false,"error":{"code":"invalidRequest","message":"Invalid application service request"}})
            }
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_protocol_is_correlated_and_recovers_after_invalid_request() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path()).unwrap();
        let input = b"{\"id\":\"bad\",\"request\":{\"op\":\"unknown\"}}\n{\"id\":\"list\",\"request\":{\"op\":\"list\"}}\n";
        let mut output = Vec::new();
        serve(&library, &input[..], &mut output).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies[0]["id"], "bad");
        assert_eq!(replies[0]["ok"], false);
        assert_eq!(replies[1]["id"], "list");
        assert_eq!(replies[1]["ok"], true);
        assert_eq!(replies[1]["value"], json!([]));
    }

    #[test]
    fn rejects_unbounded_request_before_dispatch() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path()).unwrap();
        let input = vec![b' '; MAX_REQUEST_BYTES + 1];
        let mut output = Vec::new();
        assert!(serve(&library, &input[..], &mut output).is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn requires_explicit_absolute_configuration_owner() {
        assert!(config_home(["--config-home".into(), "relative".into()]).is_err());
        assert!(config_home([
            "--config-home".into(),
            "/tmp/fixture".into(),
            "extra".into()
        ])
        .is_err());
        assert_eq!(
            config_home(["--config-home".into(), "/tmp/fixture".into()]).unwrap(),
            PathBuf::from("/tmp/fixture")
        );
    }
}
