import { PropsWithChildren } from 'react';
import { AppRoute } from '../app/router';
import { Locale, Region, pickLocaleText } from '../utils/locale';
import { LanguageToggle } from '../components/LanguageToggle';
import { RouteLink } from '../components/RouteLink';
import { IconArrowRight } from '../components/Icons';

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

export function PublicLayout({
  activeRoute,
  children,
  locale,
  region,
  onLocaleChange,
  onRegionChange,
}: PublicLayoutProps) {
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
            <RouteLink href="/" className={activeRoute === '/' ? 'active' : ''}>
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
