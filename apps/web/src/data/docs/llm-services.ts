import type { DocPage, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const sources = (...paths: string[]) => paths.map(path => ({ repo: 'llm-client' as const, path }));
const currentSource = 'https://github.com/lingxi-coder/llm-client';

export const llmServicePages: DocPage[] = [
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
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, OutputFormat};
use serde_json::{json, Value};

pub async fn structured_answer(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut request = ChatRequest::new("gpt-6-luna");
    request.output_format = OutputFormat::JsonSchema {
        name: "answer".into(), strict: true,
        schema: json!({"type":"object","properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false}),
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
