//! Session-independent Decision stdin/stdout service. Secrets never enter argv or diagnostics.
use decision_api::{DecisionModel, InvocationContext, Request};
use decision_runtime::{Protocol, RemoteDecisionService};
use serde::Deserialize;
use serde_json::json;
use std::{io::Read, sync::Arc, time::Duration};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    endpoint: String,
    protocol: Protocol,
    credential: Option<String>,
    request: Request,
}
pub async fn run() -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(17 * 1024 * 1024)
        .read_to_end(&mut bytes)?;
    let operation = async {
        if bytes.len() > 16 * 1024 * 1024 {
            anyhow::bail!("Decision request exceeds 16 MiB");
        }
        let input: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid Decision request"))?;
        let service = RemoteDecisionService::new(
            Arc::new(http_client::provider_transport()?),
            input.protocol,
            input.endpoint,
            input.credential,
        );
        service
            .evaluate(
                input.request,
                InvocationContext {
                    timeout: Duration::from_secs(30),
                    cancellation: tokio_util::sync::CancellationToken::new(),
                },
            )
            .await
            .map_err(anyhow::Error::from)
    };
    let response = match operation.await {
        Ok(result) => json!({"result":result}),
        Err(error) => json!({"error":error.to_string()}),
    };
    println!("{response}");
    Ok(())
}
