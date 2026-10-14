import type { DocPage, DocText } from './types';
import { codeLabel, docsBaseline, resolveDocSources, verificationLabels } from './sources';

export function markdownHref(href: string): string {
  if (href === '/docs') return 'overview.md';
  const page = /^\/docs\/([\w-]+)(#[\w-]+)?$/.exec(href);
  if (page) return `${page[1]}.md${page[2] ?? ''}`;
  if (href.startsWith('/docs-assets/')) return `../assets/${href.slice('/docs-assets/'.length)}`;
  if (href === '/api-index.json') return '../../../apps/web/public/api-index.json';
  return href;
}

function tableCell(text: string): string {
  return text.replace(/\|/g, '\\|').replace(/\r?\n/g, '<br>');
}

function fenced(code: string, language: string): string {
  const maxTicks = Math.max(2, ...(code.match(/`+/g) ?? []).map((value) => value.length));
  const fence = '`'.repeat(maxTicks + 1);
  return `${fence}${language}\n${code}\n${fence}`;
}

export function docMarkdown(page: DocPage, locale: 'en' | 'zh'): string {
  const tr = (value: DocText) => value[locale];
  const parts = [`# ${tr(page.title)}`, tr(page.description)];
  for (const section of page.sections) {
    parts.push(`<a id="${section.id}"></a>\n\n## ${tr(section.title)}`);
    parts.push(...(section.paragraphs ?? []).map(tr));
    if (section.bullets?.length) parts.push(section.bullets.map((text) => `- ${tr(text)}`).join('\n'));
    if (section.steps?.length) parts.push(section.steps.map((text, index) => `${index + 1}. ${tr(text)}`).join('\n'));
    for (const table of section.tables ?? []) {
      parts.push([
        `| ${table.headers.map((text) => tableCell(tr(text))).join(' | ')} |`,
        `| ${table.headers.map(() => '---').join(' | ')} |`,
        ...table.rows.map((row) => `| ${row.map((text) => tableCell(tr(text))).join(' | ')} |`),
      ].join('\n'));
    }
    if (section.diagram) {
      parts.push(`![${tr(section.diagram.alt)}](${markdownHref(section.diagram.src)})`);
      if (section.diagram.caption) parts.push(tr(section.diagram.caption));
    }
    for (const block of section.code ?? []) {
      parts.push(`### ${codeLabel(block.label, locale)}`);
      parts.push(fenced(block.code, block.language));
      if (block.verification) parts.push(`${locale === 'zh' ? '验证方式' : 'Verification method'}: ${verificationLabels[block.verification][locale]}${block.exampleId ? ` (${block.exampleId})` : ''}`);
    }
    for (const api of section.apis ?? []) parts.push(`### ${api.name}\n\n${fenced(api.signature, page.group === 'bridge' ? 'typescript' : 'text')}\n\n${tr(api.description)}`);
    if (section.links?.length) parts.push(section.links.map((link) => `- [${tr(link.label)}](${markdownHref(link.href)})`).join('\n'));
    if (section.note) parts.push(tr(section.note));
  }
  if (page.id === 'api-reference') {
    parts.push(locale === 'zh'
      ? '动态声明目录见[生成的 API JSON](../../../apps/web/public/api-index.json)和[官网 API 搜索](https://lingxios.net/docs/api-reference)。官网发布内容可能落后于本地工作树；当前快照请启动本地官网后使用 `/docs/api-reference` 查询。源码注释保留原语言。完整类型与方法文档由 `npm run docs:rustdoc` 生成，官网只链接实际生成成功的配置。'
      : 'The dynamic declaration catalog is available in the [generated API JSON](../../../apps/web/public/api-index.json) and [published API search](https://lingxios.net/docs/api-reference). The published site may lag behind your working tree; start the local website and search `/docs/api-reference` for this snapshot. Source comments retain their original language. Generate compiler-resolved type and method documentation with `npm run docs:rustdoc`; the website links only successfully built profiles.');
  }
  const sources = resolveDocSources(page);
  parts.push(`## ${locale === 'zh' ? '源码与文档基线' : 'Sources and documentation baseline'}`);
  parts.push(`[${locale === 'zh' ? '源码仓库' : 'Source repository'}](${page.sourceUrl})`);
  if (sources.length) parts.push(sources.map((source) => {
    const position = `${source.repo}/${source.path}`;
    const label = source.url ? `[${position}](${source.url})` : position;
    return `- ${label} — ${source.sourceState === 'working-tree' ? (locale === 'zh' ? '未提交源码' : 'Uncommitted source') : (locale === 'zh' ? '已提交源码' : 'Committed source')} (${source.revision})`;
  }).join('\n'));
  const repos = docsBaseline.repositories.filter((repo) => sources.some((source) => source.repo === repo.repo));
  if (repos.length) parts.push(repos.map((repo) => `- ${repo.repo}: HEAD ${repo.revision}; ${locale === 'zh' ? '所据文件指纹' : 'Referenced input fingerprint'} ${repo.treeFingerprint}${repo.productPinnedRevision ? `; ${locale === 'zh' ? '产品固定版本' : 'Product pin'} ${repo.productPinnedRevision}` : ''}`).join('\n'));
  return parts.filter(Boolean).join('\n\n') + '\n';
}
