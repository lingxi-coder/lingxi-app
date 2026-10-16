# 实时会话与背压

在宿主执行器运行有界双向供应商会话，保留工具、音频与中断生命周期。

<a id="realtime-ownership"></a>

## 传输、codec 与执行器边界

RealtimeTransport 提供拆分连接；RealtimeCodec 映射宿主输入与供应商帧；RealtimeSession 返回 control/events 与 RealtimeDriver。宿主必须并发运行 driver.run()，控制麦克风/视频采集节奏，处理播放与权限，并持久化对话状态。默认 feature 不带内置 WebSocket 传输；realtime-websocket 启用 Rustls/Tokio 传输，未启用时仍可注入传输。

| 组件 | 约定 |
| --- | --- |
| RealtimeTransport | TLS、认证与帧传输；不隐式重连/重试；丢弃两半连接时及时释放 |
| RealtimeControl | 有界非阻塞发送；同步 QueueFull；显式中断、继续、关闭与 abort |
| RealtimeEvents / RealtimeDriver | 宿主消费事件，driver 推进网络与生命周期事件 |

<a id="agent-conversation"></a>

## 原生 Agent 音频连接器

connect_audio_conversation 使用固定 ClientSnapshot、精确 AudioRoute、声明音频模型、凭据、本地函数工具与宿主历史，内部构建供应商配置。当前统一 Agent 连接器选择已准入的 OpenAI realtime 或 Gemini Live 路由；任意音频/实时适配器不会自动获得 Agent 支持。

### 使用注入传输连接并将句柄返回宿主

```rust
use std::sync::Arc;
use lingxi_llm_client::{ClientSnapshot, protocol::Secret};
use lingxi_llm_client::audio::AudioRoute;
use lingxi_llm_client::realtime::{
    connect_audio_conversation, AudioRealtimeConfig, ConnectedAudioConversation,
    RealtimeLimits, RealtimeTransport,
};

pub async fn connect(
    snapshot: &ClientSnapshot,
    transport: Arc<dyn RealtimeTransport>,
    credential: Secret<String>,
) -> Result<ConnectedAudioConversation, Box<dyn std::error::Error>> {
    let route = AudioRoute::new("openai", "host-account-1");
    let connected = connect_audio_conversation(
        snapshot, &route, AudioRealtimeConfig::default(),
        Vec::new(), Vec::new(), credential, transport, RealtimeLimits::default(),
    ).await?;
    // Host runs connected.driver.run() concurrently with connected.events.next().
    // Capture and playback use connected.input_format / connected.output_format.
    Ok(connected)
}
```

验证方式: 编译检查 (llm-realtime-connect)

连接器返回成功不等于麦克风或扬声器测试。保留返回格式与配置级能力，不根据端点名称推断历史、工具、转写或中断支持。

<a id="realtime-flow"></a>

## 有界队列与工具轮次

- RealtimeCapabilities 区分历史导入、本地工具、输入/输出转写、用量、中断、音频截断与会话恢复。agent_conversation() 要求其定义的组合，而非仅有音频帧。
- Transcript 事件声明输入/输出方向、Delta 与 Replace、item/turn ID 与 final_chunk。不能将临时替换快照当作增量追加。规范词汇未覆盖的能力保留 ProviderEvent。

1. 打开供应商连接前校验初始化帧与队列限制；句柄对外可见前发送初始化帧。
2. 并发运行 driver 与事件消费者。默认发送队列 8 项、事件队列 32 项、每供应商帧 1 MiB。发送前将宿主音频拆为小块。
3. 遇到 QueueFull 时由宿主限速，避免无限缓存。事件背压真实存在；UI 阻塞不能阻止宿主结算工具或 abort 连接。
4. ToolCall 进入现有权限/沙箱路径。保持 call_id 不变，返回 ToolResult 或有界原子 ToolResults 批次，再按需 ContinueResponse。ToolCancelled 使受影响宿主任务失效。

<a id="realtime-shutdown"></a>

## 中断、关闭与恢复

- Interrupt 是供应商命令，不会关闭连接。宿主应及时停止播放，协议支持时将音频截断同步到已播放位置。ClearAudio 在受支持协议中丢弃未提交输入。
- close() 在已接受工作后排队优雅关闭。abort() 独立打断待发送写入与事件背压，丢弃排队工作并直接释放连接。收到确认前需保持 driver 运行。
- ProviderError 与 ConnectionInterrupted 是不同标准化事件，均不会静默重连。会话恢复需要显式能力与状态；导入宿主历史不会自动请求新响应。
- Realtime Usage 在供应商提供时关联 turn_id，字段可缺失。按精确路由/账户持久化观测，取消后明确保留未知接受状态与成本。

realtime 与 audio_realtime_contract 测试使用模拟连接验证队列上限、背压下 abort、工具与生命周期标准化。真实网络韧性、音频质量与设备权限行为需单独验收。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/realtime/mod.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/realtime/mod.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/realtime/audio.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/realtime/audio.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/realtime/normalize.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/realtime/normalize.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/realtime/websocket.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/realtime/websocket.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/providers/openai/realtime.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/providers/openai/realtime.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/providers/google/live.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/providers/google/live.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/realtime.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/realtime.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/audio_realtime_contract.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/audio_realtime_contract.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/provider_realtime_clients.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/provider_realtime_clients.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; 所据文件指纹 sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; 产品固定版本 a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
