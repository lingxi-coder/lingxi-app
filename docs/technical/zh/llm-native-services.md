# 供应商资源与托管工具

显式绑定原生资源，区分供应商端执行、Harness 工具与后台工作。

<a id="native-binding"></a>

## 绑定供应商与精确连接

client.provider::<T>(profile) 校验供应商身份与连接作用域。实时客户端的供应商句柄每次操作捕获新的不可变配置；ClientSnapshot 派生句柄使用保留的修订。资源引用记录复用所需的账户、端点与连接。配置替换可能使绑定失效，而不会将其路由到其他账户。

| 类型化客户端 | 已实现资源族 |
| --- | --- |
| OpenAiClient | 文件、嵌入、图像、检索、批处理、后台响应、容器、音频、实时/Live |
| AnthropicClient | 文件、批处理、供应商 Skills、Messages 原生托管工具与云托管协议 |
| GoogleClient | 文件、嵌入、file_search、批处理、上下文缓存、Interactions、语音/声音、Vertex 语音与 Live |
| QwenClient | 文件、嵌入、图像、批处理、重排、知识库、ASR/TTS/AudioGen 与类型化实时服务 |
| XaiClient | files()、collections(config)、batch(scope)、audio(config)、deferred()、images()；原生实时连接器 |
| OpenRouterClient | files()、embeddings()、images()、audio()、batch(scope)、rerank(scope)；类型化托管服务器工具 |
| MiniMaxClient | files()、images()、audio(endpoint)、tts(config)、async_tts(config)、voices(config)；增量语音连接器 |
| ZhipuClient | files()、embeddings()、images()、batch(scope)、knowledge()、async_tasks(config)、cloud_audio(scope)、asr(route, scope)；原生实时入口 |
| KimiClient | files()、batch(scope)、chat()；文件提取独立于通用模型文件输入 |
| DeepSeekClient / GithubCopilotClient | 绑定 Chat 与账户资源；不因 Chat 兼容而推断文件/图像/语音资源 |

资源族描述代码入口，不代表上游普遍支持。检查操作的类型化作用域、具体路由与认证限制。完整方法签名与供应商载荷类型见 Rustdoc。

<a id="tool-execution-choice"></a>

## 明确工具由谁执行

| 机制 | 执行位置 | 应用接法 |
| --- | --- | --- |
| ChatRequest.tools | 你的宿主 | 校验完成的调用，执行权限策略，运行登记代码，再追加 ToolResult |
| ChatRequest.hosted_tools | 供应商基础设施 | 启用已准入的类型化工具，保留原生结果、引用与用量 |
| 托管远程 MCP | 供应商连接的远程 MCP 服务 | 传入请求级服务器授权，处理供应商审批与续接结果 |
| Harness 本地工具 / MCP | Harness 管理的宿主执行 | 接入 Harness 登记、权限、生命周期与持久会话 |

- [完成本地函数闭环](llm-requests.md#function-tool-roundtrip)

<a id="cloud-job-recovery"></a>

## 轮询前保存云端任务身份

1. 成功 submit 后立即把返回资源引用和应用任务记录一起保存，连接、账户、端点与资源作用域应保持成组。
2. 重启后使用同一账户的新鲜凭据查询保存的引用。轮询读取现有任务，不是再次 submit。
3. 区分 cancel 与 delete。取消请求供应商停止工作；删除已存储结果是受支持时另一种资源生命周期操作。
4. 如果 submit/cancel 回执丢失，保留未知结果状态，并通过受支持供应商 API 核对。不能把网络错误直接变成第二次副作用。

<a id="tools-boundaries"></a>

## 本地函数与供应商端工具

ChatRequest.tools 声明宿主执行的函数。ToolUse 事件将数据交给 Harness；权限、沙箱、工具结果持久化与交互提问属于 Harness。ChatRequest.hosted_tools 声明供应商执行的工作。原生托管输出保留在 ProviderContent 与原生元数据中，不得当作本地函数调用执行。

- 网页搜索仅使用供应商托管搜索。chat().web_search_in 添加 WebSearchConfig 并保留引用/元数据；不支持的连接、模型或搜索选项显式报错。没有内部搜索后端回退。
- 文件检索、代码执行、shell、computer use、工具检索、URL/Maps grounding、远程 MCP 与供应商 Skills 是路由特定的类型化扩展。协议标签本身不足以准入。Anthropic 供应商 Skills 与 Harness 本地 SKILL.md 发现不同。
- 托管远程 MCP 授权是 RequestOptions.mcp_authorizations 中的新鲜凭据，按声明的 server label/name 索引，不序列化到 ChatRequest 或历史。供应商审批输出可能要求宿主交互与显式续接。
- 执行安全准入固定有副作用的托管请求，抑制不安全的自动重发/回退。Token 估算不包含供应商工具/容器费用。核对时同时保留用量与服务器工具观测。

<a id="native-jobs"></a>

## 原生任务与持久轮询

由宿主持久化原生引用，之后按选定轮询间隔使用同一账户调用一次 get。OpenAI background 分开提供 submit/get/cancel/delete 与可恢复事件 API；delete 删除已存储响应，不会取消推理。SubmitOutcomeUnknown 与 CancelOutcomeUnknown 是需核对的状态，不能据此重新提交。

供应商 Batch 服务接受各自类型化请求记录或 JSONL 文件；输出身份、状态与用量保持供应商语义。xAI deferred、GLM async、MiniMax 异步 TTS 与图像任务保留各自引用类型和终止状态。它们是云端任务，独立于 Harness 任务执行、Cron 与 Workflow 调度。

### 提交一次后台响应并返回持久引用

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::ChatRequest;
use lingxi_llm_client::providers::{OpenAiClient, openai::background::BackgroundJobRef};

pub async fn submit_background(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<BackgroundJobRef, Box<dyn std::error::Error>> {
    // Supply the selected account's credential and stable account_scope.
    let provider = client.provider::<OpenAiClient>("openai")?;
    let job = provider.background().submit(request, options).await?;
    Ok(job.reference)
}
```

验证方式: 编译检查 (llm-background-submit)

<a id="native-failure"></a>

## 作用域、错误与验收证据

- 文件、容器、缓存、任务与检索引用贯穿精确账户、资源或工作空间作用域。连接改名、端点变化或凭据作用域错误必须失败，不能静默在别处重建资源。
- 发送结果不确定的修改操作保留类型化未知结果错误。可能时查询现有引用。宿主选择轮询、重试、取消与持久清理；SDK 不承担任务调度器职责。
- 引用校验、供应商绑定与模拟协议测试证明本地准入和编码行为。实际端点、工具授权、远程 MCP 授权与服务器结费需要单独记录真实账户测试。

- [执行与逐次结算](llm-execution-accounting.md)
- [作用域文件与上传恢复](llm-files.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/providers/binding.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/binding.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/openai/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/openai/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/google/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/google/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/anthropic/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/anthropic/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/qwen/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/qwen/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/xai/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/xai/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/minimax/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/minimax/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/zhipu/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/zhipu/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/openrouter/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/openrouter/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/kimi/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/kimi/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/deepseek/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/deepseek/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/github_copilot/client.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/github_copilot/client.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/file_resources.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/file_resources.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/account_resources.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/account_resources.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/openai/background/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/openai/background/mod.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/dispatch.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/dispatch.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/execution_safety.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/execution_safety.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/hosted_search.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/hosted_search.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/tests/code_interpreter.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/tests/code_interpreter.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/tests/provider_clients.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/tests/provider_clients.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/llm.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/options.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/options.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; 所据文件指纹 sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; 产品固定版本 ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
