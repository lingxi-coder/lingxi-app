import { useEffect, useMemo, useRef, useState } from 'react';
import { IconArrowRight, IconClose, IconCopy, IconDocs, IconSearch } from '../components/Icons';
import { RouteLink } from '../components/RouteLink';
import { DocCodeBlock } from '../components/DocCodeBlock';
import { DocsLayout } from '../layouts/DocsLayout';
import { docGroups, docMarkdown, docPages, getDocPage, searchDocs } from '../data/docs';
import { DocPage, DocSection } from '../data/docs/types';
import { docsBaseline, resolveDocSources, type ResolvedDocSource } from '../data/docs/sources';
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

function DocLinks({ links, locale, cards = false }: { links: NonNullable<DocSection['links']>; locale: Locale; cards?: boolean }) {
  if (cards) return <div className="developer-sdk-cards">{links.map((link) => {
    const destination = docPages.find((page) => `/docs/${page.id}` === link.href);
    return <a href={link.href} key={link.href} className="developer-sdk-card"><div className="developer-card-top"><span className="developer-sdk-icon"><IconDocs width={21} height={21} /></span>{destination?.packageName ? <span className="developer-sdk-tag">{destination.packageName}</span> : null}</div><h3>{link.label[locale]}<IconArrowRight width={16} height={16} /></h3>{destination ? <p>{destination.description[locale]}</p> : null}</a>;
  })}</div>;
  return <ul className="developer-section-links">{links.map((link) => <li key={link.href}><a href={link.href}>{link.label[locale]} <span aria-hidden="true">→</span></a></li>)}</ul>;
}

export function DocSectionContent({ section, locale, cards = false }: { section: DocSection; locale: Locale; cards?: boolean }) {
  return <>
    {section.paragraphs?.map((paragraph, idx) => <p key={idx}>{paragraph[locale]}</p>)}
    {section.bullets ? <ul>{section.bullets.map((bullet, idx) => <li key={idx}>{bullet[locale]}</li>)}</ul> : null}
    {section.steps ? <ol className="developer-steps">{section.steps.map((step, idx) => <li key={idx}>{step[locale]}</li>)}</ol> : null}
    {section.tables?.map((table, idx) => <div className="developer-table-wrap" key={idx} role="region" tabIndex={0} aria-label={section.title[locale]}><table><caption className="developer-visually-hidden">{section.title[locale]}</caption><thead><tr>{table.headers.map((header, column) => <th scope="col" key={column}>{header[locale]}</th>)}</tr></thead><tbody>{table.rows.map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, column) => <td key={column}>{cell[locale]}</td>)}</tr>)}</tbody></table></div>)}
    {section.diagram ? <figure className="developer-diagram"><div className="developer-diagram-canvas" tabIndex={0} role="region" aria-label={section.diagram.alt[locale]}><img src={section.diagram.src} alt={section.diagram.alt[locale]} loading="lazy" /></div>{section.diagram.caption ? <figcaption>{section.diagram.caption[locale]}</figcaption> : null}</figure> : null}
    {section.links?.length ? <DocLinks links={section.links} locale={locale} cards={cards} /> : null}
    {section.code?.length ? <DocCodeBlock blocks={section.code} locale={locale} /> : null}
    {section.apis?.map((api) => <div className="developer-api-doc" key={api.name}><h3>{api.name}</h3><pre><code>{api.signature}</code></pre><p>{api.description[locale]}</p></div>)}
    {section.note ? <div className="developer-note"><span aria-hidden="true">ⓘ</span><p>{section.note[locale]}</p></div> : null}
  </>;
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

export interface ApiEntry { sdk: string; name: string; kind: string; signature: string; summary: string; file: string; line: number; url?: string; sourceState: 'committed' | 'working-tree' }
interface ApiCatalog {
  generatedAt: string;
  sourceMode: 'committed' | 'working-tree';
  repositories: Array<{ sdk: string; revision: string; sourceUrl: string; dirty: boolean; treeFingerprint: string; productPinnedRevision?: string }>;
  entries: ApiEntry[];
}

export function ApiDeclaration({ entry, locale }: { entry: ApiEntry; locale: Locale }) {
  const worktree = entry.sourceState === 'working-tree';
  const url = worktree ? undefined : entry.url;
  return <article className="developer-api-entry"><div><h3>{url ? <a href={url} target="_blank" rel="noreferrer">{entry.name} ↗</a> : entry.name}</h3><span>{entry.sdk} / {entry.kind}</span></div><pre><code>{entry.signature}</code></pre>{entry.summary ? <p>{entry.summary}</p> : null}<div className="developer-api-source-row">{url ? <a className="developer-api-source" href={url} target="_blank" rel="noreferrer">{entry.file}:{entry.line}</a> : <span className="developer-api-source">{entry.file}:{entry.line}</span>}{worktree || !url ? <span className="developer-source-badge">{locale === 'zh' ? '未提交源码' : 'Working tree source'}</span> : null}</div></article>;
}

