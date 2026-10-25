# LLM Client

独立的 Rust 模型客户端，统一供应商协议、类型化请求、流式响应、图像、嵌入向量与模型目录。

<a id="choose-call-pattern"></a>

## 先选择调用方式

需要一次完整答案时先用 complete_in；需要向用户显示生成进度时用 stream_in。只有宿主需要准入检查点或逐次调用账本时才使用 prepared call。三种方式共享 ChatRequest 与请求级凭据模型。

| 入口 | 返回什么 | 宿主接下来做什么 |
| --- | --- | --- |
| chat().complete_in | ChatResponse 或 LlmError | 检查 stop_reason，保存完整助手消息与用量 |
| chat().stream_in | ModelStream，随后异步接收事件 | 显示增量，保留原生块，区分 End、EOF 与读取错误 |
| snapshot().prepare_on | 精确连接上的 PreparedCall | 准入后发送一次，解码前记录结算事实，最后清理 |
| provider::<T>(profile) | 供应商特定资源句柄 | 遵守该资源独立的账户作用域与生命周期 |

- [请求参数与工具闭环](llm-requests.md)
- [显示流式响应](llm-streaming.md)

<a id="bounded-first-call"></a>

## 为应用写一个有时限的调用

此函数由调用者提供已配置连接、模型与凭据，设置输出上限和 45 秒总时限，不改变共享客户端。调用者仍需选择该连接中的模型，并处理返回的终止原因。

### 应用辅助函数 · 已核对源码，未代表真实账户验收

```rust
use std::time::Duration;
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{ChatRequest, ChatResponse, ConversationMessage, LlmError, Secret};

pub async fn ask_once(
    client: &LlmClient,
    profile: &str,
    model: &str,
    credential: Secret<String>,
    prompt: &str,
) -> Result<ChatResponse, LlmError> {
    let mut request = ChatRequest::new(model);
    request.messages.push(ConversationMessage::user_text(prompt));
    request.max_tokens = Some(512);
    let options = RequestOptions {
        credential: Some(credential),
        total_timeout: Some(Duration::from_secs(45)),
        ..Default::default()
    };
    client.chat().complete_in(profile, &request, &options).await
}
```

验证方式: 编译检查 (llm-bounded-first-call)

complete_in 遵循已配置的高层路由策略。宿主若要求最多发送一次生成请求，应使用 prepare_on 与 dispatch_once，而不是外套重试循环。

<a id="first-answer-checks"></a>

## 读取结果，而不仅是文本

1. 先读取 response.stop_reason 再显示成功状态。达到 token 上限表示输出不完整；ToolUse 要求宿主继续处理；Refusal 是明确的结果。
2. 显示时可用 response.message.text()，重放对话时保存 response.message 本身。文本辅助方法不会保留 thinking 和工具块。
3. 将 executed_profile、model、response_id 与 usage 和宿主请求 ID 一起记录。用量缺失表示未知，不能当作免费。

- [处理失败与用量状态](llm-execution-accounting.md)

<a id="install"></a>

## 使用本文工作树

Cargo 包名是 lingxi-llm-client，当前版本为 0.3.0，Rust 导入名为 lingxi_llm_client。需要 Rust 1.94 或更新版本。本手册描述当前工作区，包含未提交 API。在 llm-client 旁新建独立消费者目录，使用本地 path。源码基线分别记录 SDK HEAD、修改文件与产品实际固定版本；版本号本身不能证明 API 可用。

### Cargo.toml · 完整隔离消费者

```toml
[package]
name = "handbook-llm-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
lingxi-llm-client = { path = "../llm-client", features = ["realtime-websocket"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
async-trait = "0.1"
futures = "0.3"
```

验证方式: 需环境验证 (llm-working-tree-manifest)

默认不启用可选 feature。内置 WebSocket 传输按需启用 responses-websocket 或 realtime-websocket；离线分词器按需启用 tokenizer-*。

<a id="first-request"></a>

## 发送第一个请求

从内置目录选择 OpenAI 连接，指定区域，并在每次请求中提供有效凭据。其他连接可能要求 build 前注册宿主认证器；登记完整目录不代表配置已可直接运行。openai 连接与 gpt-4.1-mini 模型来自内置目录；供应商账户仍需具有该模型的访问权限。

### src/main.rs · 完整示例

```rust
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, Region, Secret};
use lingxi_llm_client::{builtin_providers, LlmClientBuilder, RequestOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let client = LlmClientBuilder::new(&profiles)?
        .with_region(Region::International)
        .build()?;
    let mut request = ChatRequest::new("gpt-4.1-mini");
    request.messages.push(ConversationMessage::user_text("Hello, LingXi!"));
    let options = RequestOptions {
        credential: Some(Secret::new(std::env::var("OPENAI_API_KEY")?)),
        ..Default::default()
    };
    let response = client.chat().complete_in("openai", &request, &options).await?;
    println!("{}", response.message.text());
    Ok(())
}
```

验证方式: 编译检查 (llm-first-request)

示例自行读取 OPENAI_API_KEY。客户端不会自动查找、刷新或持久化密钥；仅此示例读取 OPENAI_API_KEY。模型名是已配置请求 ID，不保证真实账户权限。凭据必须属于实际选择的连接与账户。

<a id="service-api"></a>

## 服务入口

### LlmClientBuilder::new

```text
pub fn new(profiles: &[ProviderProfile]) -> Result<Self, LlmError>
```

注册内置编解码器、HTTP 传输、模型目录与认证器。

### LlmClientBuilder::build

```text
pub fn build(self) -> Result<LlmClient, BuildError>
```

创建可共享的客户端；并发任务可以克隆客户端句柄。

### LlmClient::chat

```text
pub fn chat(&self) -> ChatService<'_>
```

