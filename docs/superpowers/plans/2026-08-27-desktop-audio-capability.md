# 桌面音频能力 实施计划（Part B）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给桌面补上今天完全没有的音频能力（录音 / STT / TTS），并在设置里落一个诚实反映真实能力的语音页。

**Architecture:** 照搬移动端的**分工**而非技术栈。移动端是客户端用原生 API 实现能力、经 UniFFI 把 `Arc<dyn>` 注入引擎；桌面的镜像是 Electron 实现能力、经桥注入 `engine-desktop`。`bridge-server` 侧放三个代理实现，把 trait 调用变成对客户端的请求——沿用仓库既有的引擎→客户端请求/应答模式（`ComputerAccessRequest` 就是这个形状），不发明新机制。引擎侧不引入任何音频依赖。

**Tech Stack:** Rust（client-protocol / bridge-server / engine-desktop / tool-mobile）、TypeScript + React（Electron 渲染进程的 `getUserMedia` / `MediaRecorder` / `speechSynthesis`）、`node --test`、`cargo test`。

**Spec:** `docs/superpowers/specs/2026-08-27-desktop-settings-redesign-design.md`（Part B）

**依赖:** 本计划的 Task 9（语音设置页）依赖 Part A 的 Task 12/14/15（行原语、导航声明、设置外壳）。Task 1-8 可与 Part A 并行。

## Global Constraints

- `CLIENT_PROTOCOL_VERSION` 保持 `"8.0.0"`。新增全部是变体与可选字段，属 F1-09 guard 的 additive，**不得 bump，不得 re-bless**。
- **偏好模型逐字镜像移动端**，一个键都不改：`schemaVersion=2`、`recognitionMode`(`automatic`|`onDevice`)、`language`(`"auto"`|BCP-47)、`voiceSelection`(`system:<id>`|`sherpa:<modelId>:<voiceId>`|`system:default`)、`rate`(0.5–2.0)、`autoPlayReplies`。真源：`clients/ios/Sources/Voice/VoiceRuntimeConfiguration.swift` 与 `clients/android/app/src/main/java/com/lingxi/code/settings/VoiceSettingsCapabilities.kt`。
- **桌面 v1 的 `onDevice` 模式诚实地不可用**：Rust 侧没有 Sherpa 绑定。它落进移动端模型里已有的 `Unavailable` 槽位并给出 `fallbackReason`，不静默降级。
- **桌面 STT 走已配置 provider 的转写接口**：Chromium 的 `SpeechRecognition` 在 Electron 中不可用（依赖 Google 服务与密钥）。无凭据时落进 `blockingIssues`，不静默失败。
- Rust 测试：`cargo test -p <crate>`，**捕获完整输出到文件再 grep**，绝不单独 grep `FAILED`。
- Electron 测试：`cd clients/electron && npm test`；类型检查 `npm run typecheck`。

---

## 文件结构

**Rust 新建**
- `lingxi-code/apps/bridge-server/src/audio_bridge.rs` —— 三个代理实现 + 请求路由

**Rust 修改**
- `lingxi-code/client-protocol/src/{commands.rs,events.rs,lib.rs}` —— 音频请求/应答契约
- `lingxi-code/apps/bridge-server/src/router.rs` —— 应答分发
- `lingxi-code/apps/engine-desktop/src/lib.rs:8827-8829` —— 注入点
- `lingxi-code/tools/mobile/src/{speech.rs,voice.rs,lib.rs}（包名 `tool-mobile`）` —— 注册门 + 模块注释

**Electron 新建**
- `clients/electron/src/renderer/audio/{capture.ts,synthesis.ts,transcription.ts,capabilities.ts,preferences.ts}`
- `clients/electron/src/renderer/components/settings/pages/Voice.tsx`

**Electron 修改**
- `clients/electron/src/main/{settings.ts,host-utils.ts}` —— 语音偏好持久化
- `clients/electron/Info.plist`（或打包脚本注入）—— `NSMicrophoneUsageDescription`

---

## Task 1: 音频请求/应答契约

**Files:**
- Modify: `lingxi-code/client-protocol/src/events.rs`、`commands.rs`、`lib.rs`
- Test: `lingxi-code/client-protocol/tests/events_test.rs`

**Interfaces:**
- Produces:
  - `ClientEvent::AudioRequest { request_id: u64, op: AudioOpDto }`
  - `ClientCommand::AudioResponse { request_id: u64, result: AudioResultDto }`
  - `AudioOpDto { StartRecording { sample_rate_hz: u32, format: String }, StopRecording, IsRecording, Transcribe { language: Option<String> }, Synthesize { text: String, voice: Option<String> } }`
  - `AudioResultDto { Recording { audio_base64: String, mime_type: String }, Recording State { recording: bool }, Transcript { text, language, confidence }, Audio { pcm_base64: String, sample_rate_hz: u32 }, Ok, Failed { message: String } }`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn audio_request_and_response_round_trip_on_the_wire() {
    let request = ClientEvent::AudioRequest {
        request_id: 7,
        op: AudioOpDto::Transcribe { language: Some("zh-CN".to_string()) },
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["type"], "audio_request");
    assert_eq!(json["op"]["type"], "transcribe");

    let response = ClientCommand::AudioResponse {
        request_id: 7,
        result: AudioResultDto::Transcript {
            text: "你好".to_string(),
            language: Some("zh-CN".to_string()),
            confidence: Some(0.9),
        },
    };
    let round: ClientCommand =
        serde_json::from_value(serde_json::to_value(&response).unwrap()).unwrap();
    assert_eq!(round, response, "the response must survive a wire round trip");
}

