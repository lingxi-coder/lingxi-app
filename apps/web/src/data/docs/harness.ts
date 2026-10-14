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
            signature: 'async fn emit_text(&self, text: &str, utf16_code_units: Option<&[u16]>)',
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
    sources: sources('crates/runtime/src/api.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/runtime/src/desktop/session_state.rs', 'crates/runtime/src/mobile/host.rs', 'crates/runtime/src/mobile/host/assembly.rs', 'crates/runtime/src/headless/mod.rs', 'crates/runtime/src/headless/config.rs', 'crates/runtime/src/headless/host.rs', 'crates/runtime/src/headless/io.rs', 'crates/runtime/src/headless/session.rs', 'crates/permission/src/gate.rs', 'crates/session/src/storage.rs', 'crates/session/src/resumer.rs'),
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
    sources: sources('crates/tool-api/src/tool_trait.rs', 'crates/tool-api/src/context.rs', 'crates/tool-api/src/registry.rs', 'crates/tool-api/src/progress.rs', 'crates/orchestrator/src/turn_loop/tool_dispatch.rs'),
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
            signature: 'fn input_schema(&self) -> &serde_json::Value',
            description: t('Advertise the tool’s input JSON Schema.', '声明工具输入的 JSON Schema。'),
          },
          {
            name: 'Tool::validate_input',
            signature: 'async fn validate_input(&self, input: &serde_json::Value, ctx: &ToolUseContext) -> Result<(), ValidationError>',
            description: t('Validate tool-specific conditions before side effects; the default implementation succeeds.', '在副作用前校验工具专用条件；默认实现通过校验。'),
          },
          {
            name: 'Tool::check_permissions',
            signature: 'async fn check_permissions(&self, input: &serde_json::Value, ctx: &ToolUseContext) -> permission::PermissionResult',
            description: t('Compute the invocation-specific permission decision before dispatch.', '在调度前计算本次调用的权限决策。'),
          },
          {
            name: 'Tool::call',
            signature: 'async fn call(&self, input: serde_json::Value, ctx: ToolUseContext, progress_tx: ToolProgressSender) -> Result<ToolCallResult, ToolError>',
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
            signature: 'pub struct ToolProgress { pub tool_use_id: lingxi_core::types::ToolUseId, pub data: serde_json::Value }',
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
    sources: sources('crates/client/src/lib.rs', 'crates/client/Cargo.toml', 'crates/client/src/protocol/version.rs', 'crates/client/src/protocol/commands.rs', 'crates/client/src/protocol/events.rs', 'crates/client/src/adapter/output_stream.rs', 'crates/client/src/protocol/message.rs'),
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
            signature: 'pub const CLIENT_PROTOCOL_VERSION: &str = "21.0.0";',
            description: t('Protocol version in the inspected workspace. Native clients must version-lock generated binding layouts with the host they actually build.', '所检查工作区中的协议号。原生客户端生成绑定的布局需要与实际构建的宿主版本一致。'),
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
      note: t('The current working-tree protocol constant is 21.0.0. Always read the baseline and the host pin before connecting a separate client. Device callbacks, ordering under disconnect and replay presentation remain target-host checks.', '当前工作树协议常量为 21.0.0。连接独立客户端前，始终查看基线与宿主固定依赖版本。设备回调、断线时顺序及重放展示仍需目标宿主检查。'),
    },
  ],
};

for (const page of harnessPages) page.sections.push(...(integrationSections[page.id] ?? []));
