import { useEffect, useMemo, useRef, useState } from 'react';
import { IconArrowRight, IconClose, IconCopy, IconDesktop, IconDocs, IconPhone, IconSearch, IconTerminal } from '../components/Icons';
import { RouteLink } from '../components/RouteLink';
import { DocCodeBlock } from '../components/DocCodeBlock';
import { DocsLayout } from '../layouts/DocsLayout';
import { docMarkdown, docPages, getDocPage, searchDocs } from '../data/docs';
import { DocPage } from '../data/docs/types';
import { Locale, Region } from '../utils/locale';

interface DocsPageProps {
  pageId: string;
  locale: Locale;
  region: Region;
  theme: 'light' | 'dark';
  onThemeToggle: () => void;
  onLocaleChange: (locale: Locale) => void;
  onRegionChange: (region: Region) => void;
}

const sdkCards = [
  { id: 'harness', name: 'Harness Runtime', tag: 'Rust', Icon: IconDesktop, en: 'The engine behind your agents. Sessions, tools, and execution in one runtime.', zh: '为 Agent 提供执行引擎，统一管理会话、工具与运行生命周期。' },
  { id: 'llm-client', name: 'LLM Client', tag: 'Rust', Icon: IconTerminal, en: 'One client for your model providers. Typed requests, streaming, and multimodal services.', zh: '统一连接模型服务商，提供类型化请求、流式响应与多模态服务。' },
  { id: 'mobile-linux', name: 'Mobile Linux', tag: 'Rust · Kotlin · Swift', Icon: IconPhone, en: 'A Linux runtime in your mobile app. Commands, files, and terminal sessions.', zh: '将 Linux 运行环境带入移动应用，执行命令、访问文件、管理终端。' },
  { id: 'bridge-client', name: 'Bridge Client', tag: 'TypeScript · Node.js', Icon: IconDocs, en: 'Connect product hosts to the LingXi bridge. Internal workspace integration.', zh: '连接产品宿主与灵犀 Bridge 服务，适用于产品工作区内部集成。' },
];

function SdkCards({ locale }: { locale: Locale }) {
  return <div className="developer-sdk-cards">{sdkCards.map(({ id, name, tag, Icon, en, zh }) => <RouteLink href={`/docs/${id}`} key={id} className="developer-sdk-card"><div className="developer-card-top"><span className="developer-sdk-icon"><Icon width={21} height={21} /></span><span className="developer-sdk-tag">{tag}</span></div><h3>{name}<IconArrowRight width={16} height={16} /></h3><p>{locale === 'zh' ? zh : en}</p></RouteLink>)}</div>;
}

function DocsSearch({ locale, open, onClose }: { locale: Locale; open: boolean; onClose: () => void }) {
  const [query, setQuery] = useState('');
  const dialogRef = useRef<HTMLDialogElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const results = useMemo(() => searchDocs(query, locale), [query, locale]);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open) {
      previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
      inputRef.current?.focus();
    } else if (dialog.open) {
      dialog.close();
      previousFocus.current?.focus();
    }
  }, [open]);
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  return <dialog ref={dialogRef} className="developer-search-modal" onCancel={onClose} aria-label={t('Search documentation', '搜索文档')} onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="developer-search-input"><IconSearch width={21} height={21} /><input ref={inputRef} value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t('Search SDKs, APIs, and guides…', '搜索 SDK、API 和集成指南…')} aria-label={t('Search documentation', '搜索文档')} /><button type="button" onClick={onClose} aria-label={t('Close search', '关闭搜索')}><IconClose width={18} height={18} /></button></div>
    <div className="developer-search-results"><p className="developer-search-caption">{query ? t(`${results.length} results`, `${results.length} 条结果`) : t('Explore the documentation', '探索开发者文档')}</p>{results.length ? results.map(({ page, excerpt }) => <div key={page.id} onClick={onClose}><RouteLink href={page.id === 'overview' ? '/docs' : `/docs/${page.id}`} className="developer-search-result"><IconDocs width={19} height={19} /><div><strong>{page.title[locale]}</strong><p>{excerpt}</p></div><IconArrowRight width={16} height={16} /></RouteLink></div>) : <div className="developer-empty-results"><IconSearch width={24} height={24} /><strong>{t('No matching pages', '没有找到相关页面')}</strong><p>{t('Try a symbol like ChatRequest, HarnessBuilder, or boot.', '试试搜索 ChatRequest、HarnessBuilder 或 boot。')}</p></div>}</div>
    {query.trim() ? <a className="developer-api-search-link" href={`/docs/api-reference?q=${encodeURIComponent(query.trim())}`}>{t('Search all API declarations for', '在全部 API 声明中搜索')} “{query.trim()}” <IconArrowRight width={15} height={15} /></a> : null}
    <div className="developer-search-footer"><span>{t('Search SDK guides & examples', '搜索 SDK 指南与示例')}</span><span><kbd>Tab</kbd> {t('navigate', '选择')} <kbd>Esc</kbd> {t('close', '关闭')}</span></div>
  </dialog>;
}

