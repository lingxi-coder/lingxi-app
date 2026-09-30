import { bridgePages } from './bridge';
import { harnessPages } from './harness';
import { llmPages } from './llm';
import { mobilePages } from './mobile';
import { DocPage, DocText } from './types';

export const docGroups: Array<{ id: DocPage['group']; title: DocText }> = [
  { id: 'start', title: { en: 'Get started', zh: '开始使用' } },
  { id: 'harness', title: { en: 'Harness Runtime', zh: 'Harness Runtime' } },
  { id: 'llm', title: { en: 'LLM Client', zh: 'LLM Client' } },
  { id: 'mobile', title: { en: 'Mobile Linux', zh: 'Mobile Linux' } },
  { id: 'bridge', title: { en: 'Product integrations', zh: '产品集成' } },
];

const overview: DocPage = {
  id: 'overview', group: 'start',
  title: { en: 'SDKs & libraries', zh: 'SDK 与开发者工具' },
  description: {
    en: 'Build with LingXi. Connect models, orchestrate agents, and bring a Linux runtime to mobile — with a toolkit that fits your application.',
    zh: '用灵犀构建你的应用。从模型调用、Agent 编排，到移动端 Linux 运行时，选择适合你的 SDK，开始创造。',
  },
  sourceUrl: 'https://github.com/lingxi-coder/lingxi-app',
  sections: [
    { id: 'choose-sdk', title: { en: 'Find your starting point', zh: '选择你的起点' }, paragraphs: [{ en: 'The SDKs form distinct layers. Use LLM Client for direct model calls, Harness Runtime for the agent loop, and Mobile Linux for local execution on Android and iOS.', zh: '每个 SDK 负责一层清晰的能力：LLM Client 连接模型，Harness Runtime 管理 Agent 执行，Mobile Linux 为 Android 和 iOS 提供本地运行环境。你可以独立使用，也可以组合集成。' }] },
    { id: 'sdk-map', title: { en: 'At a glance', zh: '能力一览' }, paragraphs: [{ en: 'Three independent SDK repositories, plus the TypeScript bridge and native host integration in the product repository. The API index links each declaration to the exact source revision.', zh: '三个独立 SDK 仓库，以及产品仓库中的 TypeScript Bridge 和原生宿主集成。API 索引将每条声明链接到对应版本的源码。' }] },
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
  description: { en: 'Explore public source declarations across the LingXi SDKs. Search a symbol, filter by SDK, and open its implementation.', zh: '浏览灵犀各 SDK 的公开源码声明。搜索接口名称，按 SDK 筛选，直接查看对应实现。' },
  sourceUrl: 'https://github.com/lingxi-coder/lingxi-app/tree/main/apps/web',
  sections: [{ id: 'declarations', title: { en: 'Public declarations', zh: '公开接口声明' }, note: { en: 'This is a source declaration index, not a compiler-resolved export graph. Platform features and trait bounds are defined in the linked source. Integration guides describe the supported entry points.', zh: '这里是源码声明索引；最终导出关系、平台 feature 和 trait 约束以源码与编译结果为准。集成指南介绍各 SDK 的推荐入口。' } }],
};

export const docPages: DocPage[] = [overview, reference, ...harnessPages, ...llmPages, ...mobilePages, ...bridgePages];

export function getDocPage(id: string): DocPage | undefined {
  return docPages.find((page) => page.id === id);
}

export function searchDocs(query: string, locale: 'en' | 'zh') {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  if (!terms.length) return docPages.slice(0, 7).map((page) => ({ page, excerpt: page.description[locale] }));
  return docPages.flatMap((page) => {
    const blocks = [page.description[locale], ...page.sections.flatMap((section) => [section.title[locale], ...(section.paragraphs ?? []).map((p) => p[locale]), ...(section.bullets ?? []).map((p) => p[locale]), section.note?.[locale] ?? '', ...(section.apis ?? []).flatMap((api) => [api.name, api.signature, api.description[locale]]), ...(section.code ?? []).map((code) => code.code)])];
    const haystack = [page.title.en, page.title.zh, page.packageName ?? '', ...blocks].join(' ').toLocaleLowerCase();
    if (!terms.every((term) => haystack.includes(term))) return [];
    const excerpt = blocks.find((block) => terms.some((term) => block.toLocaleLowerCase().includes(term))) ?? page.description[locale];
    return [{ page, excerpt: excerpt.slice(0, 180) }];
  });
}

export function docMarkdown(page: DocPage, locale: 'en' | 'zh'): string {
  return [`# ${page.title[locale]}`, page.description[locale], ...page.sections.flatMap((section) => [
    `## ${section.title[locale]}`, ...(section.paragraphs ?? []).map((p) => p[locale]),
    ...(section.bullets ?? []).map((p) => `- ${p[locale]}`),
    ...(section.code ?? []).map((block) => `\`\`\`${block.language}\n${block.code}\n\`\`\``),
    ...(section.apis ?? []).map((api) => `### ${api.name}\n\n\`${api.signature}\`\n\n${api.description[locale]}`),
    section.note?.[locale] ?? '',
  ]), `Source: ${page.sourceUrl}`].filter(Boolean).join('\n\n');
}
