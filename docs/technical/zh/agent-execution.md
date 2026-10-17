# Agent 执行与恢复

追踪已准入轮次从宿主输入，到模型请求、流式工具执行及持久化终态的全过程。

<a id="owners"></a>

## 执行负责人

宿主负责工作准入、连接、界面与生命周期。Harness／SessionHandle 委托给已挂载 SessionService。DesktopRuntime 转交 ConversationOrchestrator；编排器负责对话修改、上下文准备、带权限的工具调度、压缩和结果结算。子 Agent runner 使用自己的上下文与输出归属，同时共享工具契约。

llm-runtime 是面向 Harness 的模型执行适配层。ProviderApiAdapter 经独立 llm-client SDK 转换宿主对话数据。供应商协议、网络传输与流解码属于 llm-client；本地工具与宿主授权属于 Harness。

![宿主准入、上下文、模型流、带权限工具和持久化结算](../assets/runtime-agent-loop.svg)

工具结果返回模型循环；取消与失败汇合到结算及清理。

<a id="prepare"></a>

## 准备请求上下文

1. 宿主接受轮次时捕获当前会话、取消令牌、工作目录和解析后的模型／profile。之后的 UI 选择不能改写已执行请求的身份。
2. 加载适用的指令记忆、命令／skill 上下文、会话历史和宿主能力提示。工具声明经过启用、会话允许列表及供应商能力投影。
3. 按需应用上下文修改器与压缩，再通过 SDK 边界提交模型请求。供应商凭据与宿主策略不能来自模型提供的工具参数。

上下文准备并不等于显示的对话记录：隐藏指令、工具引用、提示和压缩摘要可能改变请求，同时持久化可见消息标识保持稳定。

<a id="stream-tools"></a>

## 流式输出与工具执行

流式 pump 累积类型化 HistoryEvent，并检查内容 block 与终态边界。文本和完成的助手行逐步输出。每个工具调用保留 tool_use_id、规范名称、重组的 JSON 输入和原始供应商调用 ID。工具 block 完成时即可开始调度，同时后续响应 block 继续到达。

带追踪的调度校验输入、应用 hooks 与权限策略、按工具契约安排并发，并发布匹配结果。结果进入历史供下一次模型请求使用。消息分组 ID、实际持久化行 UUID 和供应商 ID 职责不同，应各自保留，不能互换。

- [工具实现契约](harness-tools.md)
- [会话存储与标识](session-storage.md)

<a id="stop-recovery"></a>

## 停止、取消与恢复

EndTurn、MaxTurns 和 Cancelled 描述成功的控制结果。供应商、流、工具或持久化错误也可能导致轮次失败。没有有效完成信号的流 EOF 不能自动视为成功。实现的恢复路径允许时，保留部分输出及配对的错误结果。

prompt-too-long 恢复属于编排／上下文层。Harness 模型服务层基于 SDK 准备后的调用驱动瞬态重试；llm-client 负责请求编码与传输，其 prepared 调度不会自动重放轮次。两种恢复路径都不能盲目重执行已启动工具。取消退出所属工作并保留结果证据，并不证明外部写入已回滚。

<a id="example-checks"></a>

## 可重放输入与验收

这是已挂载会话的完整 ClientCommand 载荷，不是 Bridge 传输封装。通过同一连接负责人路由事件与权限响应。无需供应商验证 Rust 门面时，使用“宿主生命周期与持久化”中的完整生命周期测试。

- 现有测试覆盖提示准备、准入时取消、已接受流行、截断流、工具配对及 prompt-too-long 恢复。应对审查基线运行对应 orchestrator 测试。
- 真实供应商流、断线时 UI 顺序及不可逆外部工具效果需要独立宿主／服务验收。源码存在与模拟成功不能证明这些结果。

### ClientCommand · 准入轮次

```json
{
  "type": "send_prompt",
  "text": "Read the project instructions and explain the architecture",
  "images": [],
  "turn_id": 42
}
```

验证方式: 接口摘录

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/runtime/src/api.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/runtime/src/desktop/sdk.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/conversation/drivers/prepare.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/conversation/drivers/prepare.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/conversation/drivers/streaming.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/conversation/drivers/streaming.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/conversation/prompt.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/conversation/prompt.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/streaming_loop.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/streaming_loop.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/streaming_executor.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/streaming_executor.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/turn_loop/tool_dispatch.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/turn_loop/tool_dispatch.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/turn_loop/api_recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/turn_loop/api_recovery.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/orchestrator/src/provider_adapter.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/orchestrator/src/provider_adapter.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/llm-runtime/src/model/retry.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/llm-runtime/src/model/retry.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)
- [harness-runtime/crates/llm-runtime/src/model/retry_scope.rs](https://github.com/lingxi-coder/harness-runtime/blob/bf25d5fca0d706ddd339b28bd36364f6ceca85d5/crates/llm-runtime/src/model/retry_scope.rs) — 已提交源码 (bf25d5fca0d706ddd339b28bd36364f6ceca85d5)

- harness-runtime: HEAD bf25d5fca0d706ddd339b28bd36364f6ceca85d5; 所据文件指纹 sha256:17262ac652ec09f3f2cc282728a85bd756689c38ec992eaa4e8351a3a510ea19; 产品固定版本 bf25d5fca0d706ddd339b28bd36364f6ceca85d5

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