interface ApiEntry { sdk: string; name: string; kind: string; signature: string; summary: string; file: string; line: number; url: string }
interface ApiCatalog { generatedAt: string; repositories: Array<{ sdk: string; revision: string; sourceUrl: string }>; entries: ApiEntry[] }

function ApiReference({ locale }: { locale: Locale }) {
  const [catalog, setCatalog] = useState<ApiCatalog | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [query, setQuery] = useState(() => new URLSearchParams(window.location.search).get('q') ?? '');
  const [sdk, setSdk] = useState('all');
  const [limit, setLimit] = useState(30);
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  useEffect(() => {
    const controller = new AbortController();
    setError(false);
    fetch('/api-index.json', { signal: controller.signal }).then((response) => { if (!response.ok) throw new Error('API index unavailable'); return response.json(); }).then((data: ApiCatalog) => { if (!Array.isArray(data.entries)) throw new Error('Invalid API index'); setCatalog(data); }).catch(() => { if (!controller.signal.aborted) setError(true); });
    return () => controller.abort();
  }, [attempt]);
  const matches = useMemo(() => {
    const terms = query.trim().toLowerCase().split(/\s+/u).filter(Boolean);
    return (catalog?.entries ?? []).filter((entry) => (sdk === 'all' || entry.sdk === sdk) && terms.every((term) => `${entry.name} ${entry.signature} ${entry.summary} ${entry.file}`.toLowerCase().includes(term)));
  }, [catalog, query, sdk]);
  if (error) return <div className="developer-note" role="alert">{t('The API index could not be loaded.', 'API 索引暂时无法加载。')} <button type="button" onClick={() => setAttempt((value) => value + 1)}>{t('Retry', '重试')}</button></div>;
  if (!catalog) return <p role="status">{t('Loading API declarations…', '正在加载 API 声明…')}</p>;
  return <div className="developer-api-reference">
    <div className="developer-api-filters"><label><IconSearch width={17} height={17} /><input value={query} onChange={(event) => { setQuery(event.target.value); setLimit(30); }} aria-label={t('Search API symbols', '搜索 API 符号')} placeholder={t('Search a symbol or signature…', '搜索接口名称或签名…')} /></label><select aria-label={t('Filter by SDK', '按 SDK 筛选')} value={sdk} onChange={(event) => { setSdk(event.target.value); setLimit(30); }}><option value="all">{t('All SDKs', '全部 SDK')}</option><option value="harness">Harness Runtime</option><option value="llm">LLM Client</option><option value="mobile">Mobile Linux</option><option value="bridge">Bridge Client</option></select></div>
    <div className="developer-api-count"><span>{t(`${matches.length} declarations`, `${matches.length} 条接口声明`)}</span><a href="/api-index.json" download>{t('Download index', '下载索引')} ↓</a></div>
    {matches.slice(0, limit).map((entry) => <article key={`${entry.sdk}-${entry.file}-${entry.line}-${entry.name}`} className="developer-api-entry"><div><h3><a href={entry.url} target="_blank" rel="noreferrer">{entry.name} ↗</a></h3><span>{entry.sdk} / {entry.kind}</span></div><pre><code>{entry.signature}</code></pre>{entry.summary ? <p>{entry.summary}</p> : null}<a className="developer-api-source" href={entry.url} target="_blank" rel="noreferrer">{entry.file}:{entry.line}</a></article>)}
    {!matches.length ? <p className="developer-empty-results">{t('No matching declarations. Try another symbol or SDK.', '未找到匹配的接口声明，请更换关键词或 SDK。')}</p> : null}
    {matches.length > limit ? <button className="developer-load-more" type="button" onClick={() => setLimit((value) => value + 30)}>{t('Show more declarations', '显示更多接口')} ({Math.min(limit, matches.length)} / {matches.length})</button> : null}
  </div>;
}

function SdkMap({ locale }: { locale: Locale }) {
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  return <div className="developer-table-wrap"><table><thead><tr><th>SDK</th><th>{t('Language', '语言')}</th><th>{t('Use it for', '适用场景')}</th></tr></thead><tbody>
    <tr><td><RouteLink href="/docs/harness">Harness Runtime</RouteLink></td><td>Rust</td><td>{t('Agent orchestration & host control', 'Agent 编排与宿主控制')}</td></tr>
    <tr><td><RouteLink href="/docs/llm-client">LLM Client</RouteLink></td><td>Rust</td><td>{t('Model calls & streaming', '模型调用与流式响应')}</td></tr>
    <tr><td><RouteLink href="/docs/mobile-linux">Mobile Linux</RouteLink></td><td>Rust / Kotlin / Swift</td><td>{t('Mobile Linux execution', '移动端 Linux 执行')}</td></tr>
    <tr><td><RouteLink href="/docs/bridge-client">Bridge Client</RouteLink></td><td>TypeScript / Node.js</td><td>{t('Internal product integration', '产品内部集成')}</td></tr>
  </tbody></table></div>;
}

