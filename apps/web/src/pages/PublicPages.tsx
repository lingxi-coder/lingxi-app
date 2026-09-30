import { useMemo, useState } from 'react';
import { CodeTabs } from '../components/CodeTabs';
import {
  IconArrowRight,
  IconCheck,
  IconDocs,
  IconDownload,
  IconDesktop,
  IconPhone,
  IconTerminal,
} from '../components/Icons';
import { RouteLink } from '../components/RouteLink';
import { releaseArtifacts, subscriptionPlans } from '../data/mockData';
import { Locale, Region, formatCurrency, pickLocaleText } from '../utils/locale';
import { formatReleaseBadge, getReleaseStatusMeta, pickRecommendedArtifact } from '../utils/release';

const homeSdks = [
  {
    name: 'Harness Runtime',
    Icon: IconDesktop,
    package: 'harness-runtime',
    href: '/docs/harness' as const,
    en: 'The foundation for agents. Sessions, tools, permissions, and memory.',
    zh: '构建代理的基础。会话、工具、权限与记忆，一处接入。',
  },
  {
    name: 'LLM Client',
    Icon: IconTerminal,
    package: 'llm-client',
    href: '/docs/llm-client' as const,
    en: 'Connect your models with a unified Rust client and streaming API.',
    zh: '以统一的 Rust 客户端与流式 API，连接你的模型。',
  },
  {
    name: 'Mobile Linux',
    Icon: IconPhone,
    package: 'mobile-linux-runtime',
    href: '/docs/mobile-linux' as const,
    en: 'Bring a Linux environment into your iOS and Android applications.',
    zh: '将 Linux 运行环境带入你的 iOS 与 Android 应用。',
  },
];

export function HomePage({ locale }: { locale: Locale; region: Region }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  return (
    <>
      <section className="landing-hero" aria-labelledby="landing-title">
        <RouteLink href="/docs" className="landing-announcement">
          <span className="landing-announcement-dot" aria-hidden="true" />
          {t('The LingXi SDKs. Built for developers.', '面向开发者的灵犀 SDK')}
          <IconArrowRight width={14} height={14} />
        </RouteLink>
        <h1 id="landing-title"><span>{t('Bring your ideas ', '让想法，')}</span><span className="landing-title-accent">{t('to life.', '成为现实。')}</span></h1>
        <p className="landing-description">
          {t(
            'An AI development workspace across desktop, mobile, and CLI. Build with the same tools in your own product.',
            '在桌面、手机与 CLI 上，与 AI 一起开发。也将这份能力，融入你的产品。',
          )}
        </p>
        <div className="landing-actions">
          <RouteLink href="/download" className="landing-entry landing-entry-primary">
            <IconDownload width={22} height={22} />
            <span><strong>{t('Get LingXi', '获取 LingXi')}</strong><small>{t('Desktop · Mobile · CLI', '桌面 · 移动端 · CLI')}</small></span>
            <IconArrowRight width={17} height={17} />
          </RouteLink>
          <RouteLink href="/docs" className="landing-entry">
            <IconDocs width={22} height={22} />
            <span><strong>{t('Developer docs', '开发者文档')}</strong><small>{t('Explore the SDKs & APIs', '探索 SDK 与 API')}</small></span>
            <IconArrowRight width={17} height={17} />
          </RouteLink>
        </div>
        <div className="landing-platforms"><span>{t('Wherever you build', '在你创造的每一处')}</span><span>macOS · Windows · Linux · iOS · Android</span></div>
      </section>

      <section className="landing-sdks" aria-labelledby="landing-sdk-title">
        <div className="landing-sdk-heading">
          <div><span className="landing-eyebrow">Build with LingXi</span><h2 id="landing-sdk-title">{t('Make it your own.', '从你的想法开始。')}</h2></div>
          <RouteLink href="/docs" className="landing-text-link">{t('All documentation', '全部文档')} <IconArrowRight width={16} height={16} /></RouteLink>
        </div>
        <div className="landing-sdk-grid">
          {homeSdks.map((sdk, index) => (
            <RouteLink href={sdk.href} className="landing-sdk" key={sdk.package}>
              <div className="landing-sdk-top"><span className="landing-sdk-icon"><sdk.Icon width={23} height={23} /></span><span className="landing-sdk-number">0{index + 1}</span><IconArrowRight width={18} height={18} /></div>
              <h3>{sdk.name}</h3>
              <p>{locale === 'zh' ? sdk.zh : sdk.en}</p>
              <span className="landing-package">{sdk.package}</span>
            </RouteLink>
          ))}
        </div>
      </section>
    </>
  );
}

function detectPlatform(): string {
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes('iphone')) return 'iphone';
  if (ua.includes('ipad')) return 'ipad';
  if (ua.includes('android')) return 'android';
  if (ua.includes('win')) return 'windows';
  if (ua.includes('linux')) return 'linux';
  return 'macos';
}

