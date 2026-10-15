# Attachments & provider files

Keep durable host attachments separate from scoped provider uploads, model input and generated artifacts.

<a id="identities"></a>

## Two identities, separate lifetimes

AttachmentRef is an immutable host-store reference: attachment_id, revision, filename, media_type and declared size. Keep it in conversation history and use the host attachment service for authenticated display on another device. Provider file IDs and URIs are request-time provider resources, with account, endpoint, protocol, purpose and expiry constraints; they cannot replace the host attachment record.

| Object | Owner and use | Failure boundary |
| --- | --- | --- |
| AttachmentRef | Host: durable original and immutable revision | Missing revision or access denial is returned by the resolver |
| ProviderFileRef / ProviderFileSource | SDK-scoped provider resource and codec input | Wrong account, endpoint, purpose or expired file fails validation |
| ProviderFileContent | Original bytes where a download operation is supported | Download support is independent of upload or text extraction |

<a id="resolve"></a>

## Resolve host attachments

1. Store original bytes under an immutable host ID/revision and validate the caller’s authority in AttachmentResolver.
2. Configure LlmClientBuilder::with_attachment_resolver. Add ImageSource::Attachment, DocumentSource::Attachment or VideoSource::Attachment to a message.
3. The SDK checks all declared metadata and the 64 MiB combined request bound before resolver I/O, resolves independent revisions concurrently, checks exact returned size, then plans inline or native upload input for the selected profile/model.
4. The original ChatRequest remains unchanged. Persist only host references; keep request-local prepared bytes and provider references out of the durable attachment identity.

### Build a client with an existing authorized host resolver

```rust
use std::sync::Arc;
use lingxi_llm_client::{builtin_providers, AttachmentResolver, LlmClient, LlmClientBuilder};
use lingxi_llm_client::protocol::{AttachmentRef, ChatRequest, ContentBlock, ConversationMessage, ImageSource, Region};

pub fn attachment_client(
    resolver: Arc<dyn AttachmentResolver>,
    attachment: AttachmentRef,
) -> Result<(LlmClient, ChatRequest), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let mut builder = LlmClientBuilder::new(&profiles)?.with_region(Region::International);
    builder.with_attachment_resolver(resolver);
    let client = builder.build()?;
    let mut message = ConversationMessage::user_text("Describe the attached image.");
    message.content.push(ContentBlock::Image {
        source: ImageSource::Attachment { attachment },
    });
    let mut request = ChatRequest::new("gpt-4.1-mini");
    request.messages.push(message);
    Ok((client, request))
}
```

Verification method: Compile check (llm-attachment-client)

The host resolver must read the exact authorized revision. validate_content_reuse() is opt-in; the SDK calls it before cached bytes or a shared read are used. Its default disables cross-call byte reuse, so resolve() remains the per-request authority check.

<a id="provider-management"></a>

## Manage provider files explicitly

Bind a typed provider client, then use its files() resource. Each operation captures one configuration snapshot and requires a nonempty file_account_scope (or account_scope). The low-level FileService is useful when the host already owns a Transport, profile and Authenticator. FileCapabilities is keyed by provider identity, actual endpoint/protocol, media type, purpose and model; an OpenAI-compatible chat endpoint does not imply a Files API.

- upload_stream consumes an UploadFileStream once. The declared size must equal exact EOF; unsupported routes are rejected before the input is polled. Custom transports must implement send_stream for incremental upload.
- Download, metadata, list, list_for_purpose, delete and extract_text have separate capability checks. Keep pagination cursors opaque. Foundry Files requires explicit Anthropic hosting and a canonical resource/account scope.

### Upload one host-owned file and retain its scope

```rust
use lingxi_llm_client::{FilePurpose, LlmClient, ProviderFileRef, RequestOptions, UploadFile};
use lingxi_llm_client::providers::OpenAiClient;

pub async fn upload_model_file(
    client: &LlmClient,
    file: &UploadFile,
    options: &RequestOptions,
) -> Result<ProviderFileRef, Box<dyn std::error::Error>> {
    // options must contain this account's credential and nonempty file_account_scope.
    let provider = client.provider::<OpenAiClient>("openai")?;
    let files = provider.files();
    let reference = files.upload(file, FilePurpose::ModelInput, options).await?;
    let metadata = files.get(&reference, options).await?;
    Ok(metadata.file)
}
```

Verification method: Compile check (llm-provider-upload)

<a id="retention-recovery"></a>

## Retention, uncertainty & recovery

- A stable file_account_scope allows account-isolated reuse of eligible automatic uploads. Without it, uploads are attempt-scoped. Qwen automatic uploads remain request-scoped and use explicit cleanup because they do not expire server-side.
- Eligible OpenAI, Anthropic and xAI automatic uploads use a 24-hour expiry and stop cache reuse after 23 hours. A file-identifying 404 can cause one fresh automatic upload only where replay is admitted; hosted side effects and continuations restrict replay.
- ProviderFileProcessing carries a scoped reference when Gemini processing remains pending or readiness cannot be established. Retain it and call resume_gemini_processing; do not create another upload just to restart polling.
- FileUploadError::OutcomeUnknown means provider acceptance is uncertain after dispatch. Reconcile through metadata/listing where available. with_timeout gives each low-level file operation a total budget; with_deadline shares an absolute deadline across calls. Neither adds retries.
- Explicit uploads are host-managed. Automatic Qwen deletion is attempted after response completion and retried in the background on interruption or deletion failure; process exit can leave orphan files, so the host needs periodic reconciliation.

Implementation evidence: provider_files, provider_file_clients, file_stream_upload and file_deadlines suites cover scope, byte limits and uncertain outcomes with mock transport. Real account retention, upload entitlements and cross-device attachment storage require separate acceptance.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/client/mod.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/mod.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/builder.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/builder.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/attachments/resolve.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/attachments/resolve.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/attachments/prepare.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/attachments/prepare.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/attachments/cache.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/attachments/cache.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/files/service.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/files/service.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/files/policy.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/files/policy.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/files/types.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/files/types.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/providers/file_resources.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/providers/file_resources.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/provider_files.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/provider_files.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/file_stream_upload.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/file_stream_upload.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/file_deadlines.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/file_deadlines.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)

- llm-client: HEAD 179d336607b2367cf0709a78c760c9333ec25bb3; Referenced input fingerprint sha256:64423ae6bcb4985bab0883b693378509bb7fe060e59bf4facafedcb4d2f32293; Product pin 450d52b12b6d88a377ab3332c3b8a6a1820e2b7c

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
