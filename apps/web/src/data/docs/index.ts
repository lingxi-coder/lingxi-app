import { bridgePages } from './bridge';
import { harnessPages } from './harness';
import { llmPages } from './llm';
import { mobilePages } from './mobile';
import { DocPage, DocText } from './types';
import { operationsPages } from './operations';
import { runtimeSystemPages } from './runtime-system';
import { llmServicePages } from './llm-services';
import { platformPages } from './platforms';
import { agentWorkflowPages } from './agent-workflows';
export { docMarkdown } from './markdown';

export const docGroups: Array<{ id: DocPage['group']; title: DocText }> = [
  { id: 'start', title: { en: 'Get started', zh: '开始使用' } },
  { id: 'system', title: { en: 'System internals', zh: '系统机制' } },
  { id: 'harness', title: { en: 'Harness Runtime', zh: 'Harness Runtime' } },
  { id: 'llm', title: { en: 'LLM Client', zh: 'LLM Client' } },
  { id: 'mobile', title: { en: 'Mobile Linux', zh: 'Mobile Linux' } },
  { id: 'bridge', title: { en: 'Product integrations', zh: '产品集成' } },
  { id: 'operations', title: { en: 'Development & operations', zh: '开发与运维' } },
];

const overview: DocPage = {
  id: 'overview', group: 'start',
  title: { en: 'SDKs & libraries', zh: 'SDK 与开发者工具' },
  description: {
    en: 'Build with LingXi. Connect models, orchestrate agents, and bring a Linux runtime to mobile — with a toolkit that fits your application.',
    zh: '用灵犀构建你的应用。从模型调用、Agent 编排，到移动端 Linux 运行时，选择适合你的 SDK，开始创造。',
  },
  sourceUrl: 'https://github.com/lingxi-coder/lingxi-app',
  sources: [{ repo: 'lingxi-app', path: 'Cargo.toml' }, { repo: 'harness-runtime', path: 'crates/runtime/Cargo.toml' }, { repo: 'llm-client', path: 'src/lib.rs' }, { repo: 'mobile-linux-runtime', path: 'Cargo.toml' }],
  sections: [
    { id: 'choose-sdk', title: { en: 'Find your starting point', zh: '选择你的起点' }, paragraphs: [{ en: 'The SDKs form distinct layers. Use LLM Client for direct model calls, Harness Runtime for the agent loop, and Mobile Linux for local execution on Android and iOS.', zh: '每个 SDK 负责一层清晰的能力：LLM Client 连接模型，Harness Runtime 管理 Agent 执行，Mobile Linux 为 Android 和 iOS 提供本地运行环境。你可以独立使用，也可以组合集成。' }], links: [
      { label: { en: 'Harness Runtime', zh: 'Harness Runtime' }, href: '/docs/harness' },
      { label: { en: 'LLM Client', zh: 'LLM Client' }, href: '/docs/llm-client' },
      { label: { en: 'Mobile Linux', zh: 'Mobile Linux' }, href: '/docs/mobile-linux' },
      { label: { en: 'Product Bridge', zh: '产品 Bridge' }, href: '/docs/bridge-client' },
    ] },
    { id: 'agent-guides', title: { en: 'Build an agent, step by step', zh: '从一个任务开始构建 Agent' }, paragraphs: [{ en: 'Follow a complete task instead of piecing together declarations. These guides connect the session lifecycle, output events, approvals, tools, and host responsibilities.', zh: '沿着完整任务接入，不必自己拼凑零散声明。这些指南串起会话生命周期、输出事件、审批、工具与宿主职责。' }], links: [
      { label: { en: 'Choose the API when coming from Claude Agent SDK', zh: '从 Claude Agent SDK 选择对应入口' }, href: '/docs/agent-sdk-guide' },
      { label: { en: 'Run and cancel the first turn', zh: '运行并取消第一个轮次' }, href: '/docs/agent-first-turn' },
      { label: { en: 'Connect agent events to your UI', zh: '将 Agent 事件连接到界面' }, href: '/docs/agent-ui-streaming' },
      { label: { en: 'Resume and fork a session', zh: '恢复与分支会话' }, href: '/docs/agent-session-workflows' },
      { label: { en: 'Add tools and handle approvals', zh: '添加工具并处理审批' }, href: '/docs/agent-tool-workflows' },
      { label: { en: 'Compose MCP, skills, and hooks', zh: '组合 MCP、技能与 Hooks' }, href: '/docs/agent-extension-recipes' },
      { label: { en: 'Coordinate parallel agent tasks', zh: '协调并行 Agent 任务' }, href: '/docs/agent-parallel-tasks' },
    ] },
    { id: 'sdk-map', title: { en: 'At a glance', zh: '能力一览' }, paragraphs: [{ en: 'Three independent SDK repositories, plus the TypeScript bridge and native host integration in the product repository. The API index links byte-identical committed files to their exact revision and marks changed files as working-tree sources.', zh: '三个独立 SDK 仓库，以及产品仓库中的 TypeScript Bridge 和原生宿主集成。API 索引为与提交一致的文件提供精确版本链接，并将修改文件标记为工作树源码。' }], tables: [{ headers: [{ en: 'SDK / integration', zh: 'SDK / 集成' }, { en: 'Responsibility', zh: '职责' }, { en: 'Entry points', zh: '入口' }], rows: [
      [{ en: 'Harness Runtime', zh: 'Harness Runtime' }, { en: 'Agent loop, sessions, tools, permissions and lifecycle', zh: 'Agent 循环、会话、工具、权限与生命周期' }, { en: 'Rust facade; desktop/mobile/headless', zh: 'Rust 门面；desktop/mobile/headless' }],
      [{ en: 'LLM Client', zh: 'LLM Client' }, { en: 'Model protocols, transport, streams and provider services', zh: '模型协议、传输、流与 Provider 服务' }, { en: 'Rust LingxiClient', zh: 'Rust LingxiClient' }],
      [{ en: 'Mobile Linux', zh: 'Mobile Linux' }, { en: 'Local execution, PTY and verified resources', zh: '本地执行、PTY 与验证资源' }, { en: 'Rust contracts, Kotlin/Swift facade', zh: 'Rust 契约、Kotlin/Swift 门面' }],
      [{ en: 'Product Bridge', zh: '产品 Bridge' }, { en: 'Product host transport and client DTOs', zh: '产品宿主传输与客户端 DTO' }, { en: 'Private Node.js client and native FFI', zh: '内部 Node.js 客户端与原生 FFI' }],
    ] }] },
    { id: 'integration', title: { en: 'Build your integration', zh: '开始集成' }, bullets: [
      { en: 'Connect models: choose LLM Client and configure your own provider credentials.', zh: '调用模型：使用 LLM Client，配置你自己的模型服务商凭据。' },
      { en: 'Run agents: provide Harness Runtime with a session runtime and host lifecycle.', zh: '运行 Agent：为 Harness Runtime 提供会话运行时与宿主生命周期。' },
      { en: 'Execute on mobile: use the Rust API or Android / iOS native wrappers.', zh: '在移动端执行：使用 Rust API，或接入 Android／iOS 的原生封装。' },
      { en: 'Integrate the LingXi product: use its private bridge and platform wrappers within the product build.', zh: '集成灵犀产品：在产品构建中接入内部 Bridge 与平台封装。' },
    ] },
    { id: 'source', title: { en: 'Source & versioning', zh: '源码与版本' }, paragraphs: [{ en: 'Examples are grounded in the current repositories. Git dependencies should be pinned to a reviewed revision. Native installation and runtime constraints are described on each platform page.', zh: '示例依据当前仓库整理。Git 依赖应固定到经确认的提交；各平台页说明原生安装方式与运行条件。接口摘录与源码声明索引一起帮助你定位实现。' }], note: { en: 'The bridge is an internal Node.js package. Rust crates are not Python or npm SDKs; CLI package installers install the application, not these libraries.', zh: 'Bridge 是产品内部的 Node.js 包。Rust SDK 与 Python／npm SDK 不同；CLI 安装包安装的是应用，而非这些库。' } },
  ],
};

