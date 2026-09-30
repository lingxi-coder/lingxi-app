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
  const t = (en: string, zh: string) => locale === 'zh' ? zh : en;
  const activeGroup = docPages.find((page) => page.id === pageId)?.group ?? 'start';
  const sectionIds = tocItems.map((item) => item.id).join('|');

  useEffect(() => {
    setMenuOpen(false);
  }, [pageId]);

  useEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    if (!menuOpen) {
      if (menu.open) menu.close();
      return;
    }

    menu.showModal();
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    const desktop = window.matchMedia('(min-width: 761px)');
    const closeOnDesktop = () => { if (desktop.matches) setMenuOpen(false); };
    desktop.addEventListener('change', closeOnDesktop);
    closeOnDesktop();
    return () => {
      document.body.style.overflow = previousOverflow;
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
      }, { rootMargin: `-120px 0px -${Math.max(0, window.innerHeight - 280)}px 0px`, threshold: 0 });
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
      <nav aria-label={t('Documentation navigation', '文档目录')}>
        {docGroups.map((group) => (
          <div className="developer-nav-group" key={group.id}>
            <h2>{group.title[locale]}</h2>
            {docPages.filter((page) => page.group === group.id).map((page) => (
              <div key={page.id} onClick={() => setMenuOpen(false)}>
                <RouteLink href={page.id === 'overview' ? '/docs' : `/docs/${page.id}`} className={pageId === page.id ? 'active' : ''} aria-current={pageId === page.id ? 'page' : undefined}>
                  <span>{page.title[locale]}</span>
                  {pageId === page.id ? <i aria-hidden="true" /> : null}
                </RouteLink>
              </div>
            ))}
          </div>
        ))}
      </nav>
      <div className="developer-sidebar-footer"><span className="developer-status-dot" />{t('Open source. Built to build.', '开源，让创造更简单。')}</div>
    </>
  );

  return (
    <div className="developer-shell">
      <a href="#doc-main" className="docs-skip-link">{t('Skip to content', '跳转至正文')}</a>
      <header className="developer-header">
        <RouteLink href="/" className="developer-wordmark"><Brand compact /><span className="developer-brand-divider" /><span>{t('Developer Docs', '开发者文档')}</span></RouteLink>
        <nav className="developer-topnav" aria-label={t('SDK navigation', 'SDK 导航')}>
          <RouteLink href="/docs" className={activeGroup === 'start' ? 'selected' : ''}>{t('Overview', '概览')}</RouteLink>
          <RouteLink href="/docs/harness" className={activeGroup === 'harness' ? 'selected' : ''}>Harness</RouteLink>
          <RouteLink href="/docs/llm-client" className={activeGroup === 'llm' ? 'selected' : ''}>LLM Client</RouteLink>
          <RouteLink href="/docs/mobile-linux" className={activeGroup === 'mobile' ? 'selected' : ''}>Mobile Linux</RouteLink>
        </nav>
        <div className="developer-controls">
          <a href="https://github.com/lingxi-coder" target="_blank" rel="noreferrer" className="developer-github">GitHub ↗</a>
          <ThemeToggle theme={theme} onToggle={onThemeToggle} />
          <button type="button" onClick={() => onLocaleChange(locale === 'zh' ? 'en' : 'zh')} aria-label={t('Switch to Chinese', '切换为英文')} className="developer-language">{locale === 'zh' ? 'EN' : '中文'}</button>
        </div>
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
          <button type="button" className="developer-search-trigger" onClick={onSearch}><IconSearch width={17} height={17} /><span>{t('Search docs…', '搜索文档…')}</span><kbd>⌘ K</kbd></button>
          {navigation}
        </aside>
        <main id="doc-main" className="developer-content" tabIndex={-1}>{children}</main>
        <aside className="developer-toc">
          <h2>{t('On this page', '本页目录')}</h2>
          <nav aria-label={t('Table of contents', '页内导航')}>{tocItems.map((item) => <a key={item.id} href={`#${item.id}`} aria-current={activeSection === item.id ? 'location' : undefined} onClick={() => setActiveSection(item.id)}>{item.label}</a>)}</nav>
          <div className="developer-toc-source"><p>{t('Build something great.', '创造，从这里开始。')}</p><a href="https://github.com/lingxi-coder/lingxi-app/issues" target="_blank" rel="noreferrer">{t('Suggest an improvement', '提出改进建议')} ↗</a></div>
        </aside>
      </div>
    </div>
  );
}
