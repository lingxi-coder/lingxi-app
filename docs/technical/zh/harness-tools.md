# 工具与权限

通过共享契约实现工具，并注册其能力。

<a id="tool-contract"></a>

## Tool 契约

tool-api 定义工具实现与执行组件共用的抽象契约，提供 Tool、ToolUseContext、ToolRegistry、ToolCallResult、校验错误、进度通道和调度支持。它们由配套 crate 导出，不是 Harness 的方法。

工具声明规范名称、JSON 输入 schema、启用状态、结果大小和调度提示。执行前，调度器校验输入并计算权限，hooks 可影响权限决策。仅实现 call 并不构成完整的 Tool 实现。

### Tool::name

```text
fn name(&self) -> &str
```

调度器使用的工具规范标识。

### Tool::input_schema

```text
fn input_schema(&self) -> &Utf16JsonProjection
```

声明工具输入的 JSON Schema。

### Tool::validate_input

```text
async fn validate_input(&self, input: &Utf16JsonProjection, ctx: &ToolUseContext) -> Result<(), ValidationError>
```

在副作用前校验工具专用条件；默认实现通过校验。

### Tool::check_permissions

```text
async fn check_permissions(&self, input: &Utf16JsonProjection, ctx: &ToolUseContext) -> permission::PermissionResult
```

在调度前计算本次调用的权限决策。

### Tool::call

```text
async fn call(&self, input: Utf16JsonProjection, ctx: ToolUseContext, progress_tx: ToolProgressSender) -> Result<ToolCallResult, ToolError>
```

执行已校验并经权限检查的输入，返回模型可见结果。

<a id="registry"></a>

## 共享前注册工具

ToolRegistry 包含内置、MCP、LSP 和插件分区。通过 Arc 共享注册表，不直接克隆。内置工具通过 &mut self 注册；MCP 注册使用内部同步，让服务器在注册表共享后更新工具。

### Rust · existing Tool implementation required

```rust
use std::sync::Arc;
use tool_api::{Tool, ToolRegistry};

fn registry_with(tool: Arc<dyn Tool>) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register_builtin(tool);
    Arc::new(registry)
}
```

验证方式: 编译检查 (harness-tool-registry)

### ToolRegistry::register_builtin

```text
pub fn register_builtin(&mut self, tool: Arc<dyn Tool>)
```

在共享注册表前添加内置工具。

### ToolRegistry::available_tools

```text
pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>>
```

按确定性的声明顺序读取已启用并经过滤的工具池。

### ToolRegistry::register_mcp_tools

```text
pub fn register_mcp_tools(&self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>)
```

注册或更新某一 MCP 连接的工具。

### ToolRegistry::unregister_mcp_tools

```text
pub fn unregister_mcp_tools(&self, conn_id: McpConnectionId)
```

删除已断开的 MCP 连接的工具分区。

<a id="invocation-context"></a>

## 调用上下文与进度

ToolUseContext 携带调用选项、对话历史、工具调用与助手消息标识、可选会话状态、来源会话归属，以及可选的逐调用取消令牌。实现操作时应遵循所提供的取消状态与宿主能力。

### progress_channel

```text
pub fn progress_channel() -> (ToolProgressSender, ToolProgressReceiver)
```

创建容量为 64 的有界通道，传递工具定义的进度 JSON。

### ToolProgress

```text
pub struct ToolProgress { pub tool_use_id: lingxi_core::types::ToolUseId, pub data: Utf16JsonProjection }
```

将一个进度载荷与来源工具调用关联。

<a id="capability-filtering"></a>

## 过滤会话能力

### ToolRegistry::set_session_tool_allowlist

```text
pub fn set_session_tool_allowlist(&mut self, tools: &[String])
```

设置明确的内置能力允许列表并隐藏所有动态分区。权限门依据规范工具名称，别名无法绕过。

工具可见性与逐调用权限检查是两个控制层。嵌入式桌面宿主在组装时注入 PermissionGate；客户端传输可通过 AdapterPermissionGate 处理交互式权限请求。

<a id="dispatch-failures"></a>

## 校验、拒绝与副作用失败

编排器解析规范标识、校验／转换输入、执行工具策略与 hooks、取得权限决策，再通过带追踪的调度调用工具。供应商适配器不能直接调用 Tool 主体，否则会绕过宿主权限与调用日志。

schema 拒绝、权限拒绝、调用失败和中断是不同结果。执行失败使用 ToolError，并通过 backfill_observable_input 对可观察输入脱敏；ToolCallResult 提供模型可见载荷。失败调用可能已改变外部系统，重试策略应由操作自身决定。

### 现有注册表与工具契约测试

```bash
cargo test --locked -p tool-api --lib
cargo test --locked -p test-harness --test tools_hooks_e2e
```

验证方式: 需环境验证

- [扩展生命周期](extensions-lifecycle.md)

