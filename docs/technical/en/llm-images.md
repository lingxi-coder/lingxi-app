# Image API

Generate, edit, and query native image tasks through a service independent of Chat.

<a id="generate"></a>

## Generate an image

client.images() shares configuration and transport with Chat but uses its own requests, model catalog, credentials options, capabilities, and routes. Image-output models are excluded from chat().models().

### Image function · call from an async host

```rust
use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::{
    ImageGenerationRequest, ImageOutputOptions, ImageRequestOptions, Region, Secret,
};

async fn generate(api_key: String) -> Result<(), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let client = LlmClientBuilder::new(&profiles)?
        .with_region(Region::International).build()?;
    let request = ImageGenerationRequest {
        model: "gpt-image-1.5".into(),
        prompt: "An orange cat on a blue background".into(),
        references: vec![],
        output: ImageOutputOptions::default(),
        provider_options: Default::default(),
    };
    let options = ImageRequestOptions {
        credential: Some(Secret::new(api_key)),
        ..Default::default()
    };
    let result = client.images().generate_in("openai", &request, &options).await?;
    println!("{} images", result.images.len());
    Ok(())
}
```

Verification method: Compile check (llm-image-generation)

<a id="image-workflow-choice"></a>

## Choose immediate output or a durable task

1. Call capabilities_in for the exact profile/model, then construct only supported size, reference, mask and task options.
2. After submission, store the returned reference and poll at a host-chosen interval. Keep account_scope and credentials tied to that same account.
3. Treat an unknown submission outcome as reconciliation work. Do not create another image job simply because the first response was lost.

| Path | Use when | Persist |
| --- | --- | --- |
| generate_in / edit_in | The configured route returns completed image output | Output source, request ID, executed profile and usage; save temporary media promptly |
| submit_in → get_task | The selected provider exposes native asynchronous tasks | The exact ImageTaskRef before leaving the screen or restarting the process |

<a id="image-error-actions"></a>

## Recover from image errors without duplicate generation

| ImageError::dispatch() | What it establishes | Host action |
| --- | --- | --- |
| NotSent | This image operation did not dispatch generation | Fix local input, capability or scope and make a new admitted call |
| Rejected | The provider rejected the operation | Use the underlying typed error to correct the request/account |
| Unknown | The request may have been accepted | Retain attempt metadata and reconcile before resubmitting |
| Accepted | Provider acceptance is known even if result processing failed | Recover the existing result/task where possible |

<a id="image-methods"></a>

## Generation, editing & tasks

### ImageService::models

```text
pub fn models(&self) -> Vec<ImageModelListing>
```

List visible image models in the selected region.

### ImageService::capabilities_in

```text
pub fn capabilities_in(&self, profile: &str, model: &str) -> Result<ImageCapabilities, ImageError>
```

Inspect generation, reference, editing, mask, and native task support for an exact profile.

### ImageService::generate_in

```text
pub async fn generate_in(&self, profile: &str, req: &ImageGenerationRequest, opts: &ImageRequestOptions) -> Result<ImageResponse, ImageError>
```

Return completed images. generate() resolves by model.

### ImageService::edit_in

```text
pub async fn edit_in(&self, profile: &str, req: &ImageEditRequest, opts: &ImageRequestOptions) -> Result<ImageResponse, ImageError>
```

Edit supported source images; explicit unsupported options are rejected before sending.

### ImageService::submit_in

```text
pub async fn submit_in(&self, profile: &str, req: &ImageRequest, opts: &ImageRequestOptions) -> Result<ImageTaskRef, ImageError>
```

Submit one provider-native asynchronous task and return a serializable reference.

### ImageService::get_task

```text
pub async fn get_task(&self, task: &ImageTaskRef, opts: &ImageRequestOptions) -> Result<ImageTaskSnapshot, ImageError>
```

Query once with the original profile, valid credential, and matching account_scope.

<a id="inputs"></a>

## References, masks & outputs

- Supported inputs include HTTP(S) URLs, Base64 plus media type, and application-owned AttachmentRef values. Attachment references require a builder AttachmentResolver.
- ImageGenerationRequest.references adds references where supported. MiniMax character references use ImageReferenceKind::Character.
- Masks require explicit model support. OpenAI masks apply only to image_index = 0; other indices fail before sending.
- ImageResponse retains original URLs or Base64, executed profile, request ID, outcome, and available usage. Save temporary URLs promptly in the application.

<a id="image-providers"></a>

## Provider coverage

- OpenAI: generation, editing, and masks. Gemini: generation, references, and editing.
- Qwen regional profiles: generation, references, editing, and native tasks, subject to regional model availability. Custom Wan routes require an explicit workspace endpoint.
- xAI: generation and editing. MiniMax: generation and character references. GLM/Z.AI: generation and native tasks.
- OpenRouter: generation, references, and editing depend on the upstream model and endpoint. Chat compatibility alone does not enable image routes.

Image calls do not automatically retry, switch connections, poll tasks, or persist them. A timed-out provider may still be generating. Image usage is separate from Chat token cost estimates, and target-account validation is required.

<a id="image-lifecycle"></a>

## Image dispatch, persistence and failure

capabilities_in validates an exact profile/model before resolving image attachments or dispatching. Generate/edit return ImageResponse, while submit/get_task expose only a supported provider-native task route. Image task references retain the original connection, endpoint, model and account; the host persists them and chooses polling/cancellation when the provider supports it.

Retain native URLs/Base64 and output metadata before application processing. A provider timeout may leave generation running; a lost response must not silently become another image request. ImageError dispatch distinguishes not-sent, rejected, accepted and unknown outcomes for host recovery. Do not bill image generation through Chat token price assumptions.

- Application AttachmentRef inputs require an authorized resolver; provider image URL inputs remain provider-managed. Preserve existing transparency/mask semantics in the host rather than fabricating an unsupported option.
- images tests establish capability admission, encoding and typed task behavior with simulated responses. Live account model access, output quality, temporary URL expiration and target-device display require separate acceptance.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/images/mod.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/src/images/mod.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [llm-client/src/images/wire.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/src/images/wire.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [llm-client/src/protocol/image.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/src/protocol/image.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [llm-client/src/providers/dispatch.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/src/providers/dispatch.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [llm-client/tests/images.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/tests/images.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)

- llm-client: HEAD 66ce76a2ecc679f0dba551e26ef6aaceb40d2f49; Referenced input fingerprint sha256:89fef6f87fb7803bec69d768fa11e408a54f2123d196e2c859d8faba79092d96; Product pin 3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
