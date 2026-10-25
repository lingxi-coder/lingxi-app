import { KeyboardEvent, useEffect, useId, useMemo, useRef, useState } from 'react';
import { docGroups, getDocPage, searchDocs } from '../data/docs';
import { Locale } from '../utils/locale';
import { IconArrowRight, IconClose, IconDocs, IconSearch } from './Icons';
import { RouteLink } from './RouteLink';

interface DocsSearchProps {
  locale: Locale;
  open: boolean;
  onClose: () => void;
}

const suggestedPages = ['overview', 'harness', 'llm-client', 'mobile-linux', 'bridge-client', 'api-reference'];

export function DocsSearch({ locale, open, onClose }: DocsSearchProps) {
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  const instructionsId = useId();
  const countId = useId();
  const results = useMemo(() => query.trim() ? searchDocs(query, locale) : suggestedPages.flatMap((id) => {
    const page = getDocPage(id);
    return page ? [{ page, excerpt: page.description[locale] }] : [];
  }), [query, locale]);
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || !open) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!dialog.open) dialog.showModal();
    inputRef.current?.focus();
    return () => {
      if (dialog.open) dialog.close();
      if (previousFocus?.isConnected) previousFocus.focus();
    };
  }, [open]);

  const resultLinks = () => Array.from(resultsRef.current?.querySelectorAll<HTMLAnchorElement>('[data-doc-result]') ?? []);
  const handleKeyDown = (event: KeyboardEvent<HTMLDialogElement>) => {
    if (event.nativeEvent.isComposing) return;
    const links = resultLinks();
    const target = event.target;
    const focusedIndex = links.findIndex((link) => link === target);
    const fromInput = target === inputRef.current;
    if (!fromInput && focusedIndex < 0) return;
    if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && links.length) {
      event.preventDefault();
      const next = fromInput
        ? event.key === 'ArrowDown' ? 0 : links.length - 1
        : (focusedIndex + (event.key === 'ArrowDown' ? 1 : -1) + links.length) % links.length;
      setActiveIndex(next);
      links[next]?.focus();
      links[next]?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter' && fromInput && links.length) {
      event.preventDefault();
      links[Math.min(activeIndex, links.length - 1)]?.click();
    }
  };

  return <dialog
    ref={dialogRef}
    className="developer-search-modal"
    aria-label={t('Search documentation', '搜索文档')}
    onKeyDown={handleKeyDown}
    onCancel={(event) => { event.preventDefault(); onClose(); }}
    onClick={(event) => {
      const bounds = event.currentTarget.getBoundingClientRect();
      if (event.target === event.currentTarget && (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom)) onClose();
    }}
  >
    <div className="developer-search-input">
      <IconSearch width={21} height={21} />
      <input
        ref={inputRef}
        type="search"
        value={query}
        onChange={(event) => { setQuery(event.target.value); setActiveIndex(0); }}
        placeholder={t('Search SDKs, APIs, and guides…', '搜索 SDK、API 和集成指南…')}
        aria-label={t('Search documentation', '搜索文档')}
        aria-describedby={`${countId} ${instructionsId}`}
        autoComplete="off"
      />
      <button type="button" onClick={onClose} aria-label={t('Close search', '关闭搜索')}><IconClose width={18} height={18} /></button>
    </div>
    <div ref={resultsRef} className="developer-search-results">
      <p id={countId} className="developer-search-caption" role="status" aria-live="polite">{query.trim() ? t(`${results.length} results`, `${results.length} 条结果`) : t('Explore the documentation', '探索开发者文档')}</p>
      {results.length ? results.map(({ page, excerpt }, index) => <RouteLink
        key={page.id}
        href={page.id === 'overview' ? '/docs' : `/docs/${page.id}`}
        className={`developer-search-result${index === activeIndex ? ' is-selected' : ''}`}
        data-doc-result=""
        onFocus={() => setActiveIndex(index)}
        onMouseEnter={() => setActiveIndex(index)}
        onClick={onClose}
      >
        <IconDocs width={19} height={19} />
        <div><span className="developer-search-result-group">{docGroups.find((group) => group.id === page.group)?.title[locale]}</span><strong>{page.title[locale]}</strong><p>{excerpt}</p></div>
        <IconArrowRight width={16} height={16} />
      </RouteLink>) : <div className="developer-empty-results"><IconSearch width={24} height={24} /><strong>{t('No matching pages', '没有找到相关页面')}</strong><p>{t('Try a symbol like ChatRequest, HarnessBuilder, or boot.', '试试搜索 ChatRequest、HarnessBuilder 或 boot。')}</p></div>}
    </div>
    {query.trim() ? <a className="developer-api-search-link" href={`/docs/api-reference?q=${encodeURIComponent(query.trim())}`} onClick={onClose}>{t('Search all API declarations for', '在全部 API 声明中搜索')} “{query.trim()}” <IconArrowRight width={15} height={15} /></a> : null}
    <div className="developer-search-footer" id={instructionsId}><span>{t('Search SDK guides & examples', '搜索 SDK 指南与示例')}</span><span><kbd>↑</kbd><kbd>↓</kbd> {t('navigate', '选择')} <kbd>Enter</kbd> {t('open', '打开')} <kbd>Esc</kbd> {t('close', '关闭')}</span></div>
  </dialog>;
}
