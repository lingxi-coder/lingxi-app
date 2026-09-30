import { PropsWithChildren, useEffect, useRef } from 'react';
import { AppRoute } from '../app/router';
import { Brand } from '../components/Brand';
import { Locale, Region, pickLocaleText } from '../utils/locale';
import { LanguageToggle } from '../components/LanguageToggle';
import { RouteLink } from '../components/RouteLink';
import { IconArrowRight, IconGlobe } from '../components/Icons';

interface PublicLayoutProps extends PropsWithChildren {
  activeRoute: AppRoute;
  locale: Locale;
  region: Region;
  onLocaleChange: (locale: Locale) => void;
  onRegionChange: (region: Region) => void;
}

const labels = {
  home: { en: 'Home', zh: '首页' },
  download: { en: 'Download', zh: '下载' },
  pricing: { en: 'Pricing', zh: '价格' },
  docs: { en: 'Docs', zh: '文档' },
  login: { en: 'Console', zh: '控制台' },
};

function LandingMenu({ children, locale }: PropsWithChildren<{ locale: Locale }>) {
  const menuRef = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      const menu = menuRef.current;
      if (event.key === 'Escape' && menu?.open) {
        menu.open = false;
        menu.querySelector('summary')?.focus();
      }
    };
    const closeOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !menuRef.current?.contains(event.target) && menuRef.current) menuRef.current.open = false;
    };
    document.addEventListener('keydown', closeOnEscape);
    document.addEventListener('pointerdown', closeOutside);
    return () => { document.removeEventListener('keydown', closeOnEscape); document.removeEventListener('pointerdown', closeOutside); };
  }, []);
  return <details ref={menuRef} className="landing-mobile-menu">
    <summary aria-label={locale === 'zh' ? '导航菜单' : 'Navigation menu'}><span /><span /></summary>
    <nav aria-label={locale === 'zh' ? '移动端导航' : 'Mobile navigation'} onClick={() => { if (menuRef.current) menuRef.current.open = false; }}>{children}</nav>
  </details>;
}

export function PublicLayout({
  activeRoute,
  children,
  locale,
  region,
  onLocaleChange,
  onRegionChange,
}: PublicLayoutProps) {
  if (activeRoute === '/') {
    const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
    const navigation = <>
      <RouteLink href="/download">{t('Download', '下载')}</RouteLink>
      <RouteLink href="/docs">{t('Documentation', '开发者文档')}</RouteLink>
      <a href="https://github.com/lingxi-coder/lingxi-app" target="_blank" rel="noreferrer">GitHub <span aria-hidden="true">↗</span></a>
    </>;
    return (
      <div className="landing-shell">
        <a href="#landing-main" className="landing-skip-link">{t('Skip to content', '跳转至正文')}</a>
        <div className="landing-wash" aria-hidden="true" />
        <header className="landing-header">
          <RouteLink href="/" className="landing-home-link"><Brand compact /></RouteLink>
          <nav className="landing-nav" aria-label={t('Main navigation', '主导航')}>{navigation}</nav>
          <div className="landing-header-controls">
            <button type="button" className="landing-language" onClick={() => onLocaleChange(locale === 'en' ? 'zh' : 'en')} aria-label={t('Switch to Chinese', '切换为英语')}>
              <IconGlobe width={16} height={16} /><span>{locale === 'en' ? '中文' : 'EN'}</span>
            </button>
            <LandingMenu locale={locale}>{navigation}</LandingMenu>
          </div>
        </header>
        <main id="landing-main">{children}</main>
        <footer className="landing-footer">
          <div><Brand compact /><p>{t('Tools for ideas worth building.', '为每一个值得实现的想法。')}</p></div>
          <div className="landing-footer-links">
            <RouteLink href="/download">{t('Download', '下载')}</RouteLink>
            <RouteLink href="/docs">{t('Documentation', '开发者文档')}</RouteLink>
            <a href="https://github.com/lingxi-coder/lingxi-app" target="_blank" rel="noreferrer">GitHub <span aria-hidden="true">↗</span></a>
          </div>
          <span className="landing-copyright">© {new Date().getFullYear()} LingXi</span>
        </footer>
      </div>
    );
  }
  return (
    <div className="app-shell">
      <div className="ambient-grid" aria-hidden="true" />
      <header className="public-header">
        <div className="announcement-bar">
          <span className="eyebrow">New</span>
          <span>
            {pickLocaleText(locale, {
              en: 'Product concept: one task moving clearly across desktop, phone, and CLI.',
              zh: '产品概念：让同一个任务在桌面、手机与 CLI 间清晰接续。',
            })}
          </span>
        </div>
        <div className="topbar">
          <RouteLink href="/" className="wordmark">
            LingXi
          </RouteLink>
          <nav className="topnav" aria-label="Main navigation">
            <RouteLink href="/">
              {pickLocaleText(locale, labels.home)}
            </RouteLink>
            <RouteLink href="/download" className={activeRoute === '/download' ? 'active' : ''}>
              {pickLocaleText(locale, labels.download)}
            </RouteLink>
            <RouteLink href="/pricing" className={activeRoute === '/pricing' ? 'active' : ''}>
              {pickLocaleText(locale, labels.pricing)}
            </RouteLink>
            <RouteLink href="/docs" className={activeRoute === '/docs' ? 'active' : ''}>
              {pickLocaleText(locale, labels.docs)}
            </RouteLink>
          </nav>
          <div className="header-controls">
            <LanguageToggle
              locale={locale}
              onLocaleChange={onLocaleChange}
              region={region}
              onRegionChange={onRegionChange}
            />
            <RouteLink href="/login" className="primary-link">
              {pickLocaleText(locale, labels.login)}
              <IconArrowRight width={16} height={16} />
            </RouteLink>
          </div>
        </div>
      </header>
      <main>{children}</main>
      <footer className="public-footer">
        <div>
          <div className="wordmark small">LingXi</div>
          <p>
            {pickLocaleText(locale, {
              en: 'Cross-device AI development assistance for individual builders.',
              zh: '面向个人开发者的跨设备 AI 开发助手。',
            })}
          </p>
        </div>
        <div className="footer-links">
          <RouteLink href="/download">Download</RouteLink>
          <RouteLink href="/pricing">Pricing</RouteLink>
          <RouteLink href="/docs">Docs</RouteLink>
          <RouteLink href="/login">Console</RouteLink>
        </div>
      </footer>
    </div>
  );
}
