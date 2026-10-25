# 请求与执行

构建类型化模型输入，通过草稿、准备、发送与收集阶段控制每一次实际调用。

<a id="chat-request"></a>

## ChatRequest

ChatRequest::new(model) 创建空对话与默认可选控制项。用 ConversationMessage 和 ContentBlock 填入消息。system 表示系统块，tools 与 tool_choice 表示函数工具，hosted_tools 表示供应商执行的工具，output_format 表示输出约束。

controls 承载显式协议控制项；thinking 与 service_tier 按所选模型和协议校验。continuation 绑定原账户、端点、连接与模型。重放助手消息时必须保留原生内容和签名。

### Typed input · excerpt / 类型化输入片段

```rust
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage};

let mut request = ChatRequest::new("gpt-4.1-mini");
request.messages.push(ConversationMessage::user_text("Explain ownership in Rust."));
request.max_tokens = Some(1024);
```

验证方式: 接口摘录

### ChatRequest::new

```text
pub fn new(model: impl Into<String>) -> Self
```

创建尚未填入对话或可选覆盖项的模型输入。

### ConversationMessage::user_text

```text
pub fn user_text(text: impl Into<String>) -> Self
```

创建包含一个纯文本块的用户消息。

### ChatService::complete_in

```text
pub async fn complete_in(self, profile: &str, request: &ChatRequest, options: &RequestOptions) -> Result<ChatResponse, LlmError>
```

显式选择起始连接。complete() 则根据模型解析路由。

<a id="input-field-guide"></a>

## 常用输入字段与默认值

| 字段 | 含义 / 默认值 | 何时设置 |
| --- | --- | --- |
| model | 已配置模型选择器；ChatRequest::new 必须提供 | 从所选连接选择，不猜测显示名是否可用于请求 |
| messages | 默认为空的有序对话输入 | 首条用户消息，以及无状态重放所需的完整助手/工具轮次 |
| max_tokens | 可选输出上限，不是输入上下文上限 | 限制响应体量；支持推理时为推理输出留出空间 |
| tools / tool_choice | 默认无函数；ToolChoice::Auto | 声明宿主函数，并选择 Auto、Any、None 或指定 Tool |
| output_format | 默认为 OutputFormat::Text | 需要 JSON 对象或 Schema 约束的最终答案时设置 |
| continuation | 默认不带供应商状态引用 | 使用已完成且账户匹配的 ContinuationRef，仅发送新输入 |

可选控制项由所选模型和协议校验。显式指定不支持的选项会报错，客户端不会把每个供应商特性静默转换为其他协议。

<a id="request-option-guide"></a>

## 应按请求传入的选项

修改某次模型请求不应改变其他会话共用的全局密钥、超时或推理设置。每次应用请求传入新的 RequestOptions，复用长期存活的客户端句柄。

| 选项 | 应用契约 |
| --- | --- |
| credential | 主连接的有效密钥；由宿主刷新与存储 |
| fallback_credentials | 按精确备用连接名索引的密钥；不隐式复制主密钥 |
| total_timeout | 包含响应读取的整次操作预算；流式请求需显式设置才有总时限 |
| account_scope | 供应商续接的稳定非敏感身份；下一轮须属于同一账户 |
| file_account_scope | 独立限定符合条件的供应商上传复用账户 |
| mcp_authorizations | 按声明 label/name 索引的新鲜远程凭据，不写入历史 |

<a id="function-tool-roundtrip"></a>

## 完成一次函数工具闭环

函数声明描述模型可以请求的能力，并不会执行代码。宿主校验完成的 ToolUse 结果，运行已登记实现，以原调用 ID 添加 ToolResult，再发起下一次请求。轮次上限、取消路径和权限决策由宿主负责。

这个已核对源码的示例只提供无副作用的内存文章查询。传入 request 需已有用户消息，所选路由须支持严格工具输入。它拒绝未登记/带命名空间的调用和非法参数，不按模型提供的名称任意执行，也不承担通用 Agent 运行时。