#[test]
fn adding_audio_does_not_bump_the_protocol() {
    assert_eq!(
        client_protocol::CLIENT_PROTOCOL_VERSION, "8.0.0",
        "audio adds variants only — additive under the F1-09 guard"
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p client-protocol audio 2>&1 | tee /tmp/b1.log`
Expected: 编译失败，`cannot find type \`AudioOpDto\``

- [ ] **Step 3: 最小实现**

```rust
/// 引擎请客户端执行的一次音频操作。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioOpDto {
    StartRecording { sample_rate_hz: u32, format: String },
    StopRecording,
    IsRecording,
    Transcribe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
    Synthesize {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
    },
}

/// 客户端对一次音频操作的应答。二进制载荷走 base64，与 `ImageRefDto` 同约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioResultDto {
    Ok,
    RecordingState { recording: bool },
    Recording { audio_base64: String, mime_type: String },
    Transcript {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f32>,
    },
    Audio { pcm_base64: String, sample_rate_hz: u32 },
    Failed { message: String },
}
```

`ClientEvent` 加 `AudioRequest { request_id: u64, op: AudioOpDto }`；`ClientCommand` 加 `AudioResponse { request_id: u64, result: AudioResultDto }`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p client-protocol 2>&1 | tee /tmp/b1.log; grep -E "^test result|failures:" /tmp/b1.log`
Expected: PASS，版本仍 `8.0.0`，`git diff --stat lingxi-code/client-protocol/snapshots/` 为空

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/client-protocol
git commit -m "Let the engine ask the client to record, transcribe and speak"
```

---

## Task 2: 三个代理实现

**Files:**
- Create: `lingxi-code/apps/bridge-server/src/audio_bridge.rs`
- Modify: `lingxi-code/apps/bridge-server/src/lib.rs`、`router.rs`
- Test: `lingxi-code/apps/bridge-server/src/audio_bridge.rs`

**Interfaces:**
- Consumes: Task 1 的 `AudioOpDto` / `AudioResultDto`；`platform_api::{stt::SpeechToText, tts::TextToSpeech, voice::VoiceRecorder}`
- Produces:
  - `pub struct AudioBridge`（持有请求分发与应答等待表）
  - `pub fn new_audio_bridge(sink: Arc<dyn EventSink>) -> (Arc<AudioBridge>, AudioResponder)`
  - `AudioBridge` 实现 `SpeechToText` / `TextToSpeech` / `VoiceRecorder`
  - `AudioResponder::resolve(request_id: u64, result: AudioResultDto)`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::stt::SttOpts;

    #[tokio::test]
    async fn transcribe_emits_a_request_and_resolves_on_the_client_response() {
        let (bridge, responder, mut emitted) = test_bridge();

        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge.transcribe(SttOpts { language: Some("zh-CN".into()) }).await
            }
        });

        let request = emitted.recv().await.expect("transcribe must emit an AudioRequest");
        let (request_id, op) = match request {
            ClientEvent::AudioRequest { request_id, op } => (request_id, op),
            other => panic!("expected AudioRequest, got {other:?}"),
        };
        assert!(matches!(op, AudioOpDto::Transcribe { .. }));

        responder.resolve(request_id, AudioResultDto::Transcript {
            text: "你好".into(), language: Some("zh-CN".into()), confidence: None,
        });

        let transcript = task.await.unwrap().unwrap();
        assert_eq!(transcript.text, "你好");
    }

    #[tokio::test]
    async fn a_failed_response_becomes_an_stt_error_not_an_empty_transcript() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.transcribe(SttOpts { language: None }).await }
        });
        let request_id = match emitted.recv().await.unwrap() {
            ClientEvent::AudioRequest { request_id, .. } => request_id,
            other => panic!("expected AudioRequest, got {other:?}"),
        };
        responder.resolve(request_id, AudioResultDto::Failed {
            message: "no microphone permission".into(),
        });
        let error = task.await.unwrap().unwrap_err();
        assert!(
            format!("{error}").contains("no microphone permission"),
            "a client-side failure must surface as an error carrying its reason, got: {error}"
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p bridge-server audio_bridge 2>&1 | tee /tmp/b2.log`
Expected: 编译失败，模块不存在

- [ ] **Step 3: 最小实现**

```rust
//! 音频能力的桥接代理。
//!
//! 移动端由 Swift / Kotlin 经 UniFFI 注入 `Arc<dyn SpeechToText>` 等实现；桌面
//! 没有原生实现，改由 Electron 提供，本模块把 trait 调用变成一次
//! `ClientEvent::AudioRequest` 并等待 `ClientCommand::AudioResponse`。形状与
//! 既有的 ComputerAccess 请求/应答一致。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use client_protocol::{AudioOpDto, AudioResultDto, ClientEvent};
use tokio::sync::oneshot;
use platform_api::stt::{SpeechToText, SttError, SttOpts, SttTranscript};
use platform_api::tts::{TextToSpeech, TtsAudio, TtsError, TtsOpts};
use platform_api::voice::{VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts};

/// 等待中的音频请求表。
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<AudioResultDto>>>>;

pub struct AudioBridge {
    next_id: AtomicU64,
    pending: Pending,
    emit: Arc<dyn Fn(ClientEvent) + Send + Sync>,
}

/// 应答入口，交给 router 在收到 `AudioResponse` 时调用。
#[derive(Clone)]
pub struct AudioResponder { pending: Pending }

impl AudioResponder {
    /// 兑现一个等待中的请求。未知 request_id 静默忽略（客户端可能重发）。
    pub fn resolve(&self, request_id: u64, result: AudioResultDto) {
        if let Some(tx) = self.pending.lock().expect("pending lock").remove(&request_id) {
            let _ = tx.send(result);
        }
    }
}

pub fn new_audio_bridge(
    emit: Arc<dyn Fn(ClientEvent) + Send + Sync>,
) -> (Arc<AudioBridge>, AudioResponder) {
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let bridge = Arc::new(AudioBridge {
        next_id: AtomicU64::new(1),
        pending: pending.clone(),
        emit,
    });
    (bridge, AudioResponder { pending })
}

impl AudioBridge {
    async fn request(&self, op: AudioOpDto) -> AudioResultDto {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(request_id, tx);
        (self.emit)(ClientEvent::AudioRequest { request_id, op });
        rx.await.unwrap_or(AudioResultDto::Failed {
            message: "the client disconnected before answering the audio request".to_string(),
        })
    }
}

#[async_trait]
impl SpeechToText for AudioBridge {
    async fn transcribe(&self, opts: SttOpts) -> Result<SttTranscript, SttError> {
        match self.request(AudioOpDto::Transcribe { language: opts.language }).await {
            AudioResultDto::Transcript { text, language, confidence } =>
                Ok(SttTranscript { text, language, confidence }),
            AudioResultDto::Failed { message } => Err(SttError::Other(message)),
            other => Err(SttError::Other(format!("unexpected audio result: {other:?}"))),
        }
    }
}

#[async_trait]
impl TextToSpeech for AudioBridge {
    async fn synthesize(&self, opts: TtsOpts) -> Result<TtsAudio, TtsError> {
        match self.request(AudioOpDto::Synthesize { text: opts.text, voice: opts.voice }).await {
            AudioResultDto::Audio { pcm_base64, sample_rate_hz } => {
                use base64::Engine as _;
                let pcm = base64::engine::general_purpose::STANDARD
                    .decode(pcm_base64)
                    .map_err(|e| TtsError::SynthesisFailed(format!("client returned undecodable audio: {e}")))?;
                Ok(TtsAudio { pcm, sample_rate_hz })
            }
            AudioResultDto::Failed { message } => Err(TtsError::Other(message)),
            other => Err(TtsError::Other(format!("unexpected audio result: {other:?}"))),
        }
    }
}

#[async_trait]
impl VoiceRecorder for AudioBridge {
    async fn start_recording(&self, opts: VoiceRecordingOpts) -> Result<(), VoiceError> {
        match self.request(AudioOpDto::StartRecording {
            sample_rate_hz: opts.sample_rate_hz, format: opts.format,
        }).await {
            AudioResultDto::Ok => Ok(()),
            AudioResultDto::Failed { message } => Err(VoiceError::Other(message)),
            other => Err(VoiceError::Other(format!("unexpected audio result: {other:?}"))),
        }
    }

    async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
        match self.request(AudioOpDto::StopRecording).await {
            AudioResultDto::Recording { audio_base64, mime_type } => {
                use base64::Engine as _;
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(audio_base64)
                    .map_err(|e| VoiceError::Other(format!("client returned undecodable audio: {e}")))?;
                Ok(VoiceRecording { audio_bytes, mime_type })
            }
            AudioResultDto::Failed { message } => Err(VoiceError::Other(message)),
            other => Err(VoiceError::Other(format!("unexpected audio result: {other:?}"))),
        }
    }

    async fn is_recording(&self) -> bool {
        matches!(
            self.request(AudioOpDto::IsRecording).await,
            AudioResultDto::RecordingState { recording: true }
        )
    }
}
```

> 错误变体名已核实（`platform-api/src/{stt,tts,voice}.rs`）：
> `SttError::{PermissionDenied, NoSpeech, Unavailable, Busy, Retriable(String), Other(String)}`；
> `TtsError::{Unavailable, SynthesisFailed(String), Other(String)}`；
> `VoiceError::{PermissionDenied, NotRecording, Busy, Other(String)}`。
> **没有 `Failed` 变体。** 客户端侧的失败一律映射到 `Other(message)`（TTS 的解码失败用
> `SynthesisFailed`），好让原因逐字传到上层。

`router.rs` 加分支：

```rust
            ClientCommand::AudioResponse { request_id, result } => {
                self.audio_responder.resolve(request_id, result);
            }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p bridge-server audio_bridge 2>&1 | tee /tmp/b2.log; grep -E "^test result|failures:" /tmp/b2.log`
Expected: 两条 PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/bridge-server
git commit -m "Proxy the audio traits across the bridge"
```

