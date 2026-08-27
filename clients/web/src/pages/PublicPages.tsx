import { useMemo, useState } from 'react';
import { CodeTabs } from '../components/CodeTabs';
import {
  IconArrowRight,
  IconCheck,
  IconDesktop,
  IconDocs,
  IconDownload,
  IconPhone,
  IconSpark,
  IconTerminal,
} from '../components/Icons';
import { RouteLink } from '../components/RouteLink';
import { releaseArtifacts, subscriptionPlans } from '../data/mockData';
import { Locale, Region, formatCurrency, pickLocaleText } from '../utils/locale';
import { formatReleaseBadge, getReleaseStatusMeta, pickRecommendedArtifact } from '../utils/release';

const apiTabs = [
  {
    id: 'curl',
    label: 'curl',
    code: `curl https://api.lingxi.dev/v1/responses \\
  -H "Authorization: Bearer $LINGXI_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"lingxi-code","input":"Review this diff"}'`,
  },
  {
    id: 'typescript',
    label: 'TypeScript',
    code: `const response = await fetch("https://api.lingxi.dev/v1/responses", {
  method: "POST",
  headers: {
    Authorization: \`Bearer \${process.env.LINGXI_API_KEY}\`,
    "Content-Type": "application/json"
  },
  body: JSON.stringify({ model: "lingxi-code", input: "Review this diff" })
});`,
  },
];

export function HomePage({ locale, region }: { locale: Locale; region: Region }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const capabilities: Array<[
    typeof IconDesktop,
    string,
    string,
  ]> = [
    [IconDesktop, t('See the whole change', '看清完整变更'), t('Desktop holds the workspace, diff, and long-running task state.', '桌面端承载工作区、Diff 与长时任务状态。')],
    [IconPhone, t('Approve with context', '带着上下文审批'), t('Phone approvals show the command, scope, and affected files.', '手机审批同时展示命令、范围与受影响文件。')],
    [IconTerminal, t('Recover precisely', '精确恢复现场'), t('CLI resumes the same task instead of opening a disconnected session.', 'CLI 接续同一个任务，而不是开启割裂的新会话。')],
  ];
  return (
    <>
      <section className="hero section-wrap">
        <div className="hero-copy reveal">
          <div className="section-kicker"><IconSpark width={16} height={16} /> {t('Cross-device AI development', '跨设备 AI 开发')}</div>
          <h1>{t('Keep building, wherever you are.', '让代码，在任何设备上继续。')}</h1>
          <p>{t(
            'Start a task on desktop, approve it from your phone, and recover the exact context in the CLI. LingXi keeps the work moving without hiding what changed.',
            '在桌面发起任务，用手机审批，再回到 CLI 原样接续上下文。LingXi 让工作持续推进，同时让每一次变更清晰可见。',
          )}</p>
          <div className="hero-actions">
            <RouteLink href="/login" className="button primary">{t('Start free', '免费开始')} <IconArrowRight width={17} height={17} /></RouteLink>
            <RouteLink href="/download" className="button secondary">{t('Download LingXi', '下载 LingXi')}</RouteLink>
          </div>
          <div className="hero-proof">
            <span><IconCheck width={15} height={15} /> {t('No card required', '无需信用卡')}</span>
            <span><IconCheck width={15} height={15} /> {t('Individual accounts', '个人账号')}</span>
          </div>
        </div>
        <ContinuityScene locale={locale} />
      </section>

      <section className="section-wrap product-statement">
        <span className="eyebrow">01 / Continuity</span>
        <h2>{t('One task. Three surfaces. No handoff tax.', '一个任务，三个终端，没有交接损耗。')}</h2>
        <div className="capability-grid">
          {capabilities.map(([Icon, title, body]) => (
            <article className="capability-card" key={String(title)}>
              <Icon width={22} height={22} />
              <h3>{title}</h3><p>{body}</p>
            </article>
          ))}
        </div>
      </section>

      <section className="api-section section-wrap">
        <div className="api-copy">
          <span className="eyebrow">02 / API</span>
          <h2>{t('The same capability, shaped for your product.', '同一套能力，也可以进入你的产品。')}</h2>
          <p>{t('Use a first-party LingXi key, track every request, and keep API usage separate from your personal subscription.', '使用 LingXi 第一方密钥追踪每次请求，并让 API 用量与个人订阅保持独立。')}</p>
          <RouteLink href="/docs" className="text-link">{t('Read the quick start', '查看快速开始')} <IconArrowRight width={16} height={16} /></RouteLink>
        </div>
        <CodeTabs tabs={apiTabs} />
      </section>

      <section className="section-wrap split-heading">
        <div><span className="eyebrow">03 / Surfaces</span><h2>{t('Meet LingXi where you work.', '在你工作的地方遇见 LingXi。')}</h2></div>
        <RouteLink href="/download" className="button secondary">{t('View release status', '查看发布状态')}</RouteLink>
      </section>
      <section className="section-wrap surface-row">
        {['Android', 'iOS', 'Desktop', 'CLI'].map((name, index) => (
          <article key={name} className="surface-card">
            <span>0{index + 1}</span><h3>{name}</h3>
            <p>{index === 3 ? t('Developer preview available', '开发者预览可用') : t('Beta or coming soon', '测试中或即将推出')}</p>
          </article>
        ))}
      </section>

      <section className="pricing-preview section-wrap">
        <div>
          <span className="eyebrow">04 / Pricing</span>
          <h2>{t('Subscribe for the assistant. Pay only for the API you use.', '助手按订阅，API 按实际用量。')}</h2>
          <p>{t('Two products, two clear ledgers. No surprise deductions from your personal plan.', '两类产品，两套清晰账目，不从个人套餐中混扣 API 费用。')}</p>
        </div>
        <div className="price-callout">
          <span>{t('Sample Pro price', 'Pro 样例价')}</span>
          <strong>{formatCurrency(19, region)}<small>/mo</small></strong>
          <RouteLink href="/pricing" className="text-link">{t('Compare plans', '比较套餐')} <IconArrowRight width={16} height={16} /></RouteLink>
        </div>
      </section>

      <section className="docs-cta section-wrap">
        <IconDocs width={28} height={28} />
        <div><span className="eyebrow">Quick start</span><h2>{t('From API key to first response in minutes.', '几分钟内，从 API Key 到第一次响应。')}</h2></div>
        <RouteLink href="/docs" className="button primary">{t('Open docs', '打开文档')}</RouteLink>
      </section>
    </>
  );
}

