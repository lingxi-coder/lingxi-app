import { OpenLinkPage } from '../pages/OpenLinkPage';
import { lazy, Suspense, useEffect, useState } from 'react';
import { ConsoleLayout } from '../layouts/ConsoleLayout';
import { PrivacyPage, DataDeletionPage } from '../pages/PrivacyPage';
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
import { isRemoteRoute, parseServiceRoute, useRouter } from './router';
import { useTheme } from './useTheme';

const AdminModelsPage = lazy(() => import('../pages/AdminModelsPage'));
const DocsPage = lazy(async () => {
  const [page] = await Promise.all([import('../pages/DocsPage'), import('../docs.css')]);
  return { default: page.DocsPage };
});
const RemotePage = lazy(() => import('../pages/RemotePage'));
const RemoteSignInPage = lazy(() => import('../pages/RemoteSignInPage'));
const TeamsPage = lazy(() => import('../pages/TeamsPage'));
const FriendsPage = lazy(() => import('../pages/FriendsPage'));
const ServicesPage = lazy(() => import('../pages/ServicesPage'));

export function App() {
  const route = useRouter();
  const [locale, setLocale] = usePersistentState<Locale>('lingxi.web.locale', 'zh');
  const [region, setRegion] = usePersistentState<Region>('lingxi.web.region', detectInitialRegion);
  const [theme, setTheme] = useTheme();
  // The unread badge of Friends (D14), reported by the page for its header and the document title.
  const [friendsBadge, setFriendsBadge] = useState<string | null>(null);

  useEffect(() => {
    document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en';
    document.documentElement.dataset.theme = theme;
    if (route === '/services' || route.startsWith('/services/')) {
      document.title = locale === 'zh' ? 'Bot 服务 — 灵犀 LingXi' : 'Bot services — LingXi';
    } else if (route === '/teams' || route === '/teams/join') {
      document.title = locale === 'zh' ? '团队 — 灵犀 LingXi' : 'Teams — LingXi';
    } else if (route === '/friends' || route === '/friends/add') {
      const title = locale === 'zh' ? '好友 — 灵犀 LingXi' : 'Friends — LingXi';
      document.title = friendsBadge ? `(${friendsBadge}) ${title}` : title;
    } else if (isRemoteRoute(route) || route === '/login/remote') {
      document.title = locale === 'zh' ? '灵犀远控 — LingXi Remote' : 'LingXi Remote';
    } else if (!route.startsWith('/docs')) {
      document.title = route === '/privacy' ? (locale === 'zh' ? '隐私政策 — 灵犀 LingXi' : 'Privacy policy — LingXi')
        : route === '/data-deletion' ? (locale === 'zh' ? '数据与账号删除 — 灵犀 LingXi' : 'Data and account deletion — LingXi')
        : locale === 'zh' ? '灵犀 LingXi — 你的个人辅助 Agent' : 'LingXi — Your personal AI agent';
      const description = document.querySelector<HTMLMetaElement>('meta[name="description"]');
      if (description) description.content = locale === 'zh'
        ? '灵犀是你的个人辅助 Agent。帮你梳理想法、查找资料、处理日常任务，与你一起把事情做好。'
        : 'LingXi is your personal AI agent for thinking things through, finding information, and getting everyday tasks done.';
    }
  }, [locale, route, theme, friendsBadge]);

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

  if (route.startsWith('/open/')) return <OpenLinkPage locale={locale} />;
  if (route === '/teams' || route === '/teams/join') return <Suspense fallback={<main role="status">{locale === 'zh' ? '正在加载团队…' : 'Loading teams…'}</main>}><TeamsPage locale={locale} /></Suspense>;

  if (route === '/services' || route.startsWith('/services/')) return <Suspense fallback={<main role="status">{locale === 'zh' ? '正在加载…' : 'Loading…'}</main>}><ServicesPage locale={locale} serviceId={parseServiceRoute(route) ?? undefined} /></Suspense>;

  if (route === '/friends' || route === '/friends/add') return <Suspense fallback={<main role="status">{locale === 'zh' ? '正在加载好友…' : 'Loading friends…'}</main>}><FriendsPage locale={locale} route={route} badge={friendsBadge} onBadge={setFriendsBadge} /></Suspense>;

  if (isRemoteRoute(route) || route === '/login/remote') {
    const fallback = <main role="status">{locale === 'zh' ? '正在加载…' : 'Loading…'}</main>;
    return (
      <Suspense fallback={fallback}>
        {route === '/login/remote'
          ? <RemoteSignInPage locale={locale} />
          : <RemotePage locale={locale} theme={theme} route={route} onLocaleChange={setLocale} onThemeToggle={() => setTheme((current) => (current === 'light' ? 'dark' : 'light'))} />}
      </Suspense>
    );
  }

  if (route === '/admin/models') return <Suspense fallback={<main role="status">{locale === 'zh' ? '正在加载管理后台…' : 'Loading administration…'}</main>}><AdminModelsPage locale={locale} /></Suspense>;

  if (route === '/docs' || route.startsWith('/docs/')) {
    return (
      <Suspense fallback={<main role="status">{locale === 'zh' ? '正在加载文档…' : 'Loading documentation…'}</main>}>
        <DocsPage
          pageId={route === '/docs' ? 'overview' : route.slice('/docs/'.length)}
          {...common}
          theme={theme}
          onThemeToggle={() => setTheme((current) => (current === 'light' ? 'dark' : 'light'))}
        />
      </Suspense>
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

  const page = route === '/privacy'
    ? <PrivacyPage locale={locale} />
    : route === '/data-deletion'
      ? <DataDeletionPage locale={locale} />
    : route === '/download'
    ? <DownloadPage locale={locale} />
    : route === '/pricing'
      ? <PricingPage locale={locale} region={region} />
      : route === '/login'
        ? <LoginPage locale={locale} region={region} onRegionChange={setRegion} />
        : <HomePage locale={locale} region={region} />;

  return (
    <PublicLayout {...common} activeRoute={route} theme={theme} onThemeToggle={() => setTheme((current) => current === 'light' ? 'dark' : 'light')}>
      {page}
    </PublicLayout>
  );
}
