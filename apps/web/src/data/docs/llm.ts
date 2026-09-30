import type { DocPage } from './types';

const source = 'https://github.com/lingxi-coder/llm-client/blob/1a60e73ad12e663be768232a91cbd3ea693aca6e';

export const llmPages: DocPage[] = [
  {
    id: 'llm-client',
    group: 'llm',
    title: { en: 'LLM Client', zh: 'LLM Client' },
    description: {
      en: 'A standalone Rust client for provider protocols, typed requests, streaming, images, embeddings, and model catalogs.',
      zh: '独立的 Rust 模型客户端，统一供应商协议、类型化请求、流式响应、图像、嵌入向量与模型目录。',
    },
    packageName: 'lingxi-llm-client',
    sourceUrl: `${source}/README.en.md`,
    sections: [
      {
        id: 'install',
        title: { en: 'Install from Git', zh: '从 Git 安装' },
        paragraphs: [{
          en: 'The package is lingxi-llm-client 0.3.0 and the Rust import is lingxi_llm_client. Use Rust 1.94 or later. This reference pins the source revision used to document the API; keep the application and runtime on the same revision.',
          zh: 'Cargo 包名是 lingxi-llm-client，当前版本为 0.3.0，Rust 导入名为 lingxi_llm_client。需要 Rust 1.94 或更新版本。这里固定了本文所依据的源码版本；应用与运行时应使用相同版本。',
        }],
        code: [{
          label: 'Cargo.toml · dependency excerpt / 依赖片段',
          language: 'toml',
          code: `[dependencies]
lingxi-llm-client = { git = "https://github.com/lingxi-coder/llm-client", rev = "1a60e73ad12e663be768232a91cbd3ea693aca6e" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }`,
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
          en: 'Build a client from the built-in profiles, choose a region, and supply a valid credential for each request. The openai profile and gpt-4.1-mini model are catalog entries; your provider account still needs access to the model.',
          zh: '从内置连接配置创建客户端，选择区域，并在每次请求中提供有效凭据。openai 连接与 gpt-4.1-mini 模型来自内置目录；供应商账户仍需具有该模型的访问权限。',
        }],
        code: [{
          label: 'src/main.rs · complete example / 完整示例',
          language: 'rust',
          code: `use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, Region, Secret};
use lingxi_llm_client::{builtin_providers, LlmClientBuilder, RequestOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?;
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
          en: 'The example reads OPENAI_API_KEY itself. The client does not discover, refresh, or persist secrets. Use the credential for the exact selected profile and account.',
          zh: '示例自行读取 OPENAI_API_KEY。客户端不会自动查找、刷新或持久化密钥。凭据必须属于实际选择的连接与账户。',
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
    ],
  },
  {
    id: 'llm-requests',
    group: 'llm',
    title: { en: 'Requests & execution', zh: '请求与执行' },
    description: { en: 'Compose typed model input and control each physical attempt with draft, prepare, dispatch, and collect.', zh: '构建类型化模型输入，通过草稿、准备、发送与收集阶段控制每一次实际调用。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: `${source}/src/client/prepared.rs`,
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
          language: 'rust',
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
          language: 'rust',
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
    ],
  },
  {
    id: 'llm-streaming',
    group: 'llm',
    title: { en: 'Streaming & cancellation', zh: '流式响应与取消' },
    description: { en: 'Read normalized events or per-chunk observations while preserving tools, native content, usage, and terminal errors.', zh: '读取标准化事件或逐传输块观测，保留工具、原生内容、用量与终止错误。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: `${source}/src/client/stream.rs`,
    sections: [
      {
        id: 'events',
        title: { en: 'Consume events', zh: '消费事件' },
        paragraphs: [{
          en: 'ModelStream exposes its own asynchronous next(); no StreamExt import is required. Errors can occur when opening the stream or reading it. Read through None, including after End, before treating reports as final.',
          zh: 'ModelStream 自带异步 next()，无需导入 StreamExt。打开流或读取流时均可能失败。应一直读取到 None，包括 End 事件之后，再将报告视为最终结果。',
        }],
        code: [{
          label: 'Streaming function · reusable excerpt, call from an async host / 可复用函数片段',
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
          language: 'rust',
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
          { name: 'ModelStream::collect_structured_json', signature: 'pub async fn collect_structured_json(self, format: &OutputFormat) -> Result<StructuredStreamResult<serde_json::Value>, StructuredStreamError>', description: { en: 'Consume and validate final JSON while retaining the reconstructed response and events.', zh: '消费并校验最终 JSON，同时保留重建的响应与事件。' } },
          { name: 'ModelStream::continuation', signature: 'pub fn continuation(&self) -> Option<&ContinuationRef>', description: { en: 'Read account-bound state only after a complete stream.', zh: '完整读取流之后获取绑定账户的续接状态。' } },
        ],
        note: {
          en: 'PreparedCall WebSocket execution requires a Transport implementing connect_websocket. A failed send never resends the current call. The host owns reconnection, cancellation, and whether to prepare another attempt.',
          zh: 'PreparedCall 的 WebSocket 执行需要实现 connect_websocket 的 Transport。发送失败不会重发当前调用。宿主负责重连、取消以及是否准备下一次调用。',
        },
      },
    ],
  },
  {
    id: 'llm-images',
    group: 'llm',
    title: { en: 'Image API', zh: '图像 API' },
    description: { en: 'Generate, edit, and query native image tasks through a service independent of Chat.', zh: '通过独立于 Chat 的服务生成、编辑图像并查询原生图像任务。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: `${source}/docs/images.en.md`,
    sections: [
      {
        id: 'generate',
        title: { en: 'Generate an image', zh: '生成图像' },
        paragraphs: [{
          en: 'client.images() shares configuration and transport with Chat but uses its own requests, model catalog, credentials options, capabilities, and routes. Image-output models are excluded from chat().models().',
          zh: 'client.images() 与 Chat 共享配置和传输，但具有独立的请求、模型目录、凭据选项、能力与路由。chat().models() 不包含图像输出模型。',
        }],
        code: [{
          label: 'Image function · excerpt, call from an async host / 图像函数片段',
          language: 'rust',
          code: `use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::{
    ImageGenerationRequest, ImageOutputOptions, ImageRequestOptions, Region, Secret,
};

async fn generate(api_key: String) -> Result<(), Box<dyn std::error::Error>> {
    let client = LlmClientBuilder::new(&builtin_providers()?)?
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
    ],
  },
  {
    id: 'llm-providers',
    group: 'llm',
    title: { en: 'Providers, catalogs & pricing', zh: '供应商、目录与计价' },
    description: { en: 'Bind profiles, filter by region, manage catalogs, access native resources, and estimate cost from observed facts.', zh: '绑定连接、按区域过滤、管理目录、访问原生资源，并依据观测事实估算成本。' },
    packageName: 'lingxi-llm-client',
    sourceUrl: `${source}/docs/api.en.md`,
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
          language: 'rust',
          code: `use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::Region;

let profiles = builtin_providers()?;
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
          en: 'Native resources keep their own types and support boundaries: files, retrieval, batches, background jobs, remote tools, speech, and realtime sessions vary by provider. Use the pinned source API guide for each provider resource. Realtime audio device capture and playback remain in the host.',
          zh: '原生资源保留独立类型与支持边界：文件、检索、批处理、后台任务、远程工具、语音和实时会话因供应商而异。各供应商资源的细节见固定版本源码 API 指南。实时音频设备采集与播放仍由宿主负责。',
        }],
        apis: [
          { name: 'LlmClient::provider', signature: 'pub fn provider<T: ProviderClient>(&self, profile_name: &str) -> Result<T, ProviderBindingError>', description: { en: 'Reject a mismatched provider/profile binding rather than inferring another route.', zh: '拒绝不匹配的供应商与连接绑定，不会推测其他路由。' } },
          { name: 'EmbeddingService::embed', signature: 'pub async fn embed(self, profile_name: &str, input: &EmbeddingRequest, options: &RequestOptions) -> Result<EmbeddingResponse, EmbeddingError>', description: { en: 'Embed text using an enabled independent route. Model limits, dimensions and task support are validated without automatic splitting.', zh: '通过启用的独立路由嵌入文本。校验模型限制、维度与任务支持，不会自动拆分。' } },
        ],
        code: [{
          label: 'Provider binding · excerpt with an existing client / 供应商绑定片段',
          language: 'rust',
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
    ],
  },
];
