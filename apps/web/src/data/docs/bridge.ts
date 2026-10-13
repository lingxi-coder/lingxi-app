import type { DocPage } from './types';

const repository = 'https://github.com/lingxi-coder/lingxi-app/tree/main';

export const bridgePages: DocPage[] = [
  {
    id: 'bridge-client',
    group: 'bridge',
    title: { en: 'TypeScript Bridge Client', zh: 'TypeScript Bridge 客户端' },
    description: {
      en: 'Connect a Node.js host to the local LingXi Bridge service, submit commands, consume events, and resolve permission requests.',
      zh: '将 Node.js 宿主连接到本地 LingXi Bridge 服务，提交命令、消费事件并处理权限请求。',
    },
    packageName: '@lingxi/bridge-client',
    sourceUrl: `${repository}/packages/bridge-client`,
    sources: [
      { repo: 'lingxi-app', path: 'packages/bridge-client/src/client.ts' },
      { repo: 'lingxi-app', path: 'packages/bridge-client/src/protocol.ts' },
      { repo: 'lingxi-app', path: 'packages/bridge-client/src/version.ts' },
      { repo: 'lingxi-app', path: 'packages/bridge-client/src/lockfile.ts' },
      { repo: 'lingxi-app', path: 'packages/bridge-client/src/eventCorrelation.ts' },
      { repo: 'lingxi-app', path: 'apps/bridge-server/src/server.rs' },
      { repo: 'lingxi-app', path: 'apps/bridge-server/src/router.rs' },
      { repo: 'harness-runtime', path: 'crates/bridge/src/wire.rs' },
      { repo: 'harness-runtime', path: 'crates/bridge/src/mcp_endpoint.rs' },
      { repo: 'harness-runtime', path: 'crates/client/src/protocol/commands.rs' },
      { repo: 'harness-runtime', path: 'crates/client/src/protocol/events.rs' },
      { repo: 'harness-runtime', path: 'crates/client/src/protocol/error.rs' },
    ],
    sections: [
      {
        id: 'integration-boundary',
        title: { en: 'Choose this integration', zh: '选择此集成方式' },
        paragraphs: [
          {
            en: 'The Bridge client is the product repository’s private TypeScript package. It exports protocol DTOs, lockfile discovery, validation utilities, and a Node-side WebSocket client. Electron consumes its compiled dist exports through a local file dependency.',
            zh: 'Bridge 客户端是产品仓库内的私有 TypeScript 包，导出协议 DTO、lockfile 发现、校验工具及 Node 端 WebSocket 客户端。Electron 通过本地 file 依赖消费编译后的 dist 导出。',
          },
          {
            en: 'BridgeClient imports node:events and ws. Run it in Node.js or an Electron host process; browser applications need an appropriate host transport. This package is not a hosted LLM HTTP API, and there is no public npm installation contract for it.',
            zh: 'BridgeClient 依赖 node:events 和 ws，应运行在 Node.js 或 Electron 宿主进程中；浏览器应用需要对应的宿主传输层。它不提供托管 LLM HTTP API，也没有公开 npm 安装契约。',
          },
        ],
        code: [
          {
            label: { en: 'Build from the product checkout', zh: '从产品 checkout 构建' },
            language: 'bash',
            code: 'cargo build --locked -p bridge-server --bin bridge-server\nnpm --prefix packages/bridge-client ci\nnpm --prefix packages/bridge-client run build',
          },
          {
            label: { en: 'Local package dependency', zh: '本地包依赖' },
            language: 'json',
            code: '{\n  "dependencies": {\n    "@lingxi/bridge-client": "file:../../packages/bridge-client"\n  }\n}',
          },
        ],
        note: {
          en: 'Commands run from the product repository root. The file dependency above is relative to apps/electron/package.json; adjust it for another local consumer. Start the Bridge service through the product host before connecting.',
          zh: '构建命令从产品仓库根目录执行。上面的 file 路径相对于 apps/electron/package.json，其他本地消费者需调整路径；连接前先通过产品宿主启动 Bridge 服务。',
        },
      },
      {
        id: 'connect-and-stream',
        title: { en: 'Connect and receive a turn', zh: '连接并接收回合事件' },
        paragraphs: [
          {
            en: 'connect() discovers a lockfile in ~/.lingxi/bridge unless lockfilePath or lockfileDir is supplied. It connects to ws://127.0.0.1:<port>/mcp using the mcp subprotocol and the lockfile token in X-LingXi-Ide-Authorization, then exchanges hello messages and checks protocol compatibility.',
            zh: '未指定 lockfilePath 或 lockfileDir 时，connect() 从 ~/.lingxi/bridge 发现 lockfile。它使用 mcp 子协议连接 ws://127.0.0.1:<port>/mcp，通过 X-LingXi-Ide-Authorization 携带 lockfile token，再交换 hello 消息并检查协议兼容性。',
          },
          {
            en: 'sendPrompt() submits a command without waiting for its reply. Text, tool activity, and the final turn outcome arrive through ClientEvent. Subscribe with events() before connecting or submitting: only subscribed iterators retain incoming events. The example resolves unattended permission, computer-access and questionnaire requests by denial/cancellation; a product host should present them and apply the user’s decision.',
            zh: 'sendPrompt() 提交命令后立即返回，不等待回合结果。文本、工具活动与最终回合结果通过 ClientEvent 到达。应在连接或提交之前调用 events() 订阅，只有已订阅的迭代器才会保留入站事件。示例拒绝权限与电脑访问请求，并取消问卷，以处理无人值守交互；产品宿主应展示请求并应用用户决定。',
          },
        ],
        code: [
          {
            label: { en: 'Node.js / TypeScript consumer', zh: 'Node.js／TypeScript 消费者' },
            language: 'typescript',
            verification: 'compile',
            exampleId: 'bridge-command-events',
            code: `import { BridgeClient } from '@lingxi/bridge-client';

const client = new BridgeClient({ clientName: 'documentation-example' });
const events = client.events();
client.on('error', (error: Error) => console.error(error.message));
client.on('permission', (request) => {
  client.denyPermission(request.request_id);
});
client.on('computerAccess', (request) => {
  client.denyComputerAccess(request.request_id);
});

try {
  await client.connect();
  client.sendPrompt('Describe the repository structure without changing files.');

  for await (const event of events) {
    if (event.type === 'ask_user_question') client.cancelAskUserQuestion(event.request.request_id);
    if (event.type === 'text_delta') process.stdout.write(event.text);
    if (event.type === 'turn_ended') {
      console.log(event.outcome, event.cost);
      break;
    }
  }
} finally {
  await events.return?.();
  client.close();
}`,
          },
        ],
      },
      {
        id: 'client-api',
        title: { en: 'Connection and command API', zh: '连接与命令 API' },
        apis: [
          {
            name: 'BridgeClient',
            signature: 'new BridgeClient(opts: BridgeClientOptions = {})',
            description: { en: 'Configure lockfilePath, lockfileDir, host, clientName, audioCapabilities, and handshakeTimeoutMs. The default handshake timeout is 10,000 ms.', zh: '配置 lockfilePath、lockfileDir、host、clientName、audioCapabilities 与 handshakeTimeoutMs；默认握手超时为 10,000 ms。' },
          },
          {
            name: 'connect',
            signature: 'connect(): Promise<ServerHello>',
            description: { en: 'Open the authenticated connection and finish the version handshake before sending commands.', zh: '建立已认证连接并完成版本握手，然后才能发送命令。' },
          },
          {
            name: 'sendPrompt',
            signature: 'sendPrompt(text: string, opts: { images?: ImageRefDto[]; turnId?: number; visualizationContext?: VisualizationRefDto } = {}): void',
            description: { en: 'Submit text, inline media_type/base64 images, an optional caller turn identifier, and visualization context.', zh: '提交文本、内联 media_type/base64 图片、可选的调用方回合标识及可视化上下文。' },
          },
          {
            name: 'sendCommand',
            signature: 'sendCommand(command: ClientCommand): void',
            description: { en: 'Send a typed protocol command. A disconnected client or oversized frame throws before the message is sent.', zh: '发送类型化协议命令；未连接或消息帧过大时，会在发送前抛出错误。' },
          },
          {
            name: 'cancel',
            signature: 'cancel(turnId?: number): void',
            description: { en: 'Cancel the current turn or an explicitly selected turn.', zh: '取消当前回合，或通过标识取消指定回合。' },
          },
          {
            name: 'events',
            signature: 'events(): AsyncIterableIterator<ClientEvent>',
            description: { en: 'Subscribe immediately and consume ordered events until close. Events before subscription are not replayed; all iterators share one queue and divide events. Returning the last iterator releases its buffer. openai_oauth_updated goes only to the host event listener.', zh: '立即订阅并按顺序消费事件直到关闭。订阅前的事件不会重放；所有迭代器共享一个队列并分配事件。最后一个迭代器 return 会释放缓冲；openai_oauth_updated 仅交给宿主事件监听器。' },
          },
          {
            name: 'close',
            signature: 'close(): void',
            description: { en: 'Close the underlying WebSocket. Retain the client for the full command and event lifecycle.', zh: '关闭底层 WebSocket；在命令和事件的完整生命周期内保留客户端。' },
          },
        ],
      },
      {
        id: 'permissions',
        title: { en: 'Permissions and interactive requests', zh: '权限与交互请求' },
        paragraphs: [
          {
            en: 'Permission and computer-access requests are separate listener events. Resolve each request with its request_id. The generic event feed does not replace the permission listener. Register listeners before submitting the first prompt.',
            zh: '权限请求与电脑访问请求通过独立监听事件到达。使用各自的 request_id 关联响应；普通事件流不能替代权限监听器，应在提交首个提示前注册监听。',
          },
        ],
        apis: [
          {
            name: 'approvePermission / denyPermission',
            signature: 'approvePermission(requestId: number, response: PermissionResponseDto = { type: "allow_once" }): void\ndenyPermission(requestId: number): void',
            description: { en: 'Resolve a parked tool permission request. Choose an explicit response when broader permission semantics are needed.', zh: '处理等待中的工具权限请求；需要更广的授权语义时，显式传入对应响应。' },
          },
          {
            name: 'approveComputerAccess / denyComputerAccess',
            signature: 'approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): void\ndenyComputerAccess(requestId: number): void',
            description: { en: 'Resolve computer tool access independently from ordinary tool permissions.', zh: '独立处理电脑工具的访问请求，与普通工具权限分开关联。' },
          },
          {
            name: 'answerAskUserQuestion / cancelAskUserQuestion',
            signature: 'answerAskUserQuestion(requestId: number, answers: Record<string, string>): void\ncancelAskUserQuestion(requestId: number): void',
            description: { en: 'Return a keyed answer set for an interactive questionnaire, or cancel the pending request.', zh: '为交互问卷返回按键组织的答案集合，或取消等待中的请求。' },
          },
        ],
      },
    ],
  },
  {
    id: 'platform-integration',
    group: 'bridge',
    title: { en: 'Native host integration', zh: '原生宿主集成' },
    description: {
      en: 'Understand the product’s iOS and Android UniFFI wrappers, generated bindings, native callbacks, and shared mobile session host.',
      zh: '了解产品的 iOS 与 Android UniFFI 包装、生成绑定、原生回调及共享移动会话宿主。',
    },
    packageName: 'ios-framework / android-aar',
    sourceUrl: `${repository}/apps`,
    sources: [
      { repo: 'lingxi-app', path: 'apps/ios/ffi/src/host.rs' },
      { repo: 'lingxi-app', path: 'apps/android/ffi/src/host.rs' },
      { repo: 'lingxi-app', path: 'apps/ios/ffi/src/callbacks.rs' },
      { repo: 'lingxi-app', path: 'apps/android/ffi/src/callbacks.rs' },
      { repo: 'lingxi-app', path: 'apps/ios/ffi/src/configuration.rs' },
      { repo: 'lingxi-app', path: 'apps/android/ffi/src/configuration.rs' },
      { repo: 'lingxi-app', path: 'apps/ios/native/scripts/build-xcframework.sh' },
      { repo: 'lingxi-app', path: 'apps/android/native/scripts/build-jni.sh' },
      { repo: 'lingxi-app', path: 'apps/ios/native/Sources/Conversation/EngineConversationSource.swift' },
      { repo: 'lingxi-app', path: 'apps/android/native/app/src/main/java/com/lingxi/code/conversation/EngineConversationSource.kt' },
      { repo: 'harness-runtime', path: 'crates/runtime/src/mobile/host.rs' },
      { repo: 'harness-runtime', path: 'crates/client/src/protocol/audio.rs' },
    ],
    sections: [
      {
        id: 'host-boundary',
        title: { en: 'Product wrappers and standalone SDKs', zh: '产品包装与独立 SDK' },
        paragraphs: [
          {
            en: 'apps/ios/ffi and apps/android/ffi assemble the LingXi product’s platform capabilities around Harness Runtime. They are product-owned Rust packages, ios-framework and android-aar, not separately published general-purpose Swift or Kotlin SDKs.',
            zh: 'apps/ios/ffi 与 apps/android/ffi 围绕 Harness Runtime 装配 LingXi 产品的平台能力。它们是由产品维护的 Rust 包 ios-framework 和 android-aar，不是单独发布的通用 Swift 或 Kotlin SDK。',
          },
          {
            en: 'Both wrappers re-export MobileEngineHandle and MobileEngineError from harness_runtime::mobile. The shared host owns its Tokio runtime, accepts ClientCommand, and sends results through the registered listener. The platform wrapper supplies the native callbacks and launch configuration.',
            zh: '两端都从 harness_runtime::mobile 重导出 MobileEngineHandle 与 MobileEngineError。共享宿主拥有自己的 Tokio runtime，接收 ClientCommand，并通过已注册的监听器发送结果；平台包装提供原生回调与启动配置。',
          },
          {
            en: 'For a new independent application, start with the Harness facade or the standalone Mobile Linux SDK. Follow this page when extending or embedding the LingXi product hosts. Desktop and Node hosts use the Bridge transport; native mobile hosts call their generated FFI bindings.',
            zh: '新建独立应用时，先选择 Harness facade 或独立 Mobile Linux SDK；扩展或嵌入 LingXi 产品宿主时使用本页。桌面与 Node 宿主使用 Bridge 传输，原生移动宿主调用生成的 FFI 绑定。',
          },
        ],
      },
      {
        id: 'generate-bindings',
        title: { en: 'Build bindings with the host', zh: '与宿主一起构建绑定' },
        code: [
          {
            label: { en: 'iOS / Xcode', zh: 'iOS／Xcode' },
            language: 'bash',
            code: 'bash apps/ios/native/scripts/build-xcframework.sh\ncd apps/ios/native\nxcodegen generate',
          },
          {
            label: { en: 'Android / Gradle', zh: 'Android／Gradle' },
            language: 'bash',
            code: 'bash apps/android/native/scripts/build-jni.sh --variant play',
          },
        ],
        bullets: [
          {
            en: 'iOS generation writes Swift bindings, FFI headers, and module maps under apps/ios/native/Generated, then creates apps/ios/native/Frameworks/LingxiCodeFFI.xcframework with device and simulator slices.',
            zh: 'iOS 生成器将 Swift 绑定、FFI 头文件和 module map 写入 apps/ios/native/Generated，再生成包含真机与模拟器切片的 apps/ios/native/Frameworks/LingxiCodeFFI.xcframework。',
          },
          {
            en: 'Android generation builds libandroid_aar.so for arm64-v8a and x86_64 in the selected play or direct JNI source set and generates the Kotlin component bindings using each component’s UniFFI configuration.',
            zh: 'Android 生成器为 arm64-v8a 与 x86_64 构建 libandroid_aar.so，写入选定的 play 或 direct JNI source set，并按各组件的 UniFFI 配置生成 Kotlin 绑定。',
          },
          {
            en: 'Keep bindings and libraries from the same source build. Change the Rust contract first, then regenerate active native bindings; do not hand-edit generated files or copy the historical apps/ios/ffi/swift and apps/android/ffi/kotlin scaffolds.',
            zh: '绑定与原生库必须来自同一次源码构建。先修改 Rust 契约，再重新生成当前原生绑定；不要手工编辑生成文件，也不要复制历史 apps/ios/ffi/swift 与 apps/android/ffi/kotlin scaffold。',
          },
        ],
        note: {
          en: 'Run build commands from the product repository root with its pinned Rust toolchain and the platform’s Xcode or Android SDK/NDK prerequisites. Generating or linking simulator bindings does not validate Linux guest execution on a physical device.',
          zh: '从产品仓库根目录运行构建命令，使用仓库固定的 Rust 工具链及对应的 Xcode 或 Android SDK/NDK。生成或链接模拟器绑定不能证明 Linux guest 已在真机执行。',
        },
      },
      {
        id: 'construct-host',
        title: { en: 'Construct the native host', zh: '创建原生宿主' },
        paragraphs: [
          {
            en: 'The actual iOS constructor is build_ios_engine_with_config in apps/ios/ffi/src/host.rs. The Android constructor is build_android_engine in apps/android/ffi/src/host.rs. Their configuration records carry provider selection, app-private storage, project context, host environment, and optional Mobile Linux settings.',
            zh: '当前 iOS 构造函数是 apps/ios/ffi/src/host.rs 中的 build_ios_engine_with_config；Android 构造函数是 apps/android/ffi/src/host.rs 中的 build_android_engine。配置记录包含 provider 选择、应用私有存储、项目上下文、宿主环境与可选 Mobile Linux 配置。',
          },
          {
            en: 'The signatures below are the Rust source contracts; UniFFI generates the Swift and Kotlin spellings. Construct callbacks in the native host and retain the returned engine handle while submitting commands and consuming listener events.',
            zh: '下方签名是 Rust 源码契约，Swift 与 Kotlin 的名称由 UniFFI 生成。原生宿主负责构造回调，在提交命令与消费监听事件期间保留返回的 engine handle。',
          },
        ],
        code: [
          {
            label: { en: 'iOS constructor contract', zh: 'iOS 构造函数契约' },
            language: 'rust',
            code: `pub fn build_ios_engine_with_config(
    config: IosEngineLaunchConfigFfi,
    listener: Box<dyn IosEventListener>,
    audio: Box<dyn IosAudioService>,
    camera: Box<dyn IosCamera>,
    share: Box<dyn IosShare>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
    device_control: Option<Box<dyn IosDeviceControl>>,
    location: Option<Box<dyn IosLocation>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError>`,
          },
          {
            label: { en: 'Android constructor contract', zh: 'Android 构造函数契约' },
            language: 'rust',
            code: `pub fn build_android_engine(
    config: AndroidEngineLaunchConfigFfi,
    listener: Box<dyn AndroidEventListener>,
    audio: Box<dyn AndroidAudioService>,
    camera: Box<dyn AndroidCamera>,
    share: Box<dyn AndroidShare>,
    location: Box<dyn AndroidLocation>,
    notifications: Box<dyn AndroidNotification>,
    clipboard: Box<dyn AndroidClipboard>,
    permissions: Box<dyn AndroidPermissionSink>,
    computer_use: Option<Box<dyn AndroidComputerUseHost>>,
    shell: Option<AndroidShellConfigFfi>,
    git: Option<AndroidGitConfigFfi>,
    git_credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    secure_storage: Option<Box<dyn AndroidSecureStorage>>,
    device_control: Option<Box<dyn AndroidDeviceControl>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError>`,
          },
        ],
      },
      {
        id: 'callbacks-and-commands',
        title: { en: 'Native callbacks and command flow', zh: '原生回调与命令流' },
        paragraphs: [
          {
            en: 'IosAudioService and AndroidAudioService expose the same operation model. capabilities() returns the current readiness snapshot without requesting permission; execute(request) returns a structured terminal result; cancel(identity) cancels only the matching pending operation. Native implementations own audio devices and OS permission presentation.',
            zh: 'IosAudioService 与 AndroidAudioService 使用相同的操作模型。capabilities() 只返回当前可用性快照，不发起权限请求；execute(request) 返回结构化终态结果；cancel(identity) 只取消匹配的等待操作。原生实现负责音频设备与系统权限展示。',
          },
          {
            en: 'Camera, sharing, notifications, clipboard, secure storage, device control, and location have their own callback contracts. Listener and permission callbacks connect the native UI to the shared session host. Credentials are provided at runtime through host configuration or secure storage, not compiled into bindings.',
            zh: '相机、分享、通知、剪贴板、安全存储、设备控制与定位各有回调契约。事件监听器与权限回调将原生界面连接到共享会话宿主；凭证在运行时通过宿主配置或安全存储提供，不编译进绑定。',
          },
        ],
        code: [
          {
            label: { en: 'Swift / existing MobileEngineHandle', zh: 'Swift／现有 MobileEngineHandle' },
            language: 'swift',
            code: 'try await handle.submit(command: .listModels)\ntry await handle.submit(command: .getConversationControls)',
          },
          {
            label: { en: 'Kotlin / existing MobileEngineHandle', zh: 'Kotlin／现有 MobileEngineHandle' },
            language: 'kotlin',
            code: 'handle.submit(ClientCommand.ListModels)\nhandle.submit(ClientCommand.ListSessions(limit = null))',
          },
        ],
        note: {
          en: 'These snippets assume an existing configured handle and the generated component imports used by the native app. submit accepts a command; UI output arrives asynchronously through the registered event listener. The native host owns screen attachment, background lifecycle, permission decisions, and recovery.',
          zh: '示例假定已经创建配置完整的 handle，并导入原生应用使用的生成组件。submit 接收命令，界面输出通过已注册的事件监听器异步到达；原生宿主负责界面挂接、后台生命周期、权限决定与恢复。',
        },
      },
    ],
  },
];

