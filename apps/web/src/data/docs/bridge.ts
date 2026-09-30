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
            label: 'Build from the product checkout',
            language: 'bash',
            code: 'cargo build --locked -p bridge-server --bin bridge-server\nnpm --prefix packages/bridge-client ci\nnpm --prefix packages/bridge-client run build',
          },
          {
            label: 'Local package dependency',
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
            en: 'sendPrompt() submits a command without waiting for its reply. Text, tool activity, and the final turn outcome arrive through ClientEvent. The example denies permission requests so that an unattended demonstration cannot leave a permission gate parked; a product host should present the request and apply the user’s decision.',
            zh: 'sendPrompt() 提交命令后立即返回，不等待回合结果。文本、工具活动与最终回合结果通过 ClientEvent 到达。示例拒绝权限请求，避免无人值守的演示停在权限门控上；产品宿主应展示请求并应用用户的决定。',
          },
        ],
        code: [
          {
            label: 'Node.js / TypeScript',
            language: 'typescript',
            code: `import { BridgeClient } from '@lingxi/bridge-client';

const client = new BridgeClient({ clientName: 'documentation-example' });
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

  for await (const event of client.events()) {
    if (event.type === 'text_delta') process.stdout.write(event.text);
    if (event.type === 'turn_ended') {
      console.log(event.outcome, event.cost);
      break;
    }
  }
} finally {
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
            signature: 'sendPrompt(text: string, opts: { images?: ImageRefDto[]; turnId?: number } = {}): void',
            description: { en: 'Submit text with optional stable image references and a caller-supplied turn identifier.', zh: '提交文本，可附带稳定图片引用及调用方指定的回合标识。' },
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
            description: { en: 'Consume ordered buffered events until the connection closes. All iterators share one queue; multiple consumers divide events rather than each receiving a copy.', zh: '按顺序消费缓冲事件，直到连接关闭。多个迭代器共享一个队列，因此会分配事件，而不是各自收到副本。' },
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
            label: 'iOS / Xcode',
            language: 'bash',
            code: 'bash apps/ios/native/scripts/build-xcframework.sh\ncd apps/ios/native\nxcodegen generate',
          },
          {
            label: 'Android / Gradle',
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
            label: 'iOS constructor contract',
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
            label: 'Android constructor contract',
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
            label: 'Swift / existing MobileEngineHandle',
            language: 'swift',
            code: 'try await handle.submit(command: .listModels)\ntry await handle.submit(command: .getConversationControls)',
          },
          {
            label: 'Kotlin / existing MobileEngineHandle',
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
