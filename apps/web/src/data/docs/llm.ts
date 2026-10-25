import type { DocPage, DocSection } from './types';

const source = 'https://github.com/lingxi-coder/llm-client';

const referencePages: DocPage[] = [
  {
    id: 'llm-client',
    group: 'llm',
    title: { en: 'LLM Client', zh: 'LLM Client' },
    description: {
      en: 'A standalone Rust client for provider protocols, typed requests, streaming, images, embeddings, and model catalogs.',
      zh: '独立的 Rust 模型客户端，统一供应商协议、类型化请求、流式响应、图像、嵌入向量与模型目录。',
    },
    packageName: 'lingxi-llm-client',
    sourceUrl: source,
    sources: [{ repo: 'lingxi-app', path: 'Cargo.toml' }, { repo: 'llm-client', path: 'Cargo.toml' }, { repo: 'llm-client', path: 'src/lib.rs' }, { repo: 'llm-client', path: 'src/client/builder.rs' }, { repo: 'llm-client', path: 'src/client/live.rs' }, { repo: 'llm-client', path: 'src/protocol/llm.rs' }, { repo: 'llm-client', path: 'src/presets.rs' }],
    sections: [
      {
        id: 'install',
        title: { en: 'Use the documented working tree', zh: '使用本文工作树' },
        paragraphs: [{
          en: 'The package is lingxi-llm-client 0.3.0 and the Rust import is lingxi_llm_client. Use Rust 1.94 or later. This handbook documents the current workspace, including uncommitted APIs. Place a separate consumer directory beside llm-client and use its local path. The source baseline records SDK HEAD, dirty files and the product’s actual pinned revision independently; a version number alone does not establish API availability.',
          zh: 'Cargo 包名是 lingxi-llm-client，当前版本为 0.3.0，Rust 导入名为 lingxi_llm_client。需要 Rust 1.94 或更新版本。本手册描述当前工作区，包含未提交 API。在 llm-client 旁新建独立消费者目录，使用本地 path。源码基线分别记录 SDK HEAD、修改文件与产品实际固定版本；版本号本身不能证明 API 可用。',
        }],
        code: [{
          label: { en: 'Cargo.toml · complete isolated consumer', zh: 'Cargo.toml · 完整隔离消费者' },
          language: 'toml', verification: 'manual', exampleId: 'llm-working-tree-manifest',
          code: `[package]
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
futures = "0.3"`,
        }],
        note: {
          en: 'The default feature set is empty. Enable responses-websocket or realtime-websocket for the corresponding built-in WebSocket transport, and tokenizer-* features for offline tokenizers.',
          zh: '默认不启用可选 feature。内置 WebSocket 传输按需启用 responses-websocket 或 realtime-websocket；离线分词器按需启用 tokenizer-*。',
        },
      },
      {
        id: 'first-request',
        title: { en: 'Send your first request', zh: '发送第一个请求' },
        paragraphs: [{
          en: 'Select the OpenAI profile from the built-in catalog, choose a region, and supply a valid credential for each request. Other profiles can require host-provided authenticators before build; registering the full catalog is not a ready-to-use configuration. The openai profile and gpt-4.1-mini model are catalog entries; your provider account still needs access to the model.',
          zh: '从内置目录选择 OpenAI 连接，指定区域，并在每次请求中提供有效凭据。其他连接可能要求 build 前注册宿主认证器；登记完整目录不代表配置已可直接运行。openai 连接与 gpt-4.1-mini 模型来自内置目录；供应商账户仍需具有该模型的访问权限。',
        }],
        code: [{
          label: { en: 'src/main.rs · complete example', zh: 'src/main.rs · 完整示例' }, verification: 'compile', exampleId: 'llm-first-request',
          language: 'rust',
          code: `use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, Region, Secret};
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
}`,
        }],
        note: {
          en: 'The example reads OPENAI_API_KEY itself. The client does not discover, refresh, or persist secrets; OPENAI_API_KEY is read only by this example. The model name is a configured request ID, not a guarantee of live entitlement. Use the credential for the exact selected profile and account.',
          zh: '示例自行读取 OPENAI_API_KEY。客户端不会自动查找、刷新或持久化密钥；仅此示例读取 OPENAI_API_KEY。模型名是已配置请求 ID，不保证真实账户权限。凭据必须属于实际选择的连接与账户。',
        },
      },
      {
        id: 'service-api',
        title: { en: 'Service entry points', zh: '服务入口' },
        apis: [
          { name: 'LlmClientBuilder::new', signature: 'pub fn new(profiles: &[ProviderProfile]) -> Result<Self, LlmError>', description: { en: 'Register built-in codecs, HTTP transport, directories, and authenticators.', zh: '注册内置编解码器、HTTP 传输、模型目录与认证器。' } },
          { name: 'LlmClientBuilder::build', signature: 'pub fn build(self) -> Result<LlmClient, BuildError>', description: { en: 'Create a shareable client. Clone the handle for concurrent application tasks.', zh: '创建可共享的客户端；并发任务可以克隆客户端句柄。' } },
          { name: 'LlmClient::chat', signature: "pub fn chat(&self) -> ChatService<'_>", description: { en: 'Conversation completion, streaming, hosted search, and visible chat models.', zh: '提供对话生成、流式响应、托管搜索与可见对话模型。' } },
          { name: 'LlmClient::images', signature: "pub fn images(&self) -> ImageService<'_>", description: { en: 'Independent image generation, editing, capabilities, and native tasks.', zh: '提供独立的图像生成、编辑、能力查询与原生异步任务。' } },
          { name: 'LlmClient::embeddings', signature: "pub fn embeddings(&self) -> EmbeddingService<'_>", description: { en: 'Independent text embedding routes; no automatic input splitting or normalization.', zh: '提供独立的文本嵌入路由；不会自动拆分输入或归一化向量。' } },
          { name: 'LlmClient::snapshot', signature: 'pub fn snapshot(&self) -> ClientSnapshot', description: { en: 'Capture one immutable configuration revision for preflight, execution, and pricing.', zh: '捕获不可变配置版本，让预检、执行与计价基于同一份配置。' } },
        ],
      },
      {
        id: 'ownership',
        title: { en: 'Client and host responsibilities', zh: '客户端与宿主的职责' },
        bullets: [
          { en: 'The client owns wire encoding, transport, stream parsing, provider resource APIs, catalogs, and normalized usage observations.', zh: '客户端负责协议编码、传输、流解析、供应商资源 API、模型目录和标准化用量观测。' },
          { en: 'Your application owns credentials, tool execution, permissions, conversation history, context compaction, product retry policy, and durable accounting.', zh: '应用负责凭据、工具执行、权限、对话历史、上下文压缩、产品重试策略与持久化结算。' },
          { en: 'Keep one long-lived client. Model, reasoning controls, service tier, credentials, and timeouts are request-scoped.', zh: '复用长期存活的客户端。模型、推理控制、服务等级、凭据与超时均属于请求级设置。' },
        ],
      },
      {
        "id": "composition",
        "title": {
          "en": "Composition and public services",
          "zh": "组合与公共服务"
        },
        "paragraphs": [
          {
            "en": "The client is one standalone Rust crate, not the Agent engine. A shared client publishes immutable configuration revisions; chat, images, embeddings, audio, decisions and provider-native resources capture the appropriate revision for each operation. Builder registrations determine the actual codecs, authenticators, transport, directories, attachment resolver and account sources available to a profile.",
            "zh": "客户端是一个独立 Rust crate，不是 Agent 引擎。共享客户端发布不可变配置修订；Chat、图像、嵌入、音频、决策与供应商原生资源在每次操作捕获对应修订。Builder 注册决定连接实际可用的 codec、认证器、传输、目录、附件 resolver 与账户来源。"
          }
        ],
        "bullets": [
          {
            "en": "LlmClientBuilder::with_transport injects a host HTTP implementation without constructing a network client. register_codec/register_authenticator replace the matching registry entry; custom transports must expose raw provider responses and avoid hidden retry/redirect policy.",
            "zh": "LlmClientBuilder::with_transport 注入宿主 HTTP 实现，不创建网络客户端。register_codec/register_authenticator 替换对应注册项；自定义传输应暴露原始供应商响应，避免隐藏重试/重定向策略。"
          },
          {
            "en": "BuildError::MissingRegion, MissingCodec and MissingAuthenticator reject incomplete composition before the first model request. The built-in builder supplies ApiKey, Bearer and ChatGptPlan authenticators; full catalog profiles such as CopilotBearer require the matching host registration. Select only the intended profiles for a small standalone client.",
            "zh": "BuildError::MissingRegion、MissingCodec 与 MissingAuthenticator 会在首个模型请求前拒绝不完整组合。内置 builder 提供 ApiKey、Bearer 与 ChatGptPlan 认证器；完整目录中的 CopilotBearer 等连接要求对应宿主注册。独立小型客户端只选择实际需要的连接。"
          },
          {
            "en": "Use client.snapshot() when preflight, execution and cost must share a fixed revision. A later ClientConfigManager publication changes future live calls, not previously captured snapshots.",
            "zh": "预检、执行与成本需共享固定修订时，使用 client.snapshot()。之后 ClientConfigManager 发布配置改变后续实时调用，不改变已捕获快照。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Exact-profile cloud audio",
              "zh": "精确连接云端音频"
            },
            "href": "/docs/llm-audio"
          },
          {
            "label": {
              "en": "Output contracts and decisions",
              "zh": "输出约定与决策"
            },
            "href": "/docs/llm-controls"
          },
          {
            "label": {
              "en": "Native resources and hosted tools",
              "zh": "原生资源与托管工具"
            },
            "href": "/docs/llm-native-services"
          }
        ]
      },
      {
        "id": "mock-transport",
        "title": {
          "en": "Test request contracts without a provider",
          "zh": "不连接供应商的请求约定测试"
        },
        "paragraphs": [
          {
            "en": "This complete test uses the same public builder and Chat facade as an application, but an injected Transport supplies a deterministic Responses envelope. It asserts the model and input reached the wire and exactly one send occurred. It establishes local protocol behavior only; it sends no real provider request.",
            "zh": "此完整测试使用应用相同的公共 builder 与 Chat 入口，由注入 Transport 提供确定的 Responses 响应体。测试断言模型与输入到达协议层且只发送一次，只证明本地协议行为，不向真实供应商发送请求。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Executable mock Transport test",
              "zh": "可执行 mock Transport 测试"
            },
            "language": "rust",
            "verification": "mock",
            "exampleId": "llm-transport-mock",
            "code": "#[cfg(test)]\nmod tests {\n    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};\n    use async_trait::async_trait;\n    use lingxi_llm_client::{builtin_providers, HttpRequest, HttpResponse, LlmClientBuilder, RequestOptions, StreamResponse, Transport};\n    use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, LlmError, Region, Secret};\n\n    struct MockTransport { sends: AtomicUsize }\n    #[async_trait]\n    impl Transport for MockTransport {\n        async fn send(&self, request: HttpRequest) -> Result<StreamResponse, LlmError> {\n            self.sends.fetch_add(1, Ordering::SeqCst);\n            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();\n            assert_eq!(body[\"model\"], \"gpt-4.1-mini\");\n            assert!(body.get(\"input\").is_some());\n            Ok(HttpResponse {\n                status: 200, headers: vec![],\n                body: serde_json::to_vec(&serde_json::json!({\n                    \"id\":\"resp_mock\", \"object\":\"response\", \"status\":\"completed\",\n                    \"model\":\"gpt-4.1-mini\", \"output\":[{\n                        \"id\":\"msg_mock\", \"type\":\"message\", \"status\":\"completed\", \"role\":\"assistant\",\n                        \"content\":[{\"type\":\"output_text\", \"text\":\"Mock answer\", \"annotations\":[]}]\n                    }], \"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}\n                })).unwrap().into(),\n            }.into())\n        }\n    }\n\n    #[tokio::test]\n    async fn public_client_dispatches_once() {\n        let http = Arc::new(MockTransport { sends: AtomicUsize::new(0) });\n        let profiles = builtin_providers().unwrap().into_iter()\n            .filter(|profile| profile.profile_name == \"openai\").collect::<Vec<_>>();\n        let client = LlmClientBuilder::with_transport(http.clone(), &profiles)\n            .with_region(Region::International).build().unwrap();\n        let mut request = ChatRequest::new(\"gpt-4.1-mini\");\n        request.messages.push(ConversationMessage::user_text(\"Hello\"));\n        let options = RequestOptions { credential: Some(Secret::new(\"mock-key\".into())), ..Default::default() };\n        let response = client.chat().complete_in(\"openai\", &request, &options).await.unwrap();\n        assert_eq!(response.message.text(), \"Mock answer\");\n        assert_eq!(http.sends.load(Ordering::SeqCst), 1);\n    }\n}"
          }
        ]
      },
      {
        "id": "pinned-install",
        "title": {
          "en": "Install a fixed revision separately",
          "zh": "独立安装固定修订"
        },
        "paragraphs": [
          {
            "en": "The complete SHA below is an independent fixed-revision installation example. Validate it as a separate consumer; the generated source baseline shows the actual product dependency and current llm-client workspace HEAD independently. The working-tree examples above are not evidence that every new API is available at this revision.",
            "zh": "以下完整 SHA 是独立的固定修订安装示例，应作为独立消费者验证；生成的源码基线分别展示产品实际依赖与当前 llm-client 工作树 HEAD。上方工作树示例不能证明所有新 API 在该修订可用。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Cargo.toml · separate pinned consumer",
              "zh": "Cargo.toml · 独立固定版本消费者"
            },
            "language": "toml",
            "verification": "manual",
            "exampleId": "llm-pinned-manifest",
            "code": "[package]\nname = \"handbook-llm-pinned\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\nlingxi-llm-client = { git = \"https://github.com/lingxi-coder/llm-client.git\", rev = \"87db6e234e32cbc251a128d6b370cdda36c4d0c0\" }\ntokio = { version = \"1\", features = [\"rt-multi-thread\", \"macros\"] }"
          }
        ],
        "links": [
          {
            "label": {
              "en": "System source baseline and dependency revisions",
              "zh": "系统源码基线与依赖修订"
            },
            "href": "/docs/system-overview"
          }
        ]
      },
    ],
  },
  {
    id: 'llm-requests',
    group: 'llm',
    title: { en: 'Requests & execution', zh: '请求与执行' },
    description: { en: 'Compose typed model input and control each physical attempt with draft, prepare, dispatch, and collect.', zh: '构建类型化模型输入，通过草稿、准备、发送与收集阶段控制每一次实际调用。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: source,
    sources: [{ repo: 'llm-client', path: 'src/client/options.rs' }, { repo: 'llm-client', path: 'src/client/prepared.rs' }, { repo: 'llm-client', path: 'src/client/executor.rs' }, { repo: 'llm-client', path: 'src/client/requests.rs' }, { repo: 'llm-client', path: 'src/protocol/llm.rs' }, { repo: 'llm-client', path: 'src/codecs/mod.rs' }, { repo: 'llm-client', path: 'src/transport.rs' }, { repo: 'llm-client', path: 'tests/execution_contracts.rs' }],
    sections: [
      {
        id: 'chat-request',
        title: { en: 'ChatRequest', zh: 'ChatRequest' },
        paragraphs: [{
          en: 'ChatRequest::new(model) initializes empty conversation input and default optional controls. Populate messages with ConversationMessage and ContentBlock. Use system blocks, tools and tool_choice for function tools, hosted_tools for provider-executed tools, and output_format for output contracts.',
          zh: 'ChatRequest::new(model) 创建空对话与默认可选控制项。用 ConversationMessage 和 ContentBlock 填入消息。system 表示系统块，tools 与 tool_choice 表示函数工具，hosted_tools 表示供应商执行的工具，output_format 表示输出约束。',
        }, {
          en: 'controls carries explicit wire controls; thinking and service_tier are validated against the chosen model and protocol. continuation is scoped to the original account, endpoint, profile and model. Native content and signatures must be preserved when replaying assistant turns.',
          zh: 'controls 承载显式协议控制项；thinking 与 service_tier 按所选模型和协议校验。continuation 绑定原账户、端点、连接与模型。重放助手消息时必须保留原生内容和签名。',
        }],
        apis: [
          { name: 'ChatRequest::new', signature: 'pub fn new(model: impl Into<String>) -> Self', description: { en: 'Construct model input with no conversation or optional overrides.', zh: '创建尚未填入对话或可选覆盖项的模型输入。' } },
          { name: 'ConversationMessage::user_text', signature: 'pub fn user_text(text: impl Into<String>) -> Self', description: { en: 'Create a user message containing one plain text block.', zh: '创建包含一个纯文本块的用户消息。' } },
          { name: 'ChatService::complete_in', signature: 'pub async fn complete_in(self, profile: &str, request: &ChatRequest, options: &RequestOptions) -> Result<ChatResponse, LlmError>', description: { en: 'Select the starting profile explicitly. complete() instead resolves a route by model.', zh: '显式选择起始连接。complete() 则根据模型解析路由。' } },
        ],
        code: [{
          label: 'Typed input · excerpt / 类型化输入片段',
          language: 'rust', verification: 'excerpt',
          code: `use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage};

let mut request = ChatRequest::new("gpt-4.1-mini");
request.messages.push(ConversationMessage::user_text("Explain ownership in Rust."));
request.max_tokens = Some(1024);`,
        }],
      },
      {
        id: 'request-options',
        title: { en: 'RequestOptions', zh: 'RequestOptions' },
        bullets: [
          { en: 'credential authenticates the primary profile. fallback_credentials supplies separate keys for named fallback profiles; the primary key is never implicitly forwarded.', zh: 'credential 为主连接提供认证。fallback_credentials 为指定备用连接提供独立凭据；主密钥不会隐式转发。' },
          { en: 'account_scope is a stable nonsecret identity used for continuation. file_account_scope independently scopes reusable provider uploads.', zh: 'account_scope 是用于续接的稳定非敏感身份。file_account_scope 独立限定可复用的供应商上传文件。' },
          { en: 'total_timeout bounds preparation and response reads. Ordinary completion defaults to 120 seconds, or two hours with video; streams have no total deadline by default.', zh: 'total_timeout 限制准备与响应读取的总时间。普通生成默认 120 秒，含视频为两小时；流式请求默认没有总时限。' },
          { en: 'finalizer applies synchronous host policy after encoding and before authentication. It must not dispatch requests or fetch credentials.', zh: 'finalizer 在编码后、认证前同步应用宿主策略；不得在此发送请求或获取凭据。' },
        ],
      },
      {
        id: 'prepared-call',
        title: { en: 'One attempt, one dispatch', zh: '一次准备，一次发送' },
        paragraphs: [{
          en: 'prepare_on requires an exact connection name, freezes its model row and authenticates the request. Preparation may upload attachments, but it sends no generation request. PreparedCall is immutable and cannot be cloned; dispatch consumes it. Retrying or switching connections requires fresh preparation.',
          zh: 'prepare_on 必须接收精确连接名，固定该连接的模型行并认证请求。准备过程可能上传附件，但不会发送生成请求。PreparedCall 不可变且不可克隆；发送操作会消耗它。重试或切换连接必须重新准备。',
        }],
        apis: [
          { name: 'ClientSnapshot::prepare_on', signature: 'pub async fn prepare_on(&self, profile: &str, request: &ChatRequest, options: &RequestOptions, mode: RequestMode) -> Result<PreparedCall, LlmError>', description: { en: 'Prepare exactly one connection using a retained configuration snapshot.', zh: '使用保留的配置快照为精确连接准备一次调用。' } },
          { name: 'PreparedCall::pricing_snapshot', signature: 'pub fn pricing_snapshot(&self) -> FrozenPricing', description: { en: 'Retain the selected prices before consuming the call, including on cancellation.', zh: '在消耗调用前保留所选价格，供取消等路径使用。' } },
          { name: 'PreparedCall::dispatch_once', signature: 'pub async fn dispatch_once(self) -> Result<ReceivedCall, LlmError>', description: { en: 'Send at most one generation request with no retry or connection fallback.', zh: '最多发送一次生成请求，不进行重试或连接回退。' } },
          { name: 'ReceivedCall::collect', signature: 'pub async fn collect(self) -> Result<CollectedResponse, LlmError>', description: { en: 'Collect raw response and accounting facts before decoding application output.', zh: '在解析应用输出前收集原始响应与结算事实。' } },
        ],
        code: [{
          label: 'Host execution · excerpt with existing client/request/options / 宿主执行片段',
          language: 'rust', verification: 'excerpt',
          code: `use lingxi_llm_client::codecs::RequestMode;

let snapshot = client.snapshot();
let call = snapshot.prepare_on("openai", &request, &options, RequestMode::Complete).await?;
let prices = call.pricing_snapshot();
let received = call.dispatch_once().await?;
let collected = received.collect().await?;
let usage = collected.usage_report().clone();
let inference = collected.inference_report().clone();
let answer = collected.decode();
collected.finish().await;
let response = answer?;`,
        }],
        note: {
          en: 'Retain usage and inference before decode(): unsuccessful status or malformed tool JSON can still carry accounting observations. The host decides how to persist those facts and uses finish() for attachment cleanup.',
          zh: '在 decode() 前保留用量与推理报告：失败状态或不合法的工具 JSON 仍可能携带结算观测。宿主决定如何持久化事实，并调用 finish() 完成附件清理。',
        },
      },
      {
        id: 'draft-admission',
        title: { en: 'Drafts & host admission', zh: '草稿与宿主准入' },
        apis: [
          { name: 'ClientSnapshot::prepare_draft_on', signature: 'pub async fn prepare_draft_on(&self, profile: &str, request: &ChatRequest, options: &RequestOptions, mode: RequestMode) -> Result<RequestDraft, LlmError>', description: { en: 'Resolve and encode without signing the generation request. Drafts use only the explicitly supplied total timeout.', zh: '解析与编码请求，但不对生成请求签名。草稿仅使用显式提供的总超时。' } },
          { name: 'RequestDraft::request_mut', signature: 'pub fn request_mut(&mut self) -> &mut HttpRequest', description: { en: 'Apply the final host transformation before authentication.', zh: '在认证前应用最终的宿主请求修改。' } },
          { name: 'RequestDraft::seal', signature: 'pub async fn seal(self) -> Result<PreparedCall, LlmError>', description: { en: 'Authenticate final bytes and consume the draft. A draft cannot dispatch directly.', zh: '认证最终字节并消耗草稿。草稿不能直接发送。' } },
          { name: 'PreparedCall::dispatch_once_with', signature: 'pub async fn dispatch_once_with(self, on_dispatch: impl FnOnce() -> Result<(), LlmError> + Send) -> Result<ReceivedCall, LlmError>', description: { en: 'Run a synchronous admission marker immediately before sending; rejection causes no send.', zh: '发送前立即运行同步准入标记；拒绝时不会发送。' } },
          { name: 'LlmClient::count_tokens_exact_in', signature: 'pub async fn count_tokens_exact_in(&self, profile: &str, request: &ChatRequest, options: &RequestOptions) -> Result<Option<u64>, LlmError>', description: { en: 'Use Anthropic exact input counting. Unsupported protocols return None; counting errors stay errors and no answer is generated.', zh: '使用 Anthropic 精确输入计数。不支持的协议返回 None；计数错误仍为错误，且不会生成回答。' } },
        ],
      },
      {
        "id": "wire-pipeline",
        diagram: {
          src: '/docs-assets/llm-request-flow.svg',
          alt: { en: 'Model request from snapshot and route admission through attachments, codec, authentication and transport to decoded events and host tools', zh: '模型请求由快照与路由准入经过附件、codec、认证和传输，形成解码事件并交给宿主工具' },
          caption: { en: 'The SDK owns provider communication. Harness owns tool execution, permissions, history and attempt accounting.', zh: 'SDK 负责供应商通信；Harness 负责工具执行、权限、历史与逐次结算。' },
        },
        "title": {
          "en": "From typed input to transport",
          "zh": "类型化输入到传输"
        },
        "steps": [
          {
            "en": "Capture configuration, resolve model/profile/region/protocol and check explicit controls, continuation, hosted-tool and file constraints.",
            "zh": "捕获配置，解析模型、连接、区域与协议，检查显式控制、续接、托管工具与文件约束。"
          },
          {
            "en": "Resolve host attachments, choose inline/provider-file binding and capture request-local file provenance. A generation preflight can perform attachment I/O and uploads.",
            "zh": "解析宿主附件，选择内联/供应商文件绑定并捕获请求级文件来源。生成预检可能执行附件 I/O 与上传。"
          },
          {
            "en": "WireCodec encodes the selected protocol using CodecContext/EncodeRequest. Apply the host finalizer and authenticate the final bytes once.",
            "zh": "WireCodec 使用 CodecContext/EncodeRequest 编码所选协议，应用宿主 finalizer 并对最终字节认证一次。"
          },
          {
            "en": "Transport returns status/headers and a raw body stream. For completion collect then decode; for streaming use the codec decoder and accounting observations per chunk.",
            "zh": "Transport 返回状态、响应头与原始响应体流。非流式先收集再解码；流式按传输块使用 codec decoder 与结算观测。"
          },
          {
            "en": "Return canonical content, usage, inference facts and scoped continuation. The host executes local tools, owns transcript persistence, chooses another round and performs cleanup.",
            "zh": "返回标准内容、用量、推理事实与作用域续接。宿主执行本地工具、持久化转录、选择下一轮并清理资源。"
          }
        ],
        "note": {
          "en": "A request admission failure can occur before transport; a response parsing failure can occur after the provider has accepted and charged a call. Do not use successful JSON decoding as the boundary for recording a physical attempt.",
          "zh": "请求准入失败可能发生于传输前；响应解析失败可能发生于供应商已接受并收费后。不能以 JSON 解码成功作为记录实际尝试的边界。"
        }
      },
      {
        "id": "input-replay",
        "title": {
          "en": "Tools, native content and continuation",
          "zh": "工具、原生内容与续接"
        },
        "paragraphs": [
          {
            "en": "ConversationMessage carries canonical ContentBlock values and protocol-tagged native_options. Text, thinking, signatures, tool IDs and exact tool argument JSON have distinct replay roles. ProviderContent belongs to its original protocol and is rejected on incompatible replay; raw ProviderEvent observations are not transcript content.",
            "zh": "ConversationMessage 携带标准 ContentBlock 与协议标记 native_options。文本、思考、签名、工具 ID 与精确工具参数 JSON 具有不同重放职责。ProviderContent 属于原协议，不兼容重放会拒绝；原始 ProviderEvent 观测不是转录内容。"
          },
          {
            "en": "A completed response produces a reusable ContinuationRef only with stable account_scope. The next request carries that reference and only new input. It must use the original executed profile, endpoint, model and account; raw response_id alone is not a portable continuation.",
            "zh": "仅在 account_scope 稳定时，完成响应产生可复用 ContinuationRef。下一次请求携带该引用与新输入，必须使用原实际执行连接、端点、模型与账户；单独 raw response_id 不是可移植续接。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Attempt accounting and retry policy",
              "zh": "逐次结算与重试策略"
            },
            "href": "/docs/llm-execution-accounting"
          },
          {
            "label": {
              "en": "Attachments and provider files",
              "zh": "附件与供应商文件"
            },
            "href": "/docs/llm-files"
          }
        ]
      },
    ],
  },
  {
    id: 'llm-streaming',
    group: 'llm',
    title: { en: 'Streaming & cancellation', zh: '流式响应与取消' },
    description: { en: 'Read normalized events or per-chunk observations while preserving tools, native content, usage, and terminal errors.', zh: '读取标准化事件或逐传输块观测，保留工具、原生内容、用量与终止错误。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: source,
    sources: [{ repo: 'llm-client', path: 'src/client/stream.rs' }, { repo: 'llm-client', path: 'src/client/session.rs' }, { repo: 'llm-client', path: 'src/protocol/llm.rs' }, { repo: 'llm-client', path: 'src/stream_assembly.rs' }, { repo: 'llm-client', path: 'src/framing/sse.rs' }, { repo: 'llm-client', path: 'src/framing/eventstream.rs' }, { repo: 'llm-client', path: 'tests/model_stream.rs' }, { repo: 'llm-client', path: 'tests/structured_output.rs' }],
    sections: [
      {
        id: 'events',
        title: { en: 'Consume events', zh: '消费事件' },
        paragraphs: [{
          en: 'ModelStream exposes its own asynchronous next(); no StreamExt import is required. Errors can occur when opening the stream or reading it. Read through None, including after End, before treating reports as final.',
          zh: 'ModelStream 自带异步 next()，无需导入 StreamExt。打开流或读取流时均可能失败。应一直读取到 None，包括 End 事件之后，再将报告视为最终结果。',
        }],
        code: [{
          label: { en: 'Complete streaming function', zh: '完整流式函数' }, verification: 'compile', exampleId: 'llm-stream-text',
          language: 'rust',
          code: `use lingxi_llm_client::protocol::{ChatRequest, StreamEvent};
use lingxi_llm_client::{LlmClient, RequestOptions};

async fn stream_text(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = client.chat().stream_in("openai", request, options).await?;
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { text, .. } => print!("{text}"),
            StreamEvent::End { stop_reason, .. } => eprintln!("{stop_reason:?}"),
            _ => {}
        }
    }
    eprintln!("Profile: {}", stream.executed_profile());
    eprintln!("Usage: {:?}", stream.usage_report());
    eprintln!("Complete: {}", stream.usage_is_complete());
    Ok(())
}`,
        }],
        note: {
          en: 'This display-only example ignores nontext events. Tool-enabled applications must retain tool calls, reasoning signatures, native provider content, and block boundaries when constructing the next assistant turn.',
          zh: '此显示示例忽略非文本事件。启用工具的应用在构造下一轮助手消息时，必须保留工具调用、推理签名、供应商原生内容与块边界。',
        },
      },
      {
        id: 'batches',
        title: { en: 'Observe each transport chunk', zh: '观测每个传输块' },
        paragraphs: [{
          en: 'next_batch reads at most one transport chunk and returns its events, current usage, inference facts, and finished flag. A batch can have no events while usage changes; a terminal error remains alongside its latest usage. Record observations before validating content or yielding control.',
          zh: 'next_batch 最多读取一个传输块，返回事件、当前用量、推理事实与 finished 标记。即使事件为空，用量也可能变化；终止错误与最新用量同时保留。应先记录观测，再校验内容或让出执行权。',
        }],
        apis: [
          { name: 'ModelStream::next', signature: 'pub async fn next(&mut self) -> Option<Result<StreamEvent, LlmError>>', description: { en: 'Read one provider-neutral event.', zh: '读取一个标准化事件。' } },
          { name: 'ModelStream::next_batch', signature: 'pub async fn next_batch(&mut self) -> Option<StreamBatch>', description: { en: 'Read one accounting observation without awaiting another chunk after decoding.', zh: '读取一次结算观测，解码后不会等待下一个传输块。' } },
          { name: 'StreamBatch', signature: 'pub struct StreamBatch { pub events: Vec<Result<StreamEvent, LlmError>>, pub usage: UsageReport, pub inference: InferenceReport, pub finished: bool }', description: { en: 'Retain events and accounting facts together, including usage-only chunks and terminal errors.', zh: '将事件与结算事实一起保留，包括仅用量传输块与终止错误。' } },
          { name: 'ModelStream::usage_is_complete', signature: 'pub fn usage_is_complete(&self) -> bool', description: { en: 'Check for complete, internally consistent usage before estimating actual cost.', zh: '估算实际成本前确认用量完整且内部一致。' } },
        ],
        code: [{
          label: 'Chunk observation · excerpt with an existing stream / 传输块观测片段',
          language: 'rust', verification: 'excerpt',
          code: `while let Some(batch) = stream.next_batch().await {
    // Persist these facts in the host before interpreting events.
    let usage = batch.usage;
    let inference = batch.inference;
    eprintln!("{usage:?} {inference:?}");
    for event in batch.events {
        let event = event?;
        // Dispatch or retain event in your application.
        eprintln!("{event:?}");
    }
}`,
        }],
      },
      {
        id: 'cancel',
        title: { en: 'Cancellation & deadlines', zh: '取消与时限' },
        bullets: [
          { en: 'Dropping ModelStream releases the underlying byte stream and disconnects the read. The host keeps the usage already observed and owns settlement for an interrupted attempt.', zh: '丢弃 ModelStream 会释放底层字节流并断开读取。宿主保留已观测用量，并负责中断调用的结算。' },
          { en: 'Once a stream has been returned, read failures do not trigger automatic failover. The host may prepare a fresh attempt according to its retry policy.', zh: '流已返回后，读取失败不会触发自动回退。宿主可根据重试策略重新准备调用。' },
          { en: 'Set RequestOptions.total_timeout to bound the whole stream. An omitted value means no default total stream deadline.', zh: '通过 RequestOptions.total_timeout 限制整条流的总时间。未设置时没有默认总时限。' },
          { en: 'A continuation reference is exposed only for a completed response. An interrupted stream cannot supply a valid continuation.', zh: '仅完成的响应会提供续接引用。中断的流不会提供有效续接状态。' },
        ],
      },
      {
        id: 'structured-websocket',
        title: { en: 'Structured output & WebSocket', zh: '结构化输出与 WebSocket' },
        paragraphs: [{
          en: 'collect_structured_json and collect_structured<T> reconstruct a terminal response before validating JSON/schema or deserializing. Individual deltas are not standalone JSON documents. ResponsesSession provides connection reuse and continuation; preparation establishes the connection before host admission and dispatch sends one response.create.',
          zh: 'collect_structured_json 与 collect_structured<T> 先重建最终响应，再校验 JSON、Schema 或反序列化。单个增量并不是独立 JSON 文档。ResponsesSession 提供连接复用与续接；准备阶段在宿主准入前建立连接，发送阶段仅发出一次 response.create。',
        }],
        apis: [
          { name: 'ModelStream::collect_structured_json', signature: 'pub async fn collect_structured_json(self, format: &OutputFormat) -> Result<StructuredStreamResult<JsonPayload>, StructuredStreamError>', description: { en: 'Consume and validate final JSON while retaining the reconstructed response and events.', zh: '消费并校验最终 JSON，同时保留重建的响应与事件。' } },
          { name: 'ModelStream::continuation', signature: 'pub fn continuation(&self) -> Option<&ContinuationRef>', description: { en: 'Read account-bound state only after a complete stream.', zh: '完整读取流之后获取绑定账户的续接状态。' } },
        ],
        note: {
          en: 'PreparedCall WebSocket execution requires a Transport implementing connect_websocket. A failed send never resends the current call. The host owns reconnection, cancellation, and whether to prepare another attempt.',
          zh: 'PreparedCall 的 WebSocket 执行需要实现 connect_websocket 的 Transport。发送失败不会重发当前调用。宿主负责重连、取消以及是否准备下一次调用。',
        },
      },
      {
        "id": "lossless-assembly",
        "title": {
          "en": "Preserve canonical and native stream data",
          "zh": "保留标准与原生流数据"
        },
        "paragraphs": [
          {
            "en": "Use StreamAccumulator or ModelStream::collect_response when a host needs a complete assistant message. Assembly preserves sparse block indices, completed block order, thought signatures, native replay content and exact tool input JSON. The partial assembly on failure exposes unfinished blocks and incomplete tools; it must not be treated as a successful terminal tool request.",
            "zh": "宿主需要完整助手消息时，使用 StreamAccumulator 或 ModelStream::collect_response。组装保留稀疏块索引、完成顺序、思考签名、原生重放内容与精确工具输入 JSON。失败时部分组装暴露未完成块与不完整工具，不能当作成功终止工具请求。"
          }
        ],
        "tables": [
          {
            "headers": [
              {
                "en": "Event family",
                "zh": "事件族"
              },
              {
                "en": "Consumer handling",
                "zh": "消费者处理"
              }
            ],
            "rows": [
              [
                {
                  "en": "TextDelta / TextDeltaJsUtf16",
                  "zh": "TextDelta / TextDeltaJsUtf16"
                },
                {
                  "en": "Display and assemble by block; retain exact UTF-16 units where present",
                  "zh": "按块显示并组装；存在精确 UTF-16 单元时保留"
                }
              ],
              [
                {
                  "en": "ReasoningDelta / ThoughtSignature / RedactedThinking",
                  "zh": "ReasoningDelta / ThoughtSignature / RedactedThinking"
                },
                {
                  "en": "Reasoning display and replay signatures/opaque data have separate roles",
                  "zh": "思考显示与重放签名/不透明数据职责不同"
                }
              ],
              [
                {
                  "en": "ToolCallDelta / BlockEnd",
                  "zh": "ToolCallDelta / BlockEnd"
                },
                {
                  "en": "Accumulate exact arguments and provider IDs; execute only a validated completed call",
                  "zh": "累积精确参数与供应商 ID；仅执行已校验完成的调用"
                }
              ],
              [
                {
                  "en": "ProviderContent / Native / NativeDelta",
                  "zh": "ProviderContent / Native / NativeDelta"
                },
                {
                  "en": "Preserve typed/native payload and protocol ownership",
                  "zh": "保留类型化/原生载荷与协议归属"
                }
              ],
              [
                {
                  "en": "ProviderEvent / NativeControl",
                  "zh": "ProviderEvent / NativeControl"
                },
                {
                  "en": "Observation/control only; not replayable message content or local tool calls",
                  "zh": "仅观测/控制；不是可重放消息或本地工具调用"
                }
              ],
              [
                {
                  "en": "Inference / WebSearch / FileSearch / End",
                  "zh": "Inference / WebSearch / FileSearch / End"
                },
                {
                  "en": "Retain facts, attribution and stop reason; continue until EOF",
                  "zh": "保留事实、归因与停止原因；继续读取直到 EOF"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "A final End event and complete UsageReport are different facts. Provider streams can omit usage or fail after content; use usage_is_complete() and final inference when estimating cost. model_stream and structured_output tests validate local projection, not live streaming latency.",
          "zh": "最终 End 事件与完整 UsageReport 是不同事实。供应商流可能遗漏用量或在内容之后失败；估算成本需检查 usage_is_complete() 与最终推理事实。model_stream 与 structured_output 测试验证本地投影，不证明真实流延迟。"
        }
      },
    ],
  },
  {
    id: 'llm-images',
    group: 'llm',
    title: { en: 'Image API', zh: '图像 API' },
    description: { en: 'Generate, edit, and query native image tasks through a service independent of Chat.', zh: '通过独立于 Chat 的服务生成、编辑图像并查询原生图像任务。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: source,
    sources: [{ repo: 'llm-client', path: 'src/images/mod.rs' }, { repo: 'llm-client', path: 'src/images/wire.rs' }, { repo: 'llm-client', path: 'src/protocol/image.rs' }, { repo: 'llm-client', path: 'src/providers/dispatch.rs' }, { repo: 'llm-client', path: 'tests/images.rs' }],
    sections: [
      {
        id: 'generate',
        title: { en: 'Generate an image', zh: '生成图像' },
        paragraphs: [{
          en: 'client.images() shares configuration and transport with Chat but uses its own requests, model catalog, credentials options, capabilities, and routes. Image-output models are excluded from chat().models().',
          zh: 'client.images() 与 Chat 共享配置和传输，但具有独立的请求、模型目录、凭据选项、能力与路由。chat().models() 不包含图像输出模型。',
        }],
        code: [{
          label: { en: 'Image function · call from an async host', zh: '图像函数 · 从异步宿主调用' }, verification: 'compile', exampleId: 'llm-image-generation',
          language: 'rust',
          code: `use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::{
    ImageGenerationRequest, ImageOutputOptions, ImageRequestOptions, Region, Secret,
};

async fn generate(api_key: String) -> Result<(), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let client = LlmClientBuilder::new(&profiles)?
        .with_region(Region::International).build()?;
    let request = ImageGenerationRequest {
        model: "gpt-image-1.5".into(),
        prompt: "An orange cat on a blue background".into(),
        references: vec![],
        output: ImageOutputOptions::default(),
        provider_options: Default::default(),
    };
    let options = ImageRequestOptions {
        credential: Some(Secret::new(api_key)),
        ..Default::default()
    };
    let result = client.images().generate_in("openai", &request, &options).await?;
    println!("{} images", result.images.len());
    Ok(())
}`,
        }],
      },
      {
        id: 'image-methods',
        title: { en: 'Generation, editing & tasks', zh: '生成、编辑与任务' },
        apis: [
          { name: 'ImageService::models', signature: 'pub fn models(&self) -> Vec<ImageModelListing>', description: { en: 'List visible image models in the selected region.', zh: '列出所选区域中可见的图像模型。' } },
          { name: 'ImageService::capabilities_in', signature: 'pub fn capabilities_in(&self, profile: &str, model: &str) -> Result<ImageCapabilities, ImageError>', description: { en: 'Inspect generation, reference, editing, mask, and native task support for an exact profile.', zh: '查询精确连接的生成、参考图、编辑、遮罩与原生任务支持。' } },
          { name: 'ImageService::generate_in', signature: 'pub async fn generate_in(&self, profile: &str, req: &ImageGenerationRequest, opts: &ImageRequestOptions) -> Result<ImageResponse, ImageError>', description: { en: 'Return completed images. generate() resolves by model.', zh: '返回完成的图像。generate() 则按模型解析路由。' } },
          { name: 'ImageService::edit_in', signature: 'pub async fn edit_in(&self, profile: &str, req: &ImageEditRequest, opts: &ImageRequestOptions) -> Result<ImageResponse, ImageError>', description: { en: 'Edit supported source images; explicit unsupported options are rejected before sending.', zh: '编辑受支持的输入图像；不支持的显式选项在发送前被拒绝。' } },
          { name: 'ImageService::submit_in', signature: 'pub async fn submit_in(&self, profile: &str, req: &ImageRequest, opts: &ImageRequestOptions) -> Result<ImageTaskRef, ImageError>', description: { en: 'Submit one provider-native asynchronous task and return a serializable reference.', zh: '提交一个供应商原生异步任务，返回可序列化引用。' } },
          { name: 'ImageService::get_task', signature: 'pub async fn get_task(&self, task: &ImageTaskRef, opts: &ImageRequestOptions) -> Result<ImageTaskSnapshot, ImageError>', description: { en: 'Query once with the original profile, valid credential, and matching account_scope.', zh: '使用原连接、有效凭据与相同 account_scope 查询一次。' } },
        ],
      },
      {
        id: 'inputs',
        title: { en: 'References, masks & outputs', zh: '参考图、遮罩与输出' },
        bullets: [
          { en: 'Supported inputs include HTTP(S) URLs, Base64 plus media type, and application-owned AttachmentRef values. Attachment references require a builder AttachmentResolver.', zh: '支持的输入包括 HTTP(S) URL、带媒体类型的 Base64，以及应用拥有的 AttachmentRef。附件引用需要在 builder 中配置 AttachmentResolver。' },
          { en: 'ImageGenerationRequest.references adds references where supported. MiniMax character references use ImageReferenceKind::Character.', zh: 'ImageGenerationRequest.references 在受支持的模型上添加参考图。MiniMax 角色参考图使用 ImageReferenceKind::Character。' },
          { en: 'Masks require explicit model support. OpenAI masks apply only to image_index = 0; other indices fail before sending.', zh: '遮罩需要模型明确支持。OpenAI 遮罩仅用于 image_index = 0；其他索引在发送前失败。' },
          { en: 'ImageResponse retains original URLs or Base64, executed profile, request ID, outcome, and available usage. Save temporary URLs promptly in the application.', zh: 'ImageResponse 保留原始 URL 或 Base64、执行连接、请求 ID、结果状态与可用用量。应用应及时保存临时 URL 的内容。' },
        ],
      },
      {
        id: 'image-providers',
        title: { en: 'Provider coverage', zh: '供应商覆盖' },
        bullets: [
          { en: 'OpenAI: generation, editing, and masks. Gemini: generation, references, and editing.', zh: 'OpenAI：生成、编辑与遮罩。Gemini：生成、参考图与编辑。' },
          { en: 'Qwen regional profiles: generation, references, editing, and native tasks, subject to regional model availability. Custom Wan routes require an explicit workspace endpoint.', zh: 'Qwen 区域连接：生成、参考图、编辑与原生任务，具体模型由区域账户决定。自定义 Wan 路由需显式配置工作空间端点。' },
          { en: 'xAI: generation and editing. MiniMax: generation and character references. GLM/Z.AI: generation and native tasks.', zh: 'xAI：生成与编辑。MiniMax：生成与角色参考图。GLM/Z.AI：生成与原生任务。' },
          { en: 'OpenRouter: generation, references, and editing depend on the upstream model and endpoint. Chat compatibility alone does not enable image routes.', zh: 'OpenRouter：生成、参考图与编辑取决于上游模型和端点。仅支持兼容 Chat 协议不代表支持图像路由。' },
        ],
        note: {
          en: 'Image calls do not automatically retry, switch connections, poll tasks, or persist them. A timed-out provider may still be generating. Image usage is separate from Chat token cost estimates, and target-account validation is required.',
          zh: '图像调用不会自动重试、切换连接、轮询或持久化任务。超时后供应商仍可能继续生成。图像用量独立于 Chat token 成本估算；目标账户能力需要验证。',
        },
      },
      {
        "id": "image-lifecycle",
        "title": {
          "en": "Image dispatch, persistence and failure",
          "zh": "图像发送、持久化与失败"
        },
        "paragraphs": [
          {
            "en": "capabilities_in validates an exact profile/model before resolving image attachments or dispatching. Generate/edit return ImageResponse, while submit/get_task expose only a supported provider-native task route. Image task references retain the original connection, endpoint, model and account; the host persists them and chooses polling/cancellation when the provider supports it.",
            "zh": "capabilities_in 在解析图像附件或发送前校验精确连接/模型。Generate/edit 返回 ImageResponse；submit/get_task 仅用于受支持原生任务路由。图像任务引用保留原连接、端点、模型与账户；宿主持久化并按供应商支持选择轮询/取消。"
          },
          {
            "en": "Retain native URLs/Base64 and output metadata before application processing. A provider timeout may leave generation running; a lost response must not silently become another image request. ImageError dispatch distinguishes not-sent, rejected, accepted and unknown outcomes for host recovery. Do not bill image generation through Chat token price assumptions.",
            "zh": "应用处理前保留原生 URL/Base64 与输出元数据。供应商超时后生成可能仍在运行；响应丢失不能静默转为另一次图像请求。ImageError dispatch 区分未发送、拒绝、已接受与未知结果，供宿主恢复。不能假设图像生成适用 Chat token 价格。"
          }
        ],
        "bullets": [
          {
            "en": "Application AttachmentRef inputs require an authorized resolver; provider image URL inputs remain provider-managed. Preserve existing transparency/mask semantics in the host rather than fabricating an unsupported option.",
            "zh": "应用 AttachmentRef 输入要求已授权 resolver；供应商图像 URL 输入仍由供应商管理。宿主应保留已有透明度/遮罩语义，不能编造不支持的选项。"
          },
          {
            "en": "images tests establish capability admission, encoding and typed task behavior with simulated responses. Live account model access, output quality, temporary URL expiration and target-device display require separate acceptance.",
            "zh": "images 测试使用模拟响应证明能力准入、编码与类型化任务行为。真实账户模型权限、输出质量、临时 URL 过期与目标设备显示需单独验收。"
          }
        ]
      },
    ],
  },
  {
    id: 'llm-providers',
    group: 'llm',
    title: { en: 'Providers, catalogs & pricing', zh: '供应商、目录与计价' },
    description: { en: 'Bind profiles, filter by region, manage catalogs, access native resources, and estimate cost from observed facts.', zh: '绑定连接、按区域过滤、管理目录、访问原生资源，并依据观测事实估算成本。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: source,
    sources: [{ repo: 'llm-client', path: 'src/client/store.rs' }, { repo: 'llm-client', path: 'src/client/resolve.rs' }, { repo: 'llm-client', path: 'src/configuration/model.rs' }, { repo: 'llm-client', path: 'src/configuration/sync.rs' }, { repo: 'llm-client', path: 'src/protocol/provider.rs' }, { repo: 'llm-client', path: 'src/client/frozen_pricing.rs' }, { repo: 'llm-client', path: 'src/token_count/request.rs' }, { repo: 'llm-client', path: 'data/capability-matrix-protocols.json' }, { repo: 'llm-client', path: 'data/capability-audit.json' }, { repo: 'llm-client', path: 'data/providers/openai.toml' }, { repo: 'llm-client', path: 'data/providers/anthropic.toml' }, { repo: 'llm-client', path: 'data/providers/qwen.toml' }, { repo: 'llm-client', path: 'tests/regions.rs' }],
    sections: [
      {
        id: 'profiles',
        title: { en: 'Provider profiles', zh: '供应商连接配置' },
        paragraphs: [{
          en: 'A provider identifies a service. A profile identifies a connection: protocol, endpoint, models, authentication strategy, and optional native service routes. Built-in profiles cover OpenAI, Anthropic, Gemini, DeepSeek, Kimi, Qwen, MiniMax, GLM/Z.AI, xAI, OpenRouter, and GitHub Copilot.',
          zh: 'provider 标识服务供应商；profile 标识连接，包括协议、端点、模型、认证策略和可选原生服务路由。内置配置覆盖 OpenAI、Anthropic、Gemini、DeepSeek、Kimi、Qwen、MiniMax、GLM/Z.AI、xAI、OpenRouter 与 GitHub Copilot。',
        }, {
          en: 'The built-in wire families include OpenAI Responses, Chat Completions, Anthropic Messages, and Gemini. Azure, Bedrock, Vertex, and Foundry have adapters for configured profiles; an adapter is not a built-in account or endpoint.',
          zh: '内置协议包括 OpenAI Responses、Chat Completions、Anthropic Messages 与 Gemini。Azure、Bedrock、Vertex 与 Foundry 有可配置连接的适配器；适配器并不代表内置账户或端点。',
        }],
        bullets: [
          { en: 'Use client.providers() for visible connections, client.chat().models() for chat models, and client.images().models() for image models.', zh: '通过 client.providers() 查看可见连接，通过 client.chat().models() 查看对话模型，通过 client.images().models() 查看图像模型。' },
          { en: 'Choose Region::ChinaMainland or Region::International before building. Lists, resolution, and failover respect this region; custom profiles without a regions declaration permit both.', zh: '创建客户端前选择 Region::ChinaMainland 或 Region::International。列表、解析与回退均遵守该区域；未声明 regions 的自定义连接允许两者。' },
          { en: 'Kimi, Qwen, and MiniMax regional profiles use distinct credentials. Match the profile to the account and key region; keys are not interchangeable.', zh: 'Kimi、Qwen 与 MiniMax 的区域连接使用不同凭据。连接必须与账户和密钥所在区域一致，区域间密钥不可互换。' },
        ],
        note: { en: 'CapabilitySupport::Unknown is distinct from Unsupported. Catalog metadata, implemented adapters, account permissions, and successful live calls establish different facts.', zh: 'CapabilitySupport::Unknown 与 Unsupported 不同。目录元数据、已实现适配器、账户权限和成功的真实调用分别代表不同的事实。' },
      },
      {
        id: 'managed-catalog',
        title: { en: 'Manage persistent configuration', zh: '管理持久化配置' },
        code: [{
          label: 'Managed client · async-host excerpt / 托管客户端片段',
          language: 'rust', verification: 'excerpt',
          code: `use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::Region;

let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
let (client, config) = LlmClientBuilder::new(&profiles)?
    .with_region(Region::International)
    .build_managed()?;
config.set_config_dir("./config").await?;
let snapshot = client.snapshot();
let profile = snapshot.profile("openai");`,
        }],
        apis: [
          { name: 'LlmClientBuilder::build_managed', signature: 'pub fn build_managed(self) -> Result<(LlmClient, ClientConfigManager), BuildError>', description: { en: 'Return the shareable client and a separate configuration manager.', zh: '返回共享客户端与独立的配置管理器。' } },
          { name: 'ClientConfigManager::set_config_dir', signature: 'pub async fn set_config_dir(&self, path: impl AsRef<Path>) -> Result<(), ProviderStoreError>', description: { en: 'Load and persist providers.json. Call again at startup to restore saved configuration.', zh: '加载并持久化 providers.json。每次启动时调用以恢复配置。' } },
          { name: 'ClientConfigManager::add_provider', signature: 'pub async fn add_provider(&self, profile: ProviderProfile) -> Result<(), ProviderStoreError>', description: { en: 'Add or replace a complete connection profile.', zh: '添加或完整替换一个连接配置。' } },
          { name: 'ClientConfigManager::sync_provider', signature: 'pub async fn sync_provider(&self, name: &str, credential: Option<&Secret<String>>) -> Result<usize, ProviderStoreError>', description: { en: 'Refresh one profile using its own account credential and supported directory API.', zh: '使用该连接账户的凭据及受支持的目录 API 刷新模型。' } },
          { name: 'ClientConfigManager::set_model_visibility', signature: 'pub async fn set_model_visibility(&self, name: &str, wire: &str, visible: bool) -> Result<(), ProviderStoreError>', description: { en: 'Set visibility of the selected model row.', zh: '设置所选模型行的可见性。' } },
        ],
        note: { en: 'Configuration v3 rejects v1/v2 with UnsupportedVersion; it does not silently migrate them. Updates publish a new snapshot for future operations. In-flight calls keep their captured configuration.', zh: '配置 v3 会以 UnsupportedVersion 拒绝 v1/v2，不会静默迁移。更新为后续操作发布新快照；进行中的调用保留已捕获的配置。' },
      },
      {
        id: 'native-services',
        title: { en: 'Provider-native resources', zh: '供应商原生资源' },
        paragraphs: [{
          en: 'Use client.provider::<T>(profile) to bind an exact connection to its provider client. Public clients include OpenAiClient, AnthropicClient, GoogleClient, DeepSeekClient, KimiClient, QwenClient, MiniMaxClient, ZhipuClient, XaiClient, OpenRouterClient, and GithubCopilotClient.',
          zh: '使用 client.provider::<T>(profile) 将精确连接绑定到供应商客户端。公开客户端包括 OpenAiClient、AnthropicClient、GoogleClient、DeepSeekClient、KimiClient、QwenClient、MiniMaxClient、ZhipuClient、XaiClient、OpenRouterClient 与 GithubCopilotClient。',
        }, {
          en: 'Native resources keep their own types and support boundaries: files, retrieval, batches, background jobs, remote tools, speech, and realtime sessions vary by provider. Use the generated current Rustdoc for complete resource types and signatures, and the provider resources chapter for scope and lifecycle constraints. Realtime audio device capture and playback remain in the host.',
          zh: '原生资源保留独立类型与支持边界：文件、检索、批处理、后台任务、远程工具、语音和实时会话因供应商而异。完整资源类型与签名见生成的当前 Rustdoc，作用域与生命周期限制见供应商资源章节。实时音频设备采集与播放仍由宿主负责。',
        }],
        apis: [
          { name: 'LlmClient::provider', signature: 'pub fn provider<T: ProviderClient>(&self, profile_name: &str) -> Result<T, ProviderBindingError>', description: { en: 'Reject a mismatched provider/profile binding rather than inferring another route.', zh: '拒绝不匹配的供应商与连接绑定，不会推测其他路由。' } },
          { name: 'EmbeddingService::embed', signature: 'pub async fn embed(self, profile_name: &str, input: &EmbeddingRequest, options: &RequestOptions) -> Result<EmbeddingResponse, EmbeddingError>', description: { en: 'Embed text using an enabled independent route. Model limits, dimensions and task support are validated without automatic splitting.', zh: '通过启用的独立路由嵌入文本。校验模型限制、维度与任务支持，不会自动拆分。' } },
        ],
        code: [{
          label: 'Provider binding · excerpt with an existing client / 供应商绑定片段',
          language: 'rust', verification: 'excerpt',
          code: `use lingxi_llm_client::providers::OpenAiClient;

let openai = client.provider::<OpenAiClient>("openai")?;
let audio = openai.audio();
let batches = openai.batches();
let retrieval = openai.retrieval();
let background = openai.background();`,
        }],
      },
      {
        id: 'pricing',
        title: { en: 'Usage & frozen prices', zh: '用量与固定价格' },
        paragraphs: [{
          en: 'Keep the executed profile, UsageReport and InferenceReport together. FrozenPricing captures the selected model row before dispatch, so later catalog updates cannot alter an attempt’s prices. quote() supports admission estimates; estimate() requires complete valid usage and confirmed execution facts.',
          zh: '将实际执行连接、UsageReport 与 InferenceReport 一起保留。FrozenPricing 在发送前捕获所选模型行，因此后续目录更新不会改变该次调用的价格。quote() 支持准入报价；estimate() 要求完整有效的用量与已确认的执行事实。',
        }],
        apis: [
          { name: 'FrozenPricing::capture', signature: 'pub fn capture(profile: &ProviderProfile, display_model: &str, request_model: &str) -> Result<Self, LlmError>', description: { en: 'Capture an unambiguous exact model row from configuration.', zh: '从配置中捕获无歧义的精确模型行。' } },
          { name: 'FrozenPricing::quote', signature: 'pub fn quote(&self, context: &PricingContext) -> Result<PriceQuote, LlmError>', description: { en: 'Quote selected rates without claiming actual usage.', zh: '为所选费率报价，不将其表示为实际用量。' } },
          { name: 'FrozenPricing::estimate', signature: 'pub fn estimate(&self, report: &UsageReport, inference: &InferenceReport, submission: Submission) -> Result<CostEstimate, LlmError>', description: { en: 'Estimate the observed attempt with frozen rates and retained currency. Partial usage, unpublished prices, or an unknown executed Fast tier return CostUnavailable.', zh: '用固定费率估算已观测调用并保留币种。部分用量、未公布价格或未知的实际 Fast 等级会返回 CostUnavailable。' } },
          { name: 'LlmClient::estimate_local_tokens_in', signature: 'pub fn estimate_local_tokens_in(&self, profile: &str, request: &ChatRequest) -> Result<LocalTokenEstimate, LocalTokenCountError>', description: { en: 'Estimate visible input offline using enabled mapped tokenizers; omissions are explicit and are not provider billing usage.', zh: '使用已启用且已映射的分词器离线估算可见输入；明确报告遗漏项，不代表供应商结费用量。' } },
        ],
        note: { en: 'Reasoning effort changes token consumption, not the declared unit rate. Catalogs and cost estimates do not establish live provider availability, a final invoice, or a host accounting ledger.', zh: '推理 effort 改变 token 消耗，不改变声明的单位费率。目录与成本估算不代表真实供应商可用性、最终账单或宿主结算账本。' },
      },
      {
        "id": "authentication-hosting",
        "title": {
          "en": "Authentication and hosting boundaries",
          "zh": "认证与云托管边界"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Route/auth class",
                "zh": "路由/认证类型"
              },
              {
                "en": "Host responsibility",
                "zh": "宿主职责"
              },
              {
                "en": "SDK responsibility",
                "zh": "SDK 职责"
              }
            ],
            "rows": [
              [
                {
                  "en": "API key / bearer",
                  "zh": "API key / bearer"
                },
                {
                  "en": "Select account and refresh/store valid secret",
                  "zh": "选择账户并刷新/存储有效凭据"
                },
                {
                  "en": "Encode configured headers once for the selected route",
                  "zh": "按所选路由一次性编码配置请求头"
                }
              ],
              [
                {
                  "en": "Restricted ChatGPT / Copilot routes",
                  "zh": "受限 ChatGPT / Copilot 路由"
                },
                {
                  "en": "Provide entitled account token and grant state",
                  "zh": "提供有授权的账户 token 与 grant 状态"
                },
                {
                  "en": "Enforce allowed route/resource policy; no generic API-key equivalence",
                  "zh": "执行允许的路由/资源策略；不等同通用 API key"
                }
              ],
              [
                {
                  "en": "Bedrock AWS SigV4",
                  "zh": "Bedrock AWS SigV4"
                },
                {
                  "en": "Provide credential and exact signing configuration",
                  "zh": "提供凭据与精确签名配置"
                },
                {
                  "en": "Encode hosted protocol and authenticate final bytes",
                  "zh": "编码托管协议并认证最终字节"
                }
              ],
              [
                {
                  "en": "Vertex / Azure / Foundry",
                  "zh": "Vertex / Azure / Foundry"
                },
                {
                  "en": "Provide project/location or deployment/resource and appropriate credential",
                  "zh": "提供 project/location 或 deployment/resource 与适当凭据"
                },
                {
                  "en": "Use the declared wrapper; native services opt in separately",
                  "zh": "使用已声明包装协议；原生服务需单独启用"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "A provider-compatible gateway does not become the official provider just because it shares a protocol. Exact endpoint and provider identity checks gate native resources, decisions, hosted tools and account APIs. Builder codec support and model metadata establish local capabilities; account authorization and regional endpoint access remain independent.",
            "zh": "供应商兼容网关不会因共享协议而成为官方供应商。精确端点与供应商身份检查限定原生资源、决策、托管工具与账户 API。Builder codec 支持与模型元数据证明本地能力；账户授权与区域端点访问仍独立。"
          }
        ]
      },
      {
        "id": "catalog-provenance",
        "title": {
          "en": "Catalog provenance and verification",
          "zh": "目录来源与验证"
        },
        "paragraphs": [
          {
            "en": "The checked-in provider TOML and capability matrices describe current implemented/admitted contracts, with provider/model/protocol-specific supported, unsupported or unknown facts. Directory sync can add observed models but does not invent their prices or native capabilities. A model removed from the static catalog can retain observed identity without retaining withdrawn static pricing.",
            "zh": "仓库中的供应商 TOML 与能力矩阵描述当前实现/准入约定，按供应商、模型、协议记录 supported、unsupported 或 unknown。目录同步可添加观测模型，不编造价格或原生能力。模型从静态目录移除后可保留观测身份，但不保留撤回的静态价格。"
          }
        ],
        "bullets": [
          {
            "en": "Record selected profile, wire model, source baseline and account region with validation results. Compile/mock tests prove public API/wire behavior; a live request separately proves that one account accepted one operation at that time.",
            "zh": "验证结果记录所选连接、协议模型、源码基线与账户区域。编译/模拟测试证明公共 API/协议行为；真实请求单独证明某一账户在当时接受了某一操作。"
          },
          {
            "en": "A full baseline mismatch between local SDK HEAD and the product’s dependency revision is visible in the handbook source data. Keep local path consumer verification separate from Git-pinned installation verification; do not edit production manifests or locks to make the examples compile.",
            "zh": "本地 SDK HEAD 与产品依赖修订不同会显示于手册源码数据。工作树 path 消费者与 Git 固定安装分别验证，不能为示例编译而改生产清单或锁文件。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Current protocol feature matrix source",
              "zh": "当前协议能力矩阵源码"
            },
            "href": "https://github.com/lingxi-coder/llm-client"
          },
          {
            "label": {
              "en": "Per-attempt usage and account query distinctions",
              "zh": "逐次用量与账户查询的区别"
            },
            "href": "/docs/llm-execution-accounting"
          }
        ]
      },
    ],
  },
];

// Task-oriented recipes supplement the detailed API reference above.
const practicalGuides: Record<string, { sources: string[]; sections: DocSection[] }> = {
  "llm-client": {
    "sources": [
      "src/client/chat.rs",
      "src/client/options.rs",
      "src/protocol/usage_report.rs"
    ],
    "sections": [
      {
        "id": "choose-call-pattern",
        "title": {
          "en": "Choose a call pattern",
          "zh": "先选择调用方式"
        },
        "paragraphs": [
          {
            "en": "Start with complete_in when the application needs one final answer. Switch to stream_in when people need to see progress. Use a prepared call only when the host needs an admission checkpoint or one physical-attempt ledger; the same ChatRequest and credentials model applies to all three.",
            "zh": "需要一次完整答案时先用 complete_in；需要向用户显示生成进度时用 stream_in。只有宿主需要准入检查点或逐次调用账本时才使用 prepared call。三种方式共享 ChatRequest 与请求级凭据模型。"
          }
        ],
        "tables": [
          {
            "headers": [
              {
                "en": "Entry",
                "zh": "入口"
              },
              {
                "en": "What you receive",
                "zh": "返回什么"
              },
              {
                "en": "Host action",
                "zh": "宿主接下来做什么"
              }
            ],
            "rows": [
              [
                {
                  "en": "chat().complete_in",
                  "zh": "chat().complete_in"
                },
                {
                  "en": "ChatResponse or LlmError",
                  "zh": "ChatResponse 或 LlmError"
                },
                {
                  "en": "Inspect stop_reason, preserve the assistant message and usage",
                  "zh": "检查 stop_reason，保存完整助手消息与用量"
                }
              ],
              [
                {
                  "en": "chat().stream_in",
                  "zh": "chat().stream_in"
                },
                {
                  "en": "ModelStream, then asynchronous events",
                  "zh": "ModelStream，随后异步接收事件"
                },
                {
                  "en": "Render deltas; retain native blocks; distinguish End, EOF and read errors",
                  "zh": "显示增量，保留原生块，区分 End、EOF 与读取错误"
                }
              ],
              [
                {
                  "en": "snapshot().prepare_on",
                  "zh": "snapshot().prepare_on"
                },
                {
                  "en": "PreparedCall for one exact connection",
                  "zh": "精确连接上的 PreparedCall"
                },
                {
                  "en": "Admit and dispatch once; capture accounting before decode; finish cleanup",
                  "zh": "准入后发送一次，解码前记录结算事实，最后清理"
                }
              ],
              [
                {
                  "en": "provider::<T>(profile)",
                  "zh": "provider::<T>(profile)"
                },
                {
                  "en": "Provider-specific resource handle",
                  "zh": "供应商特定资源句柄"
                },
                {
                  "en": "Use only that resource’s account scope and lifecycle",
                  "zh": "遵守该资源独立的账户作用域与生命周期"
                }
              ]
            ]
          }
        ],
        "links": [
          {
            "label": {
              "en": "Request parameters and tool round trips",
              "zh": "请求参数与工具闭环"
            },
            "href": "/docs/llm-requests"
          },
          {
            "label": {
              "en": "Display a streaming response",
              "zh": "显示流式响应"
            },
            "href": "/docs/llm-streaming"
          }
        ]
      },
      {
        "id": "bounded-first-call",
        "title": {
          "en": "A bounded application call",
          "zh": "为应用写一个有时限的调用"
        },
        "paragraphs": [
          {
            "en": "This function accepts a configured profile/model and a credential from its caller. It adds an output budget and a 45-second total deadline without changing the shared client. The caller still needs to choose a model present on that profile and handle the returned stop reason.",
            "zh": "此函数由调用者提供已配置连接、模型与凭据，设置输出上限和 45 秒总时限，不改变共享客户端。调用者仍需选择该连接中的模型，并处理返回的终止原因。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Application helper · source-checked, not live-account acceptance",
              "zh": "应用辅助函数 · 已核对源码，未代表真实账户验收"
            },
            "language": "rust",
            "verification": "compile",
            "exampleId": "llm-bounded-first-call",
            "code": "use std::time::Duration;\nuse lingxi_llm_client::{LlmClient, RequestOptions};\nuse lingxi_llm_client::protocol::{ChatRequest, ChatResponse, ConversationMessage, LlmError, Secret};\n\npub async fn ask_once(\n    client: &LlmClient,\n    profile: &str,\n    model: &str,\n    credential: Secret<String>,\n    prompt: &str,\n) -> Result<ChatResponse, LlmError> {\n    let mut request = ChatRequest::new(model);\n    request.messages.push(ConversationMessage::user_text(prompt));\n    request.max_tokens = Some(512);\n    let options = RequestOptions {\n        credential: Some(credential),\n        total_timeout: Some(Duration::from_secs(45)),\n        ..Default::default()\n    };\n    client.chat().complete_in(profile, &request, &options).await\n}"
          }
        ],
        "note": {
          "en": "complete_in uses the configured high-level routing policy. If the host requires at most one generation dispatch, use prepare_on and dispatch_once rather than adding an outer retry loop.",
          "zh": "complete_in 遵循已配置的高层路由策略。宿主若要求最多发送一次生成请求，应使用 prepare_on 与 dispatch_once，而不是外套重试循环。"
        }
      },
      {
        "id": "first-answer-checks",
        "title": {
          "en": "Read the answer as a result, not just text",
          "zh": "读取结果，而不仅是文本"
        },
        "steps": [
          {
            "en": "Read response.stop_reason before presenting a success state. A token-limit result is partial; ToolUse asks for another host-controlled step; Refusal is an explicit outcome.",
            "zh": "先读取 response.stop_reason 再显示成功状态。达到 token 上限表示输出不完整；ToolUse 要求宿主继续处理；Refusal 是明确的结果。"
          },
          {
            "en": "Use response.message.text() for display, and persist response.message itself for conversation replay. The text convenience method omits thinking and tool blocks.",
            "zh": "显示时可用 response.message.text()，重放对话时保存 response.message 本身。文本辅助方法不会保留 thinking 和工具块。"
          },
          {
            "en": "Record executed_profile, model, response_id and usage alongside the host’s request ID. Missing usage is unknown, not free.",
            "zh": "将 executed_profile、model、response_id 与 usage 和宿主请求 ID 一起记录。用量缺失表示未知，不能当作免费。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Handle failure and usage states",
              "zh": "处理失败与用量状态"
            },
            "href": "/docs/llm-execution-accounting"
          }
        ]
      }
    ]
  },
  "llm-requests": {
    "sources": [
      "src/client/chat.rs",
      "src/protocol/message.rs",
      "src/protocol/json_payload.rs",
      "src/protocol/continuation.rs"
    ],
    "sections": [
      {
        "id": "input-field-guide",
        "title": {
          "en": "Common input fields and defaults",
          "zh": "常用输入字段与默认值"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Field",
                "zh": "字段"
              },
              {
                "en": "Meaning / default",
                "zh": "含义 / 默认值"
              },
              {
                "en": "When to set it",
                "zh": "何时设置"
              }
            ],
            "rows": [
              [
                {
                  "en": "model",
                  "zh": "model"
                },
                {
                  "en": "Configured model selector; ChatRequest::new requires it",
                  "zh": "已配置模型选择器；ChatRequest::new 必须提供"
                },
                {
                  "en": "Choose from the selected connection rather than passing a display label by guess",
                  "zh": "从所选连接选择，不猜测显示名是否可用于请求"
                }
              ],
              [
                {
                  "en": "messages",
                  "zh": "messages"
                },
                {
                  "en": "Empty ordered conversation input",
                  "zh": "默认为空的有序对话输入"
                },
                {
                  "en": "First user turn plus full assistant/tool turns for stateless replay",
                  "zh": "首条用户消息，以及无状态重放所需的完整助手/工具轮次"
                }
              ],
              [
                {
                  "en": "max_tokens",
                  "zh": "max_tokens"
                },
                {
                  "en": "Optional output ceiling, not an input context limit",
                  "zh": "可选输出上限，不是输入上下文上限"
                },
                {
                  "en": "Bound response size; allow enough room for reasoning where supported",
                  "zh": "限制响应体量；支持推理时为推理输出留出空间"
                }
              ],
              [
                {
                  "en": "tools / tool_choice",
                  "zh": "tools / tool_choice"
                },
                {
                  "en": "No functions; ToolChoice::Auto",
                  "zh": "默认无函数；ToolChoice::Auto"
                },
                {
                  "en": "Advertise host functions and choose Auto, Any, None or one named Tool",
                  "zh": "声明宿主函数，并选择 Auto、Any、None 或指定 Tool"
                }
              ],
              [
                {
                  "en": "output_format",
                  "zh": "output_format"
                },
                {
                  "en": "OutputFormat::Text",
                  "zh": "默认为 OutputFormat::Text"
                },
                {
                  "en": "Ask for a JSON object or schema-constrained final answer",
                  "zh": "需要 JSON 对象或 Schema 约束的最终答案时设置"
                }
              ],
              [
                {
                  "en": "continuation",
                  "zh": "continuation"
                },
                {
                  "en": "No provider state reference",
                  "zh": "默认不带供应商状态引用"
                },
                {
                  "en": "Use a completed, same-account ContinuationRef and send only new input",
                  "zh": "使用已完成且账户匹配的 ContinuationRef，仅发送新输入"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "Optional controls are validated by the selected model and protocol. An explicitly unsupported option is an error; the client does not silently translate every provider feature into another wire format.",
          "zh": "可选控制项由所选模型和协议校验。显式指定不支持的选项会报错，客户端不会把每个供应商特性静默转换为其他协议。"
        }
      },
      {
        "id": "request-option-guide",
        "title": {
          "en": "Options that should be request-scoped",
          "zh": "应按请求传入的选项"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Option",
                "zh": "选项"
              },
              {
                "en": "Application contract",
                "zh": "应用契约"
              }
            ],
            "rows": [
              [
                {
                  "en": "credential",
                  "zh": "credential"
                },
                {
                  "en": "A valid secret for the primary connection; the host refreshes and stores it",
                  "zh": "主连接的有效密钥；由宿主刷新与存储"
                }
              ],
              [
                {
                  "en": "fallback_credentials",
                  "zh": "fallback_credentials"
                },
                {
                  "en": "Keys indexed by exact fallback profile; never an implicit copy of the primary key",
                  "zh": "按精确备用连接名索引的密钥；不隐式复制主密钥"
                }
              ],
              [
                {
                  "en": "total_timeout",
                  "zh": "total_timeout"
                },
                {
                  "en": "Whole-operation budget including reads. Streams need an explicit value for a total deadline",
                  "zh": "包含响应读取的整次操作预算；流式请求需显式设置才有总时限"
                }
              ],
              [
                {
                  "en": "account_scope",
                  "zh": "account_scope"
                },
                {
                  "en": "Stable non-secret identity for provider continuation; use the same account on the next turn",
                  "zh": "供应商续接的稳定非敏感身份；下一轮须属于同一账户"
                }
              ],
              [
                {
                  "en": "file_account_scope",
                  "zh": "file_account_scope"
                },
                {
                  "en": "Independent account identity for eligible provider upload reuse",
                  "zh": "独立限定符合条件的供应商上传复用账户"
                }
              ],
              [
                {
                  "en": "mcp_authorizations",
                  "zh": "mcp_authorizations"
                },
                {
                  "en": "Fresh remote-server credentials indexed by declared label/name; keep them out of history",
                  "zh": "按声明 label/name 索引的新鲜远程凭据，不写入历史"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "Changing a model request must not mutate a global key, timeout or reasoning setting shared by other sessions. Pass a fresh RequestOptions for each application request and reuse the long-lived client handle.",
            "zh": "修改某次模型请求不应改变其他会话共用的全局密钥、超时或推理设置。每次应用请求传入新的 RequestOptions，复用长期存活的客户端句柄。"
          }
        ]
      },
      {
        "id": "function-tool-roundtrip",
        "title": {
          "en": "Complete a function-tool round trip",
          "zh": "完成一次函数工具闭环"
        },
        "paragraphs": [
          {
            "en": "A function definition describes what the model may request; it does not execute code. The host validates a completed ToolUse result, runs its registered implementation, adds a ToolResult with the original call ID, and makes the next request. Keep a round limit, cancellation path and permission decision in the host.",
            "zh": "函数声明描述模型可以请求的能力，并不会执行代码。宿主校验完成的 ToolUse 结果，运行已登记实现，以原调用 ID 添加 ToolResult，再发起下一次请求。轮次上限、取消路径和权限决策由宿主负责。"
          },
          {
            "en": "This source-checked example exposes one harmless in-memory article lookup. The request must already contain a user message, and the selected route must support strict tool input. It rejects unregistered/namespaced calls and malformed input rather than executing model-supplied names. It does not implement a general Agent runtime.",
            "zh": "这个已核对源码的示例只提供无副作用的内存文章查询。传入 request 需已有用户消息，所选路由须支持严格工具输入。它拒绝未登记/带命名空间的调用和非法参数，不按模型提供的名称任意执行，也不承担通用 Agent 运行时。"
          }
        ],
        "code": [
          {
            "label": {
              "en": "Bounded tool loop · host-owned implementation",
              "zh": "有界工具循环 · 宿主实现"
            },
            "language": "rust",
            "verification": "compile",
            "exampleId": "llm-function-tool-roundtrip",
            "code": "use lingxi_llm_client::{LlmClient, RequestOptions};\nuse lingxi_llm_client::protocol::{\n    ChatRequest, ChatResponse, ContentBlock, ConversationMessage,\n    MessageRole, StopReason, ToolChoice, ToolSpec,\n};\n\npub async fn answer_with_lookup(\n    client: &LlmClient,\n    profile: &str,\n    mut request: ChatRequest,\n    options: &RequestOptions,\n) -> Result<ChatResponse, Box<dyn std::error::Error>> {\n    request.tools = vec![serde_json::from_str::<ToolSpec>(r#\"{\n        \"name\":\"lookup_article\", \"description\":\"Read one local help article by ID.\",\n        \"input_schema\":{\"type\":\"object\",\"properties\":{\"article\":{\"type\":\"string\"}},\n            \"required\":[\"article\"],\"additionalProperties\":false}, \"strict\":true\n    }\"#)?];\n    request.tool_choice = ToolChoice::Auto;\n    for _ in 0..3 {\n        let response = client.chat().complete_in(profile, &request, options).await?;\n        if !response.tool_input_issues.is_empty() {\n            return Err(\"tool input was not valid JSON\".into());\n        }\n        if response.stop_reason != StopReason::ToolUse { return Ok(response); }\n        let mut results = Vec::new();\n        for (id, namespace, name, input) in response.message.tool_uses() {\n            if namespace.is_some() || name != \"lookup_article\" {\n                return Err(\"tool is not registered by this host\".into());\n            }\n            let article = input.string_at(\"/article\").ok_or(\"article must be a string\")?;\n            let found = article == \"quickstart\";\n            results.push(ContentBlock::ToolResult {\n                tool_use_id: id.clone(),\n                content: if found { \"Use the LLM Client quickstart.\" } else { \"Article not found.\" }.into(),\n                is_error: Some(!found), blocks: None, cache_reference: None, toolset_name: None,\n            });\n        }\n        if results.is_empty() { return Err(\"tool-use response has no callable tool\".into()); }\n        request.messages.push(response.message);\n        request.messages.push(ConversationMessage {\n            role: MessageRole::User, content: results, native_options: Vec::new(),\n        });\n    }\n    Err(\"tool round limit reached\".into())\n}"
          }
        ],
        "note": {
          "en": "Preserve the complete assistant message before appending tool results: tool IDs, native provider content and reasoning signatures may be required by the next request. For permissions, tool scheduling and persistent sessions, integrate Harness instead.",
          "zh": "添加工具结果前保留完整助手消息：下一请求可能依赖工具 ID、供应商原生内容和推理签名。需要权限、工具调度与持久会话时，应接入 Harness。"
        },
        "links": [
          {
            "label": {
              "en": "Harness tools and permissions",
              "zh": "Harness 工具与权限"
            },
            "href": "/docs/harness-tools"
          },
          {
            "label": {
              "en": "Structured tool and output contracts",
              "zh": "工具与输出的结构化约定"
            },
            "href": "/docs/llm-controls"
          }
        ]
      }
    ]
  },
  "llm-streaming": {
    "sources": [
      "src/protocol/message.rs",
      "src/protocol/usage_report.rs"
    ],
    "sections": [
      {
        "id": "event-handling-map",
        "title": {
          "en": "What to do with each event family",
          "zh": "各类事件应该如何处理"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Event",
                "zh": "事件"
              },
              {
                "en": "Use",
                "zh": "用途"
              },
              {
                "en": "Do not infer",
                "zh": "不能据此推断"
              }
            ],
            "rows": [
              [
                {
                  "en": "Start",
                  "zh": "Start"
                },
                {
                  "en": "Retain model and optional response identity",
                  "zh": "保存模型与可用响应身份"
                },
                {
                  "en": "A final answer or a reusable continuation",
                  "zh": "已经生成最终答案或可复用续接"
                }
              ],
              [
                {
                  "en": "TextDelta / TextDeltaJsUtf16",
                  "zh": "TextDelta / TextDeltaJsUtf16"
                },
                {
                  "en": "Update the displayed block; preserve exact UTF-16 sidecars when supplied",
                  "zh": "更新显示块；有 UTF-16 sidecar 时保留精确内容"
                },
                {
                  "en": "A complete response or parseable JSON document",
                  "zh": "完整响应或可解析的 JSON 文档"
                }
              ],
              [
                {
                  "en": "ToolCallDelta / BlockEnd",
                  "zh": "ToolCallDelta / BlockEnd"
                },
                {
                  "en": "Accumulate the original block and arguments; inspect terminal assembly before execution",
                  "zh": "按原块累计参数；执行前检查终止装配结果"
                },
                {
                  "en": "Permission to execute partial arguments",
                  "zh": "可以执行尚未完整的参数"
                }
              ],
              [
                {
                  "en": "ProviderContent / ThoughtSignature",
                  "zh": "ProviderContent / ThoughtSignature"
                },
                {
                  "en": "Retain replay data and signatures in the assistant transcript",
                  "zh": "在助手历史中保存可重放内容与签名"
                },
                {
                  "en": "That display-only reasoning text can replace native replay content",
                  "zh": "仅用于显示的推理文本可以替换原生重放内容"
                }
              ],
              [
                {
                  "en": "ProviderEvent / NativeControl",
                  "zh": "ProviderEvent / NativeControl"
                },
                {
                  "en": "Keep provider observation/control data separately",
                  "zh": "单独保留供应商观测或控制数据"
                },
                {
                  "en": "A local function call to dispatch",
                  "zh": "应发送给本地函数执行的调用"
                }
              ],
              [
                {
                  "en": "End",
                  "zh": "End"
                },
                {
                  "en": "Inspect stop_reason, usage and inference; then drain through None",
                  "zh": "检查 stop_reason、usage 与 inference，随后继续读到 None"
                },
                {
                  "en": "That every terminal response is a successful, untruncated final answer",
                  "zh": "每个终止响应都是成功且完整的最终答案"
                }
              ]
            ]
          }
        ]
      },
      {
        "id": "stream-result-checklist",
        "title": {
          "en": "Build a final message safely",
          "zh": "正确构造最终消息"
        },
        "steps": [
          {
            "en": "Use StreamAccumulator to retain events while the UI shows text. Key content by the original block index, because indices may be sparse and different output kinds may interleave.",
            "zh": "界面显示文本时，用 StreamAccumulator 保留事件。按原始块索引保存内容，因为索引可能不连续，不同输出类型也可能交错出现。"
          },
          {
            "en": "When reading ends, inspect StreamAssembly.terminal, unfinished_blocks and incomplete_tools. A MaxTokens stop is terminal but may still truncate the answer or a tool argument.",
            "zh": "读取结束时检查 StreamAssembly.terminal、unfinished_blocks 与 incomplete_tools。MaxTokens 属于终止结果，但答案或工具参数仍可能被截断。"
          },
          {
            "en": "Persist usage_report and inference_report before deciding whether to render an error, execute tools or schedule another attempt. Reading failed after output began is not the same as no request having been sent.",
            "zh": "在决定显示错误、执行工具或安排下一次调用前，保存 usage_report 与 inference_report。已经输出内容后的读取失败，不等于请求从未发送。"
          },
          {
            "en": "Keep the full reconstructed assistant message for the next request. If all you retained was the concatenated visible text, do not fabricate missing signatures, tool calls or provider-native blocks.",
            "zh": "为下一请求保存完整重建的助手消息。如果只保存了拼接显示文本，不应补造缺失签名、工具调用或供应商原生块。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Lossless assembly and incomplete blocks",
              "zh": "无损装配与未完成块"
            },
            "href": "/docs/llm-streaming#lossless-assembly"
          },
          {
            "label": {
              "en": "Per-attempt accounting on failure",
              "zh": "失败后的逐次结算"
            },
            "href": "/docs/llm-execution-accounting"
          }
        ]
      },
      {
        "id": "stream-ui-behavior",
        "title": {
          "en": "Keep the UI responsive without changing protocol semantics",
          "zh": "保持界面响应，同时保留协议语义"
        },
        "paragraphs": [
          {
            "en": "Batch display updates in the UI instead of repainting for every token, but keep event ordering and accounting observations unchanged. If the UI falls behind, bound its queue rather than building an unbounded string/event buffer.",
            "zh": "界面可合并更新，避免每个 token 都重绘，但应保留事件顺序与结算观测。界面消费滞后时限制队列长度，而不是无限累积字符串或事件。"
          },
          {
            "en": "A user stop drops the stream and records the partial attempt. A later retry is a fresh request with a new application attempt identity; it is not a promise to resume exactly at the last displayed character. Provider-scoped continuation is available only after a completed response.",
            "zh": "用户停止时丢弃流并记录本次部分结果。之后重试是带新应用尝试身份的新请求，不保证从最后一个显示字符精确续传。只有完成的响应才提供供应商作用域续接。"
          }
        ]
      }
    ]
  },
  "llm-providers": {
    "sources": [
      "src/client/live.rs",
      "src/client/chat.rs",
      "src/protocol/continuation.rs"
    ],
    "sections": [
      {
        "id": "route-diagnosis",
        "title": {
          "en": "Diagnose a missing model or wrong route",
          "zh": "排查模型缺失或路由不符"
        },
        "steps": [
          {
            "en": "Confirm the profile was registered in the builder and build succeeded with the intended Region. A profile name, provider identity and protocol family are different selectors.",
            "zh": "确认 builder 已登记该连接，并用预期 Region 成功构建。连接名、供应商身份和协议族是不同的选择器。"
          },
          {
            "en": "Inspect chat().models() for chat choices, images().models() for image choices, or the specific service capability method. Do not reuse the Chat model list as a catalog for every resource.",
            "zh": "Chat 用 chat().models()，图像用 images().models()，其他服务用各自能力查询。不要把 Chat 模型列表当作所有资源的统一目录。"
          },
          {
            "en": "Resolve the configured model/profile before sending and check alias, wire model, region and visibility. A successful local resolution does not prove that the provider account has live model entitlement.",
            "zh": "发送前解析已配置模型/连接，检查别名、协议模型、区域与可见性。本地解析成功不能证明供应商账户拥有真实模型权限。"
          },
          {
            "en": "When the provider rejects the call, retain typed error classification, status and request ID. Repair credentials or the model mapping for that exact connection; do not silently send the same secret to another endpoint.",
            "zh": "供应商拒绝调用时保留类型化分类、状态与请求 ID。应修复该精确连接的凭据或模型映射，不要把同一密钥静默发送到其他端点。"
          }
        ],
        "links": [
          {
            "label": {
              "en": "Request-local credential scopes",
              "zh": "请求级凭据作用域"
            },
            "href": "/docs/llm-requests#request-option-guide"
          },
          {
            "label": {
              "en": "Error action reference",
              "zh": "错误处理参考"
            },
            "href": "/docs/llm-execution-accounting#error-action-reference"
          }
        ]
      },
      {
        "id": "capability-selection",
        "title": {
          "en": "Select capabilities before composing input",
          "zh": "先选择能力，再构造输入"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Need",
                "zh": "需求"
              },
              {
                "en": "Inspect",
                "zh": "查询入口"
              },
              {
                "en": "Failure to handle",
                "zh": "需要处理的失败"
              }
            ],
            "rows": [
              [
                {
                  "en": "Text and function tools",
                  "zh": "文本与函数工具"
                },
                {
                  "en": "Selected model/profile and codec input support",
                  "zh": "所选模型/连接与 codec 输入支持"
                },
                {
                  "en": "Unsupported tool, reasoning or output control",
                  "zh": "不支持的工具、推理或输出控制"
                }
              ],
              [
                {
                  "en": "Image generation or editing",
                  "zh": "图像生成或编辑"
                },
                {
                  "en": "images().capabilities_in(profile, model)",
                  "zh": "images().capabilities_in(profile, model)"
                },
                {
                  "en": "Unsupported mask/reference/task option",
                  "zh": "不支持的遮罩、参考图或任务选项"
                }
              ],
              [
                {
                  "en": "Speech / transcription",
                  "zh": "语音合成 / 转写"
                },
                {
                  "en": "audio().capabilities(&AudioRoute)",
                  "zh": "audio().capabilities(&AudioRoute)"
                },
                {
                  "en": "Unavailable common adapter or unsupported voice/format",
                  "zh": "统一适配器不可用或声音/格式不支持"
                }
              ],
              [
                {
                  "en": "Provider files or cloud resources",
                  "zh": "供应商文件或云端资源"
                },
                {
                  "en": "A typed provider client and resource-specific capability/scope",
                  "zh": "类型化供应商客户端及资源专属能力/作用域"
                },
                {
                  "en": "Provider binding, endpoint, account or purpose mismatch",
                  "zh": "供应商绑定、端点、账户或用途不符"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "A compatible JSON chat wire is not a capability grant. Keep the unsupported state visible to callers so they can choose another supported workflow deliberately.",
          "zh": "兼容 JSON Chat 协议不代表获得所有能力。应将不支持状态明确交给调用者，让其主动选择受支持的流程。"
        }
      }
    ]
  },
  "llm-images": {
    "sources": [
      "src/images/mod.rs"
    ],
    "sections": [
      {
        "id": "image-workflow-choice",
        "title": {
          "en": "Choose immediate output or a durable task",
          "zh": "选择即时结果或持久任务"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Path",
                "zh": "路径"
              },
              {
                "en": "Use when",
                "zh": "何时使用"
              },
              {
                "en": "Persist",
                "zh": "需要保存"
              }
            ],
            "rows": [
              [
                {
                  "en": "generate_in / edit_in",
                  "zh": "generate_in / edit_in"
                },
                {
                  "en": "The configured route returns completed image output",
                  "zh": "配置路由直接返回完成图像"
                },
                {
                  "en": "Output source, request ID, executed profile and usage; save temporary media promptly",
                  "zh": "输出来源、请求 ID、执行连接与用量，并及时保存临时媒体"
                }
              ],
              [
                {
                  "en": "submit_in → get_task",
                  "zh": "submit_in → get_task"
                },
                {
                  "en": "The selected provider exposes native asynchronous tasks",
                  "zh": "所选供应商提供原生异步任务"
                },
                {
                  "en": "The exact ImageTaskRef before leaving the screen or restarting the process",
                  "zh": "离开页面或重启进程前保存精确 ImageTaskRef"
                }
              ]
            ]
          }
        ],
        "steps": [
          {
            "en": "Call capabilities_in for the exact profile/model, then construct only supported size, reference, mask and task options.",
            "zh": "先为精确连接/模型调用 capabilities_in，再填写受支持的尺寸、参考图、遮罩与任务选项。"
          },
          {
            "en": "After submission, store the returned reference and poll at a host-chosen interval. Keep account_scope and credentials tied to that same account.",
            "zh": "提交后存储返回引用，并按宿主设定间隔查询。account_scope 与凭据始终绑定同一账户。"
          },
          {
            "en": "Treat an unknown submission outcome as reconciliation work. Do not create another image job simply because the first response was lost.",
            "zh": "提交结果未知时应核对现有状态，不能仅因首个响应丢失就新建图像任务。"
          }
        ]
      },
      {
        "id": "image-error-actions",
        "title": {
          "en": "Recover from image errors without duplicate generation",
          "zh": "避免重复生成的错误恢复"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "ImageError::dispatch()",
                "zh": "ImageError::dispatch()"
              },
              {
                "en": "What it establishes",
                "zh": "已经确定的事实"
              },
              {
                "en": "Host action",
                "zh": "宿主动作"
              }
            ],
            "rows": [
              [
                {
                  "en": "NotSent",
                  "zh": "NotSent"
                },
                {
                  "en": "This image operation did not dispatch generation",
                  "zh": "该图像操作未发送生成请求"
                },
                {
                  "en": "Fix local input, capability or scope and make a new admitted call",
                  "zh": "修正本地输入、能力或作用域后重新准入"
                }
              ],
              [
                {
                  "en": "Rejected",
                  "zh": "Rejected"
                },
                {
                  "en": "The provider rejected the operation",
                  "zh": "供应商拒绝操作"
                },
                {
                  "en": "Use the underlying typed error to correct the request/account",
                  "zh": "根据底层类型化错误修正请求或账户"
                }
              ],
              [
                {
                  "en": "Unknown",
                  "zh": "Unknown"
                },
                {
                  "en": "The request may have been accepted",
                  "zh": "请求可能已被接受"
                },
                {
                  "en": "Retain attempt metadata and reconcile before resubmitting",
                  "zh": "保留本次调用元数据，核对后再考虑重新提交"
                }
              ],
              [
                {
                  "en": "Accepted",
                  "zh": "Accepted"
                },
                {
                  "en": "Provider acceptance is known even if result processing failed",
                  "zh": "即使结果处理失败，仍已确认供应商接受"
                },
                {
                  "en": "Recover the existing result/task where possible",
                  "zh": "尽可能恢复现有结果或任务"
                }
              ]
            ]
          }
        ]
      }
    ]
  }
};

export const llmPages: DocPage[] = referencePages.map((page) => {
  const guide = practicalGuides[page.id];
  if (!guide) return page;
  const sources = [...(page.sources ?? [])];
  for (const path of guide.sources) {
    if (!sources.some((source) => source.repo === 'llm-client' && source.path === path)) {
      sources.push({ repo: 'llm-client', path });
    }
  }
  const sections = page.id === 'llm-client'
    ? [...guide.sections, ...page.sections]
    : [page.sections[0]!, ...guide.sections, ...page.sections.slice(1)];
  return { ...page, sources, sections };
});