### 有界工具循环 · 宿主实现

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{
    ChatRequest, ChatResponse, ContentBlock, ConversationMessage,
    MessageRole, StopReason, ToolChoice, ToolSpec,
};

pub async fn answer_with_lookup(
    client: &LlmClient,
    profile: &str,
    mut request: ChatRequest,
    options: &RequestOptions,
) -> Result<ChatResponse, Box<dyn std::error::Error>> {
    request.tools = vec![serde_json::from_str::<ToolSpec>(r#"{
        "name":"lookup_article", "description":"Read one local help article by ID.",
        "input_schema":{"type":"object","properties":{"article":{"type":"string"}},
            "required":["article"],"additionalProperties":false}, "strict":true
    }"#)?];
    request.tool_choice = ToolChoice::Auto;
    for _ in 0..3 {
        let response = client.chat().complete_in(profile, &request, options).await?;
        if !response.tool_input_issues.is_empty() {
            return Err("tool input was not valid JSON".into());
        }
        if response.stop_reason != StopReason::ToolUse { return Ok(response); }
        let mut results = Vec::new();
        for (id, namespace, name, input) in response.message.tool_uses() {
            if namespace.is_some() || name != "lookup_article" {
                return Err("tool is not registered by this host".into());
            }
            let article = input.string_at("/article").ok_or("article must be a string")?;
            let found = article == "quickstart";
            results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: if found { "Use the LLM Client quickstart." } else { "Article not found." }.into(),
                is_error: Some(!found), blocks: None, cache_reference: None, toolset_name: None,
            });
        }
        if results.is_empty() { return Err("tool-use response has no callable tool".into()); }
        request.messages.push(response.message);
        request.messages.push(ConversationMessage {
            role: MessageRole::User, content: results, native_options: Vec::new(),
        });
    }
    Err("tool round limit reached".into())
}
```

验证方式: 编译检查 (llm-function-tool-roundtrip)

- [Harness 工具与权限](harness-tools.md)
- [工具与输出的结构化约定](llm-controls.md)

添加工具结果前保留完整助手消息：下一请求可能依赖工具 ID、供应商原生内容和推理签名。需要权限、工具调度与持久会话时，应接入 Harness。

<a id="request-options"></a>

## RequestOptions

- credential 为主连接提供认证。fallback_credentials 为指定备用连接提供独立凭据；主密钥不会隐式转发。
- account_scope 是用于续接的稳定非敏感身份。file_account_scope 独立限定可复用的供应商上传文件。
- total_timeout 限制准备与响应读取的总时间。普通生成默认 120 秒，含视频为两小时；流式请求默认没有总时限。
- finalizer 在编码后、认证前同步应用宿主策略；不得在此发送请求或获取凭据。

<a id="prepared-call"></a>

## 一次准备，一次发送

prepare_on 必须接收精确连接名，固定该连接的模型行并认证请求。准备过程可能上传附件，但不会发送生成请求。PreparedCall 不可变且不可克隆；发送操作会消耗它。重试或切换连接必须重新准备。

### Host execution · excerpt with existing client/request/options / 宿主执行片段

```rust
use lingxi_llm_client::codecs::RequestMode;