export function DownloadPage({ locale }: { locale: Locale }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const recommended = useMemo(() => pickRecommendedArtifact(releaseArtifacts, detectPlatform()), []);
  const commandTabs = [
    { id: 'npm', label: 'npm', code: 'npm install -g lingxi@0.0.0-dev' },
    { id: 'uv', label: 'uv', code: 'uv tool install lingxi==0.0.0.dev0' },
    { id: 'pip', label: 'pip', code: 'pip install lingxi==0.0.0.dev0' },
  ];
  return (
    <div className="page-wrap">
      <header className="page-hero"><span className="eyebrow">Downloads</span><h1>{t('Choose the surface that fits the moment.', '选择适合当下的终端。')}</h1><p>{t('Release labels reflect the repository today. No placeholder button pretends a public build exists.', '发布标签严格对应当前仓库状态，不用占位按钮伪装正式公开版本。')}</p></header>
      <section className="recommended-release">
        <div><span className="eyebrow">{t('Recommended for this device', '为此设备推荐')}</span><h2>{recommended.title}</h2><p>{recommended.summary}</p></div>
        <ReleaseAction artifact={recommended} locale={locale} />
      </section>
      <section className="release-grid">
        {releaseArtifacts.map((artifact) => <ReleaseCard key={artifact.id} artifact={artifact} locale={locale} />)}
      </section>
      <section className="cli-install">
        <div><span className="eyebrow">CLI developer preview</span><h2>{t('Install from your package manager.', '通过常用包管理器安装。')}</h2><p>{t('These commands point to the development package name recorded in this repository. Public availability must still be confirmed before launch.', '命令使用仓库记录的开发包名；上线前仍需确认公共仓库实际可用性。')}</p></div>
        <CodeTabs tabs={commandTabs} />
      </section>
    </div>
  );
}

function ReleaseCard({ artifact, locale }: { artifact: typeof releaseArtifacts[number]; locale: Locale }) {
  const meta = getReleaseStatusMeta(artifact.status, locale);
  return <article className="release-card"><div className="release-card-top"><span className={`status-badge ${meta.tone}`}>{meta.label}</span><IconDownload width={19} height={19} /></div><h3>{artifact.title}</h3><strong>{formatReleaseBadge(artifact, locale)}</strong><p>{artifact.summary}</p><small>{artifact.note}</small><ReleaseAction artifact={artifact} locale={locale} /></article>;
}

function ReleaseAction({ artifact, locale }: { artifact: typeof releaseArtifacts[number]; locale: Locale }) {
  const text = artifact.status === 'coming-soon'
    ? pickLocaleText(locale, { en: 'Waitlist not open', zh: '候补尚未开放' })
    : artifact.status === 'unavailable'
      ? pickLocaleText(locale, { en: 'Not available', zh: '暂不可用' })
      : pickLocaleText(locale, { en: 'Internal beta · no public link', zh: '内部测试 · 无公开链接' });
  return <button type="button" className="button secondary small" disabled>{text}</button>;
}

export function PricingPage({ locale, region }: { locale: Locale; region: Region }) {
  const [annual, setAnnual] = useState(true);
  const [inputTokens, setInputTokens] = useState(8);
  const [outputTokens, setOutputTokens] = useState(2);
  const estimate = inputTokens * 0.18 + outputTokens * 0.72;
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  return (
    <div className="page-wrap">
      <header className="page-hero compact"><div><span className="eyebrow">Pricing</span><h1>{t('Clear plans for the assistant. Clear usage for the API.', '助手套餐清楚，API 用量也清楚。')}</h1><p className="prototype-notice">{t('Interface prototype · sample plans and prices only · no checkout is connected.', '界面原型 · 仅展示样例套餐与价格 · 未接入真实结账。')}</p></div><div className="segmented-control large"><button type="button" className={!annual ? 'active' : ''} onClick={() => setAnnual(false)}>{t('Monthly', '月付')}</button><button type="button" className={annual ? 'active' : ''} onClick={() => setAnnual(true)}>{t('Annual · save 20%', '年付 · 省 20%')}</button></div></header>
      <section className="plan-grid">
        {subscriptionPlans.map((plan) => {
          const price = annual ? plan.price.annualMonthlyUsd : plan.price.monthlyUsd;
          return <article key={plan.id} className={`plan-card ${plan.featured ? 'featured' : ''}`}>{plan.featured ? <span className="featured-label">{t('Most popular', '最受欢迎')}</span> : null}<h2>{locale === 'zh' ? plan.nameZh : plan.nameEn}</h2><p>{locale === 'zh' ? plan.taglineZh : plan.taglineEn}</p><div className="plan-price"><strong>{formatCurrency(price, region)}</strong><span>/{t('month', '月')}</span></div><ul>{(locale === 'zh' ? plan.featuresZh : plan.featuresEn).map((feature) => <li key={feature}><IconCheck width={16} height={16} />{feature}</li>)}</ul><RouteLink href="/login" className={`button ${plan.featured ? 'primary' : 'secondary'}`}>{plan.id === 'free' ? t('Start free', '免费开始') : t('Choose plan', '选择套餐')}</RouteLink></article>;
        })}
      </section>
      <section className="api-estimator">
        <div><span className="eyebrow">API pay as you go</span><h2>{t('Estimate usage independently.', '独立估算 API 用量。')}</h2><p>{t('Illustrative prototype rates only. Production model prices must come from billing configuration.', '以下仅为原型演示费率，正式模型价格必须来自计费配置。')}</p></div>
        <div className="estimator-panel">
          <label>{t('Input tokens / month', '每月输入 Token')}<input type="range" min="1" max="50" value={inputTokens} onChange={(event) => setInputTokens(Number(event.target.value))} /><strong>{inputTokens}M</strong></label>
          <label>{t('Output tokens / month', '每月输出 Token')}<input type="range" min="1" max="20" value={outputTokens} onChange={(event) => setOutputTokens(Number(event.target.value))} /><strong>{outputTokens}M</strong></label>
          <div className="estimate-total"><span>{t('Illustrative monthly API cost', '示例月度 API 费用')}</span><strong>{formatCurrency(estimate, region)}</strong></div>
        </div>
      </section>
    </div>
  );
}