---

## Task 3: 注入点与工具注册门

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs:8827-8829`
- Modify: `lingxi-code/tools/mobile/src/lib.rs`（注册门）、`speech.rs`、`voice.rs`（模块注释）
- Test: `lingxi-code/apps/engine-desktop/tests/audio_injection.rs`

**Interfaces:**
- Consumes: Task 2 的 `new_audio_bridge`
- Produces: 桌面的工具上下文在音频能力存在时携带 `voice` / `stt` / `tts`，且 `speech` / `voice` 两个工具被注册

- [ ] **Step 1: 写失败测试**

```rust
/// 没有音频桥时，桌面上下文不带音频能力，两个工具也不注册 ——
/// 这是今天的行为，先钉住它，免得后面的改动悄悄把工具在无能力时也注册了。
#[test]
fn without_an_audio_bridge_the_desktop_has_no_audio_tools() {
    let ctx = build_desktop_tool_context(None);
    assert!(ctx.stt.is_none());
    assert!(ctx.tts.is_none());
    assert!(ctx.voice.is_none());
    assert!(
        !registered_tool_names(&ctx).contains(&"speech".to_string()),
        "the speech tool must not be registered without a capability behind it"
    );
}

#[test]
fn with_an_audio_bridge_the_desktop_registers_speech_and_voice() {
    let (bridge, _responder) = new_audio_bridge(Arc::new(|_| {}));
    let ctx = build_desktop_tool_context(Some(bridge));
    assert!(ctx.stt.is_some());
    assert!(ctx.tts.is_some());
    assert!(ctx.voice.is_some());
    let names = registered_tool_names(&ctx);
    assert!(names.contains(&"speech".to_string()));
    assert!(names.contains(&"voice".to_string()));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd lingxi-code && cargo test -p engine-desktop audio_injection 2>&1 | tee /tmp/b3.log`
Expected: 第一条 PASS（钉住现状），第二条编译失败或断言失败

- [ ] **Step 3: 最小实现**

`engine-desktop/src/lib.rs` 把 8827-8829 三行的 `None` 改为按能力注入：

```rust
        voice: audio.clone().map(|b| b as Arc<dyn VoiceRecorder>),
        stt: audio.clone().map(|b| b as Arc<dyn SpeechToText>),
        tts: audio.map(|b| b as Arc<dyn TextToSpeech>),
```

`tools/mobile/src/lib.rs` 的注册函数改为在能力存在时才注册：

```rust
/// 注册音频相关工具。仅在对应能力存在时注册 —— 一个背后没有实现的工具
/// 比没有这个工具更糟。
pub fn register_audio_tools(reg: &mut ToolRegistry, ctx: &BuiltinToolContext) {
    if ctx.stt.is_some() || ctx.tts.is_some() {
        reg.register_builtin(Arc::new(SpeechTool::new(ctx.clone())));
    }
    if ctx.voice.is_some() {
        reg.register_builtin(Arc::new(VoiceTool::new(ctx.clone())));
    }
}
```

`speech.rs` / `voice.rs` 的模块注释首行「mobile-exclusive」改为记录事实：这两个工具在能力存在的任何平台上注册；桌面的能力经 `bridge-server` 的 `audio_bridge` 提供。

> crate 名 `tools/mobile` 从此不再准确，**不改名** —— 最小改动优于顺手重构。在 `lib.rs` 顶部记一句即可。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd lingxi-code && cargo test -p engine-desktop -p tool-mobile 2>&1 | tee /tmp/b3.log; grep -E "^test result|failures:" /tmp/b3.log`
Expected: 两条都 PASS

- [ ] **Step 5: 提交**

```bash
git add lingxi-code/apps/engine-desktop lingxi-code/tools/mobile
git commit -m "Register the audio tools only where an implementation exists"
```

---

## Task 4: 语音偏好（镜像移动端）

**Files:**
- Create: `clients/electron/src/renderer/audio/preferences.ts`
- Modify: `clients/electron/src/main/{settings.ts,host-utils.ts}`
- Test: `clients/electron/test/voice-preferences.test.ts`

**Interfaces:**
- Produces:
  - `export interface VoicePreferences { schemaVersion: 2; recognitionMode: 'automatic' | 'onDevice'; language: string; voiceSelection: string; rate: number; autoPlayReplies: boolean }`
  - `export function normalizeVoiceSelection(raw: string | undefined): string`
  - `export function parseVoicePreferences(value: unknown): VoicePreferences`

- [ ] **Step 1: 写失败测试 —— 与 iOS/Android 的解析结果逐字对齐**

```typescript
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeVoiceSelection, parseVoicePreferences } from '../src/renderer/audio/preferences';

// 这几条用例的期望值直接取自 iOS 的 VoicePreferencesSnapshot.normalizeVoiceSelection
// 与 Android 的 VoiceConfig，三端必须给出同样的解析结果。
test('voice selection normalizes exactly as iOS and Android do', () => {
  assert.equal(normalizeVoiceSelection(undefined), 'system:default');
  assert.equal(normalizeVoiceSelection(''), 'system:default');
  assert.equal(normalizeVoiceSelection('default'), 'system:default');
  assert.equal(normalizeVoiceSelection('  Alex  '), 'system:Alex');
  assert.equal(normalizeVoiceSelection('system:Alex'), 'system:Alex');
  assert.equal(normalizeVoiceSelection('sherpa:vits-zh:0'), 'sherpa:vits-zh:0');
});

test('rate is clamped to the mobile range', () => {
  assert.equal(parseVoicePreferences({ rate: 9 }).rate, 2);
  assert.equal(parseVoicePreferences({ rate: 0.1 }).rate, 0.5);
  assert.equal(parseVoicePreferences({ rate: 1.25 }).rate, 1.25);
});

test('a fresh install defaults to automatic, matching mobile', () => {
  const fresh = parseVoicePreferences({});
  assert.equal(fresh.recognitionMode, 'automatic');
  assert.equal(fresh.language, 'auto');
  assert.equal(fresh.voiceSelection, 'system:default');
  assert.equal(fresh.rate, 1);
  assert.equal(fresh.schemaVersion, 2);
});

test('an unknown recognition mode falls back to automatic, not to garbage', () => {
  assert.equal(parseVoicePreferences({ recognitionMode: 'telepathy' }).recognitionMode, 'automatic');
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b4.log; grep -E "not ok" /tmp/b4.log`
Expected: FAIL，模块不存在

- [ ] **Step 3: 最小实现**

```typescript
/**
 * 桌面语音偏好。键与取值逐字镜像 iOS 的 `VoicePreferencesSnapshot`
 * （`clients/ios/Sources/Voice/VoiceRuntimeConfiguration.swift`）与 Android 的
 * `VoiceConfig`，好让三端说同一种话。
 */
export interface VoicePreferences {
  schemaVersion: 2;
  recognitionMode: 'automatic' | 'onDevice';
  language: string;
  voiceSelection: string;
  rate: number;
  autoPlayReplies: boolean;
}

export const DEFAULT_VOICE_SELECTION = 'system:default';

/** iOS `normalizeVoiceSelection` 的移植。 */
export function normalizeVoiceSelection(raw: string | undefined): string {
  const value = (raw ?? '').trim();
  if (value === '' || value === 'default') return DEFAULT_VOICE_SELECTION;
  if (value.startsWith('system:') || value.startsWith('sherpa:')) return value;
  return `system:${value}`;
}

export function parseVoicePreferences(value: unknown): VoicePreferences {
  const raw = (value ?? {}) as Record<string, unknown>;
  const mode = raw['recognitionMode'];
  const rate = typeof raw['rate'] === 'number' ? raw['rate'] : 1;
  return {
    schemaVersion: 2,
    recognitionMode: mode === 'onDevice' ? 'onDevice' : 'automatic',
    language: typeof raw['language'] === 'string' && raw['language'] !== ''
      ? raw['language'] : 'auto',
    voiceSelection: normalizeVoiceSelection(
      typeof raw['voiceSelection'] === 'string' ? raw['voiceSelection'] : undefined),
    rate: Math.min(2, Math.max(0.5, rate)),
    autoPlayReplies: raw['autoPlayReplies'] === true,
  };
}
```

`main/settings.ts` 与 `host-utils.ts` 加 `voice?: VoicePreferences` 字段，解析走 `parseVoicePreferences`，写入走既有的 `update()` 模式。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b4.log; grep -E "^# (pass|fail)" /tmp/b4.log && npm run typecheck`
Expected: 四条全 PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/audio/preferences.ts clients/electron/src/main clients/electron/test/voice-preferences.test.ts
git commit -m "Speak the same voice preferences as iOS and Android"
```

---

## Task 5: 能力探测

**Files:**
- Create: `clients/electron/src/renderer/audio/capabilities.ts`
- Test: `clients/electron/test/voice-capabilities-probe.test.ts`

**Interfaces:**
- Produces:
  - `export interface VoicePlatformSnapshot { localeTag, microphonePermission, providerConfigured, systemVoices, defaultSystemVoiceId }`
  - `export interface VoiceOption { id, label, languageTag, source: 'system' | 'sherpa', familyId, isDefault?, networkRequired? }`
  - `export async function probePlatform(deps): Promise<VoicePlatformSnapshot>`

- [ ] **Step 1: 写失败测试**

```typescript
test('system voices are read from speechSynthesis and tagged system:<id>', async () => {
  const snapshot = await probePlatform({
    voices: [
      { name: 'Alex', lang: 'en-US', default: true, localService: true },
      { name: 'Tingting', lang: 'zh-CN', default: false, localService: false },
    ],
    permission: 'granted',
    providerConfigured: true,
    localeTag: 'zh-CN',
  } as never);

  assert.deepEqual(
    snapshot.systemVoices.map((v) => v.id), ['system:Alex', 'system:Tingting'],
    'every enumerated voice must carry the shared system:<id> selection grammar',
  );
  assert.equal(snapshot.defaultSystemVoiceId, 'system:Alex');
  assert.equal(
    snapshot.systemVoices[1].networkRequired, true,
    'a non-local voice must be marked as needing the network, as Android does',
  );
});

test('no voices enumerated yields an empty list, not a fabricated default', async () => {
  const snapshot = await probePlatform({
    voices: [], permission: 'denied', providerConfigured: false, localeTag: 'en-US',
  } as never);
  assert.deepEqual(snapshot.systemVoices, []);
  assert.equal(snapshot.microphonePermission, 'denied');
  assert.equal(snapshot.providerConfigured, false);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b5.log; grep -E "not ok" /tmp/b5.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

```typescript
export type VoicePermissionStatus = 'granted' | 'denied' | 'unknown';

export interface VoiceOption {
  id: string;
  label: string;
  languageTag: string;
  source: 'system' | 'sherpa';
  familyId: string;
  isDefault?: boolean;
  networkRequired?: boolean;
}

export interface VoicePlatformSnapshot {
  localeTag: string;
  microphonePermission: VoicePermissionStatus;
  /** 桌面的 STT 走 provider 转写，所以「识别器可用」等价于「有可用凭据」。 */
  providerConfigured: boolean;
  systemVoices: VoiceOption[];
  defaultSystemVoiceId: string;
}

export interface ProbeDeps {
  voices: SpeechSynthesisVoice[];
  permission: VoicePermissionStatus;
  providerConfigured: boolean;
  localeTag: string;
}

export async function probePlatform(deps: ProbeDeps): Promise<VoicePlatformSnapshot> {
  const systemVoices: VoiceOption[] = deps.voices.map((voice) => ({
    id: `system:${voice.name}`,
    label: voice.name,
    languageTag: voice.lang || deps.localeTag,
    source: 'system',
    familyId: 'system',
    isDefault: voice.default,
    networkRequired: !voice.localService,
  }));
  const fallbackDefault = systemVoices.find((v) => v.isDefault)?.id ?? 'system:default';
  return {
    localeTag: deps.localeTag,
    microphonePermission: deps.permission,
    providerConfigured: deps.providerConfigured,
    systemVoices,
    defaultSystemVoiceId: fallbackDefault,
  };
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b5.log; grep -E "^# (pass|fail)" /tmp/b5.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/audio/capabilities.ts clients/electron/test/voice-capabilities-probe.test.ts
git commit -m "Probe the desktop's real voice capabilities"
```

---

## Task 6: 能力快照解析器

**Files:**
- Modify: `clients/electron/src/renderer/audio/capabilities.ts`
- Test: `clients/electron/test/voice-capability-snapshot.test.ts`

**Interfaces:**
- Consumes: Task 4 的 `VoicePreferences`、Task 5 的 `VoicePlatformSnapshot`
- Produces:
  - `export type VoiceBlockingIssue = 'MicrophonePermissionRequired' | 'ProviderCredentialRequired' | 'OnDeviceUnsupportedOnDesktop' | 'RequestedVoiceUnavailable' | 'PlaybackVoiceUnavailable'`
  - `export interface VoiceCapabilitySnapshot { microphonePermission, requestedRecognitionBackend, effectiveRecognitionBackend, effectiveLanguage, voiceOptions, requestedVoice, effectiveVoice, blockingIssues, fallbackReason }`
  - `export function resolveCapabilities(prefs, platform): VoiceCapabilitySnapshot`

- [ ] **Step 1: 写失败测试 —— 表驱动，每条都点名具体的 issue**

```typescript
test('no microphone permission blocks recognition and says so', () => {
  const snap = resolveCapabilities(prefs(), platform({ permission: 'denied' }));
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(snap.blockingIssues.includes('MicrophonePermissionRequired'));
});

test('no provider credential blocks recognition on desktop specifically', () => {
  const snap = resolveCapabilities(prefs(), platform({ providerConfigured: false }));
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(
    snap.blockingIssues.includes('ProviderCredentialRequired'),
    'desktop STT runs through a provider, so a missing credential must be named, not silently swallowed',
  );
});

test('onDevice is honestly unavailable on desktop and explains why', () => {
  const snap = resolveCapabilities(prefs({ recognitionMode: 'onDevice' }), platform());
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(snap.blockingIssues.includes('OnDeviceUnsupportedOnDesktop'));
  assert.match(
    snap.fallbackReason ?? '', /offline/i,
    'the reason must say there is no offline model on desktop, not just fail',
  );
});

test('a fully configured desktop resolves to the provider backend with no issues', () => {
  const snap = resolveCapabilities(prefs(), platform());
  assert.equal(snap.effectiveRecognitionBackend, 'provider');
  assert.deepEqual(
    snap.blockingIssues, [],
    'if issues were reported here, the three tests above would prove nothing',
  );
  assert.equal(snap.fallbackReason, null);
});

test('a requested voice that no longer exists falls back and says so', () => {
  const snap = resolveCapabilities(prefs({ voiceSelection: 'system:Ghost' }), platform());
  assert.notEqual(snap.effectiveVoice?.id, 'system:Ghost');
  assert.ok(snap.blockingIssues.includes('RequestedVoiceUnavailable'));
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b6.log; grep -E "not ok" /tmp/b6.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

```typescript
export type VoiceBlockingIssue =
  | 'MicrophonePermissionRequired'
  | 'ProviderCredentialRequired'
  | 'OnDeviceUnsupportedOnDesktop'
  | 'RequestedVoiceUnavailable'
  | 'PlaybackVoiceUnavailable';

export interface VoiceCapabilitySnapshot {
  microphonePermission: VoicePermissionStatus;
  requestedRecognitionBackend: VoicePreferences['recognitionMode'];
  effectiveRecognitionBackend: 'provider' | 'unavailable';
  effectiveLanguage: string;
  voiceOptions: VoiceOption[];
  requestedVoice: VoiceOption | null;
  effectiveVoice: VoiceOption | null;
  blockingIssues: VoiceBlockingIssue[];
  fallbackReason: string | null;
}

/**
 * 把偏好与平台探测结果解析成一份能力快照。
 *
 * 结构镜像 Android 的 `VoiceSettingsCapabilityResolver.resolve`：requested 与
 * effective 分开，差异一律以 blockingIssues + fallbackReason 说明，不静默降级。
 */
export function resolveCapabilities(
  prefs: VoicePreferences,
  platform: VoicePlatformSnapshot,
): VoiceCapabilitySnapshot {
  const effectiveLanguage = prefs.language === 'auto' ? platform.localeTag : prefs.language;
  const micGranted = platform.microphonePermission === 'granted';
  const blockingIssues: VoiceBlockingIssue[] = [];

  if (!micGranted) blockingIssues.push('MicrophonePermissionRequired');
  if (prefs.recognitionMode === 'onDevice') blockingIssues.push('OnDeviceUnsupportedOnDesktop');
  if (prefs.recognitionMode === 'automatic' && !platform.providerConfigured) {
    blockingIssues.push('ProviderCredentialRequired');
  }

  const effectiveRecognitionBackend =
    micGranted && prefs.recognitionMode === 'automatic' && platform.providerConfigured
      ? 'provider' as const
      : 'unavailable' as const;

  const voiceOptions = platform.systemVoices;
  const requestedVoice = voiceOptions.find((v) => v.id === prefs.voiceSelection) ?? null;
  const effectiveVoice = requestedVoice
    ?? voiceOptions.find((v) => v.id === platform.defaultSystemVoiceId)
    ?? voiceOptions[0]
    ?? null;

  if (prefs.voiceSelection !== 'system:default' && !requestedVoice && effectiveVoice) {
    blockingIssues.push('RequestedVoiceUnavailable');
  }
  if (!effectiveVoice) blockingIssues.push('PlaybackVoiceUnavailable');

  const fallbackReason =
    prefs.recognitionMode === 'onDevice'
      ? 'this desktop build ships no offline recognition model; on-device mode is unavailable here'
      : !platform.providerConfigured && prefs.recognitionMode === 'automatic'
        ? 'desktop speech recognition runs through a configured provider; connect one to enable it'
        : prefs.voiceSelection !== 'system:default' && !requestedVoice && effectiveVoice
          ? 'the requested voice is no longer installed; using the nearest available voice'
          : null;

  return {
    microphonePermission: platform.microphonePermission,
    requestedRecognitionBackend: prefs.recognitionMode,
    effectiveRecognitionBackend,
    effectiveLanguage,
    voiceOptions,
    requestedVoice,
    effectiveVoice,
    blockingIssues,
    fallbackReason,
  };
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b6.log; grep -E "^# (pass|fail)" /tmp/b6.log`
Expected: 五条全 PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/audio/capabilities.ts clients/electron/test/voice-capability-snapshot.test.ts
git commit -m "Say why voice cannot run instead of failing quietly"
```

---

## Task 7: 录音与合成

**Files:**
- Create: `clients/electron/src/renderer/audio/{capture.ts,synthesis.ts}`
- Test: `clients/electron/test/voice-capture.test.ts`

**Interfaces:**
- Produces:
  - `export class MicrophoneCapture { start(opts): Promise<void>; stop(): Promise<{ audioBase64: string; mimeType: string }>; isRecording(): boolean }`
  - `export async function synthesize(text, voiceId, rate): Promise<{ pcmBase64: string; sampleRateHz: number }>`

- [ ] **Step 1: 写失败测试**

```typescript
test('stop before start is an error, not an empty recording', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps());
  await assert.rejects(
    () => capture.stop(),
    /not recording/i,
    'returning an empty clip would look like a silent microphone',
  );
});

test('start then stop returns the captured bytes and the real mime type', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps({ chunk: 'AAEC', mimeType: 'audio/webm' }));
  await capture.start({ sampleRateHz: 16000, format: 'webm' });
  assert.equal(capture.isRecording(), true);
  const result = await capture.stop();
  assert.equal(capture.isRecording(), false);
  assert.equal(result.mimeType, 'audio/webm');
  assert.ok(result.audioBase64.length > 0);
});

