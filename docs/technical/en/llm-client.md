# LLM Client

A standalone Rust client for provider protocols, typed requests, streaming, images, embeddings, and model catalogs.

<a id="install"></a>

## Use the documented working tree

The package is lingxi-llm-client 0.3.0 and the Rust import is lingxi_llm_client. Use Rust 1.94 or later. This handbook documents the current workspace, including uncommitted APIs. Place a separate consumer directory beside llm-client and use its local path. The source baseline records SDK HEAD, dirty files and the product’s actual pinned revision independently; a version number alone does not establish API availability.

### Cargo.toml · complete isolated consumer

```toml
[package]
name = "handbook-llm-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
lingxi-llm-client = { path = "../llm-client", features = ["realtime-websocket"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
async-trait = "0.1"
futures = "0.3"
```

Verification method: Environment validation (llm-working-tree-manifest)

The default feature set is empty. Enable responses-websocket or realtime-websocket for the corresponding built-in WebSocket transport, and tokenizer-* features for offline tokenizers.

<a id="first-request"></a>

## Send your first request

Select the OpenAI profile from the built-in catalog, choose a region, and supply a valid credential for each request. Other profiles can require host-provided authenticators before build; registering the full catalog is not a ready-to-use configuration. The openai profile and gpt-4.1-mini model are catalog entries; your provider account still needs access to the model.

### src/main.rs · complete example

```rust
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, Region, Secret};
use lingxi_llm_client::{builtin_providers, LlmClientBuilder, RequestOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let client = LlmClientBuilder::new(&profiles)?
        .with_region(Region::International)
        .build()?;
    let mut request = ChatRequest::new("gpt-4.1-mini");
    request.messages.push(ConversationMessage::user_text("Hello, LingXi!"));
    let options = RequestOptions {
        credential: Some(Secret::new(std::env::var("OPENAI_API_KEY")?)),
        ..Default::default()
    };
    let response = client.chat().complete_in("openai", &request, &options).await?;
    println!("{}", response.message.text());
    Ok(())
}
```

Verification method: Compile check (llm-first-request)

The example reads OPENAI_API_KEY itself. The client does not discover, refresh, or persist secrets; OPENAI_API_KEY is read only by this example. The model name is a configured request ID, not a guarantee of live entitlement. Use the credential for the exact selected profile and account.

<a id="service-api"></a>

## Service entry points

### LlmClientBuilder::new

```text
pub fn new(profiles: &[ProviderProfile]) -> Result<Self, LlmError>
```

Register built-in codecs, HTTP transport, directories, and authenticators.

### LlmClientBuilder::build

```text
pub fn build(self) -> Result<LlmClient, BuildError>
```

Create a shareable client. Clone the handle for concurrent application tasks.

### LlmClient::chat

```text
pub fn chat(&self) -> ChatService<'_>
```

Conversation completion, streaming, hosted search, and visible chat models.

### LlmClient::images

```text
pub fn images(&self) -> ImageService<'_>
```

Independent image generation, editing, capabilities, and native tasks.

### LlmClient::embeddings

```text
pub fn embeddings(&self) -> EmbeddingService<'_>
```

Independent text embedding routes; no automatic input splitting or normalization.

### LlmClient::snapshot

```text
pub fn snapshot(&self) -> ClientSnapshot
```

Capture one immutable configuration revision for preflight, execution, and pricing.

<a id="ownership"></a>

## Client and host responsibilities

- The client owns wire encoding, transport, stream parsing, provider resource APIs, catalogs, and normalized usage observations.
- Your application owns credentials, tool execution, permissions, conversation history, context compaction, product retry policy, and durable accounting.
- Keep one long-lived client. Model, reasoning controls, service tier, credentials, and timeouts are request-scoped.

<a id="composition"></a>

## Composition and public services

The client is one standalone Rust crate, not the Agent engine. A shared client publishes immutable configuration revisions; chat, images, embeddings, audio, decisions and provider-native resources capture the appropriate revision for each operation. Builder registrations determine the actual codecs, authenticators, transport, directories, attachment resolver and account sources available to a profile.