const text = (en: string, zh: string) => ({ en, zh });

bridgePages[0].sections.push(
  {
    id: 'wire-handshake',
    title: text('Wire contract and version handshake', '传输契约与版本握手'),
    paragraphs: [
      text('The current working-tree constants are BRIDGE_PROTOCOL_VERSION = 0.2.0 and CLIENT_PROTOCOL_VERSION = 21.0.0. Rust is the contract source; the TypeScript DTO file mirrors it. The envelope and DTO contract are checked independently, each requiring the same numeric major version. Malformed or mismatched versions refuse hello and bar later commands; server hello rejection uses wire error code -32600.', '当前工作树常量为 BRIDGE_PROTOCOL_VERSION = 0.2.0、CLIENT_PROTOCOL_VERSION = 21.0.0。Rust 是契约来源，TypeScript DTO 文件镜像该契约。传输包与 DTO 分别校验，双方数字主版本须各自相同。格式错误或版本不匹配会拒绝 hello 并阻止后续命令；服务端 hello 拒绝使用传输错误码 -32600。'),
      text('Frame is adjacent-tagged as type/payload; ClientCommand and ClientEvent are internally tagged with snake_case type values. Optional fields may be absent. Tool input_json/result_json payloads remain JSON strings, so clients must parse the contents deliberately and must not change their wire representation to objects. The mcp path/subprotocol names do not make product commands ordinary MCP tools/list or tools/call requests.', 'Frame 使用 type/payload 邻接标签；ClientCommand 与 ClientEvent 使用内部 type 标签及 snake_case 值。可选字段可能不存在。工具 input_json/result_json 保持 JSON 字符串形式，客户端须显式解析内容，不能把传输表示改成对象。mcp 路径／子协议名称不表示产品命令就是普通 MCP tools/list 或 tools/call 请求。'),
    ],
    steps: [
      text('Host starts the local service and writes a protected <port>.lock containing authToken.', '宿主启动本地服务，并写入含 authToken 的受保护 <port>.lock。'),
      text('Client chooses an explicit lockfile for its session, sends the authorization header and requests the mcp WebSocket upgrade.', '客户端为目标会话选择明确 lockfile，发送认证头并请求 mcp WebSocket 升级。'),
      text('Client sends request/hello; server verifies both major versions and responds with ServerHello.', '客户端发送 request/hello；服务端校验两个主版本并返回 ServerHello。'),
      text('Only after connect resolves does the host submit commands and consume events/interactive requests.', 'connect 完成后，宿主才能提交命令并消费事件／交互请求。'),
    ],
    code: [{
      label: text('Current hello request shape', '当前 hello 请求形状'), language: 'json', verification: 'excerpt',
      code: '{\n  "type": "request",\n  "payload": {\n    "id": 1,\n    "method": "hello",\n    "params": {\n      "protocol_version": "0.2.0",\n      "client_name": "documentation-example",\n      "capabilities": {\n        "supports_streaming": true,\n        "supports_tools": true,\n        "supports_skills": true,\n        "supports_commands": true,\n        "client_protocol_version": "21.0.0"\n      }\n    }\n  }\n}',
    }],
    diagram: { src: '/docs-assets/platform-host-transports.svg', alt: text('Electron uses authenticated WebSocket Bridge, native hosts use in-process UniFFI, both reach Harness.', 'Electron 使用已认证 WebSocket Bridge，原生宿主使用进程内 UniFFI，二者进入 Harness。'), caption: text('Transport framing is separate from the shared ClientCommand / ClientEvent contract.', '传输分帧与共享 ClientCommand／ClientEvent 契约分离。') },
  },
  {
    id: 'correlation-and-errors',
    title: text('Correlation, errors and connection disposal', '关联、错误与连接销毁'),
    paragraphs: [
      text('An outbound frame id correlates request/response envelopes. It is not a turn_id, session_id or broker request_id. sendCommand/sendPrompt are fire-and-forget: their request ids are not registered as awaited responses, and completion is a ClientEvent such as turn_ended. Event-replied product operations must correlate the payload request identifier together with session identity; SessionEventCorrelator rejects duplicate requests, times out missing replies and can reject a disposed session.', '出站 frame id 关联请求／响应传输包，与 turn_id、session_id 或 broker request_id 不同。sendCommand/sendPrompt 只负责提交，其请求 ID 不注册为等待响应；完成通过 turn_ended 等 ClientEvent 到达。通过事件响应的产品操作须用载荷请求标识与会话身份共同关联；SessionEventCorrelator 拒绝重复请求、对缺失响应超时，并可拒绝已销毁会话。'),
      text('Keep three failure surfaces distinct: BridgeWireError is an envelope response with numeric code/message; streamed ClientEvent.error reports runtime failure during an accepted operation; ClientError is the native submit call-result error with transport/protocol/rejected/not_found/internal classes. A successful send or native submit does not establish a successful model turn. Wait for the terminal outcome and retain diagnostic context without logging auth tokens or provider credentials.', '区分三个失败接口：BridgeWireError 是带数字 code/message 的传输响应；流式 ClientEvent.error 报告已接受操作中的运行时故障；ClientError 是原生 submit 调用结果错误，分为 transport/protocol/rejected/not_found/internal。发送或原生 submit 成功不代表模型回合成功；应等待终态结果，并保留不含认证 token 或 provider 凭证的诊断上下文。'),
      text('The endpoint reads at most 16 MiB per frame. BridgeClient checks UTF-8 serialized frame bytes before sending, protecting the whole connection from an oversized prompt, image or audio response. Socket close rejects pending envelope requests and ends subscribed iterators. Server connection teardown cancels owned turn/broker work; a host reconnect must restore persisted conversation state and clear old pending interactions, rather than automatically replaying a command whose result is uncertain.', '端点每帧最多读取 16 MiB。BridgeClient 在发送前检查序列化帧的 UTF-8 字节数，避免过大的提示、图片或音频响应影响整个连接。Socket 关闭会拒绝等待中的传输请求并结束订阅迭代器。服务端连接清理会取消归属回合及 broker 工作；宿主重连须恢复持久会话状态、清除旧等待交互，不能自动重放结果不确定的命令。'),
    ],
    tables: [{ headers: [text('Identifier', '标识'), text('Owner / use', '归属／用途')], rows: [
      [text('Frame.payload.id', 'Frame.payload.id'), text('One connection request/response exchange.', '一个连接内的请求／响应交换。')],
      [text('session_id', 'session_id'), text('Durable conversation and runtime ownership.', '持久会话与运行时归属。')],
      [text('turn_id', 'turn_id'), text('One turn and targeted cancellation; fields may be optional.', '单个回合与定向取消；字段可能可选。')],
      [text('permission / questionnaire request_id', '权限／问卷 request_id'), text('Pending interaction broker request, answered once.', '待处理交互 broker 请求，单次回答。')],
      [text('AudioOperationIdDto', 'AudioOperationIdDto'), text('Full id + generation + service_epoch identity.', '完整 id + generation + service_epoch 身份。')],
    ] }],
  },
  {
    id: 'bridge-validation',
    title: text('Integration verification and troubleshooting', '集成验证与排障'),
    paragraphs: [
      text('The product package tests cover handshake refusal, frame bounds, malformed inbound DTOs, event iterator ownership, computer access and question correlation. Run them before changing a shared contract. The source index points to declarations; protocol fixtures and Rust round-trip tests establish serialization behavior. They do not establish a remote server service, physical audio device or real provider acceptance.', '产品包测试覆盖握手拒绝、帧限制、非法入站 DTO、事件迭代器归属、电脑访问和问卷关联。修改共享契约前应运行这些检查。源码索引定位声明；协议 fixture 与 Rust round-trip 测试验证序列化行为，不能证明远程服务、真实音频设备或真实 provider 验收。'),
      text('No lockfile means the intended service is not discoverable; an HTTP 401 means an upgrade-token mismatch; incompatible hello means a server/client build mismatch; a turn with no events may indicate a late iterator subscription or a parked permission/question. For multiple sessions use the host-provided exact lockfilePath instead of assuming the most recently modified file is the desired session. Never copy lockfile authToken into a webpage, example repository or diagnostic report.', '没有 lockfile 表示目标服务无法发现；HTTP 401 表示升级 token 不匹配；hello 不兼容表示服务端／客户端构建不匹配；回合没有事件可能是迭代器订阅过晚，或权限／问卷仍在等待。多会话应使用宿主提供的明确 lockfilePath，不能假定最近修改文件就是目标会话。不要把 lockfile authToken 复制到网页、示例仓库或诊断报告。'),
    ],
    code: [{ label: text('Bridge package and protocol checks', 'Bridge 包与协议检查'), language: 'bash', verification: 'manual', code: 'npm --prefix packages/bridge-client run build\nnpm --prefix packages/bridge-client test\ncargo test --locked -p bridge --test protocol_roundtrip_test' }],
    links: [{ label: text('Desktop lifecycle', '桌面生命周期'), href: '/docs/desktop-host' }, { label: text('Native callback integration', '原生回调集成'), href: '/docs/platform-integration' }],
  },
);

