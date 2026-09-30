import { PropsWithChildren } from 'react';
import { Locale, Region } from '../utils/locale';
import { LanguageToggle } from '../components/LanguageToggle';
import { ThemeToggle } from '../components/ThemeToggle';
import { RouteLink } from '../components/RouteLink';

interface DocsLayoutProps extends PropsWithChildren {
  locale: Locale;
  region: Region;
  theme: 'light' | 'dark';
  onThemeToggle: () => void;
  onLocaleChange: (locale: Locale) => void;
  onRegionChange: (region: Region) => void;
  navItems: Array<{ id: string; label: string }>;
  tocItems: Array<{ id: string; label: string }>;
}

export function DocsLayout({
  children,
  locale,
  navItems,
  onLocaleChange,
  onRegionChange,
  onThemeToggle,
  region,
  theme,
  tocItems,
}: DocsLayoutProps) {
  return (
    <div className="docs-shell">
      <header className="docs-header">
        <RouteLink href="/" className="wordmark">
          LingXi
        </RouteLink>
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
      <div className="docs-grid">
        <aside className="docs-sidebar">
          <h2>Docs</h2>
          <nav aria-label="Docs navigation">
            {navItems.map((item) => (
              <a key={item.id} href={`#${item.id}`}>
                {item.label}
              </a>
            ))}
          </nav>
        </aside>
        <main className="docs-content">{children}</main>
        <aside className="docs-toc">
          <h2>On this page</h2>
          <nav aria-label="Table of contents">
            {tocItems.map((item) => (
              <a key={item.id} href={`#${item.id}`}>
                {item.label}
              </a>
            ))}
          </nav>
        </aside>
      </div>
    </div>
  );
}
