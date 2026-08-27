import { useMemo, useState } from 'react';
import { CodeTabs } from '../components/CodeTabs';
import { IconSearch } from '../components/Icons';
import { DocsLayout } from '../layouts/DocsLayout';
import { Locale, Region, pickLocaleText } from '../utils/locale';

interface DocsPageProps {
  locale: Locale;
  region: Region;
  theme: 'light' | 'dark';
  onThemeToggle: () => void;
  onLocaleChange: (locale: Locale) => void;
  onRegionChange: (region: Region) => void;
}

export function DocsPage(props: DocsPageProps) {
  const { locale } = props;
  const [query, setQuery] = useState('');
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const navItems = useMemo(() => [
    { id: 'quick-start', label: t('Quick start', '快速开始') },
    { id: 'install', label: t('Install clients', '安装客户端') },
    { id: 'authentication', label: t('Authentication', '身份认证') },
    { id: 'first-response', label: t('First response', '第一次响应') },
    { id: 'usage', label: t('Usage & billing', '用量与计费') },
    { id: 'errors', label: t('Errors', '错误处理') },
  ], [locale]);
  const tocItems = navItems.filter((item) => ['quick-start', 'authentication', 'first-response'].includes(item.id));
  const codeTabs = [
    { id: 'curl', label: 'curl', code: `export LINGXI_API_KEY="lx_live_..."

curl https://api.lingxi.dev/v1/responses \\
  -H "Authorization: Bearer $LINGXI_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"lingxi-code","input":"Explain this repository"}'` },
    { id: 'python', label: 'Python', code: `import os, requests

response = requests.post(
    "https://api.lingxi.dev/v1/responses",
    headers={"Authorization": f"Bearer {os.environ['LINGXI_API_KEY']}"},
    json={"model": "lingxi-code", "input": "Explain this repository"},
)
print(response.json())` },
    { id: 'node', label: 'Node.js', code: `const result = await fetch("https://api.lingxi.dev/v1/responses", {
  method: "POST",
  headers: {
    Authorization: \`Bearer \${process.env.LINGXI_API_KEY}\`,
    "Content-Type": "application/json"
  },
  body: JSON.stringify({ model: "lingxi-code", input: "Explain this repository" }),
});
console.log(await result.json());` },
  ];
  return (
    <DocsLayout {...props} navItems={navItems} tocItems={tocItems}>
      <div className="docs-search"><IconSearch width={18} height={18} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t('Search documentation…', '搜索文档…')} /><kbd>⌘ K</kbd></div>
      {query ? <div className="search-result"><span>{t('Search prototype', '搜索原型')}</span><strong>{t('Try “API key”, “billing”, or “install”.', '可以搜索“API Key”、“计费”或“安装”。')}</strong></div> : null}
      <article className="docs-article">
        <div className="breadcrumbs">Docs / {t('Quick start', '快速开始')}</div>
        <h1 id="quick-start">{t('Make your first LingXi API request', '发起第一次 LingXi API 请求')}</h1>
        <p className="lead">{t('Create a first-party API key, store it outside client code, and send a response request. The console tracks API spend separately from your LingXi subscription.', '创建第一方 API Key，将其保存在客户端代码之外，然后发起响应请求。控制台会将 API 消费与 LingXi 个人订阅分开统计。')}</p>
        <div className="docs-callout"><strong>{t('Prototype endpoint', '原型端点')}</strong><p>{t('The domain and models below describe the intended interface. They are not claims of a live production service.', '以下域名与模型用于说明目标接口，并不表示生产服务已经上线。')}</p></div>
        <h2 id="install">{t('Install a client preview', '安装客户端预览版')}</h2>
        <p>{t('Public installers are not published from this repository today. Use the Downloads page to review each platform’s verified internal, preview, coming-soon, or unavailable status.', '当前仓库尚未发布公开安装包。请前往下载页查看各平台经核实的内部测试、预览、即将推出或不可用状态。')}</p>
        <h2 id="authentication">1. {t('Create and store an API key', '创建并保存 API Key')}</h2>
        <p>{t('Open Console → API Keys. The complete secret appears once. Copy it into a server-side environment variable and close the dialog only after it is saved.', '打开“控制台 → API 密钥”。完整密钥只显示一次，请将它复制到服务端环境变量中，保存后再关闭窗口。')}</p>
        <table><thead><tr><th>{t('Variable', '变量')}</th><th>{t('Value', '值')}</th></tr></thead><tbody><tr><td><code>base_url</code></td><td><code>https://api.lingxi.dev/v1</code></td></tr><tr><td><code>api_key</code></td><td><code>LINGXI_API_KEY</code></td></tr><tr><td><code>model</code></td><td><code>lingxi-code</code></td></tr></tbody></table>
        <h2 id="first-response">2. {t('Send a response request', '发送响应请求')}</h2>
        <p>{t('Choose the language that matches your stack. Never ship the key inside browser JavaScript or a mobile binary.', '选择与你的技术栈匹配的语言。切勿把密钥放入浏览器 JavaScript 或移动端二进制中。')}</p>
        <CodeTabs tabs={codeTabs} />
        <h2 id="usage">3. {t('Review usage and billing', '查看用量与计费')}</h2>
        <p>{t('Usage can be filtered by period, timezone, API key, and model. Exported CSV files use the same active filters.', '用量可按时间范围、时区、API Key 与模型筛选；导出的 CSV 使用相同筛选条件。')}</p>
        <h2 id="errors">{t('Error handling', '错误处理')}</h2>
        <ul><li><code>401</code> — {t('invalid or disabled API key', 'API Key 无效或已停用')}</li><li><code>429</code> — {t('rate or wallet limit reached', '达到限速或余额限制')}</li><li><code>5xx</code> — {t('retry with bounded exponential backoff', '使用有界指数退避重试')}</li></ul>
      </article>
    </DocsLayout>
  );
}
