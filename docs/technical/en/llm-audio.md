# Cloud transcription & speech

Use exact-profile audio independently of chat, with explicit formats, bounded streams and dispatch-aware errors.

<a id="audio-route"></a>

## Exact audio route and host ownership

LlmClient::audio() and ClientSnapshot::audio() provide common file transcription and complete-text synthesis. AudioRoute names an exact profile and stable nonsecret account_scope. It does not resolve a profile group, infer an audio model from a chat model, use fallback_credentials, retry or fail over. The host decides whether the route follows a session or is an explicit override.

The facade checks independent AudioCapabilities. FileTranscription, LiveAsr, Synthesis, IncrementalSynthesis and NativeRealtime are distinct operations. Streaming audio output for complete text remains Synthesis; it is not incremental text input. supports() requires an available common adapter and declared model; operation() also describes provider-only entries.

### AudioService::capabilities

```text
pub fn capabilities(&self, route: &AudioRoute) -> Result<AudioCapabilities, AudioError>
```

Inspect route-specific operation/model/format/voice contracts without cloud dispatch.

### AudioService::transcribe

```text
pub async fn transcribe(
    &self, route: &AudioRoute, input: AudioInput,
    request: &TranscriptionRequest, options: &RequestOptions,
) -> Result<TranscriptionResult, AudioError>
```

Dispatch one admitted file-transcription operation; preserve text, timing, request provenance and optional usage.

### AudioService::synthesize

```text
pub async fn synthesize(
    &self, route: &AudioRoute, request: &SynthesisRequest,
    options: &RequestOptions,
) -> Result<AudioOutput, AudioError>
```

Return provider audio without capture, playback, conversion or resampling.

<a id="audio-application-recipe"></a>

## Wire speech into an application

1. Select an exact AudioRoute and query capabilities. Choose the declared audio model, operation, voice and format instead of inheriting the current chat model.
2. For synthesis, submit complete text with SynthesisRequest. For file transcription, provide the supported AudioInput and TranscriptionRequest. Device microphone access and file selection happen in the host first.
3. Consume output according to its actual type: byte stream or provider URL. Bound collected bytes; for raw PCM also retain the required sample rate/channel/encoding declaration.
4. Record terminal metadata and usage when available, then close playback resources owned by the application. Cancelling audio reads does not prove the provider performed no synthesis.

<a id="audio-troubleshooting"></a>

## Diagnose audio before retrying

| Symptom | Check |
| --- | --- |
| No supported voice/model | capabilities for the exact route and common adapter, not chat compatibility |
| Noise or wrong playback speed | Actual encoding, sample rate and channel count; the SDK does not own device resampling |
| Collection reaches a byte limit | Use incremental consumption or a host-selected larger bound; do not allocate without a bound |
| Error after dispatch | AudioError.kind and dispatch; distinguish rejected, accepted and unknown outcomes before resubmission |

<a id="audio-coverage"></a>

## Implemented common and native adapters

| Profile provider | Common adapter | Conditions and native boundaries |
| --- | --- | --- |
| OpenAI | File transcription; synthesis | Explicit native routes; native voices/translation/realtime remain separate |
| xAI | File transcription; synthesis | TTS has no model field and requires selected voice; native live ASR/incremental TTS/realtime |
| Gemini Developer API / Vertex | Synthesis | Separate models/auth; Vertex requires selected project/location; no dedicated common file ASR |
| MiniMax | File transcription; synthesis | Discover/select voice; native streaming/incremental/async TTS |
| Qwen | Synthesis | Profile extra.workspace_id; async URL ASR remains typed native service |
| GLM / Z.AI | Supported hosted file transcription | Mainland hosted TTS is provider-only; international hosted TTS is unsupported in this catalog |
| OpenRouter | Discovered transcription/speech models | No guessed model/voice/PCM geometry |

This table describes SDK admission and adapter implementation. Endpoint region and account entitlements remain separate facts. Unsupported operation/model/format or known size limits fail before input reads and transport.

<a id="audio-example"></a>

## Synthesize with a byte bound

RequestOptions supplies the exact account credential and optional total_timeout. A request-local account_scope must match AudioRoute. The common facade rejects authenticator/finalizer hooks that native services do not implement, and restricted ChatGPT/Copilot credentials do not grant cloud audio.

### Collect bounded provider audio for host playback

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::audio::{AudioOperation, AudioOutput, AudioRoute, SynthesisRequest};

pub async fn synthesize(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let route = AudioRoute::new("openai", "host-account-1");
    client.audio().capabilities(&route)?.model(AudioOperation::Synthesis, None)?;
    let output = client.audio().synthesize(
        &route, &SynthesisRequest::new("Hello, LingXi."), options,
    ).await?;
    match output {
        AudioOutput::Stream(stream) => {
            let audio = stream.collect(16 * 1024 * 1024).await?;
            println!("{} bytes; {:?}", audio.bytes.len(), audio.metadata);
        }
        AudioOutput::Url(audio) => {
            println!("Provider-owned URL; {:?}", audio.metadata);
        }
    }
    Ok(())
}
```

Verification method: Compile check (llm-speech)

This function inspects returned bytes; it does not play sound. Interpret audio.metadata before device playback. Headerless PCM is not a WAV file; absent geometry is unknown, and provider URLs are not fetched automatically.

<a id="audio-errors"></a>

## Streaming, deadlines & accounting

- AudioStream is pull-driven and preserves encoded provider bytes. collect(max_bytes) requires a positive bound; overflow reports MediaTooLarge with dispatch Accepted. Drop releases the underlying response.
- AudioError keeps kind, dispatch and the original typed provider source. Dispatch distinguishes NotSent, Rejected, Unknown and Accepted. A timeout conservatively marks uncertain acceptance and never authorizes automatic replay.
- AudioUsage retains optional token, duration, character and provider cost observations; missing values remain absent. Qwen terminal usage is available only after validated completion. Do not route audio usage through Chat token pricing by assumption.

unified_audio mock tests cover exact routes, size admission, one dispatch, formats and terminal usage. Live credentials, voices, device permissions, resampling and physical playback are independent acceptance tasks.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- llm-client/src/audio/mod.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/audio/types.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/audio/catalog.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/openai/audio/mod.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/qwen/tts/mod.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/hosting/vertex/speech/mod.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/unified_audio.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/audio.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
