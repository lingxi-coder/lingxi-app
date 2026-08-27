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
    window.scrollTo({ top: 0, behavior: 'instant' });
  }, [locale, route, theme]);

  const common = {
    locale,
    region,
    onLocaleChange: setLocale,
    onRegionChange: setRegion,
  };

  if (route === '/docs') {
    return (
      <DocsPage
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
