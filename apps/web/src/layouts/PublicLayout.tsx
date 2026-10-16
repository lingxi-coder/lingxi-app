import { PropsWithChildren, useEffect, useRef } from 'react';
import { AppRoute } from '../app/router';
import { ThemeToggle } from '../components/ThemeToggle';
import { Brand } from '../components/Brand';
import { Locale, Region, pickLocaleText } from '../utils/locale';
import { LanguageToggle } from '../components/LanguageToggle';
import { RouteLink } from '../components/RouteLink';
import { IconArrowRight, IconGlobe } from '../components/Icons';

interface PublicLayoutProps extends PropsWithChildren {
  activeRoute: AppRoute;
  theme: 'light' | 'dark';
  onThemeToggle: () => void;
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
  theme,
  onThemeToggle,
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
            <ThemeToggle theme={theme} onToggle={onThemeToggle} locale={locale} />
            <LandingMenu locale={locale}>{navigation}</LandingMenu>
          </div>
        </header>
        <main id="landing-main">{children}</main>
        <footer className="landing-footer">
          <div><Brand compact /><p>{t('Your personal AI agent.', '你的个人辅助 Agent。')}</p></div>
          <div className="landing-footer-links">
            <RouteLink href="/privacy">{t('Privacy', '隐私政策')}</RouteLink>
            <RouteLink href="/data-deletion">{t('Data deletion', '数据删除')}</RouteLink>
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
          <span className="eyebrow">LingXi</span>
          <span>
            {pickLocaleText(locale, {
              en: 'Your personal AI agent, here to help with everyday tasks.',
              zh: '心有灵犀，助你成事。你的个人辅助 Agent。',
            })}
          </span>
        </div>
        <div className="topbar">
          <RouteLink href="/" className="wordmark">
            <Brand compact />
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
            <ThemeToggle theme={theme} onToggle={onThemeToggle} locale={locale} />
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
          <div className="wordmark small"><Brand compact /></div>
          <p>
            {pickLocaleText(locale, {
              en: 'Your personal AI agent, here to help with everyday life.',
              zh: '你的个人辅助 Agent，与你一起把日常的事情做好。',
            })}
          </p>
        </div>
        <div className="footer-links">
          <RouteLink href="/privacy">{locale === 'zh' ? '隐私政策' : 'Privacy'}</RouteLink>
          <RouteLink href="/data-deletion">{locale === 'zh' ? '数据删除' : 'Data deletion'}</RouteLink>
          <RouteLink href="/download">Download</RouteLink>
          <RouteLink href="/pricing">Pricing</RouteLink>
          <RouteLink href="/docs">Docs</RouteLink>
          <RouteLink href="/admin/models">{locale === 'zh' ? '模型管理' : 'Model administration'}</RouteLink>
          <RouteLink href="/login">Console</RouteLink>
        </div>
      </footer>
    </div>
  );
}
