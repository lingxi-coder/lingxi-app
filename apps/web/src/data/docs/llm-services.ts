import type { DocPage, DocSection, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const sources = (...paths: string[]) => paths.map(path => ({ repo: 'llm-client' as const, path }));
const currentSource = 'https://github.com/lingxi-coder/llm-client';

const referencePages: DocPage[] = [
  {
    id: 'llm-files', group: 'llm', packageName: 'lingxi-llm-client', sourceUrl: currentSource,
    title: t('Attachments & provider files', '附件与供应商文件'),
    description: t('Keep durable host attachments separate from scoped provider uploads, model input and generated artifacts.', '区分宿主持久附件、账户限定的供应商上传文件、模型输入与生成产物。'),
    sources: sources('src/client/mod.rs', 'src/client/builder.rs', 'src/client/attachments/resolve.rs', 'src/client/attachments/prepare.rs', 'src/client/attachments/cache.rs', 'src/files/service.rs', 'src/files/policy.rs', 'src/files/types.rs', 'src/providers/file_resources.rs', 'tests/provider_files.rs', 'tests/file_stream_upload.rs', 'tests/file_deadlines.rs'),
    sections: [
      {
        id: 'identities', title: t('Two identities, separate lifetimes', '两类身份与独立生命周期'),
        paragraphs: [t('AttachmentRef is an immutable host-store reference: attachment_id, revision, filename, media_type and declared size. Keep it in conversation history and use the host attachment service for authenticated display on another device. Provider file IDs and URIs are request-time provider resources, with account, endpoint, protocol, purpose and expiry constraints; they cannot replace the host attachment record.', 'AttachmentRef 是宿主存储中的不可变引用，包含 attachment_id、revision、filename、media_type 与声明大小。对话历史保留它，其他设备通过宿主附件服务进行认证后显示。供应商文件 ID 与 URI 是请求时资源，受账户、端点、协议、用途与过期约束，不能替代宿主附件记录。')],
        tables: [{ headers: [t('Object', '对象'), t('Owner and use', '归属与用途'), t('Failure boundary', '失败边界')], rows: [
          [t('AttachmentRef', 'AttachmentRef'), t('Host: durable original and immutable revision', '宿主：持久原件与不可变修订'), t('Missing revision or access denial is returned by the resolver', '修订缺失或权限拒绝由 resolver 返回')],
          [t('ProviderFileRef / ProviderFileSource', 'ProviderFileRef / ProviderFileSource'), t('SDK-scoped provider resource and codec input', 'SDK 限定作用域的供应商资源与 codec 输入'), t('Wrong account, endpoint, purpose or expired file fails validation', '账户、端点、用途不符或文件过期会校验失败')],
          [t('ProviderFileContent', 'ProviderFileContent'), t('Original bytes where a download operation is supported', '支持下载操作时的原始字节'), t('Download support is independent of upload or text extraction', '下载支持独立于上传或文本提取')],
        ] }],
      },
      {
        id: 'resolve', title: t('Resolve host attachments', '解析宿主附件'),
        steps: [t('Store original bytes under an immutable host ID/revision and validate the caller’s authority in AttachmentResolver.', '以不可变宿主 ID/修订存储原始字节，并在 AttachmentResolver 中校验调用者权限。'), t('Configure LlmClientBuilder::with_attachment_resolver. Add ImageSource::Attachment, DocumentSource::Attachment or VideoSource::Attachment to a message.', '配置 LlmClientBuilder::with_attachment_resolver，在消息中加入 ImageSource::Attachment、DocumentSource::Attachment 或 VideoSource::Attachment。'), t('The SDK checks all declared metadata and the 64 MiB combined request bound before resolver I/O, resolves independent revisions concurrently, checks exact returned size, then plans inline or native upload input for the selected profile/model.', 'SDK 在 resolver I/O 前检查全部声明元数据与每次请求合计 64 MiB 上限，并发解析独立修订，检查返回字节数完全一致，再按所选连接/模型规划内联或原生上传输入。'), t('The original ChatRequest remains unchanged. Persist only host references; keep request-local prepared bytes and provider references out of the durable attachment identity.', '原始 ChatRequest 保持不变。持久化宿主引用，请求级准备字节与供应商引用不作为持久附件身份。')],
        code: [{ label: t('Build a client with an existing authorized host resolver', '使用现有已授权宿主 resolver 创建客户端'), language: 'rust', verification: 'compile', exampleId: 'llm-attachment-client', code: `use std::sync::Arc;
use lingxi_llm_client::{builtin_providers, AttachmentResolver, LlmClient, LlmClientBuilder};
use lingxi_llm_client::protocol::{AttachmentRef, ChatRequest, ContentBlock, ConversationMessage, ImageSource, Region};

pub fn attachment_client(
    resolver: Arc<dyn AttachmentResolver>,
    attachment: AttachmentRef,
) -> Result<(LlmClient, ChatRequest), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let mut builder = LlmClientBuilder::new(&profiles)?.with_region(Region::International);
    builder.with_attachment_resolver(resolver);
    let client = builder.build()?;
    let mut message = ConversationMessage::user_text("Describe the attached image.");
    message.content.push(ContentBlock::Image {
        source: ImageSource::Attachment { attachment },
    });
    let mut request = ChatRequest::new("gpt-4.1-mini");
    request.messages.push(message);
    Ok((client, request))
}` }],
        note: t('The host resolver must read the exact authorized revision. validate_content_reuse() is opt-in; the SDK calls it before cached bytes or a shared read are used. Its default disables cross-call byte reuse, so resolve() remains the per-request authority check.', '宿主 resolver 必须读取精确且已授权的修订。validate_content_reuse() 需要显式启用；SDK 在使用缓存字节或共享读取前调用它。默认禁止跨请求字节复用，resolve() 因而仍承担每次请求的权限检查。'),
      },
      {
        id: 'provider-management', title: t('Manage provider files explicitly', '显式管理供应商文件'),
        paragraphs: [t('Bind a typed provider client, then use its files() resource. Each operation captures one configuration snapshot and requires a nonempty file_account_scope (or account_scope). The low-level FileService is useful when the host already owns a Transport, profile and Authenticator. FileCapabilities is keyed by provider identity, actual endpoint/protocol, media type, purpose and model; an OpenAI-compatible chat endpoint does not imply a Files API.', '先绑定类型化供应商客户端，再使用 files() 资源。每次操作捕获一个配置快照，并要求非空 file_account_scope（或 account_scope）。宿主已有 Transport、profile 和 Authenticator 时，可使用底层 FileService。FileCapabilities 按供应商身份、实际端点/协议、媒体类型、用途与模型判断；兼容 OpenAI Chat 的端点不代表有 Files API。')],
        code: [{ label: t('Upload one host-owned file and retain its scope', '上传一个宿主持有的文件并保留作用域'), language: 'rust', verification: 'compile', exampleId: 'llm-provider-upload', code: `use lingxi_llm_client::{FilePurpose, LlmClient, ProviderFileRef, RequestOptions, UploadFile};
use lingxi_llm_client::providers::OpenAiClient;

pub async fn upload_model_file(
    client: &LlmClient,
    file: &UploadFile,
    options: &RequestOptions,
) -> Result<ProviderFileRef, Box<dyn std::error::Error>> {
    // options must contain this account's credential and nonempty file_account_scope.
    let provider = client.provider::<OpenAiClient>("openai")?;
    let files = provider.files();
    let reference = files.upload(file, FilePurpose::ModelInput, options).await?;
    let metadata = files.get(&reference, options).await?;
    Ok(metadata.file)
}` }],
        bullets: [t('upload_stream consumes an UploadFileStream once. The declared size must equal exact EOF; unsupported routes are rejected before the input is polled. Custom transports must implement send_stream for incremental upload.', 'upload_stream 只消费 UploadFileStream 一次。声明大小必须与精确 EOF 一致；不支持的路由在读取输入前拒绝。自定义传输需实现 send_stream 才能增量上传。'), t('Download, metadata, list, list_for_purpose, delete and extract_text have separate capability checks. Keep pagination cursors opaque. Foundry Files requires explicit Anthropic hosting and a canonical resource/account scope.', '下载、元数据、列表、list_for_purpose、删除与 extract_text 分别检查能力。分页游标保持不透明。Foundry Files 需要显式 Anthropic hosting 与规范资源/账户作用域。')],
      },
      {
        id: 'retention-recovery', title: t('Retention, uncertainty & recovery', '留存、不确定结果与恢复'),
        bullets: [t('A stable file_account_scope allows account-isolated reuse of eligible automatic uploads. Without it, uploads are attempt-scoped. Qwen automatic uploads remain request-scoped and use explicit cleanup because they do not expire server-side.', '稳定 file_account_scope 允许符合条件的自动上传按账户隔离复用。缺少时上传限定于单次尝试。Qwen 自动上传仍限定于请求，并显式清理，因为服务器文件不会自动过期。'), t('Eligible OpenAI, Anthropic and xAI automatic uploads use a 24-hour expiry and stop cache reuse after 23 hours. A file-identifying 404 can cause one fresh automatic upload only where replay is admitted; hosted side effects and continuations restrict replay.', '符合条件的 OpenAI、Anthropic 与 xAI 自动上传采用 24 小时过期，23 小时后停止缓存复用。能定位到该文件的 404 仅在允许重放时触发一次重新自动上传；托管副作用与续接会限制重放。'), t('ProviderFileProcessing carries a scoped reference when Gemini processing remains pending or readiness cannot be established. Retain it and call resume_gemini_processing; do not create another upload just to restart polling.', 'Gemini 处理仍未完成或无法确认就绪时，ProviderFileProcessing 携带作用域限定的引用。保留并调用 resume_gemini_processing，不要为了重新轮询而再次上传。'), t('FileUploadError::OutcomeUnknown means provider acceptance is uncertain after dispatch. Reconcile through metadata/listing where available. with_timeout gives each low-level file operation a total budget; with_deadline shares an absolute deadline across calls. Neither adds retries.', 'FileUploadError::OutcomeUnknown 表示发送后无法确定供应商是否接受。可用时通过元数据/列表核对。with_timeout 为每个底层文件操作设置总预算；with_deadline 在多次调用间共享绝对截止时间。两者都不增加重试。'), t('Explicit uploads are host-managed. Automatic Qwen deletion is attempted after response completion and retried in the background on interruption or deletion failure; process exit can leave orphan files, so the host needs periodic reconciliation.', '显式上传由宿主管理。自动 Qwen 文件在响应完成后尝试删除，中断或删除失败会后台重试；进程退出仍可能遗留孤立文件，宿主应定期核对。')],
        note: t('Implementation evidence: provider_files, provider_file_clients, file_stream_upload and file_deadlines suites cover scope, byte limits and uncertain outcomes with mock transport. Real account retention, upload entitlements and cross-device attachment storage require separate acceptance.', '实现证据：provider_files、provider_file_clients、file_stream_upload 与 file_deadlines 测试使用模拟传输覆盖作用域、字节上限与不确定结果。真实账户留存、上传权限与跨设备附件存储需单独验收。'),
      },
    ],
  },
  {
    id: 'llm-audio', group: 'llm', packageName: 'lingxi-llm-client', sourceUrl: currentSource,
    title: t('Cloud transcription & speech', '云端转写与语音合成'),
    description: t('Use exact-profile audio independently of chat, with explicit formats, bounded streams and dispatch-aware errors.', '独立于 Chat 使用精确连接的音频服务，明确格式、限制流大小并按发送状态处理错误。'),
    sources: sources('src/audio/mod.rs', 'src/audio/types.rs', 'src/audio/catalog.rs', 'src/providers/openai/audio/mod.rs', 'src/providers/qwen/tts/mod.rs', 'src/hosting/vertex/speech/mod.rs', 'tests/unified_audio.rs', 'tests/audio.rs'),
    sections: [
      {
        id: 'audio-route', title: t('Exact audio route and host ownership', '精确音频路由与宿主职责'),
        paragraphs: [t('LlmClient::audio() and ClientSnapshot::audio() provide common file transcription and complete-text synthesis. AudioRoute names an exact profile and stable nonsecret account_scope. It does not resolve a profile group, infer an audio model from a chat model, use fallback_credentials, retry or fail over. The host decides whether the route follows a session or is an explicit override.', 'LlmClient::audio() 与 ClientSnapshot::audio() 提供统一的文件转写与完整文本合成。AudioRoute 指定精确连接与稳定、非敏感的 account_scope。它不会解析连接组、从 Chat 模型推断音频模型、使用 fallback_credentials、重试或回退。由宿主决定路由跟随会话还是采用显式覆盖。'), t('The facade checks independent AudioCapabilities. FileTranscription, LiveAsr, Synthesis, IncrementalSynthesis and NativeRealtime are distinct operations. Streaming audio output for complete text remains Synthesis; it is not incremental text input. supports() requires an available common adapter and declared model; operation() also describes provider-only entries.', '入口检查独立 AudioCapabilities。FileTranscription、LiveAsr、Synthesis、IncrementalSynthesis 与 NativeRealtime 是不同操作。完整文本产生流式音频仍是 Synthesis，不是增量文本输入。supports() 要求可用统一适配器与声明模型；operation() 还描述供应商专有入口。')],
        apis: [
          { name: 'AudioService::capabilities', signature: 'pub fn capabilities(&self, route: &AudioRoute) -> Result<AudioCapabilities, AudioError>', description: t('Inspect route-specific operation/model/format/voice contracts without cloud dispatch.', '不发起云端调用，查询该路由的操作、模型、格式与声音约定。') },
          { name: 'AudioService::transcribe', signature: 'pub async fn transcribe(\n    &self, route: &AudioRoute, input: AudioInput,\n    request: &TranscriptionRequest, options: &RequestOptions,\n) -> Result<TranscriptionResult, AudioError>', description: t('Dispatch one admitted file-transcription operation; preserve text, timing, request provenance and optional usage.', '发送一次准入后的文件转写，保留文本、时间、请求来源与可选用量。') },
          { name: 'AudioService::synthesize', signature: 'pub async fn synthesize(\n    &self, route: &AudioRoute, request: &SynthesisRequest,\n    options: &RequestOptions,\n) -> Result<AudioOutput, AudioError>', description: t('Return provider audio without capture, playback, conversion or resampling.', '返回供应商音频，不负责采集、播放、转码或重采样。') },
        ],
      },
      {
        id: 'audio-coverage', title: t('Implemented common and native adapters', '已实现的统一与原生适配器'),
        tables: [{ headers: [t('Profile provider', '连接供应商'), t('Common adapter', '统一适配器'), t('Conditions and native boundaries', '条件与原生边界')], rows: [
          [t('OpenAI', 'OpenAI'), t('File transcription; synthesis', '文件转写；合成'), t('Explicit native routes; native voices/translation/realtime remain separate', '显式原生路由；原生声音、翻译与实时会话独立')],
          [t('xAI', 'xAI'), t('File transcription; synthesis', '文件转写；合成'), t('TTS has no model field and requires selected voice; native live ASR/incremental TTS/realtime', 'TTS 没有模型字段且需要选定声音；原生实时 ASR、增量 TTS 与实时会话')],
          [t('Gemini Developer API / Vertex', 'Gemini Developer API / Vertex'), t('Synthesis', '合成'), t('Separate models/auth; Vertex requires selected project/location; no dedicated common file ASR', '模型与认证独立；Vertex 需要所选 project/location；无独立统一文件 ASR')],
          [t('MiniMax', 'MiniMax'), t('File transcription; synthesis', '文件转写；合成'), t('Discover/select voice; native streaming/incremental/async TTS', '发现并选定声音；原生流式、增量与异步 TTS')],
          [t('Qwen', 'Qwen'), t('Synthesis', '合成'), t('Profile extra.workspace_id; async URL ASR remains typed native service', '连接 extra.workspace_id；异步 URL ASR 保留类型化原生入口')],
          [t('GLM / Z.AI', 'GLM / Z.AI'), t('Supported hosted file transcription', '受支持的托管文件转写'), t('Mainland hosted TTS is provider-only; international hosted TTS is unsupported in this catalog', '大陆托管 TTS 为供应商专有入口；当前目录不支持国际托管 TTS')],
          [t('OpenRouter', 'OpenRouter'), t('Discovered transcription/speech models', '已发现的 transcription/speech 模型'), t('No guessed model/voice/PCM geometry', '不猜测模型、声音或 PCM 参数')],
        ] }],
        note: t('This table describes SDK admission and adapter implementation. Endpoint region and account entitlements remain separate facts. Unsupported operation/model/format or known size limits fail before input reads and transport.', '本表描述 SDK 准入与适配器实现。端点区域与账户授权是独立事实。不支持的操作、模型、格式或已知大小限制会在读取输入与传输前拒绝。'),
      },
      {
        id: 'audio-example', title: t('Synthesize with a byte bound', '带字节上限的语音合成'),
        paragraphs: [t('RequestOptions supplies the exact account credential and optional total_timeout. A request-local account_scope must match AudioRoute. The common facade rejects authenticator/finalizer hooks that native services do not implement, and restricted ChatGPT/Copilot credentials do not grant cloud audio.', 'RequestOptions 提供该精确账户的凭据与可选 total_timeout。请求级 account_scope 必须匹配 AudioRoute。统一入口拒绝原生服务未实现的 authenticator/finalizer 钩子，受限 ChatGPT/Copilot 凭据不会授予云端音频能力。')],
        code: [{ label: t('Collect bounded provider audio for host playback', '收集限定大小的供应商音频供宿主播放'), language: 'rust', verification: 'compile', exampleId: 'llm-speech', code: `use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::audio::{AudioOperation, AudioOutput, AudioRoute, SynthesisRequest};

pub async fn synthesize(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let route = AudioRoute::new("openai", "host-account-1");
    client.audio().capabilities(&route)?.model(AudioOperation::Synthesis, None)?;
    let output = client.audio().synthesize(
        &route, &SynthesisRequest::new("Hello, LingXi."), options,
    ).await?;
    match output {
        AudioOutput::Stream(stream) => {
            let audio = stream.collect(16 * 1024 * 1024).await?;
            println!("{} bytes; {:?}", audio.bytes.len(), audio.metadata);
        }
        AudioOutput::Url(audio) => {
            println!("Provider-owned URL; {:?}", audio.metadata);
        }
    }
    Ok(())
}` }],
        note: t('This function inspects returned bytes; it does not play sound. Interpret audio.metadata before device playback. Headerless PCM is not a WAV file; absent geometry is unknown, and provider URLs are not fetched automatically.', '本函数检查返回字节，不播放声音。设备播放前解析 audio.metadata。无头 PCM 不是 WAV；缺失参数表示未知，供应商 URL 不会自动下载。'),
      },
      {
        id: 'audio-errors', title: t('Streaming, deadlines & accounting', '流、截止时间与结算'),
        bullets: [t('AudioStream is pull-driven and preserves encoded provider bytes. collect(max_bytes) requires a positive bound; overflow reports MediaTooLarge with dispatch Accepted. Drop releases the underlying response.', 'AudioStream 由调用者拉取，保留供应商编码字节。collect(max_bytes) 要求正数上限；超限返回 MediaTooLarge 且 dispatch 为 Accepted。丢弃会释放底层响应。'), t('AudioError keeps kind, dispatch and the original typed provider source. Dispatch distinguishes NotSent, Rejected, Unknown and Accepted. A timeout conservatively marks uncertain acceptance and never authorizes automatic replay.', 'AudioError 保留 kind、dispatch 与原始类型化供应商错误。Dispatch 区分 NotSent、Rejected、Unknown 与 Accepted。超时保守标记接受状态不确定，不能据此自动重放。'), t('AudioUsage retains optional token, duration, character and provider cost observations; missing values remain absent. Qwen terminal usage is available only after validated completion. Do not route audio usage through Chat token pricing by assumption.', 'AudioUsage 保留可选 token、时长、字符与供应商成本观测；缺失值仍保持缺失。Qwen 终止用量仅在完成校验后可用。不能假设音频用量适用 Chat token 计价。')],
        note: t('unified_audio mock tests cover exact routes, size admission, one dispatch, formats and terminal usage. Live credentials, voices, device permissions, resampling and physical playback are independent acceptance tasks.', 'unified_audio 模拟测试覆盖精确路由、大小准入、单次发送、格式与终止用量。真实凭据、声音、设备权限、重采样与实体播放需独立验收。'),
      },
    ],
  },
  {
    id: 'llm-realtime', group: 'llm', packageName: 'lingxi-llm-client', sourceUrl: currentSource,
    title: t('Realtime sessions & backpressure', '实时会话与背压'),
    description: t('Run bounded bidirectional provider sessions on the host executor and preserve tool, audio and interruption lifecycles.', '在宿主执行器运行有界双向供应商会话，保留工具、音频与中断生命周期。'),
    sources: sources('src/realtime/mod.rs', 'src/realtime/audio.rs', 'src/realtime/normalize.rs', 'src/realtime/websocket.rs', 'src/providers/openai/realtime.rs', 'src/providers/google/live.rs', 'tests/realtime.rs', 'tests/audio_realtime_contract.rs', 'tests/provider_realtime_clients.rs'),
    sections: [
      {
        id: 'realtime-ownership', title: t('Transport, codec and executor boundaries', '传输、codec 与执行器边界'),
        paragraphs: [t('RealtimeTransport provides a split connection; RealtimeCodec maps host inputs and provider frames; RealtimeSession returns control/events plus a RealtimeDriver. The host must run driver.run() concurrently, pace microphone/video capture, handle playback and permissions, and persist conversation state. The default feature set has no built-in WebSocket transport; realtime-websocket enables the Rustls/Tokio transport, while injected transports remain available without it.', 'RealtimeTransport 提供拆分连接；RealtimeCodec 映射宿主输入与供应商帧；RealtimeSession 返回 control/events 与 RealtimeDriver。宿主必须并发运行 driver.run()，控制麦克风/视频采集节奏，处理播放与权限，并持久化对话状态。默认 feature 不带内置 WebSocket 传输；realtime-websocket 启用 Rustls/Tokio 传输，未启用时仍可注入传输。')],
        tables: [{ headers: [t('Component', '组件'), t('Contract', '约定')], rows: [
          [t('RealtimeTransport', 'RealtimeTransport'), t('TLS/auth/frame transport; no implicit reconnect/retry; promptly release both halves on drop', 'TLS、认证与帧传输；不隐式重连/重试；丢弃两半连接时及时释放')],
          [t('RealtimeControl', 'RealtimeControl'), t('Bounded nonblocking send; synchronous QueueFull; explicit interrupt/continue/close/abort', '有界非阻塞发送；同步 QueueFull；显式中断、继续、关闭与 abort')],
          [t('RealtimeEvents / RealtimeDriver', 'RealtimeEvents / RealtimeDriver'), t('The host drains events while driver owns network progress and lifecycle events', '宿主消费事件，driver 推进网络与生命周期事件')],
        ] }],
      },
      {
        id: 'agent-conversation', title: t('Native Agent audio connector', '原生 Agent 音频连接器'),
        paragraphs: [t('connect_audio_conversation uses a fixed ClientSnapshot, exact AudioRoute, declared audio model, credentials, local function tools and host history. It constructs provider configuration internally. The current common Agent connector selects admitted OpenAI realtime or Gemini Live routes; an arbitrary audio/realtime adapter does not acquire Agent support.', 'connect_audio_conversation 使用固定 ClientSnapshot、精确 AudioRoute、声明音频模型、凭据、本地函数工具与宿主历史，内部构建供应商配置。当前统一 Agent 连接器选择已准入的 OpenAI realtime 或 Gemini Live 路由；任意音频/实时适配器不会自动获得 Agent 支持。')],
        code: [{ label: t('Connect with an injected transport; return handles to the host', '使用注入传输连接并将句柄返回宿主'), language: 'rust', verification: 'compile', exampleId: 'llm-realtime-connect', code: `use std::sync::Arc;
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
}` }],
        note: t('A connector returning successfully is not a microphone or speaker test. Keep the returned formats and configuration-specific capabilities; do not infer history/tools/transcription/interruption support from an endpoint name.', '连接器返回成功不等于麦克风或扬声器测试。保留返回格式与配置级能力，不根据端点名称推断历史、工具、转写或中断支持。'),
      },
      {
        id: 'realtime-flow', title: t('Bounded queues and tool rounds', '有界队列与工具轮次'),
        steps: [t('Validate initial frames and queue limits before opening the provider connection; initialization frames are sent before handles become visible.', '打开供应商连接前校验初始化帧与队列限制；句柄对外可见前发送初始化帧。'), t('Run the driver and event consumer concurrently. Defaults are 8 outbound entries, 32 event entries, and 1 MiB per provider frame. Split host audio into small chunks before sending.', '并发运行 driver 与事件消费者。默认发送队列 8 项、事件队列 32 项、每供应商帧 1 MiB。发送前将宿主音频拆为小块。'), t('On QueueFull, apply host pacing rather than unbounded buffering. Event backpressure is real; a stalled UI must not prevent the host from settling a tool or aborting the connection.', '遇到 QueueFull 时由宿主限速，避免无限缓存。事件背压真实存在；UI 阻塞不能阻止宿主结算工具或 abort 连接。'), t('Route ToolCall through the existing permission/sandbox path. Keep call_id unchanged; return ToolResult or a bounded atomic ToolResults batch, then ContinueResponse where required. ToolCancelled invalidates affected host work.', 'ToolCall 进入现有权限/沙箱路径。保持 call_id 不变，返回 ToolResult 或有界原子 ToolResults 批次，再按需 ContinueResponse。ToolCancelled 使受影响宿主任务失效。')],
        bullets: [t('RealtimeCapabilities separates history import, local tools, input/output transcription, usage, interruption, audio truncation and session resumption. agent_conversation() requires its defined combination, not merely audio frames.', 'RealtimeCapabilities 区分历史导入、本地工具、输入/输出转写、用量、中断、音频截断与会话恢复。agent_conversation() 要求其定义的组合，而非仅有音频帧。'), t('Transcript events declare input/output direction, Delta versus Replace, item/turn IDs and final_chunk. Do not append an interim replacement as another delta. Retain ProviderEvent for capabilities outside the normalized vocabulary.', 'Transcript 事件声明输入/输出方向、Delta 与 Replace、item/turn ID 与 final_chunk。不能将临时替换快照当作增量追加。规范词汇未覆盖的能力保留 ProviderEvent。')],
      },
      {
        id: 'realtime-shutdown', title: t('Interruption, shutdown & recovery', '中断、关闭与恢复'),
        bullets: [t('Interrupt is a provider command; it does not close the connection. Stop host playback promptly and synchronize audio truncation to the played offset where supported. ClearAudio discards uncommitted input where the protocol supports it.', 'Interrupt 是供应商命令，不会关闭连接。宿主应及时停止播放，协议支持时将音频截断同步到已播放位置。ClearAudio 在受支持协议中丢弃未提交输入。'), t('close() queues a graceful shutdown after accepted work. abort() independently interrupts pending writes and event backpressure, discards queued work and releases the connection without draining it. Keep the driver running until its acknowledgement arrives.', 'close() 在已接受工作后排队优雅关闭。abort() 独立打断待发送写入与事件背压，丢弃排队工作并直接释放连接。收到确认前需保持 driver 运行。'), t('ProviderError and ConnectionInterrupted are different normalized events. Neither silently reconnects. Session resumption is explicit capability/state, and host history import does not request a new response automatically.', 'ProviderError 与 ConnectionInterrupted 是不同标准化事件，均不会静默重连。会话恢复需要显式能力与状态；导入宿主历史不会自动请求新响应。'), t('Realtime Usage is associated with turn_id where provided and can omit fields. Persist observations with the exact route/account and keep unknown acceptance/cost explicit after cancellation.', 'Realtime Usage 在供应商提供时关联 turn_id，字段可缺失。按精确路由/账户持久化观测，取消后明确保留未知接受状态与成本。')],
        note: t('realtime and audio_realtime_contract suites use mock connections to validate queue bounds, abort under backpressure, tools and lifecycle normalization. Live network resilience, audio quality and device permission behavior need separate acceptance.', 'realtime 与 audio_realtime_contract 测试使用模拟连接验证队列上限、背压下 abort、工具与生命周期标准化。真实网络韧性、音频质量与设备权限行为需单独验收。'),
      },
    ],
  },
  {
    id: 'llm-native-services', group: 'llm', packageName: 'lingxi-llm-client', sourceUrl: currentSource,
    title: t('Provider resources & hosted tools', '供应商资源与托管工具'),
    description: t('Bind native resources explicitly and separate provider-side execution from Harness tools and background work.', '显式绑定原生资源，区分供应商端执行、Harness 工具与后台工作。'),
    sources: sources('src/providers/binding.rs', 'src/providers/openai/client.rs', 'src/providers/google/client.rs', 'src/providers/anthropic/client.rs', 'src/providers/qwen/client.rs', 'src/providers/xai/client.rs', 'src/providers/minimax/client.rs', 'src/providers/zhipu/client.rs', 'src/providers/openrouter/client.rs', 'src/providers/kimi/client.rs', 'src/providers/deepseek/client.rs', 'src/providers/github_copilot/client.rs', 'src/providers/file_resources.rs', 'src/providers/account_resources.rs', 'src/providers/openai/background/mod.rs', 'src/providers/dispatch.rs', 'src/execution_safety.rs', 'src/hosted_search.rs', 'tests/code_interpreter.rs', 'tests/provider_clients.rs'),
    sections: [
      {
        id: 'native-binding', title: t('Bind one provider and exact connection', '绑定供应商与精确连接'),
        paragraphs: [t('client.provider::<T>(profile) validates provider identity and profile scope. Live provider handles capture a fresh immutable configuration at each operation; ClientSnapshot-derived handles use that retained revision. A resource reference records the account/endpoint/profile needed for reuse. Configuration replacement can invalidate a binding instead of rerouting it to another account.', 'client.provider::<T>(profile) 校验供应商身份与连接作用域。实时客户端的供应商句柄每次操作捕获新的不可变配置；ClientSnapshot 派生句柄使用保留的修订。资源引用记录复用所需的账户、端点与连接。配置替换可能使绑定失效，而不会将其路由到其他账户。')],
        tables: [{ headers: [t('Typed client', '类型化客户端'), t('Implemented resource families', '已实现资源族')], rows: [
          [t('OpenAiClient', 'OpenAiClient'), t('files, embeddings, images, retrieval, batches, background, containers, audio, realtime/live', '文件、嵌入、图像、检索、批处理、后台响应、容器、音频、实时/Live')],
          [t('AnthropicClient', 'AnthropicClient'), t('files, batch, provider Skills, Messages-native hosted tools and hosted wrappers', '文件、批处理、供应商 Skills、Messages 原生托管工具与云托管协议')],
          [t('GoogleClient', 'GoogleClient'), t('files, embeddings, file_search, batch, context_cache, interactions, speech/voices, Vertex speech, Live', '文件、嵌入、file_search、批处理、上下文缓存、Interactions、语音/声音、Vertex 语音与 Live')],
          [t('QwenClient', 'QwenClient'), t('files, embeddings, images, batch, rerank, knowledge, ASR/TTS/AudioGen and typed realtime services', '文件、嵌入、图像、批处理、重排、知识库、ASR/TTS/AudioGen 与类型化实时服务')],
          [t('XaiClient', 'XaiClient'), t('files(), collections(config), batch(scope), audio(config), deferred(), images(); native realtime connectors', 'files()、collections(config)、batch(scope)、audio(config)、deferred()、images()；原生实时连接器')],
          [t('OpenRouterClient', 'OpenRouterClient'), t('files(), embeddings(), images(), audio(), batch(scope), rerank(scope); typed hosted server tools', 'files()、embeddings()、images()、audio()、batch(scope)、rerank(scope)；类型化托管服务器工具')],
          [t('MiniMaxClient', 'MiniMaxClient'), t('files(), images(), audio(endpoint), tts(config), async_tts(config), voices(config); incremental speech connectors', 'files()、images()、audio(endpoint)、tts(config)、async_tts(config)、voices(config)；增量语音连接器')],
          [t('ZhipuClient', 'ZhipuClient'), t('files(), embeddings(), images(), batch(scope), knowledge(), async_tasks(config), cloud_audio(scope), asr(route, scope); native realtime', 'files()、embeddings()、images()、batch(scope)、knowledge()、async_tasks(config)、cloud_audio(scope)、asr(route, scope)；原生实时入口')],
          [t('KimiClient', 'KimiClient'), t('files(), batch(scope), chat(); file extraction remains distinct from general model-file input', 'files()、batch(scope)、chat()；文件提取独立于通用模型文件输入')],
          [t('DeepSeekClient / GithubCopilotClient', 'DeepSeekClient / GithubCopilotClient'), t('Bound chat and account resources; no inferred file/image/speech resource from chat compatibility', '绑定 Chat 与账户资源；不因 Chat 兼容而推断文件/图像/语音资源')],
        ] }],
        note: t('Resource families describe code entry points, not universal upstream support. Check the operation’s typed scope, concrete route and authentication restrictions. Use Rustdoc for complete method signatures and provider payload types.', '资源族描述代码入口，不代表上游普遍支持。检查操作的类型化作用域、具体路由与认证限制。完整方法签名与供应商载荷类型见 Rustdoc。'),
      },
      {
        id: 'tools-boundaries', title: t('Local functions and provider-side tools', '本地函数与供应商端工具'),
        paragraphs: [t('ChatRequest.tools declares functions the host executes. ToolUse events carry data back to Harness; permissions, sandboxing, tool-result persistence and interaction prompts belong to Harness. ChatRequest.hosted_tools declares work the provider executes. Native hosted output survives in ProviderContent and native metadata; it must not be dispatched as a local function call.', 'ChatRequest.tools 声明宿主执行的函数。ToolUse 事件将数据交给 Harness；权限、沙箱、工具结果持久化与交互提问属于 Harness。ChatRequest.hosted_tools 声明供应商执行的工作。原生托管输出保留在 ProviderContent 与原生元数据中，不得当作本地函数调用执行。')],
        bullets: [t('Web search uses hosted provider search only. chat().web_search_in adds WebSearchConfig and preserves citations/metadata; unsupported profile/model/search options fail explicitly. There is no internal search backend fallback.', '网页搜索仅使用供应商托管搜索。chat().web_search_in 添加 WebSearchConfig 并保留引用/元数据；不支持的连接、模型或搜索选项显式报错。没有内部搜索后端回退。'), t('File search, code execution, shell, computer use, tool search, URL/Maps grounding, remote MCP and provider Skills are route-specific typed extensions. A protocol label alone does not admit them. Anthropic provider Skills differ from Harness local SKILL.md discovery.', '文件检索、代码执行、shell、computer use、工具检索、URL/Maps grounding、远程 MCP 与供应商 Skills 是路由特定的类型化扩展。协议标签本身不足以准入。Anthropic 供应商 Skills 与 Harness 本地 SKILL.md 发现不同。'), t('Hosted remote MCP authorizations are fresh secrets in RequestOptions.mcp_authorizations, indexed by declared server label/name. They are not serialized into ChatRequest or history. Provider approval output can require host interaction and explicit continuation.', '托管远程 MCP 授权是 RequestOptions.mcp_authorizations 中的新鲜凭据，按声明的 server label/name 索引，不序列化到 ChatRequest 或历史。供应商审批输出可能要求宿主交互与显式续接。'), t('Execution-safety admission pins side-effecting hosted requests and suppresses unsafe automatic resends/failover. Token estimates exclude provider tool/container charges. Preserve both usage and server-tool observations for reconciliation.', '执行安全准入固定有副作用的托管请求，抑制不安全的自动重发/回退。Token 估算不包含供应商工具/容器费用。核对时同时保留用量与服务器工具观测。')],
      },
      {
        id: 'native-jobs', title: t('Native jobs and durable polling', '原生任务与持久轮询'),
        code: [{ label: t('Submit one background response and return a durable reference', '提交一次后台响应并返回持久引用'), language: 'rust', verification: 'compile', exampleId: 'llm-background-submit', code: `use lingxi_llm_client::{LlmClient, RequestOptions};
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
}` }],
        paragraphs: [t('Persist the returned native reference in the host, then call get once per chosen polling interval with the same account. OpenAI background has distinct submit/get/cancel/delete and resumable event APIs; delete removes a stored response and does not cancel inference. SubmitOutcomeUnknown and CancelOutcomeUnknown are reconciliation states, not invitations to resubmit.', '由宿主持久化原生引用，之后按选定轮询间隔使用同一账户调用一次 get。OpenAI background 分开提供 submit/get/cancel/delete 与可恢复事件 API；delete 删除已存储响应，不会取消推理。SubmitOutcomeUnknown 与 CancelOutcomeUnknown 是需核对的状态，不能据此重新提交。'), t('Provider Batch services accept their own typed request records or JSONL files; output identities/statuses/usage stay provider-native. xAI deferred, GLM async, MiniMax async TTS and image tasks retain their own reference types and terminal states. These are cloud jobs, distinct from Harness task execution, Cron and Workflow scheduling.', '供应商 Batch 服务接受各自类型化请求记录或 JSONL 文件；输出身份、状态与用量保持供应商语义。xAI deferred、GLM async、MiniMax 异步 TTS 与图像任务保留各自引用类型和终止状态。它们是云端任务，独立于 Harness 任务执行、Cron 与 Workflow 调度。')],
      },
      {
        id: 'native-failure', title: t('Scope, errors and acceptance evidence', '作用域、错误与验收证据'),
        bullets: [t('Carry exact account/resource/workspace scopes through file, container, cache, job and retrieval references. A profile rename, changed endpoint or wrong credential scope must fail instead of silently rebuilding a resource elsewhere.', '文件、容器、缓存、任务与检索引用贯穿精确账户、资源或工作空间作用域。连接改名、端点变化或凭据作用域错误必须失败，不能静默在别处重建资源。'), t('Mutations with uncertain dispatch retain typed unknown-outcome errors. Query an existing reference where possible. Hosts choose polling, retry, cancellation and durable cleanup; the SDK does not become a task scheduler.', '发送结果不确定的修改操作保留类型化未知结果错误。可能时查询现有引用。宿主选择轮询、重试、取消与持久清理；SDK 不承担任务调度器职责。'), t('Reference validation, provider binding and mock wire tests establish local admission and encoding behavior. Actual provider endpoints, tool entitlements, remote MCP authorization and server-side billing require a separately recorded live-account run.', '引用校验、供应商绑定与模拟协议测试证明本地准入和编码行为。实际端点、工具授权、远程 MCP 授权与服务器结费需要单独记录真实账户测试。')],
        links: [{ label: t('Execution and per-attempt accounting', '执行与逐次结算'), href: '/docs/llm-execution-accounting' }, { label: t('Scoped files and upload recovery', '作用域文件与上传恢复'), href: '/docs/llm-files' }],
      },
    ],
  },
  {
    id: 'llm-controls', group: 'llm', packageName: 'lingxi-llm-client', sourceUrl: currentSource,
    title: t('Output, inference & cache controls', '输出、推理与缓存控制'),
    description: t('Apply validated request-local contracts without weakening schemas or confusing reasoning budgets with monetary limits.', '应用已校验的请求级约定，不弱化 Schema，并区分推理预算与金额限制。'),
    sources: sources('src/protocol/inference.rs', 'src/protocol/request_controls.rs', 'src/protocol/structured.rs', 'src/codecs/inference.rs', 'src/codecs/structured.rs', 'src/codecs/cache.rs', 'src/client/decisions.rs', 'src/protocol/decision.rs', 'src/embeddings/mod.rs', 'src/token_count/request.rs', 'src/providers/openrouter/response_cache.rs', 'tests/structured_output.rs', 'tests/decisions.rs', 'tests/embeddings.rs'),
    sections: [
      {
        id: 'inference-controls', title: t('Thinking, effort and service tier', 'Thinking、effort 与服务等级'),
        paragraphs: [t('ThinkingConfig contains optional mode (Disabled/Enabled/Adaptive), budget (Tokens/Dynamic) and effort. ServiceTier independently selects Standard/Fast. The selected model row and profile’s declared wire mapping determine which combinations can be encoded; unsupported explicit controls fail admission. Missing metadata means unknown, not support.', 'ThinkingConfig 包含可选 mode（Disabled/Enabled/Adaptive）、budget（Tokens/Dynamic）与 effort。ServiceTier 独立选择 Standard/Fast。所选模型行与连接声明的协议映射决定可编码组合；不支持的显式控制会准入失败。缺失元数据表示未知，不表示支持。')],
        tables: [{ headers: [t('Control', '控制项'), t('Meaning', '含义'), t('Accounting implication', '结算含义')], rows: [
          [t('ThinkingBudget::Tokens', 'ThinkingBudget::Tokens'), t('Provider reasoning-token budget or target, with declared min/max/hard_limit facts', '供应商推理 token 预算或目标，由声明的 min/max/hard_limit 描述'), t('Not a currency budget; host still enforces spend admission', '不是金额预算；宿主仍需控制支出准入')],
          [t('ReasoningEffort', 'ReasoningEffort'), t('Explicit supported level under the active mode', '当前 mode 下明确支持的等级'), t('Can change token consumption; does not itself change unit prices', '可能改变 token 消耗；本身不修改单位价格')],
          [t('ServiceTier::Fast', 'ServiceTier::Fast'), t('Endpoint/model-specific fast wire option', '端点/模型特定的快速协议选项'), t('Keep requested and executed tiers distinct; unknown executed tier cannot establish a Fast price', '区分请求与实际执行等级；执行等级未知时不能确认 Fast 价格')],
        ] }],
        note: t('InferenceFeatures overlays normalize mode-incompatible defaults. A request-local observed fast_capability affects the selected primary model only, not shared configuration or fallback accounts. InferenceReport retains provider-reported execution facts.', 'InferenceFeatures 覆盖会清理与模式不兼容的默认值。请求级观测 fast_capability 仅影响所选主模型，不影响共享配置或备用账户。InferenceReport 保留供应商报告的执行事实。'),
      },
      {
        id: 'structured-output', title: t('Strict output and local validation', '严格输出与本地校验'),
        paragraphs: [t('OutputFormat is Text, JsonObject or JsonSchema { name, schema, strict }. It is separate from strict function-tool inputs. Encoders preserve schema constraints and reject unsupported/conflicting options; they do not rewrite schemas to make a call succeed. Local schema validation disables external network/file resolution. Decode only a completed response; refusal, truncation, tools and pause are separate outcomes.', 'OutputFormat 为 Text、JsonObject 或 JsonSchema { name, schema, strict }，独立于严格函数工具输入。编码器保留 Schema 约束并拒绝不支持或冲突的选项，不为调用成功而改写 Schema。本地校验禁用外部网络/文件解析。仅对完成响应解码；拒绝、截断、工具与暂停是不同结果。')],
        code: [{ label: t('Complete and validate the same output contract', '完成并校验同一输出约定'), language: 'rust', verification: 'compile', exampleId: 'llm-structured-output', code: `use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, JsonPayload, OutputFormat};

pub async fn structured_answer(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<JsonPayload, Box<dyn std::error::Error>> {
    let mut request = ChatRequest::new("gpt-6-luna");
    request.output_format = OutputFormat::JsonSchema {
        name: "answer".into(), strict: true,
        schema: JsonPayload::parse(r#"{"type":"object","properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false}"#)?,
    };
    request.messages.push(ConversationMessage::user_text("Give a short greeting."));
    let response = client.chat().complete_in("openai", &request, options).await?;
    Ok(response.structured_json(&request.output_format)?)
}` }],
        bullets: [t('Stream collectors reconstruct terminal content and then validate; individual JSON deltas are not standalone documents. Validation errors retain response/events and usage for host accounting.', '流收集器先重建终止内容再校验；单个 JSON 增量不是独立文档。校验错误保留响应、事件与用量，供宿主结算。'), t('First-party Qwen and DeepSeek JSON Object requests require JSON/json in explicit system/user text. Their checks do not count assistant text, tool descriptions or attachment payloads. Strict schema support is admitted per model/route rather than protocol compatibility.', '第一方 Qwen 与 DeepSeek JSON Object 请求要求显式 system/user 文本出现 JSON/json；助手文本、工具描述与附件载荷不计入检查。严格 Schema 支持按模型/路由准入，而非按协议兼容性判断。')],
      },
      {
        id: 'finite-decisions', title: t('Finite single-choice decisions', '有限选项单选决策'),
        paragraphs: [t('DecisionRequest shares text/image context across one or more questions, each with unique nonempty question IDs and option IDs. DecisionService uses verified strict structured-output routes. models() lists admitted models; an arbitrary catalog structured_output flag, compatible gateway or account is insufficient.', 'DecisionRequest 在一个或多个问题间共享文本/图像上下文，每个问题与选项 ID 必须非空且唯一。DecisionService 使用已验证的严格结构化输出路由。models() 列出准入模型；任意目录 structured_output 标记、兼容网关或账户不足以准入。')],
        code: [{ label: t('One finite-choice question with retained call report', '保留调用报告的有限选项问题'), language: 'rust', verification: 'compile', exampleId: 'llm-decision', code: `use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{DecisionContextPart, DecisionOption, DecisionQuestion, DecisionRequest, DecisionResult};

pub async fn classify(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<DecisionResult, Box<dyn std::error::Error>> {
    let request = DecisionRequest {
        model: "gpt-6-luna".into(),
        context: vec![DecisionContextPart::Text { text: "The build completed successfully.".into() }],
        questions: vec![DecisionQuestion {
            id: "build_status".into(), prompt: "What is the build status?".into(),
            options: vec![
                DecisionOption { id: "passed".into(), label: "Passed".into() },
                DecisionOption { id: "failed".into(), label: "Failed".into() },
            ],
        }],
    };
    Ok(client.decisions().decide_in("openai", &request, options).await?)
}` }],
        note: t('Refused, Incomplete, InvalidResult and ProviderWithReport retain call reports where available. Answers must be offered IDs for every original question; invalid completed results do not weaken constraints or fail over. report.usage is the final attempt; prior_attempts retains earlier reported usage separately.', 'Refused、Incomplete、InvalidResult 与 ProviderWithReport 在可用时保留调用报告。每个原问题必须返回已提供的选项 ID；不合法完成结果不会弱化约束或回退。report.usage 对应最后一次尝试，prior_attempts 分开保留更早已报告的用量。'),
      },
      {
        id: 'caches-and-embeddings', title: t('Prompt caches, response caches and embeddings', '提示缓存、响应缓存与嵌入'),
        paragraphs: [t('ChatRequest.prompt_cache contains TTL and explicit original-request breakpoints; an empty policy preserves provider defaults. Native OpenAI Responses cache controls, Gemini context-cache resources and OpenRouter gateway response-cache policy have separate contracts. A response-cache HIT/MISS observation comes from known response headers, never from zero usage. Do not treat cache support on one route as permission to encode it on another.', 'ChatRequest.prompt_cache 包含 TTL 与原请求中的显式断点；空策略保留供应商默认行为。原生 OpenAI Responses 缓存控制、Gemini 上下文缓存资源与 OpenRouter 网关响应缓存策略具有独立约定。响应缓存 HIT/MISS 观测来自已知响应头，不能从零用量推断。不能因某一路由支持缓存而在其他路由编码同一字段。'), t('client.embeddings().embed uses an independent configured route with ServiceSetting inheritance/enable/disable semantics. Text vectors remain in input order; invalid indices/counts, non-finite values and inconsistent dimensions are errors. The client does not split, normalize, truncate, retry or use the Chat model catalog to reinterpret embedding model IDs. Multimodal Gemini embedding is a separate typed native API.', 'client.embeddings().embed 使用独立配置路由，并遵守 ServiceSetting 继承、启用与禁用语义。文本向量保持输入顺序；索引或数量非法、非有限值与维度不一致均报错。客户端不会拆分、归一化、截断、重试或借 Chat 模型目录解释嵌入模型 ID。多模态 Gemini embedding 是独立类型化原生 API。')],
        bullets: [t('Local tokenizer estimates do no credential/network/remote-file resolution and explicitly list unsupported/omitted content. They estimate the preferred route only and are never authoritative billing usage.', '本地 tokenizer 估算不进行凭据、网络或远程文件解析，并明确列出不支持/遗漏内容。仅估算首选路由，不能作为权威结费用量。'), t('structured_output, decisions, inference_controls and embeddings tests validate mock wire contracts and error retention. Strict-output accuracy, live cache hit rates and provider-model entitlements require live-account evidence.', 'structured_output、decisions、inference_controls 与 embeddings 测试验证模拟协议约定与错误保留。严格输出准确性、真实缓存命中率与供应商模型权限需要真实账户证据。')],
      },
    ],
  },
  {
    id: 'llm-execution-accounting', group: 'llm', packageName: 'lingxi-llm-client', sourceUrl: currentSource,
    title: t('Routing, failures & attempt accounting', '路由、失败与逐次结算'),
    description: t('Choose convenience failover or host-controlled one-shot calls and preserve actual usage before output interpretation.', '选择便利回退或宿主控制的单次调用，在解释输出前保留实际用量。'),
    sources: sources('src/client/executor.rs', 'src/client/resolve.rs', 'src/client/prepared.rs', 'src/client/route.rs', 'src/client/options.rs', 'src/client/frozen_pricing.rs', 'src/protocol/usage_report.rs', 'src/protocol/provider.rs', 'src/protocol/llm.rs', 'src/account/service.rs', 'src/account/types.rs', 'src/account/execution.rs', 'tests/execution_contracts.rs', 'tests/failover.rs', 'tests/server_fallback_pricing.rs'),
    sections: [
      {
        id: 'resolution', title: t('Resolution and fallback credentials', '路由解析与备用凭据'),
        paragraphs: [t('A provider identity, exact profile, connection group, protocol, wire model and billing mode are distinct. resolve()/complete() resolve model aliases and ordered connections; complete_in selects a starting profile or configured group. PreparedCall::prepare_on requires an exact connection. Region filters, hidden connections, wire model identity and billing compatibility constrain fallback; ambiguous model rows are not guessed.', '供应商身份、精确连接、连接组、协议、协议模型与计费模式不同。resolve()/complete() 解析模型别名与有序连接；complete_in 选择起始连接或已配置组。PreparedCall::prepare_on 要求精确连接。区域过滤、隐藏连接、协议模型身份与计费兼容性限制回退；不会猜测有歧义的模型行。'), t('RequestOptions.protocol overrides the profile default through explicitly registered protocol_profiles. Each alternate profile owns endpoint/auth/capability policy. credential and request-local authenticator belong only to the selected primary; named fallback profiles must receive their own fallback_credentials. Never reuse the primary secret by inferring that two endpoints are the same account.', 'RequestOptions.protocol 通过明确登记的 protocol_profiles 覆盖连接默认协议。各替代连接拥有自己的端点、认证与能力策略。credential 与请求级 authenticator 仅属于所选主连接；命名备用连接必须在 fallback_credentials 中获得自己的凭据。不能因为端点看似属于同一账户而复用主密钥。')],
        tables: [{ headers: [t('Execution path', '执行路径'), t('Replay policy', '重放策略'), t('Use', '用途')], rows: [
          [t('chat().complete/stream', 'chat().complete/stream'), t('Configured ordered fallback only when trigger and execution safety admit it; not a same-profile retry loop', '仅在触发条件与执行安全允许时按配置顺序回退；不是同连接重试循环'), t('Convenience application request', '应用便利请求')],
          [t('snapshot.prepare_on → dispatch_once', 'snapshot.prepare_on → dispatch_once'), t('One exact connection, one generation dispatch; no automatic retry/fallback', '一个精确连接，一次生成发送；无自动重试/回退'), t('Host admission and physical-attempt ledger', '宿主准入与实际调用账本')],
          [t('Audio / images / embeddings / native mutations', '音频/图像/嵌入/原生修改操作'), t('Operation-specific single-dispatch and unknown-outcome contracts', '操作特定的单次发送与未知结果约定'), t('Independent service lifecycle', '独立服务生命周期')],
        ] }],
      },
      {
        id: 'failure-classes', title: t('Errors and host retry decisions', '错误与宿主重试决策'),
        paragraphs: [t('LlmError::kind classifies typed errors and ProviderResponse preserves HTTP status, request ID, native body, classification and retry_after where available. FailoverTriggers independently covers rate limits/quota, overload, provider internal/timeout, network/transport timeout and authentication/permission. A standalone default profile has no implied fallback chain.', 'LlmError::kind 分类类型化错误；ProviderResponse 保留 HTTP 状态、请求 ID、原生响应体、分类与可用 retry_after。FailoverTriggers 分别覆盖限流/配额、过载、供应商内部错误/超时、网络/传输超时与认证/权限。独立默认连接没有隐式回退链。')],
        bullets: [t('InvalidRequest, UnsupportedCapability, ContextOverflow, RequestTooLarge and TLS certificate errors do not become generic endpoint-switch triggers. Fix the input/configuration, compact in the host, or repair transport trust as appropriate.', 'InvalidRequest、UnsupportedCapability、ContextOverflow、RequestTooLarge 与 TLS 证书错误不会成为通用切换端点触发器。应相应修正输入/配置、由宿主压缩上下文或修复传输信任。'), t('A returned stream does not fail over after read failure. A continuation pins the original account/profile/endpoint/model, and side-effecting hosted requests can suppress replay before dispatch.', '流返回后读取失败不会回退。续接固定原账户、连接、端点与模型；有副作用的托管请求可在发送前抑制重放。'), t('A Transport must not silently retry or follow credentialed redirects. The host records an uncertain dispatched attempt before deciding whether another call is acceptable; an error alone does not prove zero execution or zero cost.', 'Transport 不得静默重试或跟随携带凭据的重定向。宿主先记录已发送但结果不确定的尝试，再决定是否接受下一次调用；错误本身不能证明未执行或零成本。')],
      },
      {
        id: 'attempt-example', title: t('Capture observations before decoding', '解码前捕获观测'),
        code: [{ label: t('One prepared physical attempt with frozen pricing and cleanup', '固定价格并清理资源的一次实际调用'), language: 'rust', verification: 'compile', exampleId: 'llm-attempt-accounting', code: `use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::codecs::RequestMode;
use lingxi_llm_client::protocol::{ChatRequest, Submission};

pub async fn one_attempt(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = client.snapshot();
    let call = snapshot.prepare_on("openai", request, options, RequestMode::Complete).await?;
    let prices = call.pricing_snapshot();
    let received = call.dispatch_once().await?;
    let collected = received.collect().await?;
    let usage = collected.usage_report().clone();
    let inference = collected.inference_report().clone();
    // Persist these observations in the host before interpreting application content.
    let estimate = prices.estimate(&usage, &inference, Submission::Interactive);
    let response = collected.decode();
    collected.finish().await;
    println!("Usage: {usage:?}; estimate: {estimate:?}");
    println!("{}", response?.message.text());
    Ok(())
}` }],
        paragraphs: [t('Preparation freezes route and prices, resolves/uploads admitted attachments, encodes final bytes and authenticates; it does not dispatch generation. PreparedCall is consumed once. RequestDraft allows a final synchronous host transformation before seal; dispatch_once_with marks host admission immediately before send. Preparation can still perform attachment I/O or WebSocket connection work.', '准备固定路由与价格，解析/上传已准入附件，编码最终字节并认证，不发送生成请求。PreparedCall 只消耗一次。RequestDraft 允许 seal 前的最后同步宿主修改；dispatch_once_with 在发送前立即标记宿主准入。准备阶段仍可能进行附件 I/O 或 WebSocket 建连。'), t('Collect usage and inference before decode because failed statuses or malformed tool payloads may carry accounting facts. finish consumes the collected response and performs attachment cleanup. Preserve the ledger on every path, including dispatch uncertainty, partial stream usage, content validation failure and cancellation.', '在 decode 前收集用量与推理事实，因为失败状态或工具载荷非法仍可能携带结算事实。finish 消耗收集结果并执行附件清理。所有路径都应保留账本，包括发送不确定、流用量不完整、内容校验失败与取消。')],
      },
      {
        id: 'pricing-and-accounts', title: t('Frozen prices and independent account reports', '固定价格与独立账户报告'),
        paragraphs: [t('UsageReport distinguishes Missing, Partial, Complete and Invalid observations; absent fields are not zero. FrozenPricing estimates only when usage and execution facts support a valid price, retaining currency and selected model/connection. Requested Fast alone is insufficient when the executed tier is unknown. Anthropic server fallback-summary cost has separate component/completeness handling; partial components must not become a complete total.', 'UsageReport 区分 Missing、Partial、Complete 与 Invalid 观测；缺失字段不是零。FrozenPricing 仅在用量与执行事实支持有效价格时估算，保留币种与所选模型/连接。实际等级未知时，仅请求 Fast 不足以定价。Anthropic 服务器 fallback-summary 成本有独立组件/完整性处理；部分组件不能当作完整总额。'), t('client.account_usage(profile, query) does not generate model output. AccountQuery separates API/auth-user identity, ordinary versus management/service credentials, exact selector scopes and UTC time range. AccountMetric distinguishes available, unsupported, credential-required, not-reported and failed states for balance, token history, cost history, quota windows and subscription; an account aggregate is not an attempt ledger or final invoice.', 'client.account_usage(profile, query) 不生成模型输出。AccountQuery 区分 API/auth-user 身份、普通与 management/service 凭据、精确 selector 作用域及 UTC 时间范围。AccountMetric 对余额、token 历史、成本历史、配额窗口与订阅区分 available、unsupported、credential-required、not-reported 与 failed；账户汇总不是逐次账本或最终发票。')],
        code: [{ label: t('Query the exact account separately from generation', '独立于生成查询精确账户'), language: 'rust', verification: 'compile', exampleId: 'llm-account-query', code: `use lingxi_llm_client::{AccountIdentity, AccountQuery, AccountSnapshot, LlmClient};
use lingxi_llm_client::protocol::Secret;

pub async fn account_snapshot(
    client: &LlmClient,
    credential: Secret<String>,
) -> Result<AccountSnapshot, Box<dyn std::error::Error>> {
    let mut query = AccountQuery::new(AccountIdentity::ApiKey);
    query.credential = Some(credential);
    // Supply management_credential/selectors separately if this source requires them.
    Ok(client.account_usage("openai", &query).await?)
}` }],
        note: t('execution_contracts, failover and pricing tests validate local behavior with mocked observations. This handbook does not report a live charge, provider invoice, account balance or a full device acceptance result.', 'execution_contracts、failover 与计价测试使用模拟观测验证本地行为。本手册不声称已验证真实扣费、供应商发票、账户余额或完整设备验收。'),
      },
    ],
  },
];

// Task-oriented recipes supplement the detailed API reference above.
const practicalGuides: Record<string, { sources: string[]; sections: DocSection[] }> = {
  "llm-files": {
    "sources": [
      "src/files/types.rs",
      "src/files/service.rs"
    ],
    "sections": [
      {
        "id": "file-workflow-choice",
        "title": {
          "en": "Choose attachment input or explicit file management",
          "zh": "选择附件输入或显式文件管理"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Goal",
                "zh": "目标"
              },
              {
                "en": "Workflow",
                "zh": "流程"
              }
            ],
            "rows": [
              [
                {
                  "en": "Ask about a user attachment",
                  "zh": "询问用户附件内容"
                },
                {
                  "en": "Store an immutable host revision, configure AttachmentResolver, and put its AttachmentRef in ChatRequest",
                  "zh": "存储不可变宿主修订，配置 AttachmentResolver，再把 AttachmentRef 放入 ChatRequest"
                }
              ],
              [
                {
                  "en": "Reuse a provider file across requests",
                  "zh": "跨请求复用供应商文件"
                },
                {
                  "en": "Upload through the typed provider files resource; retain ProviderFileRef and its exact account/endpoint scope",
                  "zh": "通过类型化供应商文件资源上传，保存 ProviderFileRef 与其精确账户/端点作用域"
                }
              ],
              [
                {
                  "en": "Download or delete a remote resource",
                  "zh": "下载或删除远程资源"
                },
                {
                  "en": "Check that specific file operation is supported, then use the original scoped reference",
                  "zh": "检查该具体文件操作受支持，再使用原作用域引用"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "Do not replace a conversation’s host AttachmentRef with an expiring provider ID. A remote file can expire or become inaccessible while the original authorized host attachment remains the user’s durable content.",
            "zh": "不要用会过期的供应商 ID 替换对话中的宿主 AttachmentRef。远程文件可能过期或失去访问权，但已授权的宿主原始附件仍是用户的持久内容。"
          }
        ]
      },
      {
        "id": "file-error-actions",
        "title": {
          "en": "Handle upload and readiness outcomes",
          "zh": "处理上传与就绪结果"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Outcome",
                "zh": "结果"
              },
              {
                "en": "Next step",
                "zh": "下一步"
              }
            ],
            "rows": [
              [
                {
                  "en": "Unsupported operation / scope mismatch",
                  "zh": "不支持操作 / 作用域不符"
                },
                {
                  "en": "Stop before reading or uploading again; correct route, purpose, media type and account",
                  "zh": "先停止重复读取或上传，修正路由、用途、媒体类型与账户"
                }
              ],
              [
                {
                  "en": "FileUploadError::OutcomeUnknown",
                  "zh": "FileUploadError::OutcomeUnknown"
                },
                {
                  "en": "Record the uncertain attempt; use supported listing/metadata to reconcile before another upload",
                  "zh": "记录不确定尝试，通过支持的列表/元数据核对，再决定是否重新上传"
                }
              ],
              [
                {
                  "en": "ProviderFileProcessing",
                  "zh": "ProviderFileProcessing"
                },
                {
                  "en": "Retain the returned file reference and resume readiness polling",
                  "zh": "保留返回文件引用，继续就绪轮询"
                }
              ],
              [
                {
                  "en": "Expired or missing file",
                  "zh": "文件过期或缺失"
                },
                {
                  "en": "Revalidate the host attachment and replay policy; a replacement upload is not universally safe",
                  "zh": "重新校验宿主附件与重放策略，重新上传并非始终安全"
                }
              ]
            ]
          }
        ],
        "links": [
          {
            "label": {
              "en": "Keep attachment data out of retries",
              "zh": "重试时保留附件与调用边界"
            },
            "href": "/docs/llm-execution-accounting"
          }
        ]
      }
    ]
  },
  "llm-audio": {
    "sources": [
      "src/audio/types.rs",
      "src/audio/mod.rs"
    ],
    "sections": [
      {
        "id": "audio-application-recipe",
        "title": {
          "en": "Wire speech into an application",
          "zh": "在应用中接入语音"
        },
        "steps": [
          {
            "en": "Select an exact AudioRoute and query capabilities. Choose the declared audio model, operation, voice and format instead of inheriting the current chat model.",
            "zh": "选择精确 AudioRoute 并查询能力。选择声明的音频模型、操作、声音与格式，而不是继承当前 Chat 模型。"
          },
          {
            "en": "For synthesis, submit complete text with SynthesisRequest. For file transcription, provide the supported AudioInput and TranscriptionRequest. Device microphone access and file selection happen in the host first.",
            "zh": "合成时用 SynthesisRequest 提交完整文本；文件转写时提供受支持的 AudioInput 与 TranscriptionRequest。设备麦克风授权与文件选择由宿主先完成。"
          },
          {
            "en": "Consume output according to its actual type: byte stream or provider URL. Bound collected bytes; for raw PCM also retain the required sample rate/channel/encoding declaration.",
            "zh": "按实际返回类型消费字节流或供应商 URL。限制收集字节数；原始 PCM 还需保留采样率、声道与编码声明。"
          },
          {
            "en": "Record terminal metadata and usage when available, then close playback resources owned by the application. Cancelling audio reads does not prove the provider performed no synthesis.",
            "zh": "可用时记录终止元数据与用量，再关闭应用持有的播放资源。取消音频读取不能证明供应商没有执行合成。"
          }
        ]
      },
      {
        "id": "audio-troubleshooting",
        "title": {
          "en": "Diagnose audio before retrying",
          "zh": "重试前排查音频问题"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Symptom",
                "zh": "现象"
              },
              {
                "en": "Check",
                "zh": "检查项"
              }
            ],
            "rows": [
              [
                {
                  "en": "No supported voice/model",
                  "zh": "没有支持的声音/模型"
                },
                {
                  "en": "capabilities for the exact route and common adapter, not chat compatibility",
                  "zh": "检查精确路由与统一适配器能力，不依据 Chat 兼容性判断"
                }
              ],
              [
                {
                  "en": "Noise or wrong playback speed",
                  "zh": "噪声或播放速度错误"
                },
                {
                  "en": "Actual encoding, sample rate and channel count; the SDK does not own device resampling",
                  "zh": "实际编码、采样率与声道数；SDK 不负责设备重采样"
                }
              ],
              [
                {
                  "en": "Collection reaches a byte limit",
                  "zh": "收集达到字节上限"
                },
                {
                  "en": "Use incremental consumption or a host-selected larger bound; do not allocate without a bound",
                  "zh": "采用增量消费或宿主选择的更大上限，避免无上限分配"
                }
              ],
              [
                {
                  "en": "Error after dispatch",
                  "zh": "发送后报错"
                },
                {
                  "en": "AudioError.kind and dispatch; distinguish rejected, accepted and unknown outcomes before resubmission",
                  "zh": "查看 AudioError.kind 与 dispatch，区分拒绝、已接受、未知结果后再决定是否重发"
                }
              ]
            ]
          }
        ]
      }
    ]
  },
  "llm-realtime": {
    "sources": [
      "src/realtime/mod.rs"
    ],
    "sections": [
      {
        "id": "realtime-application-loop",
        "title": {
          "en": "A realtime session in five steps",
          "zh": "实时会话的五个步骤"
        },
        "steps": [
          {
            "en": "Choose a provider connector and a host-compatible transport/executor. Confirm the connector capabilities admit the required audio formats and conversation features.",
            "zh": "选择供应商连接器及宿主兼容的传输/执行器，确认连接器能力允许所需音频格式与对话特性。"
          },
          {
            "en": "Connect RealtimeSession, retain the control handle, and keep consuming the event handle while the driver runs. A connected socket alone does not execute the application event loop.",
            "zh": "连接 RealtimeSession，保留 control 句柄，在 driver 运行期间持续消费 event 句柄。套接字连接成功本身不会执行应用事件循环。"
          },
          {
            "en": "Split microphone/audio input into small bounded frames before sending. Handle queue-full errors by pausing the producer or applying an explicit host policy.",
            "zh": "发送前将麦克风/音频输入拆成有界小帧。队列已满时暂停生产者，或执行明确的宿主策略。"
          },
          {
            "en": "For a tool request, correlate the completed call ID, execute only a registered and authorized host tool, send the tool result, and request continuation when the connector requires it.",
            "zh": "收到工具请求时关联已完成调用的 ID，只执行已登记且已授权的宿主工具，发送结果，并在连接器要求时请求续接。"
          },
          {
            "en": "On user interruption, use the control interruption path and synchronize local playback. On shutdown, close/abort and retain the host conversation state needed for an explicit reconnect.",
            "zh": "用户打断时使用 control 中断路径并同步本地播放。关闭时执行 close/abort，保留显式重连所需的宿主对话状态。"
          }
        ]
      },
      {
        "id": "realtime-limits-reference",
        "title": {
          "en": "Queue and frame limits",
          "zh": "队列与帧大小限制"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "RealtimeLimits field",
                "zh": "RealtimeLimits 字段"
              },
              {
                "en": "Default",
                "zh": "默认值"
              },
              {
                "en": "Meaning",
                "zh": "含义"
              }
            ],
            "rows": [
              [
                {
                  "en": "outbound_capacity",
                  "zh": "outbound_capacity"
                },
                {
                  "en": "8",
                  "zh": "8"
                },
                {
                  "en": "Bounded queued inputs; send can report backpressure",
                  "zh": "输入队列有界，send 可能返回背压错误"
                }
              ],
              [
                {
                  "en": "event_capacity",
                  "zh": "event_capacity"
                },
                {
                  "en": "32",
                  "zh": "32"
                },
                {
                  "en": "Bounded decoded event queue; the host must continue consuming",
                  "zh": "解码事件队列有界，宿主必须持续消费"
                }
              ],
              [
                {
                  "en": "max_frame_bytes",
                  "zh": "max_frame_bytes"
                },
                {
                  "en": "1 MiB",
                  "zh": "1 MiB"
                },
                {
                  "en": "Maximum frame size, not a recommended microphone chunk size",
                  "zh": "单帧最大值，不是建议的麦克风分片大小"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "History import, interruption and continuation are separate operations. Do not assume reconnect restores provider state, starts a response, or replays microphone bytes automatically.",
          "zh": "历史导入、中断与续接是不同操作。不要假设重连会自动恢复供应商状态、开始响应或重放麦克风字节。"
        }
      }
    ]
  },
  "llm-native-services": {
    "sources": [
      "src/protocol/llm.rs",
      "src/client/options.rs"
    ],
    "sections": [
      {
        "id": "tool-execution-choice",
        "title": {
          "en": "Choose who executes the tool",
          "zh": "明确工具由谁执行"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Mechanism",
                "zh": "机制"
              },
              {
                "en": "Executes on",
                "zh": "执行位置"
              },
              {
                "en": "Application integration",
                "zh": "应用接法"
              }
            ],
            "rows": [
              [
                {
                  "en": "ChatRequest.tools",
                  "zh": "ChatRequest.tools"
                },
                {
                  "en": "Your host",
                  "zh": "你的宿主"
                },
                {
                  "en": "Validate completed tool calls, apply permission policy, run registered code and append ToolResult",
                  "zh": "校验完成的调用，执行权限策略，运行登记代码，再追加 ToolResult"
                }
              ],
              [
                {
                  "en": "ChatRequest.hosted_tools",
                  "zh": "ChatRequest.hosted_tools"
                },
                {
                  "en": "Provider infrastructure",
                  "zh": "供应商基础设施"
                },
                {
                  "en": "Enable an admitted typed tool and retain provider-native results, citations and usage",
                  "zh": "启用已准入的类型化工具，保留原生结果、引用与用量"
                }
              ],
              [
                {
                  "en": "Hosted remote MCP",
                  "zh": "托管远程 MCP"
                },
                {
                  "en": "Remote MCP service via the provider",
                  "zh": "供应商连接的远程 MCP 服务"
                },
                {
                  "en": "Supply request-local server authorization; handle any provider approval/continuation outcome",
                  "zh": "传入请求级服务器授权，处理供应商审批与续接结果"
                }
              ],
              [
                {
                  "en": "Harness local tools / MCP",
                  "zh": "Harness 本地工具 / MCP"
                },
                {
                  "en": "Harness-managed host execution",
                  "zh": "Harness 管理的宿主执行"
                },
                {
                  "en": "Use Harness registration, permissions, lifecycle and durable session integration",
                  "zh": "接入 Harness 登记、权限、生命周期与持久会话"
                }
              ]
            ]
          }
        ],
        "links": [
          {
            "label": {
              "en": "Complete a local function round trip",
              "zh": "完成本地函数闭环"
            },
            "href": "/docs/llm-requests#function-tool-roundtrip"
          }
        ]
      },
      {
        "id": "cloud-job-recovery",
        "title": {
          "en": "Persist cloud job identity before polling",
          "zh": "轮询前保存云端任务身份"
        },
        "steps": [
          {
            "en": "Persist the returned resource reference with the application job record immediately after a successful submit. Keep profile, account, endpoint and resource scope together.",
            "zh": "成功 submit 后立即把返回资源引用和应用任务记录一起保存，连接、账户、端点与资源作用域应保持成组。"
          },
          {
            "en": "After a restart, query the saved reference with fresh credentials for the same account. Polling is a read operation on the existing job, not another submit.",
            "zh": "重启后使用同一账户的新鲜凭据查询保存的引用。轮询读取现有任务，不是再次 submit。"
          },
          {
            "en": "Distinguish cancel from delete. Cancelling asks the provider to stop work; deleting a stored result is a separate resource-lifecycle action where supported.",
            "zh": "区分 cancel 与 delete。取消请求供应商停止工作；删除已存储结果是受支持时另一种资源生命周期操作。"
          },
          {
            "en": "If submit/cancel acknowledgement is lost, retain the unknown-outcome state and reconcile against supported provider APIs. Do not turn a network error into a second side effect.",
            "zh": "如果 submit/cancel 回执丢失，保留未知结果状态，并通过受支持供应商 API 核对。不能把网络错误直接变成第二次副作用。"
          }
        ]
      }
    ]
  },
  "llm-controls": {
    "sources": [
      "src/protocol/json_payload.rs",
      "src/protocol/structured.rs",
      "src/client/stream.rs"
    ],
    "sections": [
      {
        "id": "output-contract-choice",
        "title": {
          "en": "Choose the smallest output contract",
          "zh": "选择恰当的输出约定"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Contract",
                "zh": "约定"
              },
              {
                "en": "Use",
                "zh": "用途"
              },
              {
                "en": "What to validate",
                "zh": "需要校验"
              }
            ],
            "rows": [
              [
                {
                  "en": "OutputFormat::Text",
                  "zh": "OutputFormat::Text"
                },
                {
                  "en": "Human-readable prose",
                  "zh": "供人阅读的文本"
                },
                {
                  "en": "Stop reason and application-specific content requirements",
                  "zh": "终止原因与应用内容要求"
                }
              ],
              [
                {
                  "en": "OutputFormat::JsonObject",
                  "zh": "OutputFormat::JsonObject"
                },
                {
                  "en": "A JSON object whose fields the application checks",
                  "zh": "由应用自行检查字段的 JSON 对象"
                },
                {
                  "en": "Object root, required application fields and valid value types",
                  "zh": "对象根节点、应用必需字段与值类型"
                }
              ],
              [
                {
                  "en": "OutputFormat::JsonSchema",
                  "zh": "OutputFormat::JsonSchema"
                },
                {
                  "en": "A typed object/array contract admitted by the route",
                  "zh": "路由允许的类型化对象/数组约定"
                },
                {
                  "en": "Original schema, final response, schema match and target-type deserialization",
                  "zh": "原始 Schema、最终响应、Schema 匹配与目标类型反序列化"
                }
              ],
              [
                {
                  "en": "ToolSpec.strict",
                  "zh": "ToolSpec.strict"
                },
                {
                  "en": "Constrain function-tool input",
                  "zh": "约束函数工具参数"
                },
                {
                  "en": "Tool arguments separately from the final-answer output contract",
                  "zh": "工具参数校验独立于最终答案输出约定"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "Use JsonPayload::parse for an exact JSON schema source, or from_generated_value for a small schema built by your own configuration UI. Provider JSON and partial tool arguments should remain source-carrying data until a completed, validated application boundary.",
            "zh": "精确 JSON Schema 源文使用 JsonPayload::parse；由自己配置界面构造的小型 Schema 可用 from_generated_value。供应商 JSON 和未完整的工具参数应保留为携带源文的数据，直到到达完整且已校验的应用边界。"
          }
        ]
      },
      {
        "id": "structured-result-errors",
        "title": {
          "en": "Recover from structured result failures",
          "zh": "处理结构化结果失败"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "StructuredOutputErrorKind",
                "zh": "StructuredOutputErrorKind"
              },
              {
                "en": "Meaning",
                "zh": "含义"
              },
              {
                "en": "Action",
                "zh": "动作"
              }
            ],
            "rows": [
              [
                {
                  "en": "Incomplete",
                  "zh": "Incomplete"
                },
                {
                  "en": "Not a final completed answer, or response still contains tool use",
                  "zh": "不是完整最终答案，或响应仍包含工具调用"
                },
                {
                  "en": "Handle refusal/truncation/tool flow before parsing application data",
                  "zh": "先处理拒绝、截断或工具流程，再解析应用数据"
                }
              ],
              [
                {
                  "en": "NoContract",
                  "zh": "NoContract"
                },
                {
                  "en": "The request used the text contract",
                  "zh": "请求使用了文本约定"
                },
                {
                  "en": "Set the intended JSON output_format explicitly",
                  "zh": "显式设置所需 JSON output_format"
                }
              ],
              [
                {
                  "en": "InvalidJson / SchemaMismatch",
                  "zh": "InvalidJson / SchemaMismatch"
                },
                {
                  "en": "Final output is invalid JSON or violates the requested schema",
                  "zh": "最终输出 JSON 非法或违反所请求 Schema"
                },
                {
                  "en": "Retain the response and usage; reject it as application data",
                  "zh": "保留响应与用量，不将其当作应用数据接受"
                }
              ],
              [
                {
                  "en": "InvalidSchema / Deserialization",
                  "zh": "InvalidSchema / Deserialization"
                },
                {
                  "en": "Local schema or target Rust type cannot accept the value",
                  "zh": "本地 Schema 或目标 Rust 类型无法接受该值"
                },
                {
                  "en": "Fix the application contract/type rather than weakening provider constraints silently",
                  "zh": "修正应用约定/类型，不静默弱化供应商约束"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "StructuredOutputError retains the response. StructuredStreamError also retains observed events for read failure, missing End, size limits and validation failure. Logging a content error must not discard the associated usage facts.",
          "zh": "StructuredOutputError 保留响应。StructuredStreamError 对读取失败、缺少 End、大小上限与校验失败还保留已观测事件。记录内容错误时不应丢弃相关用量事实。"
        }
      },
      {
        "id": "schema-design-recipe",
        "title": {
          "en": "Make a schema usable by the application",
          "zh": "让 Schema 真正服务应用"
        },
        "steps": [
          {
            "en": "Start from the object the application consumes: stable field names, explicit types and only the required fields. Keep any enum values aligned with the application’s actual accepted IDs.",
            "zh": "从应用实际消费的对象出发，使用稳定字段名、显式类型及必要字段。枚举值应与应用真实接受的 ID 一致。"
          },
          {
            "en": "Choose a route with the required strict-schema support before sending. A compatible protocol or a model name alone does not establish support for every schema keyword.",
            "zh": "发送前选择具有所需严格 Schema 支持的路由。兼容协议或模型名称本身不能证明支持所有 Schema 关键字。"
          },
          {
            "en": "Pass the same OutputFormat to the request and structured_json/structured. For a stream, collect the complete response first; do not parse each text delta as JSON.",
            "zh": "将同一 OutputFormat 用于请求和 structured_json/structured。流式响应先收集完整结果，不将各个文本增量独立解析为 JSON。"
          },
          {
            "en": "Keep validation failure separate from transport failure and retry policy. A complete but invalid business result is not a reason to bypass the contract or automatically switch accounts.",
            "zh": "将校验失败与传输失败、重试策略分开。已完成但不合业务要求的结果，不能成为绕过约定或自动切换账户的理由。"
          }
        ]
      }
    ]
  },
  "llm-execution-accounting": {
    "sources": [
      "src/protocol/usage_report.rs"
    ],
    "sections": [
      {
        "id": "error-action-reference",
        "title": {
          "en": "Error-to-action reference",
          "zh": "错误与处理动作速查"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Category",
                "zh": "错误类别"
              },
              {
                "en": "Investigate",
                "zh": "检查内容"
              },
              {
                "en": "Retry decision",
                "zh": "重试判断"
              }
            ],
            "rows": [
              [
                {
                  "en": "Authentication / PermissionDenied",
                  "zh": "Authentication / PermissionDenied"
                },
                {
                  "en": "Credential freshness, exact profile/account and provider entitlement",
                  "zh": "凭据是否新鲜、连接/账户是否匹配、供应商权限"
                },
                {
                  "en": "Refresh or repair in the host; do not leak a key to another endpoint",
                  "zh": "由宿主刷新或修复，不向其他端点泄露密钥"
                }
              ],
              [
                {
                  "en": "RateLimited / QuotaExceeded",
                  "zh": "RateLimited / QuotaExceeded"
                },
                {
                  "en": "retry_after where reported, account limits and configured fallback triggers",
                  "zh": "可用 retry_after、账户限制与已配置回退触发器"
                },
                {
                  "en": "Honor a host retry budget and execution-safety policy",
                  "zh": "遵守宿主重试预算与执行安全策略"
                }
              ],
              [
                {
                  "en": "InvalidRequest / UnsupportedCapability",
                  "zh": "InvalidRequest / UnsupportedCapability"
                },
                {
                  "en": "Input schema, options, model and concrete route capabilities",
                  "zh": "输入 Schema、选项、模型与具体路由能力"
                },
                {
                  "en": "Fix the request; repeating the same invalid bytes is not recovery",
                  "zh": "修正请求；重复相同非法字节不是恢复"
                }
              ],
              [
                {
                  "en": "ContextOverflow / RequestTooLarge",
                  "zh": "ContextOverflow / RequestTooLarge"
                },
                {
                  "en": "Context-token limit versus serialized body byte limit",
                  "zh": "上下文 token 上限与序列化请求体字节上限"
                },
                {
                  "en": "Compact/select content in the host; do not silently drop history",
                  "zh": "由宿主压缩或筛选内容，不静默丢弃历史"
                }
              ],
              [
                {
                  "en": "Transport / TransportTimeout",
                  "zh": "Transport / TransportTimeout"
                },
                {
                  "en": "Whether dispatch happened, any visible output, partial usage and request ID",
                  "zh": "是否已发送、是否已有输出、部分用量与请求 ID"
                },
                {
                  "en": "Record uncertainty before considering another physical attempt",
                  "zh": "考虑下一次实际调用前先记录结果不确定性"
                }
              ],
              [
                {
                  "en": "ModelUnavailable / ProviderInternal / Overloaded",
                  "zh": "ModelUnavailable / ProviderInternal / Overloaded"
                },
                {
                  "en": "Provider diagnostics and the configured admission/fallback policy",
                  "zh": "供应商诊断与已配置准入/回退策略"
                },
                {
                  "en": "Only the permitted trigger and safe replay path may switch routes",
                  "zh": "只有允许的触发器与安全重放路径才可切换路由"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "Use error.kind() for broad classification and preserve the structured ProviderResponse fields when available. HTTP status alone omits provider-specific context; a retry label alone does not establish that replay is safe.",
            "zh": "通用分类使用 error.kind()，可用时同时保留 ProviderResponse 的结构化字段。仅看 HTTP 状态会丢失供应商语境，仅有可重试标签也不能证明重放安全。"
          }
        ]
      },
      {
        "id": "attempt-record-fields",
        "title": {
          "en": "What to keep in an attempt record",
          "zh": "一次调用记录应保存什么"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "Record",
                "zh": "记录项"
              },
              {
                "en": "Purpose",
                "zh": "用途"
              }
            ],
            "rows": [
              [
                {
                  "en": "Host request/attempt identity and timestamps",
                  "zh": "宿主请求/尝试身份与时间"
                },
                {
                  "en": "Correlate one user action with every physical dispatch",
                  "zh": "将用户动作与各次实际发送关联"
                }
              ],
              [
                {
                  "en": "Selected/executed profile, model and frozen pricing",
                  "zh": "所选/执行连接、模型与固定价格"
                },
                {
                  "en": "Explain routing and calculate against the actual captured configuration",
                  "zh": "解释路由，并以实际捕获配置计算"
                }
              ],
              [
                {
                  "en": "Provider request/response IDs when reported",
                  "zh": "供应商已报告的请求/响应 ID"
                },
                {
                  "en": "Reconcile errors and uncertain outcomes with provider evidence",
                  "zh": "关联供应商证据，核对错误与未知结果"
                }
              ],
              [
                {
                  "en": "UsageReport and InferenceReport before decode",
                  "zh": "解码前的 UsageReport 与 InferenceReport"
                },
                {
                  "en": "Retain measurements even when application JSON/tool validation fails",
                  "zh": "即使应用 JSON/工具校验失败也保留测量事实"
                }
              ],
              [
                {
                  "en": "Terminal stop reason, read failure or cancellation",
                  "zh": "终止原因、读取失败或取消"
                },
                {
                  "en": "Distinguish partial output from a completed answer",
                  "zh": "区分部分输出与完成答案"
                }
              ],
              [
                {
                  "en": "Resource cleanup / reconciliation state",
                  "zh": "资源清理 / 核对状态"
                },
                {
                  "en": "Track files or cloud jobs that may outlive this attempt",
                  "zh": "跟踪可能在调用结束后仍存活的文件或云端任务"
                }
              ]
            ]
          }
        ],
        "note": {
          "en": "Store account identities as non-secret scope IDs. Do not log credentials, request-local MCP authorizations or complete provider bodies that contain sensitive user content merely to obtain diagnostics.",
          "zh": "账户身份保存为非敏感作用域 ID。为了诊断，不应直接记录凭据、请求级 MCP 授权或包含敏感用户内容的完整供应商响应体。"
        }
      },
      {
        "id": "usage-state-reference",
        "title": {
          "en": "Missing, partial and complete usage",
          "zh": "缺失、部分与完整用量"
        },
        "tables": [
          {
            "headers": [
              {
                "en": "UsageState",
                "zh": "UsageState"
              },
              {
                "en": "How to present it",
                "zh": "如何呈现"
              }
            ],
            "rows": [
              [
                {
                  "en": "Missing",
                  "zh": "Missing"
                },
                {
                  "en": "No authoritative measurement was observed; display unknown",
                  "zh": "未观测到权威测量值，显示未知"
                }
              ],
              [
                {
                  "en": "Partial",
                  "zh": "Partial"
                },
                {
                  "en": "Keep measured fields but label the total incomplete",
                  "zh": "保留已测字段，并标记总量不完整"
                }
              ],
              [
                {
                  "en": "Complete",
                  "zh": "Complete"
                },
                {
                  "en": "Use the consistent reported usage and actual inference facts for supported estimates",
                  "zh": "以一致的已报告用量和实际推理事实进行受支持估算"
                }
              ],
              [
                {
                  "en": "Invalid",
                  "zh": "Invalid"
                },
                {
                  "en": "Retain diagnostic evidence but reject it as a valid complete total",
                  "zh": "保留诊断事实，不作为有效完整总量"
                }
              ]
            ]
          }
        ],
        "paragraphs": [
          {
            "en": "Input, output, cache-read and cache-write buckets are normalized to avoid double counting. Reasoning tokens are part of output tokens, and one-hour cache-write tokens are part of cache-write tokens; do not add those subsets again. Provider-reported cost, catalog estimates and account invoices are different evidence.",
            "zh": "输入、输出、缓存读取与缓存写入桶经过标准化，避免重复计数。推理 token 属于输出 token，一小时缓存写入 token 属于缓存写入 token，不能再把这些子集相加。供应商报告成本、目录估算与账户发票是不同证据。"
          }
        ]
      }
    ]
  }
};

export const llmServicePages: DocPage[] = referencePages.map((page) => {
  const guide = practicalGuides[page.id];
  if (!guide) return page;
  const sources = [...(page.sources ?? [])];
  for (const path of guide.sources) {
    if (!sources.some((source) => source.repo === 'llm-client' && source.path === path)) {
      sources.push({ repo: 'llm-client', path });
    }
  }
  return { ...page, sources, sections: [page.sections[0]!, ...guide.sections, ...page.sections.slice(1)] };
});
