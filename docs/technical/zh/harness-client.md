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
pub const CLIENT_PROTOCOL_VERSION: &str = "29.0.0";
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

本次检查的客户端协议常量为 29.0.0。连接独立客户端前，始终查看基线与宿主固定依赖版本。设备回调、断线时顺序及重放展示仍需目标宿主检查。

<a id="event-consumer"></a>

## 构建客户端事件消费者

| 事件 | 用途 | 关联方式 |
| --- | --- | --- |
| SessionStarted／SessionResumed | 挂载或替换可见对话。 | session_id 属于该连接挂载的宿主。 |
| TurnStarted | 将已准入提示词标记为活动。 | SendPrompt 回传的可选 turn_id。 |
| TextDelta | 向当前展示追加助手文本。 | 按顺序的事件流；此 DTO 不额外提供逐增量 turn_id。 |
| ToolUseStarted／ToolUseResult | 展示进度、输入、结果与错误状态。 | 通过 id 匹配，不用工具显示名称。 |
| MessageComplete | 提供完整助手消息时确定最终展示。 | 使用消息标识与有序 blocks；message 可以缺省。 |
| TurnEnded／Error | 确定宿主执行终态展示。 | 分别保留 outcome、停止原因、成本或结构化错误类别。 |

不要假定每个事件都有 session_id 或 turn_id。协议将会话选择设计为连接属性。复用多个连接的传输应维护自身经过认证的连接到会话映射。

<a id="permission-roundtrip"></a>

## 完成权限请求与响应闭环

1. 从宿主持有的 PermissionRequestSink 接收权限请求，将连接级 request_id 与展示的卡片一起保存。
2. 展示请求操作及引擎提供的动作。遵守 suppress_always_allow_rule 与 auto_mode_prompt，客户端不能自行推导更大权限的资格。
3. 通过持有该挂起请求的连接，发送带该 request_id 的 ApprovePermission 或 DenyPermission。
4. 收到 PermissionRequestResolved 后移除或确定卡片状态。Approved、Denied、Cancelled 与 Expired 都是终态；过期点击可能在取消竞态中失效。

### ClientCommand JSON · 单次允许

```json
{
  "type": "approve_permission",
  "request_id": 7,
  "response": { "type": "allow_once" }
}
```

验证方式: 接口摘录

### AdapterPermissionGate::resolve

```text
pub async fn resolve(&self, request_id: u64, response: PermissionResponseDto, tool_name_hint: &str) -> bool
```

请求未知或已没有等待接收方时返回 false。应按保存的请求标识路由，工具名提示不能替代它。

<a id="client-error-handling"></a>

## 处理错误而不猜测成功

| 信号 | 客户端响应 | 避免 |
| --- | --- | --- |
| 传输错误 | 保留部分内容、报告失败，并使用宿主重连策略。 | 假定带副作用的提示词从未执行，并自动重发。 |
| 协议／内部错误 | 保留诊断信息，验证准确的宿主／客户端绑定版本。 | 把缺失或格式错误的事件转换成已完成消息。 |
| MaxTurns | 说明配置的执行预算已结束，并保留对话。 | 仅因轮次结束，就报告用户任务完成。 |
| 未来未知 enum 变体 | 遵循宿主兼容性契约，必要时展示明确的不支持状态。 | 为无法识别的载荷伪造授权或操作成功。 |

- [取消、终态结果与部分输出](harness-sessions.md#cancellation-recipe)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/client/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/lib.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/Cargo.toml) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/version.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/version.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/commands.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/events.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/adapter/output_stream.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/message.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/message.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/adapter/turn.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/adapter/turn.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/adapter/permission_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/adapter/permission_gate.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/permission.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/permission.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