interface RustdocManifest { profiles: Array<{ id: string; label: { en: string; zh: string }; url: string; state: 'ready' | 'unavailable'; reason?: string }> }

function DocBaseline({ locale }: { locale: Locale }) {
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  return <details className="developer-baseline"><summary>{t('Source baseline', '源码基线')} <span>{docsBaseline.sourceMode === 'working-tree' ? t('Current workspace', '当前工作区') : t('Committed source', '已提交源码')}</span></summary><p>{t('Repository HEAD and product dependency pins are recorded separately. Working tree changes are not published SDK releases.', '仓库 HEAD 与产品固定依赖分别记录。工作树变更不代表已发布的 SDK 版本。')}</p><ul>{docsBaseline.repositories.map((repo) => <li key={repo.repo}><div><strong>{repo.repo}</strong>{repo.dirty ? <span className="developer-source-badge">{t('Working tree changes', '有未提交变更')}</span> : null}</div><dl><dt>HEAD</dt><dd><code>{repo.revision}</code></dd>{repo.productPinnedRevision ? <><dt>{t('Product dependency', '产品固定依赖')}</dt><dd><code>{repo.productPinnedRevision}</code></dd></> : null}<dt>{t('Input fingerprint', '输入指纹')}</dt><dd><code>{repo.treeFingerprint}</code></dd></dl></li>)}</ul></details>;
}

export function DocSourceItem({ source, locale }: { source: ResolvedDocSource; locale: Locale }) {
  return <li><span className="developer-source-repo">{source.repo}</span><div>{source.sourceState === 'committed' && source.url ? <a href={source.url} target="_blank" rel="noreferrer">{source.path} ↗</a> : <code>{source.path}</code>}{source.sourceState === 'working-tree' ? <span className="developer-source-badge">{locale === 'zh' ? '未提交源码' : 'Working tree source'}</span> : null}</div><code className="developer-source-revision">{source.revision}</code></li>;
}

function DocSources({ page, locale }: { page: DocPage; locale: Locale }) {
  const sources = resolveDocSources(page);
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  return <div className="developer-doc-sources"><h2>{t('Source attribution', '源码依据')}</h2><p>{t('These files establish this page’s workspace baseline. File links are available only when their content matches the recorded commit.', '以下文件构成本页的工作区基线。文件内容与记录的提交一致时才提供源码链接。')}</p>{sources.length ? <ul>{sources.map((source) => <DocSourceItem key={`${source.repo}/${source.path}`} source={source} locale={locale} />)}</ul> : <a href={page.sourceUrl} target="_blank" rel="noreferrer">{t('Browse repository', '查看仓库')} ↗</a>}</div>;
}

