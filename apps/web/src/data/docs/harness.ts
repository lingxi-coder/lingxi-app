import type { DocPage, DocSection, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const source = (_path: string) => 'https://github.com/lingxi-coder/harness-runtime';
const sources = (...paths: string[]) => paths.map((path) => ({ repo: 'harness-runtime' as const, path }));

export const harnessPages: DocPage[] = [
  {
    id: 'harness',
    group: 'harness',
    title: t('Harness Runtime', 'Harness Runtime'),
    description: t(
      'Embed the Rust agent runtime in a desktop, mobile, or custom host.',
      '将 Rust 智能体运行时嵌入桌面、移动端或自定义宿主。',
    ),
    packageName: 'harness-runtime',
    sourceUrl: source('crates/runtime/src/lib.rs'),
    sources: sources('crates/runtime/src/lib.rs', 'crates/runtime/src/api.rs', 'crates/runtime/Cargo.toml', 'Cargo.toml', 'rust-toolchain.toml'),
    sections: [
      {
        id: 'overview',
        title: t('The runtime boundary', '运行时边界'),
        paragraphs: [
          t(
            'Harness Runtime is the embeddable execution layer extracted from LingXi. It composes agents, sessions, model host services, permissions, tools, plugins, and persistence. The Cargo package is harness-runtime; the Rust crate is harness_runtime.',
            'Harness Runtime 是从灵犀提取的可嵌入执行层，组合智能体、会话、模型宿主服务、权限、工具、插件与持久化。Cargo 包名为 harness-runtime，Rust 导入名为 harness_runtime。',
          ),
          t(
            'The host owns product UI, entrypoints, credentials, platform capabilities, and application lifecycle. The independent llm-client owns communication with model providers. HarnessBuilder wraps services assembled by the host; constructing this facade does not start processes or make network requests.',
            '宿主负责产品界面、入口、凭据、平台能力和应用生命周期。独立的 llm-client 负责与模型供应商通信。HarnessBuilder 包装宿主已经组装的服务；构建该门面不会启动进程或发起网络请求。',
          ),
        ],
        note: t(
          'These pages describe the current workspace. The source baseline records HEAD, dirty files and the product dependency revision separately. A local API is not automatically present in the pinned product build; use Rustdoc for compiler-resolved types and features.',
          '这些页面描述当前工作区。源码基线分别记录 HEAD、未提交文件和产品依赖版本。本地接口并不自动包含在产品固定版本中；编译器解析后的类型和 feature 请查阅 Rustdoc。',
        ),
      },
      {
        id: 'installation',
        title: t('Use one source identity', '使用唯一源码身份'),
        paragraphs: [
          t(
            'For application integration, retain the dependency revisions declared by Harness. The multi-repository path overlay below is for developing the SDKs together and requires matching contracts. Replacing its pinned LLM Client with an arbitrary newer checkout can fail in llm-runtime before your application is compiled; a Cargo package version alone does not establish source compatibility.',
            '应用接入时应保留 Harness 声明的依赖 revision。下面的多仓 path overlay 用于联合开发 SDK，要求各仓契约一致。将固定的 LLM Client 替换成任意更新的工作树，可能在编译应用前就因 llm-runtime 的接口不一致而失败；Cargo 包版本号本身不能证明源码兼容。',
          ),
          t(
            'For the current working-tree examples, create sdk-consumer beside harness-runtime, llm-client and mobile-linux-runtime. Use the complete local manifest below. Direct Harness crates and explicit consumer patches select the three SDK working trees; inspect cargo metadata to verify those source identities before compiling the copied examples.',
            '验证当前工作树示例时，在 harness-runtime、llm-client 与 mobile-linux-runtime 旁创建 sdk-consumer，并使用下方完整本地清单。直接 Harness crate 与显式消费者 patch 选择三个 SDK 工作树；编译复制的示例前应通过 cargo metadata 验证这些源码身份。',
          ),
        ],
        code: [
          {
            label: t('Cargo.toml · working-tree consumer', 'Cargo.toml · 工作树消费者'),
            language: 'toml',
            verification: 'manual',
            exampleId: 'harness-working-tree-manifest',
            code: `[package]
name = "handbook-harness-consumer"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
harness-runtime = { path = "../harness-runtime/crates/runtime", default-features = false, features = ["desktop"] }
client = { path = "../harness-runtime/crates/client", features = ["adapter"] }
tool-api = { path = "../harness-runtime/crates/tool-api" }
permission = { path = "../harness-runtime/crates/permission" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }

[patch."https://github.com/lingxi-coder/llm-client.git"]
lingxi-llm-client = { path = "../llm-client" }

[patch."https://github.com/lingxi-coder/mobile-linux-runtime.git"]
mobile-linux-api = { path = "../mobile-linux-runtime/crates/mobile-linux-api" }
mobile-linux-core = { path = "../mobile-linux-runtime/crates/mobile-linux-core" }
mobile-linux-android = { path = "../mobile-linux-runtime/crates/mobile-linux-android" }
mobile-linux-ios = { path = "../mobile-linux-runtime/crates/mobile-linux-ios" }
platform-pty = { path = "../mobile-linux-runtime/crates/platform-pty" }

[patch.crates-io]
git2 = { path = "../harness-runtime/third_party/git2-rs/git2" }`,
          },
          {
            label: 'Source checkout',
            language: 'bash',
            verification: 'manual',
            code: 'git clone --recurse-submodules https://github.com/lingxi-coder/harness-runtime.git\ncd harness-runtime\ngit submodule update --init --recursive\ncargo check --locked -p harness-runtime',
          },
        ],
        note: t(
          'The repository pins Rust 1.94.0 and has no root path patch selecting the sibling llm-client. These overrides belong explicitly to the isolated consumer. For a released integration, use one reviewed full Git revision for all direct Harness packages, remove working-tree SDK overrides, and review the lockfile. Never mix Git and local versions of shared Rust types.',
          '仓库固定 Rust 1.94.0，根目录没有指向相邻 llm-client 的 path patch。这些覆盖显式属于隔离消费者。发布集成时，所有直接 Harness 包使用同一经审查完整 Git 提交，移除 SDK 工作树覆盖并审查锁文件。共享 Rust 类型不能混用 Git 与本地来源。',
        ),
      },
      {
        id: 'features',
        title: t('Choose a composition profile', '选择组合配置'),
        bullets: [
          t('engine — enabled by default; exposes the shared Harness, builder, session handle, and execution components.', 'engine — 默认启用，提供 Harness、构建器、会话句柄和共享执行组件。'),
          t('desktop — includes engine and desktop assembly, tools, client adapters, platform services, Fusion, workflow, and collaboration.', 'desktop — 包含 engine 及桌面组装、工具、客户端适配器、平台服务、Fusion、工作流与协作。'),
          t('mobile — includes engine and the shared iOS/Android Rust composition; the host supplies device and platform capabilities.', 'mobile — 包含 engine 及 iOS/Android 共享 Rust 组合；设备与平台能力由宿主注入。'),
          t('uniffi — includes mobile and native binding metadata. android-computer-use also includes mobile.', 'uniffi — 包含 mobile 及原生绑定元数据。android-computer-use 同样包含 mobile。'),
          t('realtime-websocket — enables realtime WebSocket support in the model runtime. fusion, workflow, and collaboration can also be selected explicitly.', 'realtime-websocket — 启用模型运行时的实时 WebSocket 支持。fusion、workflow 和 collaboration 也可单独选择。'),
        ],
        note: t(
          'The current Cargo feature is engine. Older README examples that call it core do not describe this revision; use the feature names in crates/runtime/Cargo.toml.',
          '当前 Cargo 功能名为 engine。旧 README 中名为 core 的示例不适用于此版本，请以 crates/runtime/Cargo.toml 中的名称为准。',
        ),
      },
      {
        id: 'builder',
        title: t('Assemble the public facade', '组装公共门面'),
        code: [
          {
            label: 'Rust · host-supplied services',
            language: 'rust',
            verification: 'compile',
            exampleId: 'harness-assemble',
            code: 'use std::sync::Arc;\nuse harness_runtime::{\n    Harness, HarnessBuilder, LifecycleService, SessionService,\n};\n\nfn assemble(\n    session: Arc<dyn SessionService>,\n    lifecycle: Arc<dyn LifecycleService>,\n) -> Harness {\n    HarnessBuilder::new(session, lifecycle).build()\n}',
          },
        ],
        apis: [
          {
            name: 'HarnessBuilder::new',
            signature: 'pub fn new(session: Arc<dyn SessionService>, lifecycle: Arc<dyn LifecycleService>) -> Self',
            description: t('Accept services from the same assembled runtime composition.', '接收来自同一已组装运行时的会话与生命周期服务。'),
          },
          {
            name: 'HarnessBuilder::build',
            signature: 'pub fn build(self) -> Harness',
            description: t('Retain the injected services behind opaque SDK handles.', '将注入服务保存在 SDK 的不透明句柄后。'),
          },
          {
            name: 'Harness::session',
            signature: 'pub fn session(&self) -> SessionHandle',
            description: t('Return a cheap clone of the handle for the composition’s currently mounted session.', '返回指向当前组合中已挂载会话的轻量句柄副本。'),
          },
        ],
      },
    ],
  },
  {
    id: 'harness-sessions',
    group: 'harness',
    title: t('Sessions & turns', '会话与轮次'),
    description: t(
      'Run prompts, observe output, cancel work, and read session state.',
      '运行提示词、接收输出、取消任务并读取会话状态。',
    ),
    packageName: 'harness-runtime',
    sourceUrl: source('crates/runtime/src/api.rs'),
    sources: sources('crates/runtime/src/api.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/core/src/host/orchestrator.rs', 'crates/client/src/adapter/output_stream.rs'),
    sections: [
      {
        id: 'run',
        title: t('Run a turn', '运行一个轮次'),
        paragraphs: [
          t(
            'RunInput contains the user prompt and a Vec<PathBuf> of image paths. The existing workspace and model implementation resolves those images. SessionHandle delegates execution, permission decisions, and persistence to the injected SessionService.',
            'RunInput 包含用户提示词和 Vec<PathBuf> 图像路径。图像由既有工作空间与模型实现解析。SessionHandle 将执行、权限决策和持久化委托给注入的 SessionService。',
          ),
        ],
        code: [
          {
            label: 'Rust · shared session interface',
            language: 'rust',
            verification: 'compile',
            exampleId: 'harness-run-turn',
            code: 'use harness_runtime::{\n    CancellationToken, HandleError, Harness, RunInput, TurnOutcome,\n};\n\nasync fn run_turn(\n    harness: &Harness,\n    prompt: String,\n    cancel: CancellationToken,\n) -> Result<TurnOutcome, HandleError> {\n    harness.session().run(\n        RunInput { prompt, images: Vec::new() },\n        cancel,\n    ).await\n}',
          },
        ],
        apis: [
          {
            name: 'RunInput',
            signature: 'pub struct RunInput { pub prompt: String, pub images: Vec<PathBuf> }',
            description: t('Cloneable, defaultable input for the existing agent execution path.', '可克隆且有默认值的智能体执行输入。'),
          },
          {
            name: 'SessionHandle::run',
            signature: 'pub async fn run(&self, input: RunInput, cancel: CancellationToken) -> Result<TurnOutcome, HandleError>',
            description: t('Execute a turn through the composition’s existing agent loop.', '通过组合中既有的智能体循环执行轮次。'),
          },
        ],
      },
      {
        id: 'outcomes',
        title: t('Cancellation & outcomes', '取消与结果'),
        paragraphs: [
          t(
            'CancellationToken is re-exported from tokio-util. Keep a clone in the host, pass the token into run, and call cancel() on the host’s clone when the user interrupts. Await the run future so the existing execution path can unwind before releasing its resources.',
            'CancellationToken 从 tokio-util 重新导出。宿主保留一个副本，将令牌传给 run，并在用户中断时调用宿主副本的 cancel()。等待 run future 完成，让既有执行路径完成退出后再释放资源。',
          ),
        ],
        apis: [
          {
            name: 'TurnOutcome',
            signature: 'pub enum TurnOutcome { EndTurn, MaxTurns, Cancelled }',
            description: t('Distinguish a natural model stop, the turn budget being reached, and cancellation.', '区分模型自然结束、达到轮次预算和取消。'),
          },
          {
            name: 'HandleError',
            signature: 'pub enum HandleError { ActionFailed(String), Unimplemented(String) }',
            description: t('Coarse host-handle failures. The string carries the human-readable reason.', '宿主句柄的粗粒度失败类型，字符串携带可读原因。'),
          },
        ],
        note: t(
          'Cancellation is part of the runtime execution contract. A cancellation outcome is distinct from proof that every external side effect has been rolled back.',
          '取消是运行时执行契约的一部分。取消结果不代表所有外部副作用均已回滚。',
        ),
      },
      {
        id: 'output',
        title: t('Observe streamed output', '接收流式输出'),
        paragraphs: [
          t(
            'run returns a terminal outcome; streaming output goes to the OutputStream supplied when the runtime is composed. The interface includes text, tool call/result, message boundary, notices, and lifecycle callbacks. Hosts that need client DTOs can use client::adapter::AdapterOutputStream.',
            'run 返回终态结果；流式输出发送到组装运行时时提供的 OutputStream。该接口包含文本、工具调用与结果、消息边界、通知和生命周期回调。需要客户端 DTO 的宿主可使用 client::adapter::AdapterOutputStream。',
          ),
        ],
        apis: [
          {
            name: 'OutputStream::emit_text',
            signature: 'async fn emit_text(&self, text: &Utf16JsonProjection)',
            description: t('Deliver assistant text to the host’s output destination.', '将助手文本交给宿主输出目标。'),
          },
        ],
      },
      {
        id: 'snapshots',
        title: t('Read session snapshots', '读取会话快照'),
        apis: [
          {
            name: 'SessionHandle::id',
            signature: 'pub async fn id(&self) -> SessionId',
            description: t('Identity of the currently mounted session.', '当前挂载会话的标识。'),
          },
          {
            name: 'SessionHandle::transcript',
            signature: 'pub async fn transcript(&self) -> Vec<ConversationMessage>',
            description: t('Read the existing ordered conversation transcript.', '读取既有的有序对话记录。'),
          },
          {
            name: 'SessionHandle::cost',
            signature: 'pub async fn cost(&self) -> CostSnapshot',
            description: t('Read cumulative usage and cost, including token counts and API duration.', '读取累计用量与成本，包括 token 数量和 API 耗时。'),
          },
        ],
        note: t(
          'The facade exposes the mounted session. Creating, listing, resuming, or switching sessions belongs to the host composition and client commands.',
          '公共门面访问已挂载的会话。创建、列出、恢复或切换会话由宿主组合及客户端命令负责。',
        ),
      },
    ],
  },
  {
    id: 'harness-lifecycle',
    group: 'harness',
    title: t('Host lifecycle & persistence', '宿主生命周期与持久化'),
    description: t(
      'Compose host services and shut down through the runtime’s durability barriers.',
      '组合宿主服务，并通过运行时持久化屏障完成关闭。',
    ),
    packageName: 'harness-runtime',
    sourceUrl: source('crates/runtime/src/desktop/sdk.rs'),
    sources: sources('crates/runtime/src/api.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/runtime/src/desktop/session_state.rs', 'crates/runtime/src/mobile/host.rs', 'crates/runtime/src/mobile/host/assembly.rs', 'crates/runtime/src/headless/mod.rs', 'crates/runtime/src/headless/config.rs', 'crates/runtime/src/headless/host.rs', 'crates/runtime/src/headless/io.rs', 'crates/runtime/src/headless/session.rs', 'crates/runtime/src/desktop/configuration.rs', 'crates/permission/src/gate.rs', 'crates/session/src/storage.rs', 'crates/session/src/resumer.rs'),
    sections: [
      {
        id: 'host-services',
        title: t('Implement host services', '实现宿主服务'),
        paragraphs: [
          t(
            'SessionService and LifecycleService are Send + Sync async traits. Use services from one runtime composition so the session handle and shutdown owner agree about the resources being drained. Service implementations retain their execution, permission, and persistence rules.',
            'SessionService 与 LifecycleService 是 Send + Sync 异步 trait。服务应来自同一运行时组合，让会话句柄与关闭负责人对需要排空的资源保持一致。服务实现保留自身的执行、权限和持久化规则。',
          ),
        ],
        apis: [
          {
            name: 'SessionService::run',
            signature: 'async fn run(&self, input: RunInput, cancel: CancellationToken) -> Result<TurnOutcome, HandleError>',
            description: t('The host-provided execution method behind SessionHandle::run.', 'SessionHandle::run 背后的宿主执行方法。'),
          },
          {
            name: 'SessionService::session_id',
            signature: 'async fn session_id(&self) -> SessionId',
            description: t('Report the session mounted by this composition.', '报告该组合当前挂载的会话。'),
          },
          {
            name: 'SessionService::transcript',
            signature: 'async fn transcript(&self) -> Vec<ConversationMessage>',
            description: t('Read the ordered transcript from the composition.', '读取组合的有序对话记录。'),
          },
          {
            name: 'SessionService::cost',
            signature: 'async fn cost(&self) -> CostSnapshot',
            description: t('Read the composition’s cumulative model usage and cost.', '读取组合的累计模型用量与成本。'),
          },
          {
            name: 'LifecycleService::shutdown',
            signature: 'async fn shutdown(&self) -> ShutdownReport',
            description: t('Drain resources through the composition’s shutdown coordinator.', '通过组合的关闭协调器排空资源。'),
          },
        ],
      },
      {
        id: 'platform-assembly',
        title: t('Use the platform composition', '使用平台组合'),
        paragraphs: [
          t(
            'desktop::build_harness injects the host’s OutputStream and PermissionGate into the production desktop composition and returns the shared Harness facade. Unlike HarnessBuilder, platform assembly has explicit startup I/O and configuration semantics.',
            'desktop::build_harness 将宿主的 OutputStream 和 PermissionGate 注入生产桌面组合，并返回共享 Harness 门面。平台组装具有明确的启动 I/O 与配置行为，与 HarnessBuilder 的包装过程不同。',
          ),
          t(
            'mobile::build_mobile takes MobileConfig, an Arc<dyn Platform>, a ClientEventListener, and a PermissionRequestSink. It returns MobileRuntime; device capabilities and platform services are supplied through Platform.',
            'mobile::build_mobile 接收 MobileConfig、Arc<dyn Platform>、ClientEventListener 和 PermissionRequestSink，返回 MobileRuntime。设备能力与平台服务通过 Platform 注入。',
          ),
          t(
            'The desktop wrapper below requires the companion permission crate from the same Harness source identity. The complete consumer manifest includes that dependency. Headless uses the same desktop feature and accepts explicit host services, byte I/O and signals; it does not install signal handlers, take over global stdio or call process::exit.',
            '下方桌面包装器需要来自同一 Harness 源码身份的配套 permission crate；完整消费者清单已包含它。Headless 使用同一 desktop feature，接收显式宿主服务、字节 I/O 与信号，不安装信号处理器、接管全局 stdio 或调用 process::exit。',
          ),
        ],
        code: [
          {
            label: t('Rust · desktop assembly with a host permission gate', 'Rust · 带宿主权限门的桌面组装'),
            language: 'rust', verification: 'compile', exampleId: 'harness-desktop-build',
            code: `use std::sync::Arc;
use harness_runtime::{
    desktop::{self, BuildError, DesktopConfig},
    Harness, OutputStream,
};
use permission::gate::PermissionGate;

async fn desktop_harness(
    config: DesktopConfig,
    output: Arc<dyn OutputStream>,
    permissions: Arc<dyn PermissionGate>,
) -> Result<Harness, BuildError> {
    desktop::build_harness(config, output, permissions).await
}`,
          },
          {
            label: t('Rust · headless run with host-owned byte streams', 'Rust · 使用宿主字节流的 Headless 执行'),
            language: 'rust', verification: 'compile', exampleId: 'harness-headless-run',
            code: `use std::sync::Arc;
use harness_runtime::{
    desktop::DesktopConfig,
    headless::{self, HeadlessConfig, HeadlessExit, HeadlessHostServices,
        HeadlessIo, HeadlessOptions, HeadlessSignals, SessionStart},
};

async fn headless_prompt(
    desktop: DesktopConfig,
    services: Arc<dyn HeadlessHostServices>,
    io: HeadlessIo,
    signals: HeadlessSignals,
    prompt: String,
) -> HeadlessExit {
    headless::run(
        HeadlessConfig {
            desktop,
            options: HeadlessOptions { prompt: Some(prompt), ..Default::default() },
            session_start: SessionStart::New,
        },
        services,
        io,
        signals,
    ).await
}`,
          },
        ],
        note: t(
          'HeadlessIo::new takes injected AsyncRead and stdout/stderr AsyncWrite implementations. Text input reads to EOF; use an empty input reader for a prompt-only call, or explicitly mark a terminal input. Keep the sender returned by HeadlessSignals::channel to forward host interrupts/shutdown. Inspect HeadlessExit.execution, shutdown and delivery independently; compilation does not execute a model turn.',
          'HeadlessIo::new 接收注入的 AsyncRead 和 stdout／stderr AsyncWrite 实现。文本输入读取至 EOF；仅提示词调用使用空输入 reader，终端输入则显式标记。保留 HeadlessSignals::channel 返回的 sender，转发宿主中断／关闭。分别检查 HeadlessExit.execution、shutdown 与 delivery；编译示例不会执行模型轮次。',
        ),
        apis: [
          {
            name: 'desktop::build_harness',
            signature: 'pub async fn build_harness(config: DesktopConfig, output: Arc<dyn OutputStream>, permissions: Arc<dyn PermissionGate>) -> Result<Harness, BuildError>',
            description: t('Assemble an embedded desktop runtime with a host permission gate. Requires desktop.', '使用宿主权限门组装嵌入式桌面运行时，需启用 desktop。'),
          },
          {
            name: 'mobile::build_mobile',
            signature: 'pub async fn build_mobile(cfg: MobileConfig, platform: Arc<dyn Platform>, listener: Arc<dyn ClientEventListener>, permission_sink: Arc<dyn PermissionRequestSink>) -> Result<MobileRuntime, MobileBuildError>',
            description: t('Assemble the shared mobile runtime from explicit host inputs. Requires mobile.', '从明确的宿主输入组装共享移动运行时，需启用 mobile。'),
          },
        ],
      },
      {
        id: 'shutdown',
        title: t('Drain before closing', '排空后关闭'),
        paragraphs: [
          t(
            'First stop admitting new work, then await outstanding turns. Call Harness::shutdown only after that host barrier. Inspect complete and errors; if shutdown is incomplete, handle the failures and retry sequentially. The desktop implementation delegates to its existing session lifecycle coordinator.',
            '先停止接收新任务，再等待正在执行的轮次。完成宿主屏障后调用 Harness::shutdown。检查 complete 和 errors；关闭未完成时处理失败并顺序重试。桌面实现委托给既有会话生命周期协调器。',
          ),
        ],
        code: [
          {
            label: 'Rust · after active turns have drained',
            language: 'rust',
            verification: 'compile',
            exampleId: 'harness-shutdown',
            code: 'use harness_runtime::{Harness, ShutdownReport};\n\nasync fn shutdown_after_turns(harness: &Harness) -> ShutdownReport {\n    harness.shutdown().await\n}',
          },
        ],
        apis: [
          {
            name: 'Harness::shutdown',
            signature: 'pub async fn shutdown(&self) -> ShutdownReport',
            description: t('Request resource drain from the injected lifecycle service.', '请求注入的生命周期服务排空资源。'),
          },
          {
            name: 'ShutdownReport',
            signature: 'pub struct ShutdownReport { pub complete: bool, pub errors: Vec<String>, pub publications: Vec<lingxi_core::host::FusionPublicationReceipt> }',
            description: t('Report shutdown barriers, retryable failures, and durable Fusion publication receipts.', '报告关闭屏障、可重试失败及持久化 Fusion 发布回执。'),
          },
        ],
      },
      {
        id: 'persistence',
        title: t('Persistence belongs to the composition', '持久化由组合负责'),
        paragraphs: [
          t(
            'The session crate exports JsonlWriter, JsonlReader, SessionStorage, SessionResumer, and RolloutRecorder. The facade does not introduce a separate storage format. SessionStorage appends transcripts under an exclusive flock and fsync; its load method recovers the stored conversation.',
            'session crate 导出 JsonlWriter、JsonlReader、SessionStorage、SessionResumer 和 RolloutRecorder。公共门面不会引入独立存储格式。SessionStorage 在独占 flock 下追加对话记录并执行 fsync；load 方法恢复已保存的对话。',
          ),
          t(
            'SessionResumer restores stored messages with an empty file-state cache so files are read again instead of trusting historical reads. Reattaching permissions, plugins, MCP connections, and other live services remains a host responsibility. The production JsonlWriter and rollout format are separate lower-level surfaces; consult the session source before selecting one.',
            'SessionResumer 恢复已保存消息时创建空文件状态缓存，让文件重新读取而不是信任历史读取。重新挂载权限、插件、MCP 连接等实时服务仍是宿主职责。生产 JsonlWriter 与 rollout 格式是独立的底层接口，选择前请查阅 session 源码。',
          ),
        ],
        note: t(
          'Transcript snapshots, event delivery, and durable writes are different boundaries. Await the composition’s durability and shutdown barriers before treating resources as closed.',
          '对话快照、事件交付和持久化写入是不同边界。确认资源关闭前，应等待组合的持久化与关闭屏障。',
        ),
      },
    ],
  },
  {
    id: 'harness-tools',
    group: 'harness',
    title: t('Tools & permissions', '工具与权限'),
    description: t(
      'Implement tools using the shared contract and register their capabilities.',
      '通过共享契约实现工具，并注册其能力。',
    ),
    packageName: 'tool-api',
    sourceUrl: source('crates/tool-api/src/tool_trait.rs'),
    sources: sources('crates/tool-api/src/tool_trait.rs', 'crates/tool-api/src/context.rs', 'crates/tool-api/src/registry.rs', 'crates/tool-api/src/progress.rs', 'crates/orchestrator/src/turn_loop/tool_dispatch.rs', 'crates/core/src/host/permission_gate.rs', 'crates/hooks/src/definition.rs', 'crates/hooks/src/events.rs', 'crates/hooks/src/registry.rs', 'crates/mcp/src/registry.rs'),
    sections: [
      {
        id: 'tool-contract',
        title: t('The Tool contract', 'Tool 契约'),
        paragraphs: [
          t(
            'tool-api defines the abstract contract used by tool implementations and execution components. It exposes Tool, ToolUseContext, ToolRegistry, ToolCallResult, validation errors, progress channels, and dispatch support. These are companion crate exports, rather than methods on Harness.',
            'tool-api 定义工具实现与执行组件共用的抽象契约，提供 Tool、ToolUseContext、ToolRegistry、ToolCallResult、校验错误、进度通道和调度支持。它们由配套 crate 导出，不是 Harness 的方法。',
          ),
          t(
            'A tool advertises its canonical name, JSON input schema, enablement, result size, and scheduling hints. Before execution, the dispatcher validates input and computes permissions. Hooks may affect that permission decision. Implementing call alone is not the complete Tool implementation.',
            '工具声明规范名称、JSON 输入 schema、启用状态、结果大小和调度提示。执行前，调度器校验输入并计算权限，hooks 可影响权限决策。仅实现 call 并不构成完整的 Tool 实现。',
          ),
        ],
        apis: [
          {
            name: 'Tool::name',
            signature: 'fn name(&self) -> &str',
            description: t('Canonical dispatcher identity for the tool.', '调度器使用的工具规范标识。'),
          },
          {
            name: 'Tool::input_schema',
            signature: 'fn input_schema(&self) -> &Utf16JsonProjection',
            description: t('Advertise the tool’s input JSON Schema.', '声明工具输入的 JSON Schema。'),
          },
          {
            name: 'Tool::validate_input',
            signature: 'async fn validate_input(&self, input: &Utf16JsonProjection, ctx: &ToolUseContext) -> Result<(), ValidationError>',
            description: t('Validate tool-specific conditions before side effects; the default implementation succeeds.', '在副作用前校验工具专用条件；默认实现通过校验。'),
          },
          {
            name: 'Tool::check_permissions',
            signature: 'async fn check_permissions(&self, input: &Utf16JsonProjection, ctx: &ToolUseContext) -> permission::PermissionResult',
            description: t('Compute the invocation-specific permission decision before dispatch.', '在调度前计算本次调用的权限决策。'),
          },
          {
            name: 'Tool::call',
            signature: 'async fn call(&self, input: Utf16JsonProjection, ctx: ToolUseContext, progress_tx: ToolProgressSender) -> Result<ToolCallResult, ToolError>',
            description: t('Execute validated, permission-checked input and return the model-facing result.', '执行已校验并经权限检查的输入，返回模型可见结果。'),
          },
        ],
      },
      {
        id: 'registry',
        title: t('Register tools before sharing', '共享前注册工具'),
        paragraphs: [
          t(
            'ToolRegistry stores builtin, MCP, LSP, and plugin partitions. Share it with Arc instead of cloning the registry. Builtins are registered through &mut self; MCP registration uses interior synchronization so a server can refresh its tools after the registry is shared.',
            'ToolRegistry 包含内置、MCP、LSP 和插件分区。通过 Arc 共享注册表，不直接克隆。内置工具通过 &mut self 注册；MCP 注册使用内部同步，让服务器在注册表共享后更新工具。',
          ),
        ],
        code: [
          {
            label: 'Rust · existing Tool implementation required',
            language: 'rust',
            verification: 'compile',
            exampleId: 'harness-tool-registry',
            code: 'use std::sync::Arc;\nuse tool_api::{Tool, ToolRegistry};\n\nfn registry_with(tool: Arc<dyn Tool>) -> Arc<ToolRegistry> {\n    let mut registry = ToolRegistry::new();\n    registry.register_builtin(tool);\n    Arc::new(registry)\n}',
          },
        ],
        apis: [
          {
            name: 'ToolRegistry::register_builtin',
            signature: 'pub fn register_builtin(&mut self, tool: Arc<dyn Tool>)',
            description: t('Add a builtin before sharing the registry.', '在共享注册表前添加内置工具。'),
          },
          {
            name: 'ToolRegistry::available_tools',
            signature: 'pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>>',
            description: t('Read the enabled, filtered tool pool in its deterministic advertised order.', '按确定性的声明顺序读取已启用并经过滤的工具池。'),
          },
          {
            name: 'ToolRegistry::register_mcp_tools',
            signature: 'pub fn register_mcp_tools(&self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>)',
            description: t('Register or refresh the tools attached to an MCP connection.', '注册或更新某一 MCP 连接的工具。'),
          },
          {
            name: 'ToolRegistry::unregister_mcp_tools',
            signature: 'pub fn unregister_mcp_tools(&self, conn_id: McpConnectionId)',
            description: t('Remove the tool partition for a disconnected MCP connection.', '删除已断开的 MCP 连接的工具分区。'),
          },
        ],
      },
      {
        id: 'invocation-context',
        title: t('Invocation context & progress', '调用上下文与进度'),
        paragraphs: [
          t(
            'ToolUseContext carries invocation options, conversation history, tool-use and assistant-message identities, optional session state, origin-session attribution, and an optional per-call cancellation token. Respect the supplied cancellation and host capabilities when implementing operations.',
            'ToolUseContext 携带调用选项、对话历史、工具调用与助手消息标识、可选会话状态、来源会话归属，以及可选的逐调用取消令牌。实现操作时应遵循所提供的取消状态与宿主能力。',
          ),
        ],
        apis: [
          {
            name: 'progress_channel',
            signature: 'pub fn progress_channel() -> (ToolProgressSender, ToolProgressReceiver)',
            description: t('Create a bounded 64-slot channel for tool-defined progress JSON.', '创建容量为 64 的有界通道，传递工具定义的进度 JSON。'),
          },
          {
            name: 'ToolProgress',
            signature: 'pub struct ToolProgress { pub tool_use_id: lingxi_core::types::ToolUseId, pub data: Utf16JsonProjection }',
            description: t('Correlate one progress payload with its originating tool use.', '将一个进度载荷与来源工具调用关联。'),
          },
        ],
      },
      {
        id: 'capability-filtering',
        title: t('Filter session capabilities', '过滤会话能力'),
        apis: [
          {
            name: 'ToolRegistry::set_session_tool_allowlist',
            signature: 'pub fn set_session_tool_allowlist(&mut self, tools: &[String])',
            description: t('Set an explicit builtin capability allowlist and hide all dynamic partitions. Canonical builtin names govern the gate; aliases do not bypass it.', '设置明确的内置能力允许列表并隐藏所有动态分区。权限门依据规范工具名称，别名无法绕过。'),
          },
        ],
        note: t(
          'Tool visibility and per-call permission checks are separate controls. Desktop embedded hosts inject PermissionGate during assembly; client transports can resolve interactive requests through AdapterPermissionGate.',
          '工具可见性与逐调用权限检查是两个控制层。嵌入式桌面宿主在组装时注入 PermissionGate；客户端传输可通过 AdapterPermissionGate 处理交互式权限请求。',
        ),
      },
    ],
  },
  {
    id: 'harness-plugins',
    group: 'harness',
    title: t('Plugins', '插件'),
    description: t(
      'Load plugin components, inspect their lifecycle, and handle unload failures.',
      '加载插件组件、检查生命周期并处理卸载失败。',
    ),
    packageName: 'plugin',
    sourceUrl: source('crates/plugin/src/manager.rs'),
    sources: sources('crates/plugin/src/manager.rs', 'crates/plugin/src/lifecycle.rs', 'crates/plugin/src/manifest.rs', 'crates/plugin/src/discovery.rs'),
    sections: [
      {
        id: 'components',
        title: t('Plugin components', '插件组件'),
        paragraphs: [
          t(
            'The plugin crate exports PluginManifest, PluginComponents, PluginSource, PluginState, PluginManager, strict policy, trust, and discovery helpers. Declared components include commands, agents, skills, hooks, output styles, MCP servers, LSP servers, and workflows. The manager materializes these into the host’s registries.',
            'plugin crate 导出 PluginManifest、PluginComponents、PluginSource、PluginState、PluginManager、严格策略、信任与发现辅助接口。可声明组件包括命令、智能体、skills、hooks、输出样式、MCP 服务器、LSP 服务器和工作流。管理器将其装配到宿主注册表。',
          ),
          t(
            'The host supplies registry, filesystem, and related dependencies when constructing PluginManager. This is a companion crate interface; Harness does not expose a mutable plugin registry.',
            '构建 PluginManager 时，宿主提供注册表、文件系统及相关依赖。这是配套 crate 接口；Harness 不提供可变插件注册表。',
          ),
        ],
      },
      {
        id: 'local-install',
        title: t('Install a prepared local plugin', '安装已准备的本地插件'),
        paragraphs: [
          t(
            'PluginManager::install currently supports PluginSource::LocalPath. It discovers the existing directory in place, reads its manifest and components, enables the plugin, and returns its PluginId. The manager’s Git, marketplace, MCP bundle, and builtin install branches return typed errors. Network materialization belongs to the host’s production install pipeline.',
            'PluginManager::install 当前支持 PluginSource::LocalPath：原地发现现有目录，读取 manifest 与组件，启用插件并返回 PluginId。管理器中的 Git、市场、MCP bundle 和内置插件安装分支返回类型化错误。网络来源的落盘由宿主生产安装流程负责。',
          ),
        ],
        code: [
          {
            label: 'Rust · host-assembled PluginManager required',
            language: 'rust',
            code: 'use std::path::PathBuf;\nuse plugin::{PluginManager, PluginManagerError, PluginSource};\n\nasync fn install_prepared(\n    manager: &PluginManager,\n    directory: PathBuf,\n) -> Result<(), PluginManagerError> {\n    let id = manager.install(\n        PluginSource::LocalPath { path: directory },\n    ).await?;\n    let _state = manager.plugin_state(&id).await;\n    Ok(())\n}',
          },
        ],
        apis: [
          {
            name: 'PluginManager::install',
            signature: 'pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginManagerError>',
            description: t('Discover and enable a local plugin directory using the existing host registries.', '通过既有宿主注册表发现并启用本地插件目录。'),
          },
          {
            name: 'PluginManager::enable',
            signature: 'pub async fn enable(&self, id: &PluginId, manifest: PluginManifest, install_dir: PathBuf) -> Result<(), PluginManagerError>',
            description: t('Load an already materialized manifest and install directory into the registries.', '将已落盘的 manifest 和安装目录加载到注册表。'),
          },
        ],
      },
      {
        id: 'lifecycle',
        title: t('Track plugin lifecycle', '跟踪插件生命周期'),
        bullets: [
          t('Declared → Fetching → Fetched → Loaded describes preparation and activation.', 'Declared → Fetching → Fetched → Loaded 表示准备与激活过程。'),
          t('Disabled means contributed components have been removed. Failed and Blocked retain failure or policy context.', 'Disabled 表示已移除插件贡献的组件。Failed 与 Blocked 保留失败或策略上下文。'),
          t('DisablingFailed preserves a partial unload failure so a later disable pass can retry the remaining teardown.', 'DisablingFailed 保留部分卸载失败，让后续 disable 继续尝试剩余清理。'),
        ],
        apis: [
          {
            name: 'PluginManager::plugin_state',
            signature: 'pub async fn plugin_state(&self, id: &PluginId) -> Option<PluginState>',
            description: t('Read a snapshot of one plugin’s lifecycle state.', '读取某一插件的生命周期状态快照。'),
          },
          {
            name: 'PluginManager::loaded_plugin_ids',
            signature: 'pub async fn loaded_plugin_ids(&self) -> Vec<PluginId>',
            description: t('Include Loaded and DisablingFailed plugins so unload failures remain visible to retry passes.', '包含 Loaded 和 DisablingFailed 插件，让卸载失败对重试流程保持可见。'),
          },
          {
            name: 'PluginManager::disable',
            signature: 'pub async fn disable(&self, id: &PluginId) -> Result<(), PluginManagerError>',
            description: t('Unload contributed components and transition to Disabled, or retain DisablingFailed on teardown error.', '卸载插件贡献组件并进入 Disabled，清理失败时保留 DisablingFailed。'),
          },
        ],
      },
      {
        id: 'policy',
        title: t('Apply host policy', '应用宿主策略'),
        paragraphs: [
          t(
            'Plugin trust, managed marketplace restrictions, strict plugin-only policy, component validation, and user-config resolution participate in loading. Use the existing manager and discovery path so plugin components pass through those controls before they mutate registries.',
            '插件信任、受管市场限制、严格插件策略、组件校验和用户配置解析参与加载。使用既有管理器与发现路径，让插件组件在修改注册表前经过这些控制。',
          ),
        ],
        note: t(
          'A parsed manifest field is not evidence that its optional fetch or runtime behavior is implemented. The linked source identifies the enabled component paths and explicit unsupported branches.',
          'manifest 字段可被解析不代表其可选获取或运行行为已经实现。链接源码列明已启用组件路径与明确不支持的分支。',
        ),
      },
    ],
  },
  {
    id: 'harness-client',
    group: 'harness',
    title: t('Client protocol & adapters', '客户端协议与适配器'),
    description: t(
      'Share commands, events, presentation, and runtime adapters across clients.',
      '在客户端之间共享命令、事件、展示与运行时适配器。',
    ),
    packageName: 'client',
    sourceUrl: source('crates/client/src/lib.rs'),
    sources: sources('crates/client/src/lib.rs', 'crates/client/Cargo.toml', 'crates/client/src/protocol/version.rs', 'crates/client/src/protocol/commands.rs', 'crates/client/src/protocol/events.rs', 'crates/client/src/adapter/output_stream.rs', 'crates/client/src/protocol/message.rs', 'crates/client/src/adapter/turn.rs', 'crates/client/src/adapter/permission_gate.rs', 'crates/client/src/protocol/permission.rs'),
    sections: [
      {
        id: 'modules',
        title: t('Three client modules', '三个客户端模块'),
        paragraphs: [
          t(
            'The companion Rust package is client. protocol provides transport-independent commands, events, DTOs, and a protocol version. presentation contains shared tool summaries, structured diffs, text rendering, and styles. adapter translates runtime output and interactive requests into client DTOs.',
            '配套 Rust 包名为 client。protocol 提供不依赖传输方式的命令、事件、DTO 与协议版本。presentation 提供共享工具摘要、结构化 diff、文本渲染和样式。adapter 将运行时输出与交互请求转为客户端 DTO。',
          ),
          t(
            'The default feature set exposes protocol only. presentation enables rendering helpers; adapter includes presentation and runtime integration. uniffi adds native metadata for the enabled modules. test-support enables adapter and MockSink and belongs on development dependencies.',
            '默认功能集仅提供 protocol。presentation 启用展示辅助功能；adapter 包含 presentation 与运行时集成。uniffi 为已启用模块增加原生元数据。test-support 启用 adapter 和 MockSink，应放在开发依赖中。',
          ),
        ],
        code: [
          {
            label: t('Cargo.toml · same local source as the runtime', 'Cargo.toml · 与运行时一致的本地来源'),
            language: 'toml',
            verification: 'excerpt',
            code: '[dependencies]\nclient = { path = "../harness-runtime/crates/client", features = ["adapter"] }',
          },
        ],
      },
      {
        id: 'wire-contract',
        title: t('Commands & events', '命令与事件'),
        paragraphs: [
          t(
            'ClientCommand and ClientEvent use internally tagged snake_case JSON with a type field and are non-exhaustive Rust enums. Live commands target the session mounted on the connection. ResumeSession names a target session; normal prompt submission does not carry session_id.',
            'ClientCommand 和 ClientEvent 使用 type 字段内部标记、snake_case JSON，是非穷尽 Rust enum。实时命令作用于连接已挂载的会话。ResumeSession 指定目标会话，普通提示词提交不携带 session_id。',
          ),
          t(
            'SendPrompt carries text, optional prompt_mode and turn_id, and an images vector. Cancel accepts an optional turn_id. The event feed includes TextDelta, ToolUseStarted, ToolUseResult, MessageComplete, TurnStarted, TurnEnded, session lifecycle events, and structured errors.',
            'SendPrompt 携带 text、可选 prompt_mode 与 turn_id，以及 images 数组。Cancel 接收可选 turn_id。事件流包含 TextDelta、ToolUseStarted、ToolUseResult、MessageComplete、TurnStarted、TurnEnded、会话生命周期事件和结构化错误。',
          ),
        ],
        code: [
          {
            label: 'ClientCommand JSON · SendPrompt',
            language: 'json',
            code: '{\n  "type": "send_prompt",\n  "text": "Explain this project",\n  "images": [],\n  "turn_id": 1\n}',
          },
          {
            label: 'ClientCommand JSON · Cancel',
            language: 'json',
            code: '{\n  "type": "cancel",\n  "turn_id": 1\n}',
          },
        ],
        apis: [
          {
            name: 'CLIENT_PROTOCOL_VERSION',
            signature: 'pub const CLIENT_PROTOCOL_VERSION: &str = "29.0.0";',
            description: t('Protocol version declared in this inspected source tree. Native clients must version-lock generated binding layouts with the host they actually build.', '此检查源码树声明的协议版本。原生客户端生成绑定的布局应与实际构建的宿主版本一致。'),
          },
        ],
        note: t(
          'These JSON examples show the command payload, not a transport envelope. Bridge frames and native binding entrypoints are defined by their respective hosts.',
          '这些 JSON 示例表示命令载荷，不包含传输封装。桥接帧与原生绑定入口由各自宿主定义。',
        ),
      },
      {
        id: 'output-adapter',
        title: t('Connect an event destination', '连接事件目标'),
        paragraphs: [
          t(
            'ClientEventSink is the shared outbound seam. AdapterOutputStream implements the runtime OutputStream and lowers its callbacks into ClientEvent values. The sink can enqueue events for a WebSocket transport or forward them through a mobile ListenerSink; adapter code remains independent of the transport.',
            'ClientEventSink 是共享输出边界。AdapterOutputStream 实现运行时 OutputStream，并把回调转换为 ClientEvent。sink 可将事件加入 WebSocket 传输队列，或通过移动端 ListenerSink 转发；适配器代码不依赖具体传输。',
          ),
        ],
        code: [
          {
            label: 'Rust · host-supplied event sink',
            language: 'rust',
            verification: 'compile',
            exampleId: 'harness-output-adapter',
            code: 'use std::sync::Arc;\nuse client::adapter::{AdapterOutputStream, ClientEventSink};\n\nfn make_output(sink: Arc<dyn ClientEventSink>) -> AdapterOutputStream {\n    AdapterOutputStream::new(sink)\n}',
          },
        ],
        apis: [
          {
            name: 'ClientEventSink::emit',
            signature: 'async fn emit(&self, event: client::protocol::events::ClientEvent)',
            description: t('Forward one lowered event. Keep delivery cheap so transport work does not stall execution.', '转发一个已转换事件。保持交付轻量，避免传输工作阻塞执行。'),
          },
          {
            name: 'AdapterOutputStream::new',
            signature: 'pub fn new(sink: Arc<dyn ClientEventSink>) -> Self',
            description: t('Create a connection-scoped output adapter around the shared sink.', '围绕共享 sink 创建连接级输出适配器。'),
          },
          {
            name: 'ClientEventListener::on_event',
            signature: 'async fn on_event(&self, event: client::protocol::events::ClientEvent)',
            description: t('Mobile host callback for each client event; exposed as a callback interface when uniffi is enabled.', '每个客户端事件的移动宿主回调；启用 uniffi 时作为回调接口导出。'),
          },
        ],
      },
      {
        id: 'turn-events',
        title: t('Turn events & interactive requests', '轮次事件与交互请求'),
        paragraphs: [
          t(
            'TurnEventEmitter emits turn-start and completion/error events around a result supplied by the caller. It does not execute a turn. AdapterPermissionGate, AskUserQuestionBroker, and ComputerAccessBroker provide connection-scoped interactive request handling; hosts still own execution and response routing.',
            'TurnEventEmitter 围绕调用方提供的结果发出轮次开始、完成或错误事件。它不会执行轮次。AdapterPermissionGate、AskUserQuestionBroker 和 ComputerAccessBroker 提供连接级交互请求处理；执行与响应路由仍由宿主负责。',
          ),
        ],
        apis: [
          {
            name: 'TurnEventEmitter::new',
            signature: 'pub fn new(sink: Arc<dyn ClientEventSink>) -> Self',
            description: t('Use the same event destination as the output and permission adapters.', '使用与输出及权限适配器一致的事件目标。'),
          },
          {
            name: 'TurnEventEmitter::emit_turn_started',
            signature: 'pub async fn emit_turn_started(&self, turn_id: Option<u64>)',
            description: t('Emit a correlated start event for a host-admitted turn.', '为宿主接收的轮次发送带关联标识的开始事件。'),
          },
          {
            name: 'TurnEventEmitter::complete',
            signature: 'pub async fn complete(&self, result: Result<orchestrator::streaming_loop::PumpedTurn, orchestrator::OrchestratorError>)',
            description: t('Translate a pumped turn result into message completion or error events.', '将流式轮次结果转换为消息完成或错误事件。'),
          },
        ],
        note: t(
          'Client DTO compatibility does not establish device or transport correctness. Validate generated bindings, event routing, permissions, and shutdown in the target host.',
          '客户端 DTO 兼容不等于设备或传输行为正确。应在目标宿主中验证生成绑定、事件路由、权限与关闭流程。',
        ),
      },
    ],
  },
];

const integrationSections: Record<string, DocSection[]> = {
  harness: [
    {
      id: 'embedding-contract',
      title: t('Host admission, resources and failures', '宿主准入、资源与失败'),
      paragraphs: [
        t('Use a Tokio runtime whose worker stack is at least harness_runtime::RUNTIME_THREAD_STACK_SIZE (8 MiB). Platform assembly loads configuration, constructs registries and starts owned services; it can fail before a session is available. The facade constructor alone cannot validate credentials or platform readiness.', '使用工作线程栈至少为 harness_runtime::RUNTIME_THREAD_STACK_SIZE（8 MiB）的 Tokio runtime。平台组装加载配置、构建注册表并启动受其管理的服务，可能在会话可用前失败。仅构建门面无法验证凭据或平台就绪状态。'),
        t('Choose desktop::build_harness for a Rust host that owns its permission surface, mobile::build_mobile for explicit device services, or headless::run for byte-oriented noninteractive execution. Headless is enabled by desktop and reports execution, durable shutdown and output delivery separately.', 'Rust 宿主自己提供权限界面时选择 desktop::build_harness；显式设备服务选择 mobile::build_mobile；按字节进行非交互执行选择 headless::run。Headless 由 desktop 启用，分别报告执行、持久化关闭和输出交付。'),
      ],
      links: [{ label: t('Agent execution and recovery', 'Agent 执行与恢复'), href: '/docs/agent-execution' }],
      note: t('Existing sdk_surface tests check the opaque SDK contract; platform composition and real provider execution require their own checks. A build result is not a live model or device acceptance result.', '现有 sdk_surface 测试检查不透明 SDK 契约；平台组装与真实供应商执行需要各自验证。构建结果不是模型服务或设备验收结果。'),
    },
  ],
  'harness-sessions': [
    {
      id: 'session-errors', title: t('Diagnose a failed turn', '诊断失败轮次'),
      steps: [
        t('Record the mounted session ID and host turn_id before admission. Keep model/provider profile and request IDs as separate identities.', '准入前记录已挂载 session ID 与宿主 turn_id。模型／供应商 profile 和 request ID 使用各自独立标识。'),
        t('Consume output as it arrives; terminal outcomes do not reconstruct discarded deltas. Correlate a tool result with its tool_use_id rather than its displayed name.', '随输出到达而消费；终态结果无法重建已丢弃的增量。用 tool_use_id 关联工具结果，不用显示名称。'),
        t('On interruption, cancel the owned token and await run. Preserve partial output and the returned outcome or error before permitting another session operation.', '中断时取消所属令牌并等待 run。允许其他会话操作前，保留部分输出及返回结果或错误。'),
        t('For cold resume, load durable session state through the host composition. An ID plus an empty new runtime is not a restored transcript.', '冷恢复时，通过宿主组合加载持久化会话状态。一个 ID 加上空的新 runtime 并不等于恢复后的对话。'),
      ],
      note: t('Streaming protocol checks include truncated streams and tool-result pairing. Separate tests must cover output delivery, permission response routing and durable publication; TurnOutcome alone establishes none of those barriers.', '流协议检查包含截断流与工具结果配对。输出交付、权限响应路由和持久化发布需分别测试；TurnOutcome 单独不能证明这些屏障。'),
    },
  ],
  'harness-lifecycle': [
    {
      id: 'mock-lifecycle', title: t('Exercise the copied facade without a provider', '无需供应商即可验证复制的门面示例'),
      paragraphs: [t('Put this complete test module in the consumer library. Add async-trait = "0.1" and tokio = { version = "1", features = ["macros", "rt"] } to dev-dependencies. It tests service injection, mounted identity, cancellation and shutdown; it does not simulate a model or a filesystem.', '将以下完整测试模块放入消费者库。在 dev-dependencies 中加入 async-trait = "0.1" 与 tokio = { version = "1", features = ["macros", "rt"] }。它验证服务注入、挂载标识、取消和关闭，不模拟模型或文件系统。')],
      code: [{
        label: t('Rust · complete mock lifecycle test', 'Rust · 完整模拟生命周期测试'), language: 'rust', verification: 'mock', exampleId: 'harness-lifecycle-mock',
        code: `#[cfg(test)]
mod lifecycle_example {
    use std::sync::Arc;
    use harness_runtime::{CancellationToken, ConversationMessage, CostSnapshot,
        HandleError, HarnessBuilder, LifecycleService, RunInput, SessionId,
        SessionService, ShutdownReport, TurnOutcome};

    struct Host { id: SessionId }

    #[async_trait::async_trait]
    impl SessionService for Host {
        async fn run(&self, _input: RunInput, cancel: CancellationToken)
            -> Result<TurnOutcome, HandleError> {
            Ok(if cancel.is_cancelled() { TurnOutcome::Cancelled }
                else { TurnOutcome::EndTurn })
        }
        async fn session_id(&self) -> SessionId { self.id }
        async fn transcript(&self) -> Vec<ConversationMessage> { Vec::new() }
        async fn cost(&self) -> CostSnapshot { CostSnapshot::default() }
    }

    #[async_trait::async_trait]
    impl LifecycleService for Host {
        async fn shutdown(&self) -> ShutdownReport {
            ShutdownReport { complete: true, ..Default::default() }
        }
    }

    #[tokio::test]
    async fn facade_cancellation_and_shutdown() {
        let host = Arc::new(Host { id: SessionId::new() });
        let harness = HarnessBuilder::new(host.clone(), host.clone()).build();
        assert_eq!(harness.session().id().await, host.id);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = harness.session().run(RunInput::default(), cancel).await.unwrap();
        assert!(matches!(outcome, TurnOutcome::Cancelled));
        assert!(harness.shutdown().await.complete);
    }
}`,
      }],
      note: t('In production, stop new admissions and await every active run future before shutdown. A failed durability barrier keeps ShutdownReport.complete false; retry the same owner sequentially. Do not replace the live runtime just to hide an incomplete report.', '生产中先停止新准入并等待所有活动 run future，再关闭。持久化屏障失败时 ShutdownReport.complete 保持 false，应对同一负责人顺序重试。不能为了隐藏未完成报告而替换活动 runtime。'),
    },
  ],
  'harness-tools': [
    {
      id: 'dispatch-failures', title: t('Validation, denial and failed side effects', '校验、拒绝与副作用失败'),
      paragraphs: [
        t('The orchestrator resolves canonical identity, validates/coerces input, runs tool policy and hooks, obtains the permission decision, and invokes the tool through tracked dispatch. Do not call a Tool body directly from a provider adapter: that would bypass host authority and the invocation journal.', '编排器解析规范标识、校验／转换输入、执行工具策略与 hooks、取得权限决策，再通过带追踪的调度调用工具。供应商适配器不能直接调用 Tool 主体，否则会绕过宿主权限与调用日志。'),
        t('Schema rejection, permission denial, call failure and interruption are different outcomes. Use ToolError for failed execution and keep observable input redacted through backfill_observable_input; ToolCallResult supplies the model-facing payload. A failed call may have already changed an external system, so retry policy must belong to the operation.', 'schema 拒绝、权限拒绝、调用失败和中断是不同结果。执行失败使用 ToolError，并通过 backfill_observable_input 对可观察输入脱敏；ToolCallResult 提供模型可见载荷。失败调用可能已改变外部系统，重试策略应由操作自身决定。'),
      ],
      code: [{ label: t('Existing registry and tool-contract tests', '现有注册表与工具契约测试'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p tool-api --lib\ncargo test --locked -p test-harness --test tools_hooks_e2e' }],
      links: [{ label: t('Extension lifecycle', '扩展生命周期'), href: '/docs/extensions-lifecycle' }],
      note: t('These commands are reproducible checks, not a claim that every registered tool is safe on every device. Device input, shell isolation and external service acceptance are checked in their host.', '这些命令是可重复检查，不代表每个注册工具在所有设备上都已验收。设备输入、shell 隔离与外部服务需要在各自宿主验证。'),
    },
  ],
  'harness-plugins': [
    {
      id: 'teardown-recovery', title: t('Refresh and recover registry ownership', '更新注册表并恢复其归属'),
      paragraphs: [
        t('Enablement contributes components through the production manager; disabling must remove those exact contributions and stop owned workers. A DisablingFailed plugin stays visible to loaded_plugin_ids for retry, but is not authority for replaying a live Mod. Keep the failure and retry teardown before claiming the plugin is unloaded.', '启用经生产管理器贡献组件；禁用必须删除这些对应组件并停止所属 worker。DisablingFailed 插件继续出现在 loaded_plugin_ids 中以便重试，但不具备重放活动 Mod 的权限。保留失败并重试清理，之后才能宣称插件已卸载。'),
        t('A refreshed MCP or LSP catalog is a live registry update, not a second builtin copy. Plugin-supplied workflows enter the same workflow registry used by the runtime launcher. Marketplace parsing, trust policy and successful runtime activation are separate checkpoints.', '更新后的 MCP 或 LSP 目录是活动注册表更新，不是复制一组内置工具。插件工作流进入运行时 launcher 使用的同一个工作流注册表。市场解析、信任策略与运行时成功激活是独立检查点。'),
      ],
      code: [{ label: t('Plugin unit and LSP registration checks', '插件单元与 LSP 注册检查'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p plugin --lib\ncargo test --locked -p lsp --test plugin_only_registration_test' }],
      note: t('Existing tests exercise component cleanup, failure retention and verified-plugin-only LSP registration. Installing an actual third-party package additionally requires host trust, transport and process cleanup acceptance.', '现有测试覆盖组件清理、失败保留及仅允许已验证插件注册 LSP。安装真实第三方包还需验收宿主信任、传输与进程清理。'),
    },
  ],
  'harness-client': [
    {
      id: 'resume-presentation', title: t('Cold resume and compatibility failures', '冷恢复与兼容性失败'),
      paragraphs: [
        t('MessageComplete and resumed history use the same ordered MessageDto block schema. MessageDto.sent_at_ms carries the original persisted Unix time; source transcript UUID mapping supplies historical timestamps instead of matching by text or message position. Inline images and visualization references likewise retain their own identity.', 'MessageComplete 与恢复历史使用同一有序 MessageDto block schema。MessageDto.sent_at_ms 携带原始持久化 Unix 时间；通过源码对话 UUID 映射恢复历史时间，而不是按文本或消息位置匹配。内联图像与可视化引用也保留独立标识。'),
        t('Rust enums are non-exhaustive; hosts handle unknown future variants without inventing successes. Native enum and record layouts require matching generated bindings and the protocol contract gate. A JSON payload that deserializes does not establish a compatible UniFFI binary layout or correct transport handshake.', 'Rust enum 是非穷尽类型；宿主处理未来未知变体时不能伪造成功。原生 enum 与 record 布局需要匹配生成绑定及协议契约检查。JSON 载荷可反序列化不代表 UniFFI 二进制布局兼容或传输握手正确。'),
      ],
      code: [{ label: t('Shared client contracts', '共享客户端契约'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p client --features test-support' }],
      note: t('The inspected client protocol constant is 29.0.0. Always read the baseline and the host pin before connecting a separate client. Device callbacks, ordering under disconnect and replay presentation remain target-host checks.', '本次检查的客户端协议常量为 29.0.0。连接独立客户端前，始终查看基线与宿主固定依赖版本。设备回调、断线时顺序及重放展示仍需目标宿主检查。'),
    },
  ],
};

// Task-oriented recipes complement the signatures above. Their inputs are
// host services; none of these recipes claim to create an authenticated host.
const usageSections: Record<string, DocSection[]> = {
  harness: [
    {
      id: 'choose-entrypoint', title: t('Choose the entry point for your application', '为你的应用选择入口'),
      paragraphs: [t('Start with the host you are building. A chat window, a batch worker and a mobile application have different input, permission and shutdown requirements, even when they use the same agent loop.', '先确定要构建哪一种宿主。聊天窗口、批处理 worker 与移动应用需要不同的输入、权限和关闭方式，即使它们使用同一个 Agent 循环。')],
      tables: [{ headers: [t('You want to', '目标'), t('Start with', '使用入口'), t('Supply from the host', '宿主需要提供')], rows: [
        [t('Embed a Rust chat or desktop UI', '嵌入 Rust 聊天或桌面界面'), t('desktop::build_harness', 'desktop::build_harness'), t('DesktopConfig, OutputStream and PermissionGate', 'DesktopConfig、OutputStream 与 PermissionGate')],
        [t('Run a noninteractive job', '运行非交互任务'), t('headless::run', 'headless::run'), t('HeadlessHostServices, byte readers/writers, and host signals', 'HeadlessHostServices、字节读写流与宿主信号')],
        [t('Integrate Android or iOS', '集成 Android 或 iOS'), t('mobile::build_mobile', 'mobile::build_mobile'), t('MobileConfig, Platform, event listener and permission sink', 'MobileConfig、Platform、事件监听器与权限请求 sink')],
        [t('Wrap an already assembled composition', '包装已组装的运行时'), t('HarnessBuilder::new', 'HarnessBuilder::new'), t('SessionService and LifecycleService from the same composition', '来自同一组合的 SessionService 与 LifecycleService')],
      ] }],
      links: [{ label: t('Platform assembly and shutdown', '平台组装与关闭'), href: '/docs/harness-lifecycle#platform-assembly' }],
    },
    {
      id: 'first-agent-workflow', title: t('Wire your first agent workflow', '接通第一个 Agent 工作流'),
      steps: [
        t('Prepare an application workspace and model/provider configuration. Choose a composition profile and keep all direct Harness dependencies on the same source revision.', '准备应用工作空间与模型／供应商配置。选择组合配置，让直接依赖的 Harness 包保持相同源码版本。'),
        t('Create the output destination before assembling the runtime. If your UI consumes ClientEvent DTOs, wrap one shared ClientEventSink with AdapterOutputStream.', '先创建输出目标，再组装运行时。如果界面消费 ClientEvent DTO，用 AdapterOutputStream 包装一个共享 ClientEventSink。'),
        t('Provide a permission gate that can reach the user or apply an explicit noninteractive policy. Admit a prompt only after the host has a mounted session.', '提供能联系用户、或执行明确非交互策略的权限门。宿主挂载会话后再接收提示词。'),
        t('Pass RunInput and a host-owned CancellationToken to session().run. Read text and tool activity from the output stream while awaiting its terminal result.', '向 session().run 传入 RunInput 与宿主持有的 CancellationToken。等待终态结果的同时，从输出流接收文本和工具活动。'),
        t('Keep the returned outcome, session identity and cost snapshot. When closing the application, stop new prompts, drain active turns and then inspect shutdown().complete.', '保留返回结果、会话标识与成本快照。关闭应用时停止新提示词，等待活动轮次排空，再检查 shutdown().complete。'),
      ],
      links: [
        { label: t('Copy the complete service-injection example', '复制完整服务注入示例'), href: '/docs/harness#builder' },
        { label: t('Run, cancel and inspect a turn', '执行、取消与检查轮次'), href: '/docs/harness-sessions' },
        { label: t('Connect a client event sink', '连接客户端事件 sink'), href: '/docs/harness-client#output-adapter' },
      ],
    },
    {
      id: 'startup-troubleshooting', title: t('Troubleshoot startup', '排查启动问题'),
      tables: [{ headers: [t('Symptom', '现象'), t('Check first', '先检查'), t('Next action', '下一步')], rows: [
        [t('The builder succeeds but nothing runs', '构建器成功，但没有执行任务'), t('HarnessBuilder retains services; it does not submit a prompt.', 'HarnessBuilder 只保留服务，不会提交提示词。'), t('Call the mounted SessionHandle::run and consume its output destination.', '调用已挂载的 SessionHandle::run，并消费它的输出目标。')],
        [t('Text never reaches the interface', '界面收不到文本'), t('OutputStream or ClientEventSink ownership and delivery.', 'OutputStream 或 ClientEventSink 的归属与交付。'), t('Verify the same sink is attached before startup; keep transport writes off the execution callback.', '确认启动前接入同一个 sink，避免在执行回调里阻塞传输写入。')],
        [t('An operation waits on approval', '操作一直等待授权'), t('The host permission request/response route.', '宿主权限请求与响应路由。'), t('Resolve the exact request_id, or supply the intended noninteractive policy before running.', '响应准确的 request_id，或在运行前提供预期的非交互策略。')],
        [t('Shared Rust types do not match', '共享 Rust 类型不兼容'), t('Duplicate Git/path identities in cargo metadata.', 'cargo metadata 中重复的 Git／path 身份。'), t('Use the complete consumer manifest and one reviewed source identity for companion crates.', '使用完整消费者清单，并让配套 crate 采用同一经审查源码身份。')],
      ] }],
    },
  ],
  'harness-sessions': [
    {
      id: 'run-parameters', title: t('Run input and return values', '运行输入与返回值'),
      tables: [{ headers: [t('Value', '值'), t('Meaning', '含义'), t('Integration rule', '集成规则')], rows: [
        [t('RunInput.prompt: String', 'RunInput.prompt: String'), t('User text passed to the mounted execution path.', '传给已挂载执行路径的用户文本。'), t('Supply the actual user request; system instructions belong to host configuration.', '传入真实用户请求；系统指令由宿主配置负责。')],
        [t('RunInput.images: Vec<PathBuf>', 'RunInput.images: Vec<PathBuf>'), t('Image paths resolved by the workspace/model implementation.', '由工作空间／模型实现解析的图像路径。'), t('Keep the files readable on the execution host while the request is in use.', '请求使用图像期间，保持执行宿主上的文件可读。')],
        [t('CancellationToken', 'CancellationToken'), t('A shared cancellation signal, not a timeout duration.', '共享取消信号，不是超时时长。'), t('Keep a clone for the stop button or host deadline; await run after cancelling.', '为停止按钮或宿主截止时间保留副本；取消后等待 run。')],
        [t('Result<TurnOutcome, HandleError>', 'Result<TurnOutcome, HandleError>'), t('A terminal execution result, rather than streamed assistant text.', '执行终态结果，而不是流式助手文本。'), t('Read output from OutputStream and branch separately on outcome versus error.', '从 OutputStream 读取输出，分别处理 outcome 与 error。')],
      ] }],
      note: t('Cloning SessionHandle shares the mounted service. It does not create another session, fork history, or grant concurrent turn admission; the composition owns those decisions.', '克隆 SessionHandle 会共享已挂载服务，不会创建新会话、分叉历史或授予并发轮次准入；这些决策由组合负责。'),
    },
    {
      id: 'cancellation-recipe', title: t('Connect a stop button without losing the result', '连接停止按钮并保留结果'),
      paragraphs: [t('Give the caller a token clone before starting this function. The caller can cancel that clone from its UI or deadline handler. This function waits for cleanup in the existing run path and returns the original outcome, including a normal completion that won a race with cancellation.', '开始以下函数前，给调用方一个令牌副本。调用方可从界面或截止时间处理器取消该副本。函数等待既有 run 路径退出，并返回原始结果，包括在取消竞态中先完成的正常结果。')],
      code: [{ label: t('Rust · complete function, assembled host required', 'Rust · 完整函数，需要已组装宿主'), language: 'rust', verification: 'manual', exampleId: 'harness-cancellable-turn', code: `use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn submit(
    harness: &Harness,
    prompt: String,
    cancel: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    let session = harness.session();
    let session_id = session.id().await;
    let result = session.run(RunInput { prompt, images: vec![] }, cancel).await;
    let cost = session.cost().await;
    println!("session={session_id:?}, api_calls={}", cost.api_calls);
    result
}

// Before submit: let cancel = CancellationToken::new();
// Keep cancel.clone() in the host; call cancel.cancel() when the user stops.` }],
      bullets: [
        t('EndTurn means the execution path ended naturally; MaxTurns means its turn budget ended; Cancelled is an explicit cancellation outcome.', 'EndTurn 表示执行路径自然结束，MaxTurns 表示轮次预算结束，Cancelled 表示明确取消。'),
        t('An error or cancellation does not undo a tool’s completed file write, command, or external request. Inspect the operation before retrying it.', '错误或取消不会撤销工具已完成的文件写入、命令或外部请求。重试前检查该操作。'),
        t('Keep partial text as partial text until the host receives the final message or error boundary. A disconnected UI is not proof that the runtime stopped.', '收到最终消息或错误边界前，将部分文本保留为部分文本。界面断开不代表运行时已停止。'),
      ],
    },
    {
      id: 'stream-rendering', title: t('Render a stream and reconcile the final message', '渲染流并合并最终消息'),
      steps: [
        t('Append text deltas to the active assistant presentation. Preserve source text through the supplied UTF-16 JSON projection instead of round-tripping arbitrary external payloads through lossy strings.', '将文本增量追加到当前助手展示。通过提供的 UTF-16 JSON projection 保留源文本，避免把任意外部载荷经有损字符串往返转换。'),
        t('Create tool activity rows from the tool call identity. Update the same row when its matching result arrives; multiple calls can use the same tool name.', '根据工具调用标识创建活动行。匹配结果到达时更新同一行；多个调用可能使用相同工具名。'),
        t('Use final message boundaries or MessageComplete DTOs to settle the assistant row. Do not append a second copy of the same completed message after already rendering its deltas.', '用最终消息边界或 MessageComplete DTO 确定助手行。已经渲染增量后，不要再次追加同一完整消息。'),
        t('Read cost() when you need cumulative accounting. A transcript snapshot, a text delta and a cost update serve different purposes.', '需要累计统计时读取 cost()。对话快照、文本增量与成本更新承担不同职责。'),
      ],
      links: [{ label: t('Event fields and permission response routing', '事件字段与权限响应路由'), href: '/docs/harness-client#event-consumer' }],
    },
  ],
  'harness-lifecycle': [
    {
      id: 'headless-options', title: t('Configure a batch or streaming worker', '配置批处理或流式 worker'),
      paragraphs: [t('headless::run is an embeddable application entry point with injected I/O. Set the input/output contract before handing readers and writers to the runtime; closing an input stream and cancelling a turn are different operations.', 'headless::run 是注入 I/O 的可嵌入应用入口。把读写流交给运行时前先确定输入／输出契约；关闭输入流与取消轮次是不同操作。')],
      tables: [{ headers: [t('HeadlessOptions field', 'HeadlessOptions 字段'), t('Default', '默认值'), t('Use it for', '用途')], rows: [
        [t('prompt / prompt_images', 'prompt / prompt_images'), t('None / empty', 'None／空'), t('Initial text and images supplied directly by the host.', '宿主直接提供的初始文本与图像。')],
        [t('input_format / output_format', 'input_format / output_format'), t('Text / Text', 'Text／Text'), t('Choose plain text or the supported structured stream formats; use the corresponding reader/writer.', '选择纯文本或支持的结构化流格式，配套相应读写器。')],
        [t('no_stream', 'no_stream'), t('false', 'false'), t('Select the batched provider path for a one-shot prompt.', '为一次性提示词选择供应商批量响应路径。')],
        [t('include_partial_messages / include_hook_events', 'include_partial_messages / include_hook_events'), t('false / false', 'false／false'), t('Opt into partial assistant frames or hook frames in stream-json output.', '在 stream-json 输出中按需包含助手部分消息或 hook 帧。')],
        [t('max_budget_usd / json_schema', 'max_budget_usd / json_schema'), t('None / None', 'None／None'), t('Set an invocation budget or a source-preserving structured-output schema.', '设置本次调用预算，或保留源信息的结构化输出 schema。')],
        [t('permission_prompts_none', 'permission_prompts_none'), t('false', 'false'), t('Disable interactive prompts when the host intentionally has no prompt transport; this is not a blanket permission grant.', '宿主明确没有提示传输时禁用交互提示；它不代表授予全部权限。')],
      ] }],
    },
    {
      id: 'shutdown-result', title: t('Treat execution, persistence and delivery separately', '分别处理执行、持久化与交付'),
      tables: [{ headers: [t('Inspect', '检查项'), t('What it establishes', '能说明什么'), t('What to retain on failure', '失败时保留什么')], rows: [
        [t('HeadlessExit.execution', 'HeadlessExit.execution'), t('Whether model/runtime execution started and how it ended.', '模型／运行时执行是否开始，以及如何结束。'), t('reason, errors and session_id when available.', 'reason、errors 与可用时的 session_id。')],
        [t('HeadlessExit.shutdown', 'HeadlessExit.shutdown'), t('Whether resource drain and persistence barriers completed.', '资源排空与持久化屏障是否完成。'), t('shutdown_errors and the optional cleanup handle.', 'shutdown_errors 与可选 cleanup 句柄。')],
        [t('HeadlessExit.delivery', 'HeadlessExit.delivery'), t('Whether final output and writer shutdown completed.', '最终输出与 writer 关闭是否完成。'), t('delivery_errors; do not report missing output as successful delivery.', 'delivery_errors；输出缺失时不要报告交付成功。')],
        [t('ShutdownReport.complete', 'ShutdownReport.complete'), t('Whether the injected lifecycle service finished every required barrier.', '注入的生命周期服务是否完成全部必要屏障。'), t('errors and durable publications, then retry shutdown sequentially.', 'errors 与持久化 publications，再顺序重试关闭。')],
      ] }],
      note: t('A successful model response can coexist with failed output delivery. An incomplete shutdown is also meaningful even when the final text was delivered. Keep those states visible to the embedding application.', '模型响应成功时仍可能发生输出交付失败；最终文本已交付时，关闭未完成也仍有意义。嵌入应用应分别展示这些状态。'),
    },
    {
      id: 'resume-checklist', title: t('Resume a conversation after restarting the host', '宿主重启后恢复对话'),
      steps: [
        t('Select the session through the host’s existing resume path, not by changing the value returned from SessionHandle::id.', '通过宿主既有恢复路径选择会话，不要修改 SessionHandle::id 的返回值来模拟恢复。'),
        t('Restore the durable ordered transcript and its message identities. Reattach the model service, output destination, permission policy and extension owners for this new host lifetime.', '恢复持久化的有序对话及消息标识。为新宿主生命周期重新接入模型服务、输出目标、权限策略与扩展负责人。'),
        t('Treat historical file reads according to the selected resume implementation. The lower-level SessionResumer documented above starts with an empty cache; do not copy a cache from another workspace to make an edit pass.', '按照所选恢复实现处理历史文件读取。上文的底层 SessionResumer 从空缓存开始；不要复制另一工作空间的缓存来让编辑通过。'),
        t('Mount the restored session before admitting the next prompt. A new transcript snapshot should come from that mounted runtime.', '接收下一条提示词前，先挂载恢复后的会话。新的对话快照应来自该已挂载运行时。'),
      ],
      links: [{ label: t('Session snapshots', '会话快照'), href: '/docs/harness-sessions#snapshots' }],
    },
  ],
  'harness-tools': [
    {
      id: 'custom-tool-checklist', title: t('Implement a useful custom tool', '实现可用的自定义工具'),
      paragraphs: [t('A custom tool needs a complete Tool implementation and a host composition that registers it. Begin with a small operation whose input, result and side effects you can explain to both the model and the user.', '自定义工具需要完整的 Tool 实现，以及负责注册它的宿主组合。从一个能向模型和用户解释清楚输入、结果与副作用的小操作开始。')],
      tables: [{ headers: [t('Implement', '实现项'), t('Contract', '契约'), t('Common mistake', '常见错误')], rows: [
        [t('name, input_schema and input_repair_metadata', 'name、input_schema 与 input_repair_metadata'), t('A canonical identity, source-preserving schema and explicit input-repair facts.', '规范标识、保留源信息的 schema，以及明确的输入修复事实。'), t('Registering aliases as separate capabilities or advertising an unknown repair contract.', '把别名注册为独立能力，或声明未知的修复契约。')],
        [t('is_enabled, is_read_only and is_concurrency_safe', 'is_enabled、is_read_only 与 is_concurrency_safe'), t('Visibility and scheduling facts for this actual invocation.', '本次实际调用的可见性与调度事实。'), t('Marking a mutating operation read-only to avoid approval or serialization.', '为了绕过授权或串行执行，把修改操作标为只读。')],
        [t('description, prompt and max_result_size_chars', 'description、prompt 与 max_result_size_chars'), t('Model-facing guidance and an explicit result-size policy.', '模型可见说明与明确的结果大小策略。'), t('Returning unbounded raw output without considering the model-facing result.', '返回无界原始输出，不考虑模型最终看到的结果。')],
        [t('validate_input and check_permissions', 'validate_input 与 check_permissions'), t('Tool-specific validation and a decision before side effects.', '在副作用前执行工具专用校验与权限决策。'), t('Starting network/filesystem work during validation or permission checks.', '在校验或权限检查过程中提前执行网络／文件系统操作。')],
        [t('call and ToolCallResult', 'call 与 ToolCallResult'), t('Execution under the dispatcher with an explicit model result.', '在调度器控制下执行，并提供明确的模型结果。'), t('Calling the tool body directly from a transport or provider adapter.', '从传输层或供应商适配器直接调用工具主体。')],
      ] }],
      note: t('Current tool inputs and schemas use lingxi_core::types::utf16_json::Utf16JsonProjection. JSON is still the data format, but arbitrary external schemas and strings should retain their source projection instead of being treated as ordinary serde_json::Value.', '当前工具输入与 schema 使用 lingxi_core::types::utf16_json::Utf16JsonProjection。数据格式仍然是 JSON，但任意外部 schema 与字符串应保留其源 projection，不能一概按普通 serde_json::Value 处理。'),
    },
    {
      id: 'tool-results-and-errors', title: t('Return results that the model can act on', '返回模型可以继续使用的结果'),
      paragraphs: [t('ToolCallResult separates data, an optional faithful model-facing string and error state. The dispatcher can derive model text from data, or use model_content when supplied. MCP error results can be returned as accepted results with is_error set; a transport failure remains a failed call.', 'ToolCallResult 区分 data、可选的保真模型字符串与错误状态。调度器可以从 data 推导模型文本，或使用提供的 model_content。MCP 的逻辑错误结果可以作为 is_error 为真的已接收结果返回；传输失败仍是调用失败。')],
      code: [{ label: t('Rust · result construction excerpt', 'Rust · 构造结果摘录'), language: 'rust', verification: 'excerpt', code: `use tool_api::{source_text_projection, ToolCallResult};

fn completed_summary() -> ToolCallResult {
    ToolCallResult::from_data(source_text_projection("Saved the report."))
}` }],
      tables: [{ headers: [t('Failure', '失败'), t('Meaning', '含义'), t('Handle it by', '处理方式')], rows: [
        [t('InvalidInput / ValidationError', 'InvalidInput／ValidationError'), t('The request does not satisfy the tool contract.', '请求不符合工具契约。'), t('Return actionable validation detail; ask for corrected input before retrying.', '返回可操作的校验信息，修正输入后再重试。')],
        [t('PermissionDenied', 'PermissionDenied'), t('Policy or the user refused this invocation.', '策略或用户拒绝本次调用。'), t('Surface the denial and let the agent choose an allowed alternative.', '展示拒绝原因，让 Agent 选择被允许的替代操作。')],
        [t('InteractionRequired', 'InteractionRequired'), t('This host cannot provide the required live interaction channel.', '当前宿主无法提供所需实时交互通道。'), t('Supply the intended host interaction or choose a suitable noninteractive operation.', '接入预期宿主交互，或选择合适的非交互操作。')],
        [t('Aborted / Io', 'Aborted／Io'), t('Execution was interrupted or the underlying operation failed.', '执行被中断，或底层操作失败。'), t('Keep partial state and check idempotency before reissuing a side-effecting operation.', '保留部分状态，重新执行副作用操作前检查幂等性。')],
        [t('EditWithoutRead', 'EditWithoutRead'), t('The session has no accepted read state for the edit target.', '会话没有该编辑目标的有效读取状态。'), t('Read the current target in the mounted session, then construct a fresh edit.', '在已挂载会话中读取当前目标，再构造新的编辑。')],
      ] }],
    },
    {
      id: 'hooks-and-mcp', title: t('Add hooks and MCP without bypassing dispatch', '通过调度路径接入 hooks 与 MCP'),
      paragraphs: [t('Use hooks to observe or control lifecycle boundaries, and MCP to add tools owned by an external server connection. These are different extension mechanisms: a hook does not replace the tool registry, and connecting a server does not authorize all its tool calls.', 'hooks 用于观察或控制生命周期边界，MCP 用于增加由外部服务器连接持有的工具。这是两种不同扩展机制：hook 不替代工具注册表，连接服务器也不代表授权其全部工具调用。')],
      tables: [{ headers: [t('Need', '需求'), t('Source-backed seam', '源码提供的接口'), t('Lifecycle rule', '生命周期规则')], rows: [
        [t('Validate or annotate a tool invocation', '校验或注解工具调用'), t('HookEventType::PreToolUse / PostToolUse / PostToolUseFailure', 'HookEventType::PreToolUse／PostToolUse／PostToolUseFailure'), t('Choose the correct event and respect its control result; do not run the side effect from an observation hook.', '选择正确事件并遵循其控制结果；不要从观察 hook 直接执行原副作用。')],
        [t('Update user/project hooks', '更新用户／项目 hooks'), t('HookRegistry::replace_source_hooks', 'HookRegistry::replace_source_hooks'), t('Replace one source bucket after validation; retain plugin and session hook ownership.', '校验后替换单一来源分区，保留插件与会话 hook 的归属。')],
        [t('Add server tools', '增加服务器工具'), t('McpRegistry::connect and ToolRegistry::register_mcp_tools', 'McpRegistry::connect 与 ToolRegistry::register_mcp_tools'), t('Retain connection identity for refresh/disconnect. Let the production composition coordinate connection and tool registration.', '为更新／断开保留连接标识，由生产组合协调连接与工具注册。')],
        [t('Remove a server', '移除服务器'), t('ToolRegistry::unregister_mcp_tools', 'ToolRegistry::unregister_mcp_tools'), t('Remove only that connection partition and complete the owned transport teardown.', '只删除该连接分区，并完成其传输清理。')],
      ] }],
      note: t('HookDefinition includes blocking, timeout, priority and source. Blocking hooks are awaited; nonblocking hooks use the asynchronous registry. A timeout or failed hook is not a successful policy decision. Configure failure behavior at the supported event boundary.', 'HookDefinition 包含 blocking、timeout、priority 与 source。阻塞 hook 会被等待，非阻塞 hook 使用异步注册表。超时或 hook 失败不能当作成功策略决策；应在支持的事件边界配置失败行为。'),
      links: [{ label: t('Hook and extension lifecycle', 'Hook 与扩展生命周期'), href: '/docs/extensions-lifecycle' }],
    },
  ],
  'harness-plugins': [
    {
      id: 'plugin-directory', title: t('Prepare a local plugin directory', '准备本地插件目录'),
      paragraphs: [t('For PluginSource::LocalPath, provide an already materialized directory containing .lingxi-plugin/plugin.json. The discovery path can find optional commands/, agents/, skills/, output-styles/ and hooks/hooks.json. Keep those files available while the runtime uses the discovered components.', '使用 PluginSource::LocalPath 时，提供已经落盘、包含 .lingxi-plugin/plugin.json 的目录。发现路径可以识别可选的 commands/、agents/、skills/、output-styles/ 与 hooks/hooks.json。运行时使用发现的组件期间，应保持这些文件可用。')],
      code: [{ label: t('Local plugin layout', '本地插件目录'), language: 'text', verification: 'excerpt', code: `report-helper/
  .lingxi-plugin/plugin.json
  commands/
  agents/
  skills/
  hooks/hooks.json` }],
      steps: [
        t('Prepare the directory with a manifest and the component files it declares. Validate the content before activation.', '准备 manifest 以及其声明的组件文件，激活前校验内容。'),
        t('Pass its path to the host-assembled PluginManager. Record the returned PluginId, then inspect plugin_state for the actual result.', '将路径交给宿主组装的 PluginManager。记录返回的 PluginId，再通过 plugin_state 查看实际状态。'),
        t('Check the contributed registry entries rather than assuming every optional component became active.', '检查贡献到注册表的项目，不要假定每个可选组件都已激活。'),
        t('When removing the plugin, call disable on the same manager and retry any retained DisablingFailed teardown.', '移除插件时，对同一个管理器调用 disable，并重试保留为 DisablingFailed 的清理。'),
      ],
    },
    {
      id: 'plugin-install-paths', title: t('Select the correct install path', '选择正确的安装路径'),
      tables: [{ headers: [t('Source', '来源'), t('PluginManager::install', 'PluginManager::install'), t('Host action', '宿主操作')], rows: [
        [t('LocalPath', 'LocalPath'), t('Discovers and enables the prepared directory in place.', '原地发现并启用已准备目录。'), t('Provide a readable manifest and retain the directory lifecycle.', '提供可读 manifest，并管理目录生命周期。')],
        [t('Git / Marketplace / OfficialMarketplace', 'Git／Marketplace／OfficialMarketplace'), t('Returns a typed unsupported/fetch-path error.', '返回类型化的不支持／获取路径错误。'), t('Use the production installation/materialization pipeline before activation.', '先使用生产安装／落盘流程，再激活。')],
        [t('Mcpb', 'Mcpb'), t('Does not unpack the bundle through this method.', '此方法不会解包 bundle。'), t('Use the supported host install pipeline for the bundle.', '使用宿主支持的 bundle 安装流程。')],
        [t('BuiltIn', 'BuiltIn'), t('Is not installed through this method.', '不经此方法安装。'), t('Use the composition’s compiled builtin registration.', '使用组合中已编译的内置注册路径。')],
      ] }],
      note: t('A PluginSource variant describes an input source. Its existence does not promise that this particular manager method implements network fetching, authentication or archive materialization.', 'PluginSource 变体用于描述来源输入。变体存在不代表管理器的此方法实现了网络获取、认证或归档落盘。'),
    },
    {
      id: 'plugin-troubleshooting', title: t('Diagnose activation and removal', '诊断激活与移除'),
      tables: [{ headers: [t('Observation', '现象'), t('Interpretation', '解释'), t('Recovery', '恢复方式')], rows: [
        [t('No manifest found', '找不到 manifest'), t('A flat LocalPath discovery did not find the expected plugin directory.', 'LocalPath 的平层发现没有找到预期插件目录。'), t('Check the exact directory and .lingxi-plugin/plugin.json; do not pass the top-level versioned cache as a plugin.', '检查准确目录与 .lingxi-plugin/plugin.json，不要把顶层版本缓存当作单个插件。')],
        [t('Blocked', 'Blocked'), t('Policy rejected the source before activation.', '策略在激活前拒绝来源。'), t('Inspect trust and managed policy; do not register the components manually to bypass it.', '检查信任与受管策略，不要手动注册组件来绕过策略。')],
        [t('Failed / Validation / Loader', 'Failed／Validation／Loader'), t('Manifest, configuration or loading failed.', 'manifest、配置或加载失败。'), t('Correct the reported cause before enabling the candidate again.', '修正已报告原因后再启用候选插件。')],
        [t('DisablingFailed remains loaded', 'DisablingFailed 仍然出现在已加载列表'), t('Some owned resources still need teardown.', '仍有所属资源需要清理。'), t('Retain the owner and retry disable; report success only after its components are removed.', '保留负责人并重试 disable，组件移除后再报告成功。')],
      ] }],
    },
  ],
  'harness-client': [
    {
      id: 'event-consumer', title: t('Build a client event consumer', '构建客户端事件消费者'),
      tables: [{ headers: [t('Event', '事件'), t('Use it for', '用途'), t('Correlation', '关联方式')], rows: [
        [t('SessionStarted / SessionResumed', 'SessionStarted／SessionResumed'), t('Mount or replace the visible conversation.', '挂载或替换可见对话。'), t('session_id belongs to the connection’s mounted host.', 'session_id 属于该连接挂载的宿主。')],
        [t('TurnStarted', 'TurnStarted'), t('Mark an admitted prompt active.', '将已准入提示词标记为活动。'), t('Echoed optional turn_id from SendPrompt.', 'SendPrompt 回传的可选 turn_id。')],
        [t('TextDelta', 'TextDelta'), t('Append assistant text to the active presentation.', '向当前展示追加助手文本。'), t('The ordered event feed; this DTO does not add a per-delta turn_id.', '按顺序的事件流；此 DTO 不额外提供逐增量 turn_id。')],
        [t('ToolUseStarted / ToolUseResult', 'ToolUseStarted／ToolUseResult'), t('Show progress, input, result and error state.', '展示进度、输入、结果与错误状态。'), t('Match id, not the tool display name.', '通过 id 匹配，不用工具显示名称。')],
        [t('MessageComplete', 'MessageComplete'), t('Settle the completed assistant message when supplied.', '提供完整助手消息时确定最终展示。'), t('Use the message’s identity and ordered blocks; message can be absent.', '使用消息标识与有序 blocks；message 可以缺省。')],
        [t('TurnEnded / Error', 'TurnEnded／Error'), t('Settle the host’s terminal execution presentation.', '确定宿主执行终态展示。'), t('Keep outcome, stop reason, costs or structured error kind separately.', '分别保留 outcome、停止原因、成本或结构化错误类别。')],
      ] }],
      note: t('Do not assume every event has session_id or turn_id. The protocol deliberately makes session selection a connection attribute. Transports that multiplex connections must retain their own authenticated connection-to-session mapping.', '不要假定每个事件都有 session_id 或 turn_id。协议将会话选择设计为连接属性。复用多个连接的传输应维护自身经过认证的连接到会话映射。'),
    },
    {
      id: 'permission-roundtrip', title: t('Complete the permission request round trip', '完成权限请求与响应闭环'),
      steps: [
        t('Receive the permission request from the host-owned PermissionRequestSink and store its connection-scoped request_id with the rendered card.', '从宿主持有的 PermissionRequestSink 接收权限请求，将连接级 request_id 与展示的卡片一起保存。'),
        t('Display the requested operation and the engine-provided actions. Respect suppress_always_allow_rule and auto_mode_prompt; the client must not infer eligibility for broader permission.', '展示请求操作及引擎提供的动作。遵守 suppress_always_allow_rule 与 auto_mode_prompt，客户端不能自行推导更大权限的资格。'),
        t('Send ApprovePermission or DenyPermission with that request_id through the connection that owns the parked request.', '通过持有该挂起请求的连接，发送带该 request_id 的 ApprovePermission 或 DenyPermission。'),
        t('Remove or settle the card when PermissionRequestResolved arrives. Approved, Denied, Cancelled and Expired are terminal states; a stale click can lose a race with cancellation.', '收到 PermissionRequestResolved 后移除或确定卡片状态。Approved、Denied、Cancelled 与 Expired 都是终态；过期点击可能在取消竞态中失效。'),
      ],
      code: [{ label: t('ClientCommand JSON · one-time approval', 'ClientCommand JSON · 单次允许'), language: 'json', verification: 'excerpt', code: '{\n  "type": "approve_permission",\n  "request_id": 7,\n  "response": { "type": "allow_once" }\n}' }],
      apis: [{ name: 'AdapterPermissionGate::resolve', signature: 'pub async fn resolve(&self, request_id: u64, response: PermissionResponseDto, tool_name_hint: &str) -> bool', description: t('Returns false when the parked request is unknown or no longer has a receiver. Route by the stored request identity; the tool-name hint does not replace it.', '请求未知或已没有等待接收方时返回 false。应按保存的请求标识路由，工具名提示不能替代它。') }],
    },
    {
      id: 'client-error-handling', title: t('Handle errors without guessing success', '处理错误而不猜测成功'),
      tables: [{ headers: [t('Signal', '信号'), t('Client response', '客户端响应'), t('Avoid', '避免')], rows: [
        [t('Transport error', '传输错误'), t('Keep partial content, report the failure and apply the host’s reconnect policy.', '保留部分内容、报告失败，并使用宿主重连策略。'), t('Automatically resending a side-effecting prompt as if it had never run.', '假定带副作用的提示词从未执行，并自动重发。')],
        [t('Protocol / Internal error', '协议／内部错误'), t('Retain diagnostics and validate the exact host/client binding versions.', '保留诊断信息，验证准确的宿主／客户端绑定版本。'), t('Converting missing or malformed events into completed messages.', '把缺失或格式错误的事件转换成已完成消息。')],
        [t('MaxTurns', 'MaxTurns'), t('Explain that the configured execution budget ended and preserve the conversation.', '说明配置的执行预算已结束，并保留对话。'), t('Reporting the user’s task as complete solely because a turn ended.', '仅因轮次结束，就报告用户任务完成。')],
        [t('Unknown future enum variant', '未来未知 enum 变体'), t('Follow the host’s compatibility contract and provide an explicit unsupported state when needed.', '遵循宿主兼容性契约，必要时展示明确的不支持状态。'), t('Inventing a permission approval or successful operation for an unrecognized payload.', '为无法识别的载荷伪造授权或操作成功。')],
      ] }],
      links: [{ label: t('Cancellation, terminal outcomes and partial output', '取消、终态结果与部分输出'), href: '/docs/harness-sessions#cancellation-recipe' }],
    },
  ],
};

for (const page of harnessPages) {
  page.sections.push(...(integrationSections[page.id] ?? []), ...(usageSections[page.id] ?? []));
}