test('a denied microphone surfaces the permission error verbatim', async () => {
  const capture = new MicrophoneCapture(fakeMediaDeps({ denyPermission: true }));
  await assert.rejects(() => capture.start({ sampleRateHz: 16000, format: 'webm' }), /permission/i);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b7.log; grep -E "not ok" /tmp/b7.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

`capture.ts` 用 `getUserMedia` + `MediaRecorder`，依赖以构造参数注入好让测试可替身；`stop()` 把收集到的 chunk 拼成 Blob 再转 base64。

`synthesis.ts` 用 `window.speechSynthesis`：按 `voiceSelection` 的 `system:<name>` 后缀在 `getVoices()` 里查嗓音，设置 `rate`，播放。

> `speechSynthesis` 直接播放而不返回 PCM。因此桌面的 `Synthesize` 应答返回空 PCM 加一个「已就地播放」的约定：`AudioResultDto::Audio { pcm_base64: "", sample_rate_hz: 0 }`。**这一约定必须在 `synthesis.ts` 与 `audio_bridge.rs` 两处的注释里同时写明**，否则引擎侧会把空 PCM 当成失败。若后续需要真实 PCM，改用 `OfflineAudioContext` 录制，是独立一轮。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b7.log; grep -E "^# (pass|fail)" /tmp/b7.log`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/audio clients/electron/test/voice-capture.test.ts
git commit -m "Capture the microphone and speak through the system voices"
```

---

## Task 8: STT 走 provider 转写

**Files:**
- Create: `clients/electron/src/renderer/audio/transcription.ts`
- Test: `clients/electron/test/voice-transcription.test.ts`

**Interfaces:**
- Consumes: Task 7 的 `MicrophoneCapture`；`bridge` 的 provider 凭据元数据
- Produces: `export async function transcribe(deps, language): Promise<{ text: string; language?: string; confidence?: number }>`

- [ ] **Step 1: 写失败测试**

```typescript
test('transcription refuses to run with no configured provider', async () => {
  await assert.rejects(
    () => transcribe({ providerConfigured: false } as never, 'zh-CN'),
    /provider/i,
    'the failure must name the missing provider, matching ProviderCredentialRequired',
  );
});

test('transcription returns the provider text and echoes the requested language', async () => {
  const result = await transcribe({
    providerConfigured: true,
    capture: fakeCapture('AAEC'),
    post: async () => ({ text: '你好世界' }),
  } as never, 'zh-CN');
  assert.equal(result.text, '你好世界');
  assert.equal(result.language, 'zh-CN');
});

test('a provider error is surfaced, not turned into an empty transcript', async () => {
  await assert.rejects(
    () => transcribe({
      providerConfigured: true,
      capture: fakeCapture('AAEC'),
      post: async () => { throw new Error('401 unauthorized'); },
    } as never, 'zh-CN'),
    /401/,
    'an empty transcript would read as "you said nothing"',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b8.log; grep -E "not ok" /tmp/b8.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

```typescript
export interface TranscribeDeps {
  providerConfigured: boolean;
  capture: { stop(): Promise<{ audioBase64: string; mimeType: string }> };
  post(audioBase64: string, mimeType: string, language?: string): Promise<{ text: string }>;
}

/**
 * 桌面的语音识别。
 *
 * 走已配置 provider 的转写接口，而不是系统识别器 —— Chromium 的
 * `SpeechRecognition` 在 Electron 中不可用（依赖 Google 的服务与密钥）。
 * 没有凭据时抛错并点名 provider，与 `ProviderCredentialRequired` 对应。
 */
export async function transcribe(
  deps: TranscribeDeps,
  language?: string,
): Promise<{ text: string; language?: string; confidence?: number }> {
  if (!deps.providerConfigured) {
    throw new Error(
      'desktop speech recognition runs through a configured provider; connect one in Settings',
    );
  }
  const clip = await deps.capture.stop();
  const response = await deps.post(clip.audioBase64, clip.mimeType, language);
  return { text: response.text, language };
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b8.log; grep -E "^# (pass|fail)" /tmp/b8.log`
Expected: 三条全 PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/audio/transcription.ts clients/electron/test/voice-transcription.test.ts
git commit -m "Transcribe through the provider the desktop already trusts"
```

---

## Task 9: 语音设置页

**Files:**
- Create: `clients/electron/src/renderer/components/settings/pages/Voice.tsx`
- Test: `clients/electron/test/settings-voice-page.test.ts`

**Interfaces:**
- Consumes: Part A 的 `Card` / `Row`（Task 12）与 `nav.ts` 的 `voice` 页（Task 14）；本计划 Task 4 / 6
- Produces: `Voice.tsx`

> **依赖 Part A 的 Task 12/14/15。** 若 Part A 尚未落地，先做本计划的 Task 1-8。

- [ ] **Step 1: 写失败测试**

```typescript
test('the page renders the probed voices, never a hardcoded list', () => {
  const model = voicePageModel(prefs(), platform({
    voices: [{ name: 'Alex', lang: 'en-US', default: true, localService: true }],
  }));
  assert.deepEqual(model.voiceOptions.map((v) => v.label), ['Alex']);
});

test('every blocking issue renders as a visible explanation', () => {
  const model = voicePageModel(prefs({ recognitionMode: 'onDevice' }), platform());
  assert.ok(
    model.notices.some((n) => /offline/i.test(n)),
    'an unavailable mode must explain itself on screen, not just disable a control',
  );
});

test('the on-device option is offered but marked unavailable, not hidden', () => {
  const model = voicePageModel(prefs(), platform());
  const onDevice = model.recognitionOptions.find((o) => o.id === 'onDevice');
  assert.ok(onDevice, 'hiding the option would make the desktop look like it has no such concept');
  assert.equal(onDevice?.disabled, true);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b9.log; grep -E "not ok" /tmp/b9.log`
Expected: FAIL

- [ ] **Step 3: 最小实现**

页面内容（全部 `设备` 归属，不分层）：

- **识别**卡片：`recognitionMode` 两段式（自动 / 仅本设备，后者禁用并附 `fallbackReason`）、`language`、麦克风权限状态行（未授权时给出打开系统设置的入口，复用既有的 `openSystemSettings`）。
- **朗读**卡片：`voiceSelection`（来自 `probePlatform` 的真实嗓音列表，网络嗓音标注）、`rate` 滑杆（0.5–2.0）、`autoPlayReplies` 开关。
- **状态**卡片：把 `blockingIssues` 逐条渲染为可读说明，`fallbackReason` 渲染为一行提示。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b9.log; grep -E "^# (pass|fail)" /tmp/b9.log && npm run typecheck`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add clients/electron/src/renderer/components/settings/pages/Voice.tsx clients/electron/test/settings-voice-page.test.ts
git commit -m "Show the voice settings the desktop can actually honour"
```

---

## Task 10: 麦克风权限声明与真机验收

**Files:**
- Modify: `clients/electron/Info.plist` 或 `clients/electron/scripts/package-mac.mjs`
- Test: `clients/electron/test/packaging.test.mjs`

- [ ] **Step 1: 写失败测试**

```javascript
test('the packaged app declares a microphone usage string', () => {
  const plist = readPackagedInfoPlist();
  assert.ok(
    plist.NSMicrophoneUsageDescription,
    'macOS kills the app on first getUserMedia without this key',
  );
  assert.ok(
    plist.NSMicrophoneUsageDescription.length > 10,
    'the string is shown to the user and must actually explain the use',
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b10.log; grep -E "not ok" /tmp/b10.log`
Expected: FAIL，键缺失

- [ ] **Step 3: 最小实现**

加入键值：

```xml
<key>NSMicrophoneUsageDescription</key>
<string>LingXi 需要访问麦克风，以便你用语音向它提问。录音只在你按下录制时进行。</string>
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/b10.log; grep -E "^# (pass|fail)" /tmp/b10.log`
Expected: PASS

- [ ] **Step 5: 真机验收（不可省）**

```bash
cd lingxi-code && cargo build -p bridge-server --release
cd ../clients/electron && npm run build && npm run dev
```

逐条确认并把实际输出贴进 PR，**不要只写「验收通过」**：

1. 打开**语音**页：麦克风权限状态、嗓音列表来自系统枚举而非硬编码（列表里应出现你的 macOS 实际安装的嗓音名）。
2. 首次录音触发 macOS 的麦克风授权弹窗；授权后录一段并确认转写返回真实文本。
3. 拒绝麦克风权限，确认页面出现 `MicrophonePermissionRequired` 对应的说明，且录音按钮禁用。
4. 断开全部 provider 凭据，确认 STT 落进 `ProviderCredentialRequired` 而不是静默失败或返回空文本。
5. 切到「仅本设备」模式，确认它被标为不可用并给出「桌面无离线模型」的原因。
6. 选一个嗓音并朗读一段，确认用的是所选嗓音、语速跟随滑杆。
7. 手动删掉所选嗓音后重开设置，确认出现 `RequestedVoiceUnavailable` 并回退到最近可用嗓音。

- [ ] **Step 6: 提交**

```bash
git add clients/electron
git commit -m "Ask for the microphone before using it"
```

---

## 自查记录

- **Spec 覆盖**：B1（分工与注入点）→ Task 2/3；B2（偏好模型）→ Task 4；B3（能力快照）→ Task 5/6；B4（三后端）→ Task 7/8；B5（工具注册）→ Task 3；T5（音频测试）分散在 Task 2/4/5/6/7/8；T6.6-6.7（真机语音）→ Task 10 Step 5。
- **类型一致性**：`VoicePreferences` 的六个字段与 iOS `VoicePreferencesSnapshot`、Android `VoiceConfig` 逐字对齐；`normalizeVoiceSelection` 的用例期望值直接取自 iOS 实现；`AudioOpDto` / `AudioResultDto` 的变体在 Task 1 定义、Task 2 消费，名称一致；`VoiceBlockingIssue` 的五个值在 Task 6 定义、Task 9 消费。
- **已记录的约定**：`speechSynthesis` 就地播放而不返回 PCM，因此 `Synthesize` 应答返回空 PCM —— 这一约定必须在 `synthesis.ts` 与 `audio_bridge.rs` 两处注释同时写明（Task 7 Step 3）。
- **跨计划依赖**：Task 9 依赖 Part A 的 Task 12/14/15；Task 1-8 可与 Part A 并行。