function DocArticle({ page, locale }: { page: DocPage; locale: Locale }) {
  const [copyStatus, setCopyStatus] = useState('');
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  const index = docPages.findIndex((candidate) => candidate.id === page.id);
  const previous = docPages[index - 1];
  const next = docPages[index + 1];
  const copyPage = async () => { try { await navigator.clipboard.writeText(docMarkdown(page, locale)); setCopyStatus(t('Page copied', '已复制页面')); } catch { setCopyStatus(t('Select page text to copy manually', '请选中页面文字手动复制')); } };
  return <article className="developer-article">
    <div className="developer-breadcrumb">{t('Documentation', '开发者文档')} <span>/</span> {page.group === 'start' ? t('Get started', '开始使用') : page.packageName ?? page.group}</div>
    <div className="developer-title-row"><h1>{page.title[locale]}</h1><button type="button" className="developer-copy-page" onClick={copyPage}><IconCopy width={15} height={15} />{t('Copy page', '复制页面')}</button></div>
    {copyStatus ? <span className="developer-copy-status" role="status">{copyStatus}</span> : null}
    <p className="developer-lead">{page.description[locale]}</p>
    {page.id === 'overview' ? <div className="developer-note developer-intro-note"><IconDocs width={18} height={18} /><p>{t('Looking for a specific interface?', '在找某个具体接口？')} <RouteLink href="/docs/api-reference">{t('Explore the API reference', '查看 API 参考')} <span>→</span></RouteLink></p></div> : <div className="developer-package-row">{page.packageName ? <code>{page.packageName}</code> : <span>{t('Source documentation', '源码文档')}</span>}<a href={page.sourceUrl} target="_blank" rel="noreferrer">{t('View source', '查看源码')} ↗</a></div>}
    {page.sections.map((section) => <section key={section.id} id={section.id} className="developer-section"><h2><a href={`#${section.id}`}>{section.title[locale]}<span className="developer-anchor" aria-hidden="true">#</span></a></h2>{section.paragraphs?.map((paragraph, idx) => <p key={idx}>{paragraph[locale]}</p>)}
      {page.id === 'overview' && section.id === 'choose-sdk' ? <SdkCards locale={locale} /> : null}
      {page.id === 'overview' && section.id === 'sdk-map' ? <SdkMap locale={locale} /> : null}
      {section.bullets ? <ul>{section.bullets.map((bullet, idx) => <li key={idx}>{bullet[locale]}</li>)}</ul> : null}
      {section.code?.length ? <DocCodeBlock key={`${page.id}-${section.id}-${locale}`} blocks={section.code} locale={locale} /> : null}
      {section.apis?.map((api) => <div className="developer-api-doc" key={api.name}><h3>{api.name}</h3><pre><code>{api.signature}</code></pre><p>{api.description[locale]}</p></div>)}
      {section.note ? <div className="developer-note"><span aria-hidden="true">ⓘ</span><p>{section.note[locale]}</p></div> : null}
      {page.id === 'api-reference' && section.id === 'declarations' ? <ApiReference locale={locale} /> : null}
    </section>)}
    <div className="developer-doc-source"><span>{t('Grounded in the SDK source', '依据 SDK 源码整理')}</span><a href={page.sourceUrl} target="_blank" rel="noreferrer">{t('View source on GitHub', '在 GitHub 查看源码')} ↗</a></div>
    <nav className="developer-page-navigation" aria-label={t('Previous and next page', '上下页导航')}>{previous ? <RouteLink href={previous.id === 'overview' ? '/docs' : `/docs/${previous.id}`} className="developer-previous-page"><span>← {t('Previous', '上一页')}</span><strong>{previous.title[locale]}</strong></RouteLink> : <div />}{next ? <RouteLink href={`/docs/${next.id}`} className="developer-next-page"><span>{t('Next', '下一页')} →</span><strong>{next.title[locale]}</strong></RouteLink> : null}</nav>
    <footer className="developer-doc-footer"><span>© {new Date().getFullYear()} LingXi</span><RouteLink href="/">{t('Back to home', '返回主页')} ↗</RouteLink></footer>
  </article>;
}

export function DocsPage(props: DocsPageProps) {
  const { locale, pageId } = props;
  const [searchOpen, setSearchOpen] = useState(false);
  const page = getDocPage(pageId) ?? docPages[0];
  useEffect(() => {
    const handler = (event: KeyboardEvent) => { if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); setSearchOpen((value) => !value); } };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, []);
  useEffect(() => {
    document.title = `${page.title[locale]} · LingXi Docs`;
    const description = document.querySelector<HTMLMetaElement>('meta[name="description"]');
    if (description) description.content = page.description[locale];
  }, [page, locale]);
  return <DocsLayout {...props} pageId={page.id} tocItems={page.sections.map((section) => ({ id: section.id, label: section.title[locale] }))} onSearch={() => setSearchOpen(true)}><DocArticle key={page.id} page={page} locale={locale} /><DocsSearch locale={locale} open={searchOpen} onClose={() => setSearchOpen(false)} /></DocsLayout>;
}