let snapshot = client.snapshot();
let call = snapshot.prepare_on("openai", &request, &options, RequestMode::Complete).await?;
let prices = call.pricing_snapshot();
let received = call.dispatch_once().await?;
let collected = received.collect().await?;
let usage = collected.usage_report().clone();
let inference = collected.inference_report().clone();
let answer = collected.decode();
collected.finish().await;
let response = answer?;
```

验证方式: 接口摘录

### ClientSnapshot::prepare_on

```text
pub async fn prepare_on(&self, profile: &str, request: &ChatRequest, options: &RequestOptions, mode: RequestMode) -> Result<PreparedCall, LlmError>
```

使用保留的配置快照为精确连接准备一次调用。

### PreparedCall::pricing_snapshot

```text
pub fn pricing_snapshot(&self) -> FrozenPricing
```

在消耗调用前保留所选价格，供取消等路径使用。

### PreparedCall::dispatch_once

```text
pub async fn dispatch_once(self) -> Result<ReceivedCall, LlmError>
```

最多发送一次生成请求，不进行重试或连接回退。

### ReceivedCall::collect

```text
pub async fn collect(self) -> Result<CollectedResponse, LlmError>
```

在解析应用输出前收集原始响应与结算事实。

在 decode() 前保留用量与推理报告：失败状态或不合法的工具 JSON 仍可能携带结算观测。宿主决定如何持久化事实，并调用 finish() 完成附件清理。

<a id="draft-admission"></a>

## 草稿与宿主准入

### ClientSnapshot::prepare_draft_on

```text
pub async fn prepare_draft_on(&self, profile: &str, request: &ChatRequest, options: &RequestOptions, mode: RequestMode) -> Result<RequestDraft, LlmError>
```

解析与编码请求，但不对生成请求签名。草稿仅使用显式提供的总超时。

### RequestDraft::request_mut

```text
pub fn request_mut(&mut self) -> &mut HttpRequest
```

在认证前应用最终的宿主请求修改。

### RequestDraft::seal

```text
pub async fn seal(self) -> Result<PreparedCall, LlmError>
```

认证最终字节并消耗草稿。草稿不能直接发送。

### PreparedCall::dispatch_once_with

```text
pub async fn dispatch_once_with(self, on_dispatch: impl FnOnce() -> Result<(), LlmError> + Send) -> Result<ReceivedCall, LlmError>
```

发送前立即运行同步准入标记；拒绝时不会发送。

### LlmClient::count_tokens_exact_in

```text
pub async fn count_tokens_exact_in(&self, profile: &str, request: &ChatRequest, options: &RequestOptions) -> Result<Option<u64>, LlmError>
```

使用 Anthropic 精确输入计数。不支持的协议返回 None；计数错误仍为错误，且不会生成回答。

<a id="wire-pipeline"></a>

## 类型化输入到传输

1. 捕获配置，解析模型、连接、区域与协议，检查显式控制、续接、托管工具与文件约束。
2. 解析宿主附件，选择内联/供应商文件绑定并捕获请求级文件来源。生成预检可能执行附件 I/O 与上传。
3. WireCodec 使用 CodecContext/EncodeRequest 编码所选协议，应用宿主 finalizer 并对最终字节认证一次。
4. Transport 返回状态、响应头与原始响应体流。非流式先收集再解码；流式按传输块使用 codec decoder 与结算观测。
5. 返回标准内容、用量、推理事实与作用域续接。宿主执行本地工具、持久化转录、选择下一轮并清理资源。

![模型请求由快照与路由准入经过附件、codec、认证和传输，形成解码事件并交给宿主工具](../assets/llm-request-flow.svg)

SDK 负责供应商通信；Harness 负责工具执行、权限、历史与逐次结算。

请求准入失败可能发生于传输前；响应解析失败可能发生于供应商已接受并收费后。不能以 JSON 解码成功作为记录实际尝试的边界。

<a id="input-replay"></a>

## 工具、原生内容与续接

ConversationMessage 携带标准 ContentBlock 与协议标记 native_options。文本、思考、签名、工具 ID 与精确工具参数 JSON 具有不同重放职责。ProviderContent 属于原协议，不兼容重放会拒绝；原始 ProviderEvent 观测不是转录内容。

仅在 account_scope 稳定时，完成响应产生可复用 ContinuationRef。下一次请求携带该引用与新输入，必须使用原实际执行连接、端点、模型与账户；单独 raw response_id 不是可移植续接。

- [逐次结算与重试策略](llm-execution-accounting.md)
- [附件与供应商文件](llm-files.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- llm-client/src/client/options.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/prepared.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/executor.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/requests.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/llm.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/codecs/mod.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/transport.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/execution_contracts.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/chat.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/message.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/json_payload.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/continuation.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; 所据文件指纹 sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; 产品固定版本 ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
