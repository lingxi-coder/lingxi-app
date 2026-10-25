import { PropsWithChildren, useEffect, useRef, useState } from 'react';
import { Locale } from '../utils/locale';
import { Brand } from '../components/Brand';
import { IconClose, IconSearch } from '../components/Icons';
import { RouteLink } from '../components/RouteLink';
import { ThemeToggle } from '../components/ThemeToggle';
import { docGroups, docPages } from '../data/docs';

interface DocsLayoutProps extends PropsWithChildren {
  locale: Locale;
  pageId: string;
  theme: 'light' | 'dark';
  onThemeToggle: () => void;
  onLocaleChange: (locale: Locale) => void;
  onSearch: () => void;
  tocItems: Array<{ id: string; label: string }>;
}

export function DocsLayout({ children, locale, pageId, theme, onThemeToggle, onLocaleChange, onSearch, tocItems }: DocsLayoutProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [activeSection, setActiveSection] = useState(tocItems[0]?.id ?? '');
  const menuRef = useRef<HTMLDialogElement>(null);
  const topnavRef = useRef<HTMLElement>(null);
  const mobileTocRef = useRef<HTMLDetailsElement>(null);
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  const activeGroup = docPages.find((page) => page.id === pageId)?.group ?? 'start';
  const isReference = pageId === 'api-reference';
  const isOverview = !isReference && ['start', 'system', 'operations'].includes(activeGroup);
  const visibleGroups = docGroups.filter((group) => group.id === 'start' || (isOverview
    ? ['system', 'operations'].includes(group.id)
    : !isReference && group.id === activeGroup));
  const sdkLinks = [
    { href: '/docs/harness' as const, label: 'Harness Runtime' },
    { href: '/docs/llm-client' as const, label: 'LLM Client' },
    { href: '/docs/mobile-linux' as const, label: 'Mobile Linux' },
    { href: '/docs/bridge-client' as const, label: t('Product integrations', '产品集成') },
  ];
  const topLinks = [
    { href: '/docs' as const, label: t('Overview', '概览'), selected: isOverview },
    { href: '/docs/harness' as const, label: 'Harness', selected: activeGroup === 'harness' },
    { href: '/docs/llm-client' as const, label: 'LLM Client', selected: activeGroup === 'llm' },
    { href: '/docs/mobile-linux' as const, label: 'Mobile Linux', selected: activeGroup === 'mobile' },
    { href: '/docs/bridge-client' as const, label: t('Integrations', '产品集成'), selected: activeGroup === 'bridge' },
    { href: '/docs/api-reference' as const, label: t('API reference', 'API 参考'), selected: isReference },
  ];
  const sectionIds = tocItems.map((item) => item.id).join('|');

  useEffect(() => {
    setMenuOpen(false);
    if (mobileTocRef.current) mobileTocRef.current.open = false;
  }, [pageId]);

  useEffect(() => {
    const nav = topnavRef.current;
    const active = nav?.querySelector<HTMLElement>('[aria-current="page"]');
    if (!nav || !active) return;
    const bounds = nav.getBoundingClientRect();
    const selected = active.getBoundingClientRect();
    if (selected.left < bounds.left || selected.right > bounds.right) {
      nav.scrollLeft += selected.left - bounds.left - 20;
    }
  }, [pageId, locale]);

  useEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    if (!menuOpen) {
      if (menu.open) menu.close();
      return;
    }

    menu.showModal();
    const desktop = window.matchMedia('(min-width: 761px)');
    const closeOnDesktop = () => { if (desktop.matches) setMenuOpen(false); };
    desktop.addEventListener('change', closeOnDesktop);
    closeOnDesktop();
    return () => {
      desktop.removeEventListener('change', closeOnDesktop);
      if (menu.open) menu.close();
    };
  }, [menuOpen]);

  useEffect(() => {
    const ids = sectionIds.split('|').filter(Boolean);
    const sections = ids.map((id) => document.getElementById(id)).filter((section): section is HTMLElement => section !== null);
    setActiveSection(ids[0] ?? '');
    let observer: IntersectionObserver;
    const observeSections = () => {
      observer?.disconnect();
      const visibleSections = new Set<string>();
      observer = new IntersectionObserver((entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) visibleSections.add(entry.target.id);
          else visibleSections.delete(entry.target.id);
        }
        const current = ids.find((id) => visibleSections.has(id));
        if (current) setActiveSection(current);
      }, { rootMargin: `-144px 0px -${Math.max(0, window.innerHeight - 304)}px 0px`, threshold: 0 });
      sections.forEach((section) => observer.observe(section));
    };
    observeSections();
    window.addEventListener('resize', observeSections);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', observeSections);
    };
  }, [pageId, sectionIds]);

  const navigation = (
    <>
      <div className="developer-sidebar-heading"><span className="developer-sidebar-symbol" aria-hidden="true">L</span><div><strong>LingXi SDK</strong><span>{t('Developer documentation', '开发者文档')}</span></div></div>
      <nav aria-label={t('Documentation navigation', '文档目录')}>
        {visibleGroups.map((group) => {
          const pages = docPages.filter((page) => page.group === group.id);
          const links = pages.map((page) => (
            <RouteLink key={page.id} href={page.id === 'overview' ? '/docs' : `/docs/${page.id}`} onClick={() => setMenuOpen(false)} className={pageId === page.id ? 'active' : ''} aria-current={pageId === page.id ? 'page' : undefined}>
              <span>{page.title[locale]}</span>
              {pageId === page.id ? <i aria-hidden="true" /> : null}
            </RouteLink>
          ));
          return group.id === 'start'
            ? <div className="developer-nav-group" key={group.id}><h2>{group.title[locale]}</h2>{links}</div>
            : <details className="developer-nav-group" key={`${activeGroup}-${group.id}`} open={group.id === activeGroup}><summary>{group.title[locale]}<span className="developer-nav-count">{pages.length}</span><span className="developer-nav-chevron" aria-hidden="true">⌄</span></summary><div className="developer-nav-items">{links}</div></details>;
        })}
        {isReference || isOverview ? <div className="developer-nav-group developer-sdk-navigation"><h2>{t('SDKs & integrations', 'SDK 与集成')}</h2>{sdkLinks.map((link) => <RouteLink key={link.href} href={link.href} onClick={() => setMenuOpen(false)}><span>{link.label}</span><span aria-hidden="true">→</span></RouteLink>)}</div> : null}
      </nav>
      <a className="developer-sidebar-footer" href="https://github.com/lingxi-coder" target="_blank" rel="noreferrer"><span className="developer-status-dot" />{t('Explore the source on GitHub', '在 GitHub 探索源码')}<span aria-hidden="true">↗</span></a>
    </>
  );

  const tocLinks = (mobile = false) => tocItems.map((item) => <a key={item.id} href={`#${item.id}`} aria-current={activeSection === item.id ? 'location' : undefined} onClick={() => { setActiveSection(item.id); if (mobile && mobileTocRef.current) mobileTocRef.current.open = false; }}>{item.label}</a>);

  return (
    <div className="developer-shell">
      <a href="#doc-main" className="docs-skip-link">{t('Skip to content', '跳转至正文')}</a>
      <header className="developer-header">
        <div className="developer-header-main">
          <RouteLink href="/" className="developer-wordmark"><Brand compact /><span className="developer-brand-divider" /><span>{t('Developer Docs', '开发者文档')}</span></RouteLink>
          <button type="button" className="developer-header-search" onClick={onSearch}><IconSearch width={17} height={17} /><span>{t('Search documentation…', '搜索开发者文档…')}</span><kbd>⌘ K</kbd></button>
          <div className="developer-controls">
            <a href="https://github.com/lingxi-coder" target="_blank" rel="noreferrer" className="developer-github">GitHub ↗</a>
            <ThemeToggle theme={theme} onToggle={onThemeToggle} locale={locale} />
            <button type="button" onClick={() => onLocaleChange(locale === 'zh' ? 'en' : 'zh')} aria-label={t('Switch to Chinese', '切换为英文')} className="developer-language">{locale === 'zh' ? 'EN' : '中文'}</button>
          </div>
        </div>
        <nav ref={topnavRef} className="developer-topnav" aria-label={t('SDK navigation', 'SDK 导航')}>{topLinks.map((link) => <RouteLink key={link.href} href={link.href} className={link.selected ? 'selected' : ''} aria-current={link.selected ? 'page' : undefined}>{link.label}</RouteLink>)}</nav>
      </header>
      <div className="developer-mobile-bar">
        <button type="button" aria-controls="developer-mobile-menu" aria-haspopup="dialog" aria-expanded={menuOpen} onClick={() => setMenuOpen(true)}><span className="developer-menu-icon" aria-hidden="true"><i /><i /><i /></span>{t('Documentation', '文档目录')}</button>
        <button type="button" onClick={onSearch} aria-label={t('Search documentation', '搜索文档')}><IconSearch width={18} height={18} />{t('Search', '搜索')}</button>
      </div>
      <dialog
        id="developer-mobile-menu"
        ref={menuRef}
        className="developer-mobile-menu"
        aria-label={t('Documentation navigation', '文档目录')}
        onCancel={() => setMenuOpen(false)}
        onClick={(event) => {
          const bounds = event.currentTarget.getBoundingClientRect();
          if (event.target === event.currentTarget && (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom)) setMenuOpen(false);
        }}
      >
        <div className="developer-mobile-menu-heading"><span>{t('Documentation', '文档目录')}</span><button type="button" onClick={() => setMenuOpen(false)} aria-label={t('Close documentation menu', '关闭文档目录')}><IconClose width={20} height={20} /></button></div>
        <div className="developer-mobile-menu-content">{navigation}</div>
      </dialog>
      <div className="developer-grid">
        <aside className="developer-sidebar">
          {navigation}
        </aside>
        <main id="doc-main" className="developer-content" tabIndex={-1}>
          {tocItems.length ? <details ref={mobileTocRef} className="developer-mobile-toc"><summary>{t('On this page', '本页目录')}<span aria-hidden="true">⌄</span></summary><nav aria-label={t('On this page', '本页目录')}>{tocLinks(true)}</nav></details> : null}
          {children}
        </main>
        <aside className="developer-toc">
          <h2>{t('On this page', '本页目录')}</h2>
          <nav aria-label={t('Table of contents', '页内导航')}>{tocLinks()}</nav>
          <div className="developer-toc-source"><p>{t('Build something great.', '创造，从这里开始。')}</p><a href="https://github.com/lingxi-coder/lingxi-app/issues" target="_blank" rel="noreferrer">{t('Suggest an improvement', '提出改进建议')} ↗</a></div>
        </aside>
      </div>
    </div>
  );
}
