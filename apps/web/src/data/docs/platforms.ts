import type { DocPage, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const product = 'https://github.com/lingxi-coder/lingxi-app/tree/main';

export const platformPages: DocPage[] = [
  {
    id: 'desktop-host', group: 'bridge',
    title: t('Desktop · Electron host', '桌面 · Electron 宿主'),
    description: t('Session-owned Bridge sidecars, renderer IPC, event replay, native services and signed packaging.', '归属于会话的 Bridge sidecar、renderer IPC、事件重放、原生服务与签名打包。'),
    packageName: 'lingxi-code-desktop / bridge-server', sourceUrl: `${product}/apps/electron/src/main/bridge.ts`,
    sources: [
      { repo: 'lingxi-app', path: 'apps/electron/src/main/index.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/bridge.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/bridgeDiscovery.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/validation.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/host-utils.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/preload/index.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/preload/event-replay.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/src/renderer/bridge/useBridge.ts' },
      { repo: 'lingxi-app', path: 'apps/electron/scripts/package-macos-flare.sh' },
      { repo: 'lingxi-app', path: 'apps/electron/package.json' },
    ],
    sections: [
      {
        id: 'desktop-ownership', title: t('Composition and ownership', '装配与归属'),
        paragraphs: [
          t('Electron main is the desktop composition host. SessionRuntimeManager owns per-session SessionRuntime objects, each with project context, Bridge connection, optional child-process ownership, lifecycle generation and pending interactions. Renderer components consume the preload API and shared DTOs; they do not start the Rust sidecar or read its lockfile token directly.', 'Electron main 是桌面装配宿主。SessionRuntimeManager 拥有各会话的 SessionRuntime，每个对象携带项目上下文、Bridge 连接、可选子进程归属、生命周期 generation 及待处理交互。Renderer 组件消费 preload API 与共享 DTO，不直接启动 Rust sidecar 或读取其 lockfile token。'),
          t('The Rust bridge-server resolves product launch state and routes ClientCommand into Harness desktop assembly. Provider communication remains in LLM Client; native device audio, account/credential services, filesystem selection and windows belong to the host. A browser docs page is a separate application and does not share this in-process Electron transport.', 'Rust bridge-server 解析产品启动状态，并将 ClientCommand 路由到 Harness desktop 装配。Provider 通信仍由 LLM Client 负责；原生设备音频、账号／凭证服务、文件系统选择及窗口由宿主负责。浏览器文档页面是独立应用，不共享此 Electron 进程内传输。'),
        ],
        diagram: { src: '/docs-assets/platform-host-transports.svg', alt: t('Renderer IPC reaches Electron main, which authenticates its Bridge sidecar; mobile uses UniFFI.', 'Renderer IPC 进入 Electron main，再认证其 Bridge sidecar；移动端使用 UniFFI。') },
      },
      {
        id: 'desktop-flow', title: t('Startup and renderer attachment', '启动与 renderer 挂接'),
        paragraphs: [
          t('Packaged builds resolve bridge-server only from process.resourcesPath/bin. Development can use an explicit serverBin or LINGXI_BRIDGE_SERVER_BIN, then search workspace target/debug and target/release. This avoids a packaged application silently adopting a development executable. Reusable processes are checked for live process identity, private owned lockfiles and compatible launch identity before adoption.', '打包构建仅从 process.resourcesPath/bin 解析 bridge-server。开发环境可使用明确的 serverBin 或 LINGXI_BRIDGE_SERVER_BIN，再搜索工作区 target/debug 与 target/release，避免打包应用静默采用开发可执行文件。复用进程前检查存活进程身份、私有且归属正确的 lockfile，以及兼容的启动身份。'),
          t('The preload keeps a sequenced runtime replay buffer for renderer attachment. Conversation hydration and live delivery must share source generation and session identity, so older resume results cannot overwrite the active conversation. Disconnect/restart rejects outstanding correlated operations and clears the previous interactions; the host distinguishes its owned process from an adopted external process when disposing.', 'Preload 为 renderer 挂接保留有序号的运行时重放缓冲。会话 hydration 与实时交付须共享 source generation 及会话身份，避免旧恢复结果覆盖当前对话。断开／重启会拒绝待关联操作并清除旧交互；销毁时宿主区分自有进程与采用的外部进程。'),
        ],
        steps: [
          t('Resolve public settings, credential references, active project and session.', '解析公开设置、凭证引用、活动项目与会话。'),
          t('Resolve or start the sidecar using the host launch identity and exact discovery file.', '根据宿主启动身份及明确发现文件解析或启动 sidecar。'),
          t('Connect Bridge, verify hello and install permission, question, event and audio handlers.', '连接 Bridge、校验 hello，安装权限、问卷、事件与音频处理器。'),
          t('Hydrate persisted transcript, attach sequenced replay and render subsequent live events.', '恢复持久 transcript，挂接有序重放，再渲染后续实时事件。'),
        ],
      },
      {
        id: 'desktop-ipc', title: t('IPC, native services and credentials', 'IPC、原生服务与凭证'),
        paragraphs: [
          t('BrowserWindow enables sandbox and contextIsolation. Preload exposes explicit command and subscription methods; main validates the sender, session reference, command variant and payload before forwarding. A new runtime command requires matching Rust/TypeScript DTOs, main admission, preload typing and renderer ownership. A typed renderer value alone is not admission evidence.', 'BrowserWindow 开启 sandbox 与 contextIsolation。Preload 暴露明确的命令及订阅方法；main 在转发前校验发送方、会话引用、命令变体与载荷。新增运行时命令需要匹配的 Rust／TypeScript DTO、main 准入、preload 类型及 renderer 归属；仅有 renderer 类型值不能证明准入。'),
          t('Main resolves provider credentials at launch through the secure host services and sends only what the selected engine needs. Diagnostics use sanitizeDiagnostic and bounded buffers. openai_oauth_updated is handled as a credential event and is excluded from Bridge iterator replay. On macOS the signed Credential Broker verifies packaged identity, so unpackaged npm dev is unsuitable for proving secure credential persistence.', 'Main 在启动时通过安全宿主服务解析 provider 凭证，仅发送选定 engine 所需的值。诊断使用 sanitizeDiagnostic 与有上限缓冲。openai_oauth_updated 作为凭证事件处理，并排除在 Bridge 迭代器重放之外。macOS 的已签名 Credential Broker 校验打包身份，因此未打包 npm dev 不适合证明安全凭证持久化。'),
        ],
        tables: [{ headers: [t('Boundary', '边界'), t('Owned behavior', '所负责行为')], rows: [
          [t('Renderer → preload/main', 'Renderer → preload/main'), t('Typed IPC, trusted sender and session validation.', '类型化 IPC、可信发送方与会话校验。')],
          [t('Main → Bridge', 'Main → Bridge'), t('Lockfile authorization, hello, lifecycle and broker responses.', 'Lockfile 认证、hello、生命周期及 broker 响应。')],
          [t('Main → native services', 'Main → 原生服务'), t('Audio leases, OS dialogs, credential storage and desktop windows.', '音频租约、系统弹窗、凭证存储与桌面窗口。')],
          [t('Bridge → Harness', 'Bridge → Harness'), t('Session/turn execution, permission gates, tools and persisted history.', '会话／回合执行、权限门控、工具及持久历史。')],
        ] }],
      },
      {
        id: 'desktop-development', title: t('Development, packaging and failure diagnosis', '开发、打包与故障诊断'),
        paragraphs: [
          t('Build the private Bridge client before Electron so file dependencies contain matching dist exports. A source/typecheck success establishes JavaScript integration; sidecar and package verification additionally check resource layout, signing and actual process launch. The macOS release path is package:mac:flare, which preserves source path remapping, inside-out signing and package verification with the configured Flare team.', '在 Electron 前构建私有 Bridge 客户端，确保 file 依赖含有匹配的 dist 导出。源码／类型检查成功验证 JavaScript 集成；sidecar 与包验证还检查资源布局、签名及实际进程启动。macOS 发布路径使用 package:mac:flare，保留源码路径映射、由内到外签名，并使用已配置 Flare team 验证打包。'),
          t('Diagnose by boundary: missing packaged binary → resources; HTTP 401 → discovery token; hello refusal → protocol/build pairing; pending question/permission → broker state; stale UI → replay generation; rejected macOS credentials → signed application/broker identity. Focused renderer tests do not validate operating-system grants, audio devices or Windows/Linux packaging.', '按边界排查：打包二进制缺失对应资源；HTTP 401 对应发现 token；hello 拒绝对应协议／构建配对；等待问卷／权限对应 broker 状态；旧 UI 对应重放 generation；macOS 凭证拒绝对应已签名应用／broker 身份。Renderer 专项测试不能验证系统授权、音频设备或 Windows／Linux 打包。'),
        ],
        code: [{ label: t('Desktop source checks', '桌面源码检查'), language: 'bash', verification: 'manual', code: 'cargo build --locked -p bridge-server --bin bridge-server\nnpm --prefix packages/bridge-client ci\nnpm --prefix packages/bridge-client run build\nnpm --prefix apps/electron ci\nnpm --prefix apps/electron run typecheck\nnpm --prefix apps/electron test\nnpm --prefix apps/electron run build' }, { label: t('macOS signed package workflow', 'macOS 签名打包流程'), language: 'bash', verification: 'manual', code: 'cd apps/electron\nnpm run package:mac:flare -- --check\nnpm run package:mac:flare\nnpm run verify:package' }],
        links: [{ label: t('Bridge wire details', 'Bridge 传输详情'), href: '/docs/bridge-client' }, { label: t('Build matrix', '构建矩阵'), href: '/docs/platform-builds' }],
      },
    ],
  },
  {
    id: 'cli-tui', group: 'bridge',
    title: t('CLI, TUI and headless hosts', 'CLI、TUI 与无界面宿主'),
    description: t('Use the product command line, distinguish terminal and print modes, and integrate deterministic output without bypassing runtime admission.', '使用产品命令行、区分终端与 print 模式，并在保留运行时准入的前提下集成确定输出。'),
    packageName: 'cli / tui / tui-core', sourceUrl: `${product}/apps/cli/host/src/entrypoint.rs`,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/Cargo.toml' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/entrypoint.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/init.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/mode.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/headless_host.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/sigint.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/exit_codes.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/tui/src/lib.rs' },
      { repo: 'harness-runtime', path: 'crates/runtime/src/headless/mod.rs' },
    ],
    sections: [
      {
        id: 'cli-assembly', title: t('Host and runtime boundary', '宿主与运行时边界'),
        paragraphs: [
          t('The cli package builds lingxi-cli. apps/cli/host owns argv/environment resolution, startup, stdout/stderr, signals and terminal handoff; apps/cli/tui renders the interactive terminal and tui-core shares UI state. Initialization produces Harness DesktopConfig and delegates agent assembly to the desktop facade. CLI and Electron are different hosts around the same runtime layers.', 'cli 包构建 lingxi-cli。apps/cli/host 负责 argv／环境解析、启动、stdout／stderr、信号及终端切换；apps/cli/tui 渲染交互终端，tui-core 共享 UI 状态。初始化产生 Harness DesktopConfig，并将 Agent 装配交给 desktop facade。CLI 与 Electron 是同一运行时分层上的不同宿主。'),
          t('A reusable application should depend on the Harness public facade rather than import the product CLI crate. Product headless execution still receives a DesktopHostServices implementation for OAuth descriptors, environment updates, launch identity and runtime-ready hooks. Output selection does not replace provider credentials, project validation, permission policy or persisted session setup.', '可复用应用应依赖 Harness 公共 facade，不应导入产品 CLI crate。产品 headless 执行仍接收 DesktopHostServices 实现，提供 OAuth 描述、环境更新、启动身份及 runtime-ready hooks。选择输出格式不能替代 provider 凭证、项目校验、权限策略或持久会话建立。'),
        ],
      },
      {
        id: 'cli-mode-selection', title: t('Mode selection and output contracts', '模式选择与输出契约'),
        paragraphs: [
          t('decide_mode selects Print for a nonempty prompt; otherwise --no-tui or non-terminal stdin/stdout selects StdioRepl; a full terminal selects Tui. Startup and argv gates further establish valid combinations. Use -p explicitly for automated one-shot runs. Print-only controls include output/input format, budget/turn limits, structured schema and fallback model; interactive behavior should not be inferred from those flags.', 'decide_mode 对非空 prompt 选择 Print；否则 --no-tui 或 stdin／stdout 非终端时选择 StdioRepl，完整终端选择 Tui。启动与 argv 门控进一步限定有效组合。自动单次运行应明确使用 -p。Print 专用控制包括输出／输入格式、预算／回合限制、结构化 schema 与 fallback model，不能据此推断交互行为。'),
          t('text prints the human-facing result; json returns a single structured result; stream-json is newline-delimited realtime input/output for programmatic consumers. Preserve record framing and separate diagnostic stderr from protocol stdout. The stream-json shape belongs to the Harness headless protocol, whereas Bridge uses Frame over WebSocket and native clients use UniFFI; consumers must choose one transport contract.', 'text 输出面向人的结果，json 返回单个结构化结果，stream-json 为程序消费者提供换行分隔的实时输入／输出。保留记录分帧，将诊断 stderr 与协议 stdout 分离。stream-json 形状属于 Harness headless 协议；Bridge 使用 WebSocket 上的 Frame，原生客户端使用 UniFFI，消费者须选择一个传输契约。'),
        ],
        tables: [{ headers: [t('Mode', '模式'), t('Use', '用途'), t('Output', '输出')], rows: [
          [t('Tui', 'Tui'), t('Interactive terminal with full stdin/stdout TTY.', 'stdin／stdout 均为 TTY 的交互终端。'), t('Terminal UI and direct interaction.', '终端 UI 与直接交互。')],
          [t('StdioRepl', 'StdioRepl'), t('--no-tui or non-TTY fallback.', '--no-tui 或非 TTY 回退。'), t('Line-oriented conversation.', '按行会话。')],
          [t('Print', 'Print'), t('One-shot prompt, automation and scripts.', '单次提示、自动化与脚本。'), t('text / json / stream-json.', 'text / json / stream-json。')],
        ] }],
      },
      {
        id: 'cli-permission-and-session', title: t('Permission, session and cancellation behavior', '权限、会话与取消行为'),
        paragraphs: [
          t('Runtime initialization loads permitted configuration sources and resolves model/provider state before execution. Explicit --model overrides environment and persisted selection. Print mode enables deny_unresolved_ask so unattended execution cannot leave a user permission prompt parked; --permission-prompts none applies the same no-dialog policy. Preapproved rules still act through the runtime permission machinery.', '运行时初始化先加载允许的配置来源，并在执行前解析模型／provider 状态。明确 --model 优先于环境与持久选择。Print 模式开启 deny_unresolved_ask，避免无人值守执行停在用户权限提示；--permission-prompts none 采用相同无弹窗策略。预批准规则仍通过运行时权限机制处理。'),
          t('Resume restores persisted session state through the shared session lifecycle; UI row state alone is insufficient. Ctrl+C cancels owned turn work and uses the product signal/cleanup path. TUI queued prompts and slash operations carry submission order and source generation so cancellation or session reset cannot publish an old result into the replacement session. Background session attach is a separate product lifecycle from shell process output.', 'Resume 通过共享会话生命周期恢复持久状态，仅有 UI 行状态并不足够。Ctrl+C 取消归属回合工作，并使用产品信号／清理路径。TUI 排队提示与 slash 操作携带提交顺序和 source generation，避免取消或会话重置后旧结果发布到替换会话。后台会话 attach 是独立于 shell 进程输出的产品生命周期。'),
        ],
        code: [{ label: t('CLI consumer examples from the product root', '从产品根目录执行 CLI 示例'), language: 'bash', verification: 'manual', code: 'cargo build --locked -p cli --bin lingxi-cli\n./target/debug/lingxi-cli --help\n./target/debug/lingxi-cli -p "Explain the project structure" --output-format json\n./target/debug/lingxi-cli -p "Summarize the selected files" --output-format stream-json --verbose\n./target/debug/lingxi-cli --no-tui' }],
        note: t('Prompt runs require your configured provider and may call a real model. --help validates the CLI surface without a model call. These examples do not enable a permission bypass.', '提示运行需要已配置 provider，可能调用真实模型。--help 不调用模型即可验证 CLI 接口。这些示例不启用权限绕过。'),
      },
      {
        id: 'cli-diagnostics', title: t('Exit status and verification', '退出状态与验证'),
        paragraphs: [
          t('The product exit constants are success 0, runtime/argv error 1 and SIGINT 130; reserved not-implemented paths use 64. Automation must inspect both process status and the final structured record, because argument validation failure can produce no turn output. A nonzero provider result is not resolved by re-parsing it as a successful JSON response.', '产品退出常量为成功 0、运行时／argv 错误 1、SIGINT 130；保留的未实现路径使用 64。自动化须同时检查进程状态与最终结构化记录，因为参数校验失败可能不产生回合输出。不能把非零 provider 结果重新解析成成功 JSON 响应来消除故障。'),
          t('Run CLI mode/argv and host tests to establish dispatch and headless behavior, then use deterministic fake-provider checks for serialization. Real-provider cancellation, terminal restoration across shells and operating-system process-tree cleanup require their own runtime acceptance. Do not call a parser/unit suite full terminal or model parity evidence.', '运行 CLI mode／argv 与宿主测试，验证分派及 headless 行为，再使用确定的 fake-provider 检查序列化。真实 provider 取消、不同 shell 的终端恢复及系统进程树清理需要各自运行验收，不能把 parser／单元套件称为完整终端或模型对齐证据。'),
        ],
        code: [{ label: t('Focused CLI checks', 'CLI 专项检查'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p cli --lib\ncargo check --locked -p tui -p tui-core' }],
        links: [{ label: t('Harness integration', 'Harness 接入'), href: '/docs/harness' }, { label: t('Desktop host', '桌面宿主'), href: '/docs/desktop-host' }],
      },
    ],
  },
  {
    id: 'web-product', group: 'bridge',
    title: t('Web · SDK site and UI prototype', 'Web · SDK 官网与 UI 原型'),
    description: t('The current browser application publishes SDK documentation and demonstrates account/console UI without a connected runtime backend.', '当前浏览器应用发布 SDK 文档，并在未连接运行时后端的情况下展示账号／控制台 UI。'),
    packageName: 'lingxi-web', sourceUrl: `${product}/apps/web/src/app/App.tsx`,
    sources: [
      { repo: 'lingxi-app', path: 'apps/web/src/app/App.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/app/router.ts' },
      { repo: 'lingxi-app', path: 'apps/web/src/app/usePersistentState.ts' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/DocsPage.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/LoginPage.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/ConsolePages.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/data/mockData.ts' },
      { repo: 'lingxi-app', path: 'apps/web/src/utils/apiKeys.ts' },
      { repo: 'lingxi-app', path: 'apps/web/package.json' },
      { repo: 'lingxi-app', path: 'apps/web/wrangler.jsonc' },
    ],
    sections: [
      {
        id: 'web-current-boundary', title: t('Current implementation boundary', '当前实现边界'),
        paragraphs: [
          t('apps/web is a React/Vite application with public pages, SDK documentation and a developer-console UI prototype. Documentation routes render local authored data and the generated source declaration index. Login displays an explicit prototype notice: submission sets UI state and navigates to the console; it sends no verification message and creates no real account.', 'apps/web 是 React／Vite 应用，包含公开页面、SDK 文档与开发者控制台 UI 原型。文档路由渲染本地人工维护数据及生成的源码声明索引。Login 明确展示原型说明：提交只设置 UI 状态并进入控制台，不发送验证消息，也不创建真实账号。'),
          t('Console account usage, balances and key records originate from mockData. API-key actions update local component state with prototype helpers, not a production authorization service. The site is not a hosted LingXi inference API, WebSocket Bridge host or remote agent session service. Working desktop/native execution is provided by their corresponding product hosts.', '控制台账号用量、余额与密钥记录来自 mockData。API-key 操作通过原型辅助函数更新本地组件状态，不是生产授权服务。本站不是托管 LingXi 推理 API、WebSocket Bridge 宿主或远程 Agent 会话服务。实际桌面／原生执行由相应产品宿主提供。'),
        ],
      },
      {
        id: 'web-route-and-state', title: t('Routes, local state and documentation data', '路由、本地状态与文档数据'),
        paragraphs: [
          t('The router normalizes path casing and separators, admits a fixed public/console route list and checks /docs/<id> against docPages. Unknown paths resolve to the public home page. App persists locale, region and theme in browser storage, updates document language and theme, and restores a section hash on route changes. Browser persistence is preference state, not durable runtime conversation storage.', 'Router 规范路径大小写与分隔符，准入固定公开／控制台路由，并通过 docPages 检查 /docs/<id>。未知路径回到公开首页。App 在浏览器存储中保存语言、地区与主题，更新文档语言和主题，并在路由变更时恢复章节 hash。浏览器持久化属于偏好状态，不是持久运行时会话存储。'),
          t('Technical prose, bilingual structured tables, steps, code labels, SVG descriptions and source references live in src/data/docs. DocsPage consumes that schema for navigation, search, API browsing and section rendering; repository Markdown is generated from the same records. Keep factual tables in this data instead of encoding a second copy inside JSX.', '技术正文、双语结构表格、步骤、代码标签、SVG 描述与来源引用位于 src/data/docs。DocsPage 消费同一 schema 以生成导航、搜索、API 浏览和章节渲染；仓库 Markdown 也从这些记录生成。事实表格应保留在数据中，不应在 JSX 再维护一份副本。'),
        ],
        tables: [{ headers: [t('Route', '路由'), t('Current data/source', '当前数据／来源')], rows: [
          [t('/docs and /docs/<id>', '/docs 与 /docs/<id>'), t('Authored bilingual guide data and generated API/source index.', '人工维护双语指南数据与生成 API／来源索引。')],
          [t('/login', '/login'), t('Form prototype with local state and delayed navigation.', '使用本地状态及延迟导航的表单原型。')],
          [t('/console/*', '/console/*'), t('Mock usage/billing/key records and local UI actions.', '模拟用量／账单／密钥记录及本地 UI 动作。')],
          [t('/download /pricing', '/download /pricing'), t('Public product information and configured prototype records.', '公开产品信息及已配置原型记录。')],
        ] }],
      },
      {
        id: 'web-build-and-hosting', title: t('Build, preview and static hosting', '构建、预览与静态托管'),
        paragraphs: [
          t('Build runs Vite and writes dist. The checked-in Cloudflare configuration serves dist as SPA assets and declares a route; it contains no worker entry for account, billing or agent execution. A successful static page response establishes frontend serving only. Publication and backend connection require separate authorized work and acceptance.', 'Build 运行 Vite 并写入 dist。已检入的 Cloudflare 配置把 dist 作为 SPA assets 服务并声明路由，没有账号、计费或 Agent 执行 worker 入口。静态页面响应成功只证明前端服务可用；发布及后端连接需要独立授权工作与验收。'),
          t('Run documentation synchronization from the complete local workspace when refreshing source identity. Documentation-only CI can check saved content/output parity without sibling SDK checkouts. A stale API index is a documentation generation failure and should not be interpreted as a provider/network failure. Preserve current locale/theme behavior during schema additions.', '刷新源码身份时，在完整本地工作区运行文档同步。纯文档 CI 可在没有相邻 SDK checkout 时检查保存内容／输出一致性。API 索引过期属于文档生成故障，不能解释成 provider／网络故障。Schema 新增期间保留当前语言／主题行为。'),
        ],
        code: [{ label: t('Website development from product root', '从产品根目录开发官网'), language: 'bash', verification: 'manual', code: 'npm --prefix apps/web ci\nnpm --prefix apps/web run docs:sync\nnpm --prefix apps/web run docs:check\nnpm --prefix apps/web run typecheck\nnpm --prefix apps/web test\nnpm --prefix apps/web run build\nnpm --prefix apps/web run dev -- --host 127.0.0.1' }],
      },
      {
        id: 'web-verification', title: t('Functional checks and open integrations', '功能检查与开放集成'),
        paragraphs: [
          t('Check direct /docs/<id>#section entry, locale switching, search across translated prose, API filters, code copying, SVG accessibility text, light/dark themes and narrow-screen tables. Exported Markdown must retain the same content and links as the website. A route screenshot cannot establish auth, provider communication or a functioning marketplace/backend.', '检查直接进入 /docs/<id>#section、语言切换、双语正文搜索、API 筛选、代码复制、SVG 无障碍文字、浅色／深色主题及窄屏表格。导出的 Markdown 须保留官网相同内容与链接。路由截图不能证明认证、provider 通信或 marketplace／后端运行。'),
          t('A future live console needs a separately specified account/session/credential backend and a browser-safe transport. Node BridgeClient imports ws and node:events and authenticates via a local lockfile header, so it cannot simply be imported into the current browser bundle. Those future decisions are integration work, not a capability already present in this site.', '未来真实控制台需要独立定义账号／会话／凭证后端及浏览器可用传输。Node BridgeClient 导入 ws 与 node:events，并通过本地 lockfile 认证头连接，因此不能直接导入当前浏览器 bundle。这些未来决定属于集成工作，不是本站已有能力。'),
        ],
        links: [{ label: t('Local Node Bridge integration', '本地 Node Bridge 接入'), href: '/docs/bridge-client' }, { label: t('Desktop runtime host', '桌面运行时宿主'), href: '/docs/desktop-host' }],
      },
    ],
  },
  {
    id: 'platform-builds', group: 'bridge',
    title: t('Native builds, bindings and release acceptance', '原生构建、绑定与发布验收'),
    description: t('Build matched Rust/native artifacts, select distribution variants and separate compile, simulator and physical-device acceptance.', '构建匹配的 Rust／原生产物、选择分发变体，并区分编译、模拟器与实体设备验收。'),
    sourceUrl: `${product}/apps/ios/native/scripts/build-xcframework.sh`,
    sources: [
      { repo: 'lingxi-app', path: 'apps/ios/native/scripts/build-xcframework.sh' },
      { repo: 'lingxi-app', path: 'apps/ios/native/scripts/build-linux-runtime.sh' },
      { repo: 'lingxi-app', path: 'apps/ios/native/scripts/verify-linux-runtime.sh' },
      { repo: 'lingxi-app', path: 'apps/ios/native/project.yml' },
      { repo: 'lingxi-app', path: 'apps/android/native/scripts/build-jni.sh' },
      { repo: 'lingxi-app', path: 'apps/android/native/app/build.gradle.kts' },
      { repo: 'lingxi-app', path: 'apps/android/ffi/uniffi.toml' },
      { repo: 'lingxi-app', path: 'apps/ios/ffi/uniffi.toml' },
      { repo: 'lingxi-app', path: 'apps/electron/scripts/package-macos-flare.sh' },
      { repo: 'mobile-linux-runtime', path: 'scripts/checks/check-resource-contracts.sh' },
    ],
    sections: [
      {
        id: 'build-matrix', title: t('Platform and distribution matrix', '平台与分发矩阵'),
        paragraphs: [
          t('Public product capabilities are selected by host, target OS, native callback readiness and distribution flavor. Product version fields are artifact-specific; iOS marketing/build versions, Android versionName/versionCode, Electron package version and Rust crate versions must not be presented as one universal SDK revision. Source baselines and product dependency pins provide the reproducible identity.', '公开产品能力由宿主、目标 OS、原生回调就绪状态与分发 flavor 共同选择。产品版本字段属于各产物；iOS marketing／build 版本、Android versionName／versionCode、Electron package 版本与 Rust crate 版本不能被当作统一 SDK 修订。源码基线与产品依赖 pin 才提供可复现身份。'),
        ],
        tables: [{ headers: [t('Target', '目标'), t('Current build contract', '当前构建契约'), t('Acceptance boundary', '验收边界')], rows: [
          [t('Android product', 'Android 产品'), t('minSdk 26; compileSdk 37; targetSdk 35; arm64-v8a and x86_64.', 'minSdk 26；compileSdk 37；targetSdk 35；arm64-v8a 与 x86_64。'), t('JNI packaging and instrumented device behavior.', 'JNI 打包及设备 instrumentation 行为。')],
          [t('Android play / direct', 'Android play / direct'), t('play is store with high-risk native offloads disabled; direct is full with separate application id.', 'play 为 store，关闭高风险原生 offload；direct 为 full，使用独立应用 ID。'), t('Compile-time flavor selection plus runtime permission decisions.', '编译时 flavor 选择加运行时权限决定。')],
          [t('iOS product', 'iOS 产品'), t('iOS 18.0; device arm64; arm64/x86_64 simulator; Store/Full configurations.', 'iOS 18.0；真机 arm64；arm64／x86_64 模拟器；Store／Full 配置。'), t('Signed-device guest execution; simulator only API/link checks.', '签名真机 guest 执行；模拟器仅 API／链接检查。')],
          [t('Desktop macOS', 'Desktop macOS'), t('Electron plus Rust sidecar and signed credential broker through package:mac:flare.', '通过 package:mac:flare 构建 Electron、Rust sidecar 与签名凭证 broker。'), t('Static package verification and packaged runtime smoke.', '静态包验证及打包后运行 smoke。')],
          [t('CLI / Web', 'CLI / Web'), t('Rust binary / static React-Vite site.', 'Rust 二进制／静态 React-Vite 官网。'), t('Terminal/provider behavior / browser documentation behavior.', '终端／provider 行为／浏览器文档行为。')],
        ] }],
      },
      {
        id: 'binding-artifacts', title: t('Generate one matching binding/library set', '生成一组匹配的绑定／库'),
        paragraphs: [
          t('Android build-jni.sh builds android-aar for both supported ABIs, generates each UniFFI component using its configuration and stages owned Kotlin packages and libandroid_aar.so before replacement. It preserves handwritten Java and other native helpers. play/direct select different JNI outputs. Do not copy only a new binding or only a new library into a live Gradle build.', 'Android build-jni.sh 为两种受支持 ABI 构建 android-aar，按配置生成各 UniFFI 组件，并在替换前暂存归属 Kotlin 包及 libandroid_aar.so，保留手写 Java 与其他原生辅助程序。play／direct 选择不同 JNI 输出。不要向正在运行的 Gradle 构建只复制新绑定或只复制新库。'),
          t('iOS build-xcframework.sh generates Swift/C headers/modulemaps, builds device and simulator Rust slices and assembles LingxiCodeFFI.xcframework. MobileLinuxNativeSupport contains iSH glue without another Rust core; static frameworks link into the app and are not embedded dylibs. Optional isolated output roots stage matching frameworks and bindings before promotion.', 'iOS build-xcframework.sh 生成 Swift／C 头文件／modulemap、构建真机及模拟器 Rust 切片，再组装 LingxiCodeFFI.xcframework。MobileLinuxNativeSupport 含 iSH glue，不含另一份 Rust core；静态框架链接进应用，并非嵌入 dylib。可选隔离输出根目录先暂存匹配框架与绑定，再发布到构建目录。'),
          t('LINGXI_SIM_ARM64_ONLY=1 makes a simulator-only iOS framework for local tests and is not shippable. LINGXI_DEVICE_ONLY=1 selects the device path. Reusing generated bindings or staged native runtime is valid only when the build scripts validate matching sources and artifacts; a convenience flag is not a release provenance check.', 'LINGXI_SIM_ARM64_ONLY=1 生成仅供本地测试的模拟器框架，不能分发；LINGXI_DEVICE_ONLY=1 选择真机路径。只有构建脚本验证源码及产物匹配后才可复用生成绑定或暂存原生运行时；便利 flag 不能替代发布来源验证。'),
        ],
        code: [{ label: t('Product native build entry points', '产品原生构建入口'), language: 'bash', verification: 'manual', code: 'bash apps/android/native/scripts/build-jni.sh --variant play\nbash apps/android/native/scripts/build-jni.sh --variant direct\nbash apps/ios/native/scripts/build-xcframework.sh\n(cd apps/ios/native && xcodegen generate)' }],
      },
      {
        id: 'release-flow', title: t('Release assembly and resource identity', '发布装配与资源身份'),
        paragraphs: [
          t('Use the product locked SDK revision together with its native-support release, archive SHA-256, manifest, SBOM and platform resource set. Android packages extracted executable helper libraries; iOS uses converted fakefs ZIP, RootfsPatch.bundle, VDSO and stable workspace mounts. Changing a source pin requires refreshing all dependent native artifacts and verifying package inventory, not only editing a manifest.', '将产品锁定 SDK revision 与对应 native-support 发布、压缩包 SHA-256、manifest、SBOM 及平台资源集配套使用。Android 打包提取后的可执行辅助库；iOS 使用转换的 fakefs ZIP、RootfsPatch.bundle、VDSO 与稳定工作区挂载。修改源码 pin 后须刷新所有依赖原生产物并验证包清单，不能只编辑 manifest。'),
          t('Android store/full flags describe compiled capabilities and still require runtime permissions. iOS XcodeGen preserves target, distribution and signing settings from project.yml. macOS packaging must keep the configured team and correct development application/broker profiles; the flare wrapper performs preflight and verified signing. Credentials are runtime inputs and must not be compiled into framework or APK resources.', 'Android store／full 标志描述编译能力，运行时仍须获得权限。iOS XcodeGen 从 project.yml 保留 target、分发与签名设置。macOS 打包须保留已配置 team 及正确 development 应用／broker profile；flare 包装负责预检与经过验证的签名。凭证属于运行时输入，不能编译到 framework 或 APK 资源。'),
        ],
        steps: [t('Select the source/dependency revisions and distribution configuration.', '选择源码／依赖 revision 及分发配置。'), t('Generate matched bindings, Rust libraries and native support; verify resource hashes and ABI inventory.', '生成匹配绑定、Rust 库及 native support；验证资源哈希与 ABI 清单。'), t('Build and sign the final product using the platform entry points.', '通过平台入口构建并签名最终产品。'), t('Run static package, simulator/host and physical-device checks at their separate acceptance levels.', '分别运行静态包、模拟器／宿主及实体设备检查。')],
      },
      {
        id: 'release-checks', title: t('Testing levels and common failures', '测试层级与常见故障'),
        paragraphs: [
          t('Host Rust compilation verifies facade features and FFI type shapes; Swift/Kotlin compilation verifies generated converters; simulator tests verify UI and linking; device smoke verifies actual native execution. Archive checks establish resource trust before execution, and packaged smoke establishes the launched sidecar/service identity. Record which exact level passed rather than expanding a green build into hardware acceptance.', '宿主 Rust 编译验证 facade feature 与 FFI 类型；Swift／Kotlin 编译验证生成转换器；模拟器测试验证 UI 与链接；设备 smoke 验证实际原生执行。压缩包检查在执行前建立资源信任，打包 smoke 验证启动后的 sidecar／服务身份。记录实际通过的层级，不能把构建绿灯扩大成硬件验收。'),
          t('Undefined FFI symbols indicate a binding/library/component mismatch; missing Android helper or wrong ELF ABI is packaging; unavailable simulator guest is expected; RestartRequired is iOS process identity; profile rejection is signing; source-check success with no matching release provenance is not a distributable artifact. Device network/memory, OS permission, suspension/resume and audio-owner cleanup remain explicit acceptance items.', '未定义 FFI 符号表示绑定／库／组件不匹配；Android helper 缺失或 ELF ABI 错误属于打包；模拟器 guest 不可用是预期行为；RestartRequired 对应 iOS 进程身份；profile 拒绝属于签名；只有源码检查成功而没有匹配发布来源的产物不能视为可分发。设备网络／内存、系统权限、挂起／恢复及音频归属清理仍须明确验收。'),
        ],
        links: [{ label: t('Native callback facade', '原生回调 facade'), href: '/docs/platform-integration' }, { label: t('Standalone Android SDK', '独立 Android SDK'), href: '/docs/mobile-android' }, { label: t('Standalone iOS SDK', '独立 iOS SDK'), href: '/docs/mobile-ios' }],
      },
    ],
  },
];
