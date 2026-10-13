# 云端转写与语音合成

独立于 Chat 使用精确连接的音频服务，明确格式、限制流大小并按发送状态处理错误。

<a id="audio-route"></a>

## 精确音频路由与宿主职责

LlmClient::audio() 与 ClientSnapshot::audio() 提供统一的文件转写与完整文本合成。AudioRoute 指定精确连接与稳定、非敏感的 account_scope。它不会解析连接组、从 Chat 模型推断音频模型、使用 fallback_credentials、重试或回退。由宿主决定路由跟随会话还是采用显式覆盖。

入口检查独立 AudioCapabilities。FileTranscription、LiveAsr、Synthesis、IncrementalSynthesis 与 NativeRealtime 是不同操作。完整文本产生流式音频仍是 Synthesis，不是增量文本输入。supports() 要求可用统一适配器与声明模型；operation() 还描述供应商专有入口。

### AudioService::capabilities

```text
pub fn capabilities(&self, route: &AudioRoute) -> Result<AudioCapabilities, AudioError>
```

不发起云端调用，查询该路由的操作、模型、格式与声音约定。

### AudioService::transcribe

```text
pub async fn transcribe(
    &self, route: &AudioRoute, input: AudioInput,
    request: &TranscriptionRequest, options: &RequestOptions,
) -> Result<TranscriptionResult, AudioError>
```

发送一次准入后的文件转写，保留文本、时间、请求来源与可选用量。

### AudioService::synthesize

```text
pub async fn synthesize(
    &self, route: &AudioRoute, request: &SynthesisRequest,
    options: &RequestOptions,
) -> Result<AudioOutput, AudioError>
```

返回供应商音频，不负责采集、播放、转码或重采样。

<a id="audio-coverage"></a>

## 已实现的统一与原生适配器

| 连接供应商 | 统一适配器 | 条件与原生边界 |
| --- | --- | --- |
| OpenAI | 文件转写；合成 | 显式原生路由；原生声音、翻译与实时会话独立 |
| xAI | 文件转写；合成 | TTS 没有模型字段且需要选定声音；原生实时 ASR、增量 TTS 与实时会话 |
| Gemini Developer API / Vertex | 合成 | 模型与认证独立；Vertex 需要所选 project/location；无独立统一文件 ASR |
| MiniMax | 文件转写；合成 | 发现并选定声音；原生流式、增量与异步 TTS |
| Qwen | 合成 | 连接 extra.workspace_id；异步 URL ASR 保留类型化原生入口 |
| GLM / Z.AI | 受支持的托管文件转写 | 大陆托管 TTS 为供应商专有入口；当前目录不支持国际托管 TTS |
| OpenRouter | 已发现的 transcription/speech 模型 | 不猜测模型、声音或 PCM 参数 |

本表描述 SDK 准入与适配器实现。端点区域与账户授权是独立事实。不支持的操作、模型、格式或已知大小限制会在读取输入与传输前拒绝。

<a id="audio-example"></a>

## 带字节上限的语音合成

RequestOptions 提供该精确账户的凭据与可选 total_timeout。请求级 account_scope 必须匹配 AudioRoute。统一入口拒绝原生服务未实现的 authenticator/finalizer 钩子，受限 ChatGPT/Copilot 凭据不会授予云端音频能力。

### 收集限定大小的供应商音频供宿主播放

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

验证方式: 编译检查 (llm-speech)

本函数检查返回字节，不播放声音。设备播放前解析 audio.metadata。无头 PCM 不是 WAV；缺失参数表示未知，供应商 URL 不会自动下载。

<a id="audio-errors"></a>

## 流、截止时间与结算

- AudioStream 由调用者拉取，保留供应商编码字节。collect(max_bytes) 要求正数上限；超限返回 MediaTooLarge 且 dispatch 为 Accepted。丢弃会释放底层响应。
- AudioError 保留 kind、dispatch 与原始类型化供应商错误。Dispatch 区分 NotSent、Rejected、Unknown 与 Accepted。超时保守标记接受状态不确定，不能据此自动重放。
- AudioUsage 保留可选 token、时长、字符与供应商成本观测；缺失值仍保持缺失。Qwen 终止用量仅在完成校验后可用。不能假设音频用量适用 Chat token 计价。

unified_audio 模拟测试覆盖精确路由、大小准入、单次发送、格式与终止用量。真实凭据、声音、设备权限、重采样与实体播放需独立验收。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/audio/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/audio/mod.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/audio/types.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/audio/types.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/audio/catalog.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/audio/catalog.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/providers/openai/audio/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/providers/openai/audio/mod.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/providers/qwen/tts/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/providers/qwen/tts/mod.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/hosting/vertex/speech/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/hosting/vertex/speech/mod.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/unified_audio.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/unified_audio.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/audio.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/audio.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)

- llm-client: HEAD ce32fd07402c13fbfcfa185942162a87907ea0a1; 所据文件指纹 sha256:a78ca8575ddb70589a8cc04ff4f043825bd52db40d751181584ea10118bfe2b4; 产品固定版本 87db6e234e32cbc251a128d6b370cdda36c4d0c0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
