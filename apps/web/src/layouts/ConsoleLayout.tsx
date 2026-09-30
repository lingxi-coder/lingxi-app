import { PropsWithChildren } from 'react';
import { AppRoute } from '../app/router';
import { IconChart, IconDocs, IconKey, IconWallet } from '../components/Icons';
import { RouteLink } from '../components/RouteLink';
import { ThemeToggle } from '../components/ThemeToggle';
import { LanguageToggle } from '../components/LanguageToggle';
import { Locale, Region, pickLocaleText } from '../utils/locale';

interface ConsoleLayoutProps extends PropsWithChildren {
  activeRoute: AppRoute;
  locale: Locale;
  region: Region;
  theme: 'light' | 'dark';
  onThemeToggle: () => void;
  onLocaleChange: (locale: Locale) => void;
  onRegionChange: (region: Region) => void;
}

const consoleNav = [
  { route: '/console', label: { en: 'Overview', zh: '总览' }, icon: IconChart },
  { route: '/console/usage', label: { en: 'Usage', zh: '用量' }, icon: IconChart },
  { route: '/console/api-keys', label: { en: 'API Keys', zh: 'API 密钥' }, icon: IconKey },
  { route: '/console/billing', label: { en: 'Billing', zh: '账单' }, icon: IconWallet },
  { route: '/docs', label: { en: 'Docs', zh: '文档' }, icon: IconDocs },
] as const;

export function ConsoleLayout({
  activeRoute,
  children,
  locale,
  onLocaleChange,
  onRegionChange,
  region,
  theme,
  onThemeToggle,
}: ConsoleLayoutProps) {
  return (
    <div className="console-shell">
      <aside className="console-sidebar">
        <RouteLink href="/" className="wordmark">
          LingXi
        </RouteLink>
        <nav className="console-nav" aria-label="Console navigation">
          {consoleNav.map((item) => {
            const Icon = item.icon;
            return (
              <RouteLink key={item.route} href={item.route} className={activeRoute === item.route ? 'active' : ''}>
                <Icon width={17} height={17} />
                <span>{pickLocaleText(locale, item.label)}</span>
              </RouteLink>
            );
          })}
        </nav>
        <div className="console-sidebar-foot">
          <p>
            {pickLocaleText(locale, {
              en: 'Personal account v1. Subscription and API billing stay separate by design.',
              zh: 'v1 仅支持个人账号。订阅与 API 计费保持独立。',
            })}
          </p>
        </div>
      </aside>
      <div className="console-main">
        <div className="prototype-banner" role="status">
          <strong>{pickLocaleText(locale, { en: 'Interface prototype', zh: '界面原型' })}</strong>
          <span>{pickLocaleText(locale, {
            en: 'All account, plan, usage, key, balance, and invoice records shown below are local sample data. No backend action is performed.',
            zh: '下方账号、套餐、用量、密钥、余额与发票均为本地样例数据，不会执行任何后端操作。',
          })}</span>
        </div>
        <header className="console-header">
          <div>
            <span className="eyebrow">Developer Console</span>
            <h1 className="console-title">
              {pickLocaleText(locale, {
                en: 'Run LingXi across desktop, phone, and CLI.',
                zh: '让 LingXi 在桌面、手机和 CLI 间连续协作。',
              })}
            </h1>
          </div>
          <div className="header-controls">
            <LanguageToggle
              locale={locale}
              onLocaleChange={onLocaleChange}
              region={region}
              onRegionChange={onRegionChange}
            />
            <ThemeToggle theme={theme} onToggle={onThemeToggle} />
          </div>
        </header>
        <main className="console-content">{children}</main>
      </div>
    </div>
  );
}