这些命令是可重复检查，不代表每个注册工具在所有设备上都已验收。设备输入、shell 隔离与外部服务需要在各自宿主验证。

<a id="custom-tool-checklist"></a>

## 实现可用的自定义工具

自定义工具需要完整的 Tool 实现，以及负责注册它的宿主组合。从一个能向模型和用户解释清楚输入、结果与副作用的小操作开始。

| 实现项 | 契约 | 常见错误 |
| --- | --- | --- |
| name、input_schema 与 input_repair_metadata | 规范标识、保留源信息的 schema，以及明确的输入修复事实。 | 把别名注册为独立能力，或声明未知的修复契约。 |
| is_enabled、is_read_only 与 is_concurrency_safe | 本次实际调用的可见性与调度事实。 | 为了绕过授权或串行执行，把修改操作标为只读。 |
| description、prompt 与 max_result_size_chars | 模型可见说明与明确的结果大小策略。 | 返回无界原始输出，不考虑模型最终看到的结果。 |
| validate_input 与 check_permissions | 在副作用前执行工具专用校验与权限决策。 | 在校验或权限检查过程中提前执行网络／文件系统操作。 |
| call 与 ToolCallResult | 在调度器控制下执行，并提供明确的模型结果。 | 从传输层或供应商适配器直接调用工具主体。 |

当前工具输入与 schema 使用 lingxi_core::types::utf16_json::Utf16JsonProjection。数据格式仍然是 JSON，但任意外部 schema 与字符串应保留其源 projection，不能一概按普通 serde_json::Value 处理。

<a id="tool-results-and-errors"></a>

## 返回模型可以继续使用的结果

ToolCallResult 区分 data、可选的保真模型字符串与错误状态。调度器可以从 data 推导模型文本，或使用提供的 model_content。MCP 的逻辑错误结果可以作为 is_error 为真的已接收结果返回；传输失败仍是调用失败。

| 失败 | 含义 | 处理方式 |
| --- | --- | --- |
| InvalidInput／ValidationError | 请求不符合工具契约。 | 返回可操作的校验信息，修正输入后再重试。 |
| PermissionDenied | 策略或用户拒绝本次调用。 | 展示拒绝原因，让 Agent 选择被允许的替代操作。 |
| InteractionRequired | 当前宿主无法提供所需实时交互通道。 | 接入预期宿主交互，或选择合适的非交互操作。 |
| Aborted／Io | 执行被中断，或底层操作失败。 | 保留部分状态，重新执行副作用操作前检查幂等性。 |
| EditWithoutRead | 会话没有该编辑目标的有效读取状态。 | 在已挂载会话中读取当前目标，再构造新的编辑。 |

### Rust · 构造结果摘录

```rust
use tool_api::{source_text_projection, ToolCallResult};

fn completed_summary() -> ToolCallResult {
    ToolCallResult::from_data(source_text_projection("Saved the report."))
}
```

验证方式: 接口摘录

<a id="hooks-and-mcp"></a>

## 通过调度路径接入 hooks 与 MCP

hooks 用于观察或控制生命周期边界，MCP 用于增加由外部服务器连接持有的工具。这是两种不同扩展机制：hook 不替代工具注册表，连接服务器也不代表授权其全部工具调用。

| 需求 | 源码提供的接口 | 生命周期规则 |
| --- | --- | --- |
| 校验或注解工具调用 | HookEventType::PreToolUse／PostToolUse／PostToolUseFailure | 选择正确事件并遵循其控制结果；不要从观察 hook 直接执行原副作用。 |
| 更新用户／项目 hooks | HookRegistry::replace_source_hooks | 校验后替换单一来源分区，保留插件与会话 hook 的归属。 |
| 增加服务器工具 | McpRegistry::connect 与 ToolRegistry::register_mcp_tools | 为更新／断开保留连接标识，由生产组合协调连接与工具注册。 |
| 移除服务器 | ToolRegistry::unregister_mcp_tools | 只删除该连接分区，并完成其传输清理。 |

- [Hook 与扩展生命周期](extensions-lifecycle.md)

HookDefinition 包含 blocking、timeout、priority 与 source。阻塞 hook 会被等待，非阻塞 hook 使用异步注册表。超时或 hook 失败不能当作成功策略决策；应在支持的事件边界配置失败行为。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/tool-api/src/tool_trait.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/tool_trait.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tool-api/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/context.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tool-api/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/registry.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/tool-api/src/progress.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/tool-api/src/progress.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/orchestrator/src/turn_loop/tool_dispatch.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/orchestrator/src/turn_loop/tool_dispatch.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/core/src/host/permission_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/core/src/host/permission_gate.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/hooks/src/definition.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/hooks/src/definition.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/hooks/src/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/hooks/src/events.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/hooks/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/hooks/src/registry.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/mcp/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/mcp/src/registry.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)

- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
