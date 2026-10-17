# 客户端协议与适配器

在客户端之间共享命令、事件、展示与运行时适配器。

<a id="modules"></a>

## 三个客户端模块

配套 Rust 包名为 client。protocol 提供不依赖传输方式的命令、事件、DTO 与协议版本。presentation 提供共享工具摘要、结构化 diff、文本渲染和样式。adapter 将运行时输出与交互请求转为客户端 DTO。

默认功能集仅提供 protocol。presentation 启用展示辅助功能；adapter 包含 presentation 与运行时集成。uniffi 为已启用模块增加原生元数据。test-support 启用 adapter 和 MockSink，应放在开发依赖中。

### Cargo.toml · 与运行时一致的本地来源

```toml
[dependencies]
client = { path = "../harness-runtime/crates/client", features = ["adapter"] }
```

验证方式: 接口摘录

<a id="wire-contract"></a>

## 命令与事件

ClientCommand 和 ClientEvent 使用 type 字段内部标记、snake_case JSON，是非穷尽 Rust enum。实时命令作用于连接已挂载的会话。ResumeSession 指定目标会话，普通提示词提交不携带 session_id。

SendPrompt 携带 text、可选 prompt_mode 与 turn_id，以及 images 数组。Cancel 接收可选 turn_id。事件流包含 TextDelta、ToolUseStarted、ToolUseResult、MessageComplete、TurnStarted、TurnEnded、会话生命周期事件和结构化错误。

### ClientCommand JSON · SendPrompt

```json
{
  "type": "send_prompt",
  "text": "Explain this project",
  "images": [],
  "turn_id": 1
}
```

### ClientCommand JSON · Cancel

```json
{
  "type": "cancel",
  "turn_id": 1
}
```

### CLIENT_PROTOCOL_VERSION

```text
pub const CLIENT_PROTOCOL_VERSION: &str = "25.0.0";
```

此检查源码树声明的协议版本。原生客户端生成绑定的布局应与实际构建的宿主版本一致。

这些 JSON 示例表示命令载荷，不包含传输封装。桥接帧与原生绑定入口由各自宿主定义。

<a id="output-adapter"></a>

## 连接事件目标

ClientEventSink 是共享输出边界。AdapterOutputStream 实现运行时 OutputStream，并把回调转换为 ClientEvent。sink 可将事件加入 WebSocket 传输队列，或通过移动端 ListenerSink 转发；适配器代码不依赖具体传输。

### Rust · host-supplied event sink

```rust
use std::sync::Arc;
use client::adapter::{AdapterOutputStream, ClientEventSink};

fn make_output(sink: Arc<dyn ClientEventSink>) -> AdapterOutputStream {
    AdapterOutputStream::new(sink)
}
```

验证方式: 编译检查 (harness-output-adapter)

### ClientEventSink::emit

```text
async fn emit(&self, event: client::protocol::events::ClientEvent)
```

转发一个已转换事件。保持交付轻量，避免传输工作阻塞执行。

### AdapterOutputStream::new

```text
pub fn new(sink: Arc<dyn ClientEventSink>) -> Self
```

围绕共享 sink 创建连接级输出适配器。

### ClientEventListener::on_event

```text
async fn on_event(&self, event: client::protocol::events::ClientEvent)
```

每个客户端事件的移动宿主回调；启用 uniffi 时作为回调接口导出。

<a id="turn-events"></a>

## 轮次事件与交互请求

TurnEventEmitter 围绕调用方提供的结果发出轮次开始、完成或错误事件。它不会执行轮次。AdapterPermissionGate、AskUserQuestionBroker 和 ComputerAccessBroker 提供连接级交互请求处理；执行与响应路由仍由宿主负责。

### TurnEventEmitter::new

```text
pub fn new(sink: Arc<dyn ClientEventSink>) -> Self
```

使用与输出及权限适配器一致的事件目标。

### TurnEventEmitter::emit_turn_started

```text
pub async fn emit_turn_started(&self, turn_id: Option<u64>)
```

为宿主接收的轮次发送带关联标识的开始事件。

### TurnEventEmitter::complete

```text
pub async fn complete(&self, result: Result<orchestrator::streaming_loop::PumpedTurn, orchestrator::OrchestratorError>)
```

将流式轮次结果转换为消息完成或错误事件。

客户端 DTO 兼容不等于设备或传输行为正确。应在目标宿主中验证生成绑定、事件路由、权限与关闭流程。

<a id="resume-presentation"></a>

## 冷恢复与兼容性失败

MessageComplete 与恢复历史使用同一有序 MessageDto block schema。MessageDto.sent_at_ms 携带原始持久化 Unix 时间；通过源码对话 UUID 映射恢复历史时间，而不是按文本或消息位置匹配。内联图像与可视化引用也保留独立标识。

Rust enum 是非穷尽类型；宿主处理未来未知变体时不能伪造成功。原生 enum 与 record 布局需要匹配生成绑定及协议契约检查。JSON 载荷可反序列化不代表 UniFFI 二进制布局兼容或传输握手正确。

### 共享客户端契约

```bash
cargo test --locked -p client --features test-support
```

验证方式: 需环境验证

当前工作树协议常量为 21.0.0。连接独立客户端前，始终查看基线与宿主固定依赖版本。设备回调、断线时顺序及重放展示仍需目标宿主检查。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/client/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/src/lib.rs) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)
- [harness-runtime/crates/client/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/Cargo.toml) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)
- [harness-runtime/crates/client/src/protocol/version.rs](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/src/protocol/version.rs) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/src/protocol/commands.rs) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/src/protocol/events.rs) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/src/adapter/output_stream.rs) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)
- [harness-runtime/crates/client/src/protocol/message.rs](https://github.com/lingxi-coder/harness-runtime/blob/8daf175e172bb00002e3d3c002b8ec6ef8276786/crates/client/src/protocol/message.rs) — 已提交源码 (8daf175e172bb00002e3d3c002b8ec6ef8276786)

- harness-runtime: HEAD 8daf175e172bb00002e3d3c002b8ec6ef8276786; 所据文件指纹 sha256:a83d44fb79099d3a923f8fd33bfd18789edd02c2b1d5c683a16c589fffaf2578; 产品固定版本 8daf175e172bb00002e3d3c002b8ec6ef8276786

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