function ApiReference({ locale }: { locale: Locale }) {
  const [catalog, setCatalog] = useState<ApiCatalog | null>(null);
  const [rustdoc, setRustdoc] = useState<RustdocManifest | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [query, setQuery] = useState(() => new URLSearchParams(window.location.search).get('q') ?? '');
  const [sdk, setSdk] = useState('all');
  const [limit, setLimit] = useState(30);
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  useEffect(() => {
    const controller = new AbortController();
    setError(false);
    fetch('/rustdoc-manifest.json', { signal: controller.signal }).then((response) => response.ok ? response.json() : null).then((data: RustdocManifest | null) => { if (Array.isArray(data?.profiles)) setRustdoc(data); }).catch(() => {});
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
    <div className="developer-api-baseline"><span>{t('Index source', '索引来源')}: {catalog.sourceMode === 'working-tree' ? t('Working tree', '当前工作树') : t('Committed source', '已提交源码')}</span><p>{t('This searchable declaration index does not resolve feature gates, re-exports, or trait methods. Use the compiler-generated Rust reference for the complete API.', '此声明索引用于检索，不解析 feature、重新导出和 trait 方法。完整 API 请查看编译器生成的 Rust 参考。')}</p><ul className="developer-api-revisions">{catalog.repositories.map((repo) => <li key={repo.sdk}><strong>{repo.sdk}</strong><code title={repo.revision}>{repo.revision.slice(0, 12)}</code>{repo.dirty ? <span className="developer-source-badge">{t('Working tree changes', '有未提交变更')}</span> : null}{repo.productPinnedRevision ? <span>{t('Product pin', '产品依赖')} <code title={repo.productPinnedRevision}>{repo.productPinnedRevision.slice(0, 12)}</code></span> : null}</li>)}</ul></div>
    <div className="developer-rustdoc" aria-label={t('Compiler-generated Rust API reference', '编译器生成的 Rust API 参考')}><h3>{t('Complete Rust API reference', '完整 Rust API 参考')}</h3>{rustdoc?.profiles.some((profile) => profile.state === 'ready') ? <ul>{rustdoc.profiles.filter((profile) => profile.state === 'ready').map((profile) => <li key={profile.id}><a href={profile.url}>{profile.label[locale]} <span aria-hidden="true">↗</span></a></li>)}</ul> : <p>{t('Compiler documentation has not been built for this documentation bundle. Build the Rustdoc profiles in the complete workspace to enable these links.', '当前文档包尚未生成编译器参考。在完整工作区构建 Rustdoc feature 组合后，这里将显示可用链接。')}</p>}</div>
    <div className="developer-api-filters"><label><IconSearch width={17} height={17} /><input value={query} onChange={(event) => { setQuery(event.target.value); setLimit(30); }} aria-label={t('Search API symbols', '搜索 API 符号')} placeholder={t('Search a symbol or signature…', '搜索接口名称或签名…')} /></label><select aria-label={t('Filter by SDK', '按 SDK 筛选')} value={sdk} onChange={(event) => { setSdk(event.target.value); setLimit(30); }}><option value="all">{t('All SDKs', '全部 SDK')}</option><option value="harness">Harness Runtime</option><option value="llm">LLM Client</option><option value="mobile">Mobile Linux</option><option value="bridge">Bridge Client</option></select></div>
    <div className="developer-api-count"><span role="status" aria-live="polite">{t(`${matches.length} declarations`, `${matches.length} 条接口声明`)}</span><a href="/api-index.json" download>{t('Download index', '下载索引')} ↓</a></div>
    {matches.slice(0, limit).map((entry) => <ApiDeclaration key={`${entry.sdk}-${entry.file}-${entry.line}-${entry.name}`} entry={entry} locale={locale} />)}
    {!matches.length ? <p className="developer-empty-results">{t('No matching declarations. Try another symbol or SDK.', '未找到匹配的接口声明，请更换关键词或 SDK。')}</p> : null}
    {matches.length > limit ? <button className="developer-load-more" type="button" onClick={() => setLimit((value) => value + 30)}>{t('Show more declarations', '显示更多接口')} ({Math.min(limit, matches.length)} / {matches.length})</button> : null}
  </div>;
}

function DocArticle({ page, locale }: { page: DocPage; locale: Locale }) {
  const [copyStatus, setCopyStatus] = useState('');
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  const index = docPages.findIndex((candidate) => candidate.id === page.id);
  const previous = docPages[index - 1];
  const next = docPages[index + 1];
  const copyPage = async () => { try { await navigator.clipboard.writeText(docMarkdown(page, locale)); setCopyStatus(t('Page copied', '已复制页面')); } catch { setCopyStatus(t('Select page text to copy manually', '请选中页面文字手动复制')); } };
  return <article className="developer-article">
    <div className="developer-breadcrumb">{t('Documentation', '开发者文档')} <span>/</span> {page.packageName ?? docGroups.find((group) => group.id === page.group)?.title[locale]}</div>
    <div className="developer-title-row"><h1>{page.title[locale]}</h1><button type="button" className="developer-copy-page" onClick={copyPage}><IconCopy width={15} height={15} />{t('Copy page', '复制页面')}</button></div>
    {copyStatus ? <span className="developer-copy-status" role="status">{copyStatus}</span> : null}
    <p className="developer-lead">{page.description[locale]}</p>
    <DocBaseline locale={locale} />
    {page.id === 'overview' ? <div className="developer-note developer-intro-note"><IconDocs width={18} height={18} /><p>{t('Looking for a specific interface?', '在找某个具体接口？')} <RouteLink href="/docs/api-reference">{t('Explore the API reference', '查看 API 参考')} <span>→</span></RouteLink></p></div> : <div className="developer-package-row">{page.packageName ? <code>{page.packageName}</code> : <span>{t('Source documentation', '源码文档')}</span>}<a href={page.sourceUrl} target="_blank" rel="noreferrer">{t('Browse repository', '查看仓库')} ↗</a></div>}
    {page.sections.map((section) => <section key={section.id} id={section.id} className="developer-section"><h2><a href={`#${section.id}`}>{section.title[locale]}<span className="developer-anchor" aria-hidden="true">#</span></a></h2><DocSectionContent key={`${page.id}-${section.id}-${locale}`} section={section} locale={locale} cards={page.id === 'overview' && section.id === 'choose-sdk'} />
      {page.id === 'api-reference' && section.id === 'declarations' ? <ApiReference locale={locale} /> : null}
    </section>)}
    <DocSources page={page} locale={locale} />
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