function ContinuityScene({ locale }: { locale: Locale }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  return (
    <div className="continuity-scene reveal delay-1" aria-label={t('Cross-device task continuity illustration', '跨设备任务连续性示意图')}>
      <div className="flow-line" aria-hidden="true"><i /><i /><i /></div>
      <div className="device desktop-device">
        <div className="device-bar"><span /><span /><span /><b>LingXi / storefront</b></div>
        <div className="device-body">
          <div className="mini-sidebar"><i /><i /><i /><i /></div>
          <div className="mini-work"><span className="mini-label">TASK 08</span><h4>{t('Repair checkout race', '修复结账竞态')}</h4><p>3 files changed · tests running</p><div className="diff-lines"><i /><i /><i /><i /></div></div>
        </div>
      </div>
      <div className="device phone-device">
        <div className="phone-notch" /><span className="mini-label">APPROVAL</span><h4>{t('Run migration?', '运行迁移？')}</h4><p>staging · 3 tables</p><button type="button">{t('Approve', '批准')}</button>
      </div>
      <div className="device terminal-device"><div className="terminal-head">lingxi — zsh</div><code><em>$</em> lingxi resume task_08<br /><span>✓ context restored</span><br />tests: 42 passed</code></div>
      <div className="scene-note"><IconSpark width={15} height={15} /> {t('Context stays attached', '上下文始终相连')}</div>
    </div>
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