bridgePages[1].sections.push(
  {
    id: 'native-facade',
    title: text('Public native facade and shared ownership', '公开原生 facade 与共享归属'),
    paragraphs: [
      text('Generated bindings expose the annotated Rust facade, not every declaration in the product repository. Engine constructors build an Arc<MobileEngineHandle> and translate product callbacks into the shared Harness services. build_android_cron_store/build_ios_cron_store create scheduled-task storage without constructing an LLM client or reading provider credentials; their managed project-path validation still applies.', '生成绑定暴露带注解的 Rust facade，不是产品仓库中的每个声明。Engine 构造函数建立 Arc<MobileEngineHandle>，并将产品回调转换为共享 Harness 服务。build_android_cron_store/build_ios_cron_store 创建计划任务存储时不构造 LLM client、不读取 provider 凭证，但仍执行托管项目路径校验。'),
      text('Engine launch configuration names provider, model, project, app-private paths and host environment. Android construction prepares its required PRoot runtime; legacy shell and Git configuration fields do not replace that runtime. Native secure-storage callbacks provide credential access while the UI receives display-safe state. Host capability registration determines which tools are exposed, so an absent location/camera/automation service must not be reported as supported.', 'Engine 启动配置指定 provider、模型、项目、应用私有路径与宿主环境。Android 构造会准备所要求的 PRoot 运行时；旧 shell 与 Git 配置字段不能替代该运行时。原生安全存储回调提供凭证访问，UI 接收可展示状态。宿主能力注册决定暴露的工具，缺少定位／相机／自动化服务时不能报告为支持。'),
    ],
    tables: [{ headers: [text('Facade', 'Facade'), text('Result / consumer responsibility', '结果／消费者职责')], rows: [
      [text('build_ios_engine_with_config / build_android_engine', 'build_ios_engine_with_config / build_android_engine'), text('Create and retain the engine plus every native callback implementation.', '创建并保留 engine 及所有原生回调实现。')],
      [text('MobileEngineHandle.submit(ClientCommand)', 'MobileEngineHandle.submit(ClientCommand)'), text('Await call-result ClientError; receive accepted command output through listener callbacks.', '等待调用结果 ClientError；已接受命令输出通过监听回调到达。')],
      [text('MobileEngineHandle.mobile_linux_status / verify_mobile_linux_rootfs / repair_mobile_linux_rootfs / reset_mobile_linux_rootfs', 'MobileEngineHandle.mobile_linux_status / verify_mobile_linux_rootfs / repair_mobile_linux_rootfs / reset_mobile_linux_rootfs'), text('Read or change the managed image lifecycle; expose typed errors and restart requirements.', '读取或改变托管镜像生命周期；展示类型化错误及重启要求。')],
      [text('IosEventListener / AndroidEventListener', 'IosEventListener / AndroidEventListener'), text('on_event enqueues DTOs; on_workflow_progress carries session/task/run-owned structured progress.', 'on_event 将 DTO 入队；on_workflow_progress 携带归属于会话／任务／运行的结构化进度。')],
      [text('IosPermissionSink / AndroidPermissionSink', 'IosPermissionSink / AndroidPermissionSink'), text('Enqueue the prompt and return promptly; resolve later through submit.', '将提示入队并及时返回；随后通过 submit 处理决定。')],
      [text('IosAudioService / AndroidAudioService', 'IosAudioService / AndroidAudioService'), text('Snapshot capabilities, execute a structured operation, cancel its full identity.', '读取能力快照、执行结构化操作、按完整身份取消。')],
    ] }],
  },
  {
    id: 'native-lifecycle',
    title: text('Screen attachment, audio identity and disposal', '界面挂接、音频身份与销毁'),
    paragraphs: [
      text('Native listeners must enqueue onto the UI executor and return promptly; waiting for a user decision inside on_request would block the call boundary. Register and retain the permission sink with the engine, then answer through ClientCommand. UI replacement uses a new source generation and closes the previous source; late callbacks must remain attached to their original session/source rather than updating the replacement conversation.', '原生监听器须向 UI executor 入队并及时返回；在 on_request 内等待用户决定会阻塞调用边界。与 engine 一起注册并保留权限 sink，再通过 ClientCommand 回答。UI 替换使用新的 source generation 并关闭旧 source，迟到回调必须仍归属于原始会话／source，不能更新替换后的对话。'),
      text('AudioOperationIdDto combines a UUID id, monotonic generation and AudioService service_epoch. AudioOwnerDto describes stable session, UI or system resource ownership; initiator attribution is separate. Capabilities must not prompt for permission. Execute returns one structured terminal result within the remaining timeout budget, and cancel must match the complete identity so an obsolete request cannot stop a replacement service operation.', 'AudioOperationIdDto 组合 UUID id、单调 generation 与 AudioService service_epoch。AudioOwnerDto 表示稳定会话、UI 或系统资源归属，initiator 调用归因独立。capabilities 不能弹出权限请求。execute 在剩余超时预算内返回一个结构化终态结果，cancel 必须匹配完整身份，避免过期请求停止替换服务的新操作。'),
      text('Releasing the engine aborts its task notification watcher and schedules bounded owner/audio, session Cron and LSP cleanup on the process automation runtime. Drop must not synchronously join a callback that needs Swift MainActor; the native source owns closing and detaching its UI first. An app process lifetime and the iSH kernel lifetime remain independent of a conversation handle.', '释放 engine 会终止任务通知 watcher，并在进程 automation runtime 上安排有预算限制的归属／音频、会话 Cron 与 LSP 清理。Drop 不能同步等待需要 Swift MainActor 的回调；原生 source 先负责关闭和解除 UI 挂接。应用进程与 iSH 内核生命周期仍独立于会话句柄。'),
    ],
    steps: [text('Initialize app-private storage, resource paths and runtime configuration off the UI thread.', '在 UI 线程之外初始化应用私有存储、资源路径及运行时配置。'), text('Construct native services, event listener and permission sink; build one session engine.', '构造原生服务、事件监听器及权限 sink；建立会话 engine。'), text('Submit typed commands, enqueue listener events, and correlate interactions with their original source.', '提交类型化命令、将监听事件入队，并按原始 source 关联交互。'), text('Close the source on replacement/disposal, end owned audio and discard stale callback generations.', '替换／销毁时关闭 source、结束归属音频，并丢弃过期回调 generation。')],
  },
  {
    id: 'native-failures-and-acceptance',
    title: text('Native failure handling and acceptance', '原生失败处理与验收'),
    paragraphs: [
      text('Treat binding/library mismatch as a build contract failure and regenerate both from the same source revision. Off-platform constructors return PlatformUnavailable; a simulator can instantiate product API paths while its Linux backend remains unavailable. Missing rootfs, denied OS permission, provider authentication failure and command rejection are different failures and should preserve their typed context.', '绑定／库不匹配属于构建契约故障，应从同一源码修订重新生成二者。非目标平台构造函数返回 PlatformUnavailable；模拟器可实例化产品 API 路径，但 Linux 后端仍不可用。rootfs 缺失、系统权限拒绝、provider 认证失败及命令拒绝是不同故障，应保留其类型化上下文。'),
      text('Host unit tests verify adapters and shared-session behavior; generated Swift/Kotlin compilation checks cross-component converters. A signed-device acceptance run must cover turn streaming, permission deny/allow/cancel, question responses, camera/share callbacks, audio epoch replacement, session disposal and rootfs restart behavior. This handbook provides contracts and repeatable commands, without claiming that documentation examples have validated those physical devices.', '宿主单元测试验证 adapter 与共享会话行为；生成 Swift／Kotlin 编译检查跨组件转换器。已签名设备验收须覆盖回合流式事件、权限拒绝／允许／取消、问卷响应、相机／分享回调、音频 epoch 替换、会话销毁及 rootfs 重启行为。本手册提供契约与可重复命令，不声称文档示例已经验收这些实体设备。'),
    ],
    code: [{ label: text('Native facade host compilation', '原生 facade 宿主编译'), language: 'bash', verification: 'manual', code: 'cargo check --locked -p ios-framework --features uniffi\ncargo check --locked -p android-aar --features uniffi' }],
    links: [{ label: text('Native build and release matrix', '原生构建与发布矩阵'), href: '/docs/platform-builds' }, { label: text('Mobile Linux lifecycle', 'Mobile Linux 生命周期'), href: '/docs/mobile-linux' }],
  },
);

for (const page of bridgePages) {
  for (const section of page.sections) {
    for (const block of section.code ?? []) block.verification ??= 'excerpt';
  }
}