- LlmClientBuilder::with_transport injects a host HTTP implementation without constructing a network client. register_codec/register_authenticator replace the matching registry entry; custom transports must expose raw provider responses and avoid hidden retry/redirect policy.
- BuildError::MissingRegion, MissingCodec and MissingAuthenticator reject incomplete composition before the first model request. The built-in builder supplies ApiKey, Bearer and ChatGptPlan authenticators; full catalog profiles such as CopilotBearer require the matching host registration. Select only the intended profiles for a small standalone client.
- Use client.snapshot() when preflight, execution and cost must share a fixed revision. A later ClientConfigManager publication changes future live calls, not previously captured snapshots.

- [Exact-profile cloud audio](llm-audio.md)
- [Output contracts and decisions](llm-controls.md)
- [Native resources and hosted tools](llm-native-services.md)

<a id="mock-transport"></a>

## Test request contracts without a provider

This complete test uses the same public builder and Chat facade as an application, but an injected Transport supplies a deterministic Responses envelope. It asserts the model and input reached the wire and exactly one send occurred. It establishes local protocol behavior only; it sends no real provider request.

### Executable mock Transport test

```rust
#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use async_trait::async_trait;
    use lingxi_llm_client::{builtin_providers, HttpRequest, HttpResponse, LlmClientBuilder, RequestOptions, StreamResponse, Transport};
    use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, LlmError, Region, Secret};

    struct MockTransport { sends: AtomicUsize }
    #[async_trait]
    impl Transport for MockTransport {
        async fn send(&self, request: HttpRequest) -> Result<StreamResponse, LlmError> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "gpt-4.1-mini");
            assert!(body.get("input").is_some());
            Ok(HttpResponse {
                status: 200, headers: vec![],
                body: serde_json::to_vec(&serde_json::json!({
                    "id":"resp_mock", "object":"response", "status":"completed",
                    "model":"gpt-4.1-mini", "output":[{
                        "id":"msg_mock", "type":"message", "status":"completed", "role":"assistant",
                        "content":[{"type":"output_text", "text":"Mock answer", "annotations":[]}]
                    }], "usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}
                })).unwrap().into(),
            }.into())
        }
    }

    #[tokio::test]
    async fn public_client_dispatches_once() {
        let http = Arc::new(MockTransport { sends: AtomicUsize::new(0) });
        let profiles = builtin_providers().unwrap().into_iter()
            .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
        let client = LlmClientBuilder::with_transport(http.clone(), &profiles)
            .with_region(Region::International).build().unwrap();
        let mut request = ChatRequest::new("gpt-4.1-mini");
        request.messages.push(ConversationMessage::user_text("Hello"));
        let options = RequestOptions { credential: Some(Secret::new("mock-key".into())), ..Default::default() };
        let response = client.chat().complete_in("openai", &request, &options).await.unwrap();
        assert_eq!(response.message.text(), "Mock answer");
        assert_eq!(http.sends.load(Ordering::SeqCst), 1);
    }
}
```

Verification method: Mock run (llm-transport-mock)

<a id="pinned-install"></a>

## Install a fixed revision separately

The complete SHA below is an independent fixed-revision installation example. Validate it as a separate consumer; the generated source baseline shows the actual product dependency and current llm-client workspace HEAD independently. The working-tree examples above are not evidence that every new API is available at this revision.

### Cargo.toml · separate pinned consumer

```toml
[package]
name = "handbook-llm-pinned"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
lingxi-llm-client = { git = "https://github.com/lingxi-coder/llm-client.git", rev = "87db6e234e32cbc251a128d6b370cdda36c4d0c0" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Verification method: Environment validation (llm-pinned-manifest)

- [System source baseline and dependency revisions](system-overview.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- lingxi-app/Cargo.toml — Uncommitted source (995de273863f3dafba84e2419db1a2bff3e9ede1)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/Cargo.toml) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/lib.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/client/builder.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/builder.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/client/live.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/live.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/protocol/llm.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/presets.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/presets.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- lingxi-app: HEAD 995de273863f3dafba84e2419db1a2bff3e9ede1; Referenced input fingerprint sha256:59a48c2ec865b01628e3194e04bcfde4ed1ec59fa38a475e88d90798852020a9
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