const reference: DocPage = {
  id: 'api-reference', group: 'start',
  title: { en: 'API reference', zh: 'API 参考' },
  description: { en: 'Find the API for your task, follow a complete integration guide, and search the underlying public declarations.', zh: '按任务找到推荐 API，跟随完整集成指南，再检索对应的公开接口声明。' },
  sourceUrl: 'https://github.com/lingxi-coder/lingxi-app',
  sources: [{ repo: 'lingxi-app', path: 'packages/bridge-client/src/protocol.ts' }, { repo: 'harness-runtime', path: 'crates/runtime/src/api.rs' }, { repo: 'llm-client', path: 'src/lib.rs' }, { repo: 'mobile-linux-runtime', path: 'crates/mobile-linux-api/src/lib.rs' }],
  sections: [
    {
      id: 'find-api', title: { en: 'Start with your task', zh: '从你要完成的任务开始' },
      paragraphs: [{ en: 'An integration needs more than a type signature. Start with the guide for your task: it explains the inputs to construct, events or values to consume, resources you own, and failure paths to handle.', zh: '集成一个能力需要的不只是类型签名。先选择要完成的任务：指南会说明如何构造输入、消费事件或返回值、管理资源，以及处理失败。' }],
      tables: [{ headers: [{ en: 'Task', zh: '任务' }, { en: 'API to look for', zh: '重点接口' }, { en: 'What the guide covers', zh: '指南解决的问题' }], rows: [
        [{ en: 'Run an agent', zh: '运行 Agent' }, { en: 'HarnessBuilder, SessionService, RunInput', zh: 'HarnessBuilder、SessionService、RunInput' }, { en: 'Host services, a session, one turn, streamed events, and shutdown', zh: '宿主服务、会话、单轮执行、流事件与关闭' }],
        [{ en: 'Call a model', zh: '调用模型' }, { en: 'ChatRequest, RequestOptions, ChatResponse', zh: 'ChatRequest、RequestOptions、ChatResponse' }, { en: 'Provider selection, request credentials, response text, and errors', zh: '供应商选择、请求凭据、响应正文与错误' }],
        [{ en: 'Build a streaming UI', zh: '构建流式界面' }, { en: 'ModelStream, StreamEvent', zh: 'ModelStream、StreamEvent' }, { en: 'Deltas, final output, cancellation, and structured results', zh: '增量、最终输出、取消与结构化结果' }],
        [{ en: 'Execute commands on mobile', zh: '在移动端执行命令' }, { en: 'LinuxCommandRequest, LinuxProcessHandle', zh: 'LinuxCommandRequest、LinuxProcessHandle' }, { en: 'Boot, working directories, output, stdin, exit, and host lifecycle', zh: '启动、工作目录、输出、标准输入、退出与宿主生命周期' }],
      ] }],
      links: [
        { label: { en: 'Agent sessions and turns', zh: 'Agent 会话与轮次' }, href: '/docs/harness-sessions' },
        { label: { en: 'Send the first model request', zh: '发送第一个模型请求' }, href: '/docs/llm-client#first-request' },
        { label: { en: 'Consume a model stream', zh: '消费模型流' }, href: '/docs/llm-streaming' },
        { label: { en: 'Run a complete mobile command', zh: '运行完整移动端命令' }, href: '/docs/mobile-rust#complete-command' },
      ],
    },
    { id: 'declarations', title: { en: 'Search API declarations', zh: '检索 API 声明' }, paragraphs: [{ en: 'Search a type or method name, then narrow the SDK. Open its source to inspect the complete definition. Compiler-generated Rustdoc is available under the source and build profiles below.', zh: '输入类型或方法名，再按 SDK 缩小范围。打开源码可查看完整定义；编译器生成的 Rustdoc 位于下方可展开的源码与构建配置中。' }] },
    {
      id: 'api-failures', title: { en: 'When an integration fails', zh: '接入失败时从哪里排查' },
      tables: [{ headers: [{ en: 'Symptom', zh: '现象' }, { en: 'First check', zh: '先检查' }], rows: [
        [{ en: 'A symbol or method is unavailable', zh: '找不到类型或方法' }, { en: 'The selected source revision, Cargo features, imports, and the exact signature in Rustdoc', zh: '实际依赖的源码版本、Cargo feature、导入路径和 Rustdoc 中的签名' }],
        [{ en: 'A request fails before output appears', zh: '请求在产生输出前失败' }, { en: 'The selected provider/profile, per-request credentials, and the reported error class', zh: '选中的供应商或连接、每次请求的凭据和返回的错误类型' }],
        [{ en: 'The agent waits while the model is idle', zh: '模型已空闲，Agent 仍在等待' }, { en: 'Pending permission or question events, tool completion, and event consumption in the host', zh: '待处理权限或问题事件、工具完成情况，以及宿主是否持续消费事件' }],
        [{ en: 'A mobile command cannot find a file', zh: '移动端命令找不到文件' }, { en: 'Guest-visible paths, cwd, mounts, and rootfs readiness rather than host filesystem paths', zh: 'Guest 可见路径、cwd、挂载与 rootfs 就绪状态，区分宿主文件系统路径' }],
      ] }],
      links: [
        { label: { en: 'Request errors and accounting', zh: '请求错误与结算' }, href: '/docs/llm-execution-accounting' },
        { label: { en: 'Tools and permissions', zh: '工具与权限' }, href: '/docs/harness-tools' },
        { label: { en: 'Mobile execution policies', zh: '移动端执行策略' }, href: '/docs/mobile-authorization' },
      ],
    },
  ],
};