提供对话生成、流式响应、托管搜索与可见对话模型。

### LlmClient::images

```text
pub fn images(&self) -> ImageService<'_>
```

提供独立的图像生成、编辑、能力查询与原生异步任务。

### LlmClient::embeddings

```text
pub fn embeddings(&self) -> EmbeddingService<'_>
```

提供独立的文本嵌入路由；不会自动拆分输入或归一化向量。

### LlmClient::snapshot

```text
pub fn snapshot(&self) -> ClientSnapshot
```

捕获不可变配置版本，让预检、执行与计价基于同一份配置。

<a id="ownership"></a>

## 客户端与宿主的职责

- 客户端负责协议编码、传输、流解析、供应商资源 API、模型目录和标准化用量观测。
- 应用负责凭据、工具执行、权限、对话历史、上下文压缩、产品重试策略与持久化结算。
- 复用长期存活的客户端。模型、推理控制、服务等级、凭据与超时均属于请求级设置。

<a id="composition"></a>

## 组合与公共服务

客户端是一个独立 Rust crate，不是 Agent 引擎。共享客户端发布不可变配置修订；Chat、图像、嵌入、音频、决策与供应商原生资源在每次操作捕获对应修订。Builder 注册决定连接实际可用的 codec、认证器、传输、目录、附件 resolver 与账户来源。

- LlmClientBuilder::with_transport 注入宿主 HTTP 实现，不创建网络客户端。register_codec/register_authenticator 替换对应注册项；自定义传输应暴露原始供应商响应，避免隐藏重试/重定向策略。
- BuildError::MissingRegion、MissingCodec 与 MissingAuthenticator 会在首个模型请求前拒绝不完整组合。内置 builder 提供 ApiKey、Bearer 与 ChatGptPlan 认证器；完整目录中的 CopilotBearer 等连接要求对应宿主注册。独立小型客户端只选择实际需要的连接。
- 预检、执行与成本需共享固定修订时，使用 client.snapshot()。之后 ClientConfigManager 发布配置改变后续实时调用，不改变已捕获快照。

- [精确连接云端音频](llm-audio.md)
- [输出约定与决策](llm-controls.md)
- [原生资源与托管工具](llm-native-services.md)

<a id="mock-transport"></a>

## 不连接供应商的请求约定测试

此完整测试使用应用相同的公共 builder 与 Chat 入口，由注入 Transport 提供确定的 Responses 响应体。测试断言模型与输入到达协议层且只发送一次，只证明本地协议行为，不向真实供应商发送请求。

### 可执行 mock Transport 测试

```rust
#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use async_trait::async_trait;
    use lingxi_llm_client::{builtin_providers, HttpRequest, HttpResponse, LlmClientBuilder, RequestOptions, StreamResponse, Transport};
    use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, LlmError, Region, Secret};

    struct MockTransport { sends: AtomicUsize }
    #[async_trait]
    impl Transport for MockTransport {
        async fn send(&self, request: HttpRequest) -> Result<StreamResponse, LlmError> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "gpt-4.1-mini");
            assert!(body.get("input").is_some());
            Ok(HttpResponse {
                status: 200, headers: vec![],
                body: serde_json::to_vec(&serde_json::json!({
                    "id":"resp_mock", "object":"response", "status":"completed",
                    "model":"gpt-4.1-mini", "output":[{
                        "id":"msg_mock", "type":"message", "status":"completed", "role":"assistant",
                        "content":[{"type":"output_text", "text":"Mock answer", "annotations":[]}]
                    }], "usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}
                })).unwrap().into(),
            }.into())
        }
    }

    #[tokio::test]
    async fn public_client_dispatches_once() {
        let http = Arc::new(MockTransport { sends: AtomicUsize::new(0) });
        let profiles = builtin_providers().unwrap().into_iter()
            .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
        let client = LlmClientBuilder::with_transport(http.clone(), &profiles)
            .with_region(Region::International).build().unwrap();
        let mut request = ChatRequest::new("gpt-4.1-mini");
        request.messages.push(ConversationMessage::user_text("Hello"));
        let options = RequestOptions { credential: Some(Secret::new("mock-key".into())), ..Default::default() };
        let response = client.chat().complete_in("openai", &request, &options).await.unwrap();
        assert_eq!(response.message.text(), "Mock answer");
        assert_eq!(http.sends.load(Ordering::SeqCst), 1);
    }
}
```

验证方式: 模拟运行 (llm-transport-mock)

<a id="pinned-install"></a>

## 独立安装固定修订

以下完整 SHA 是独立的固定修订安装示例，应作为独立消费者验证；生成的源码基线分别展示产品实际依赖与当前 llm-client 工作树 HEAD。上方工作树示例不能证明所有新 API 在该修订可用。

### Cargo.toml · 独立固定版本消费者

```toml
[package]
name = "handbook-llm-pinned"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
lingxi-llm-client = { git = "https://github.com/lingxi-coder/llm-client.git", rev = "87db6e234e32cbc251a128d6b370cdda36c4d0c0" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

验证方式: 需环境验证 (llm-pinned-manifest)

- [系统源码基线与依赖修订](system-overview.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/8409379b84c6616332e51978cfbc31c5ba712d78/Cargo.toml) — 已提交源码 (8409379b84c6616332e51978cfbc31c5ba712d78)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/Cargo.toml) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/lib.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/builder.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/builder.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/live.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/live.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/llm.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/presets.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/presets.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/chat.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/chat.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/options.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/options.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/usage_report.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/usage_report.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- lingxi-app: HEAD 8409379b84c6616332e51978cfbc31c5ba712d78; 所据文件指纹 sha256:40c003672f89d603760e13d5912a3db44dce330055f42839b55ed26f18dc28e5
- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; 所据文件指纹 sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; 产品固定版本 ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
