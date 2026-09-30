import { useEffect } from 'react';
import { ConsoleLayout } from '../layouts/ConsoleLayout';
import { DocsPage } from '../pages/DocsPage';
import { LoginPage } from '../pages/LoginPage';
import {
  ApiKeysPage,
  BillingPage,
  ConsoleOverviewPage,
  UsagePage,
} from '../pages/ConsolePages';
import { DownloadPage, HomePage, PricingPage } from '../pages/PublicPages';
import { PublicLayout } from '../layouts/PublicLayout';
import { detectInitialRegion, Locale, Region } from '../utils/locale';
import { usePersistentState } from './usePersistentState';
import { useRouter } from './router';

export function App() {
  const route = useRouter();
  const [locale, setLocale] = usePersistentState<Locale>('lingxi.web.locale', 'zh');
  const [region, setRegion] = usePersistentState<Region>('lingxi.web.region', detectInitialRegion);
  const [theme, setTheme] = usePersistentState<'light' | 'dark'>('lingxi.web.theme', 'light');

  useEffect(() => {
    document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en';
    document.documentElement.dataset.theme = theme;
    if (!route.startsWith('/docs')) {
      document.title = locale === 'zh' ? '灵犀 LingXi — 让想法，成为现实。' : 'LingXi — Bring your ideas to life.';
      const description = document.querySelector<HTMLMetaElement>('meta[name="description"]');
      if (description) description.content = locale === 'zh'
        ? '灵犀 LingXi：跨平台 AI 开发助手。探索 Harness Runtime、LLM Client 与 Mobile Linux SDK 和开发者文档。'
        : 'LingXi is a cross-platform AI development assistant. Explore the Harness Runtime, LLM Client, and Mobile Linux SDKs and developer documentation.';
    }
  }, [locale, route, theme]);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      let target: HTMLElement | null = null;
      try { target = document.getElementById(decodeURIComponent(window.location.hash.slice(1))); } catch { /* Invalid fragments use the page top. */ }
      if (target) target.scrollIntoView({ behavior: 'instant' });
      else window.scrollTo({ top: 0, behavior: 'instant' });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [route]);

  const common = {
    locale,
    region,
    onLocaleChange: setLocale,
    onRegionChange: setRegion,
  };

  if (route === '/docs' || route.startsWith('/docs/')) {
    return (
      <DocsPage
        pageId={route === '/docs' ? 'overview' : route.slice('/docs/'.length)}
        {...common}
        theme={theme}
        onThemeToggle={() => setTheme((current) => (current === 'light' ? 'dark' : 'light'))}
      />
    );
  }

  if (route.startsWith('/console')) {
    const content = route === '/console/usage'
      ? <UsagePage locale={locale} region={region} />
      : route === '/console/api-keys'
        ? <ApiKeysPage locale={locale} />
        : route === '/console/billing'
          ? <BillingPage locale={locale} region={region} />
          : <ConsoleOverviewPage locale={locale} region={region} />;

    return (
      <ConsoleLayout
        {...common}
        activeRoute={route}
        theme={theme}
        onThemeToggle={() => setTheme((current) => (current === 'light' ? 'dark' : 'light'))}
      >
        {content}
      </ConsoleLayout>
    );
  }

  const page = route === '/download'
    ? <DownloadPage locale={locale} />
    : route === '/pricing'
      ? <PricingPage locale={locale} region={region} />
      : route === '/login'
        ? <LoginPage locale={locale} region={region} onRegionChange={setRegion} />
        : <HomePage locale={locale} region={region} />;

  return (
    <PublicLayout {...common} activeRoute={route}>
      {page}
    </PublicLayout>
  );
}