export const docPages: DocPage[] = [overview, reference, ...harnessPages, ...agentWorkflowPages, ...llmPages, ...mobilePages, ...bridgePages, ...runtimeSystemPages, ...llmServicePages, ...platformPages, ...operationsPages];

export function getDocPage(id: string): DocPage | undefined {
  return docPages.find((page) => page.id === id);
}

export function searchDocs(query: string, locale: 'en' | 'zh') {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  if (!terms.length) return docPages.slice(0, 7).map((page) => ({ page, excerpt: page.description[locale] }));
  return docPages.flatMap((page) => {
    const blocks = [page.description[locale], ...page.sections.flatMap((section) => [
      section.title[locale], ...(section.paragraphs ?? []).map((text) => text[locale]),
      ...(section.bullets ?? []).map((text) => text[locale]), ...(section.steps ?? []).map((text) => text[locale]),
      ...(section.tables ?? []).flatMap((table) => [...table.headers, ...table.rows.flat()].map((text) => text[locale])),
      ...(section.links ?? []).map((link) => link.label[locale]), section.diagram?.alt[locale] ?? '', section.diagram?.caption?.[locale] ?? '',
      section.note?.[locale] ?? '', ...(section.apis ?? []).flatMap((api) => [api.name, api.signature, api.description[locale]]),
      ...(section.code ?? []).flatMap((code) => [typeof code.label === 'string' ? code.label : code.label[locale], code.code]),
    ])];
    const haystack = [page.title.en, page.title.zh, page.packageName ?? '', ...blocks].join(' ').toLocaleLowerCase();
    if (!terms.every((term) => haystack.includes(term))) return [];
    const excerpt = blocks.find((block) => terms.some((term) => block.toLocaleLowerCase().includes(term))) ?? page.description[locale];
    return [{ page, excerpt: excerpt.slice(0, 180) }];
  });
}
