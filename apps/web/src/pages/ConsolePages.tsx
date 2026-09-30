import { FormEvent, ReactNode, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { navigateTo } from '../app/router';
import { IconArrowRight, IconCheck, IconClose, IconCopy, IconKey, IconWallet } from '../components/Icons';
import { LineChart } from '../components/charts/LineChart';
import { apiKeyRecords, billingAccount, releaseArtifacts, usageSummary } from '../data/mockData';
import { ApiKeyRecord } from '../types/ApiKeyRecord';
import { createLocalApiKey, deleteApiKey, renameApiKey, setApiKeyStatus } from '../utils/apiKeys';
import { Locale, Region, formatCurrency, pickLocaleText } from '../utils/locale';
import { pickRecommendedArtifact } from '../utils/release';

function metric(value: number): string {
  return new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(value);
}

export function ConsoleOverviewPage({ locale, region }: { locale: Locale; region: Region }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const recommended = pickRecommendedArtifact(releaseArtifacts, 'macos');
  const quickActions: Array<[typeof IconKey, string, '/console/api-keys' | '/console/billing' | '/docs']> = [
    [IconKey, t('Create an API key', '创建 API Key'), '/console/api-keys'],
    [IconWallet, t('Top up API wallet', '充值 API 余额'), '/console/billing'],
    [IconArrowRight, t('Open technical docs', '打开技术文档'), '/docs'],
  ];
  return (
    <div className="dashboard-stack">
      <section className="overview-banner"><div><span className="eyebrow">{t('Good morning', '早上好')}</span><h2>{t('The work is moving. You stay in control.', '工作正在推进，控制权仍在你手中。')}</h2><p>{t('This dashboard separates personal plan limits from first-party API spend.', '此总览会明确区分个人套餐额度与第一方 API 消费。')}</p></div><button type="button" className="button primary" onClick={() => navigateTo('/console/api-keys')}>{t('Create API key', '创建 API Key')} <IconArrowRight width={16} height={16} /></button></section>
      <section className="metric-grid four">
        <MetricCard label={t('Current plan', '当前套餐')} value="Pro" detail={t('Annual · renews Nov 2', '年付 · 11 月 2 日续费')} />
        <MetricCard label={t('Plan usage', '套餐用量')} value="62%" detail={t('Resets in 9 days', '9 天后重置')} progress={62} />
        <MetricCard label={t('API wallet', 'API 余额')} value={formatCurrency(usageSummary.walletBalanceUsd, region)} detail={t('Auto-recharge on', '自动充值已开启')} />
        <MetricCard label={t('API spend', 'API 消费')} value={formatCurrency(usageSummary.spendUsd, region)} detail={t('Last 30 days', '最近 30 天')} />
      </section>
      <section className="dashboard-grid">
        <article className="panel wide"><PanelHeading title={t('API spend trend', 'API 消费趋势')} action={t('View usage', '查看用量')} onAction={() => navigateTo('/console/usage')} /><LineChart data={usageSummary.trend} /></article>
        <article className="panel quick-panel"><PanelHeading title={t('Quick actions', '快捷操作')} />{quickActions.map(([Icon, label, route]) => <button type="button" className="quick-action" key={label} onClick={() => navigateTo(route)}><Icon width={18} height={18} /><span>{label}</span><IconArrowRight width={15} height={15} /></button>)}</article>
      </section>
      <section className="dashboard-grid lower">
        <article className="panel"><PanelHeading title={t('Finish setup', '完成设置')} /><div className="check-list">{[
          [true, t('Create your account', '创建账号')],
          [true, t('Install the CLI preview', '安装 CLI 预览版')],
          [false, t('Create your first API key', '创建第一个 API Key')],
          [false, t('Set a balance alert', '设置余额预警')],
        ].map(([done, label]) => <div className={done ? 'done' : ''} key={String(label)}><span><IconCheck width={14} height={14} /></span>{label}</div>)}</div></article>
        <article className="panel recommended-download"><PanelHeading title={t('Recommended download', '推荐下载')} /><span className="status-badge accent">Beta</span><h3>{recommended.title}</h3><p>{recommended.note}</p><button type="button" className="button secondary small" onClick={() => navigateTo('/download')}>{t('View release notes', '查看发布说明')}</button></article>
      </section>
    </div>
  );
}

function MetricCard({ label, value, detail, progress }: { label: string; value: string; detail: string; progress?: number }) {
  return <article className="metric-card"><span>{label}</span><strong>{value}</strong>{progress !== undefined ? <div className="progress"><i style={{ width: `${progress}%` }} /></div> : null}<small>{detail}</small></article>;
}

function PanelHeading({ title, action, onAction }: { title: string; action?: string; onAction?: () => void }) {
  return <div className="panel-heading"><h3>{title}</h3>{action ? <button type="button" className="text-button" onClick={onAction}>{action} <IconArrowRight width={14} height={14} /></button> : null}</div>;
}

export function UsagePage({ locale, region }: { locale: Locale; region: Region }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const [dimension, setDimension] = useState<'model' | 'key' | 'source'>('model');
  const [metricName, setMetricName] = useState<'spend' | 'requests'>('spend');
  const breakdown = dimension === 'model' ? usageSummary.byModel : dimension === 'key' ? usageSummary.byKey : usageSummary.bySource;
  const exportCsv = () => {
    const csv = ['day,requests,spend_usd', ...usageSummary.trend.map((row) => `${row.day},${row.requests},${row.spendUsd}`)].join('\n');
    const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv' }));
    const anchor = document.createElement('a');
    anchor.href = url; anchor.download = 'lingxi-usage.csv'; anchor.click(); URL.revokeObjectURL(url);
  };
  return (
    <div className="dashboard-stack">
      <section className="page-title-row"><div><span className="eyebrow">Usage analytics</span><h2>{t('Understand every first-party API request.', '看清每一次第一方 API 请求。')}</h2><p>{t('Updated 3 minutes ago. Times follow the selected timezone.', '3 分钟前更新，时间以所选时区为准。')}</p></div><button type="button" className="button secondary small" onClick={exportCsv}>{t('Export CSV', '导出 CSV')}</button></section>
      <section className="filter-bar"><label>{t('Period', '时间范围')}<select><option>{t('Last 30 days', '最近 30 天')}</option><option>{t('Last 7 days', '最近 7 天')}</option></select></label><label>{t('Timezone', '时区')}<select><option>America/Los_Angeles</option><option>Asia/Shanghai</option><option>UTC</option></select></label><label>API Key<select><option>{t('All keys', '全部密钥')}</option><option>Desktop Beta</option></select></label><label>{t('Model', '模型')}<select><option>{t('All models', '全部模型')}</option><option>LingXi Code Fast</option></select></label></section>
      <section className="metric-grid four"><MetricCard label={t('Cost', '费用')} value={formatCurrency(usageSummary.spendUsd, region)} detail={t('API wallet only', '仅 API 余额')} /><MetricCard label={t('Requests', '请求数')} value={metric(usageSummary.requests)} detail="+12.4%" /><MetricCard label={t('Input tokens', '输入 Token')} value={metric(usageSummary.tokensIn)} detail="128.4M" /><MetricCard label={t('Output tokens', '输出 Token')} value={metric(usageSummary.tokensOut)} detail="48.6M" /></section>
      <section className="panel"><div className="panel-heading"><h3>{metricName === 'spend' ? t('API spend', 'API 消费') : t('API requests', 'API 请求')}</h3><div className="segmented-control"><button type="button" className={metricName === 'spend' ? 'active' : ''} onClick={() => setMetricName('spend')}>{t('Cost', '费用')}</button><button type="button" className={metricName === 'requests' ? 'active' : ''} onClick={() => setMetricName('requests')}>{t('Requests', '请求')}</button></div></div><LineChart data={metricName === 'spend' ? usageSummary.trend : usageSummary.trend.map((point) => ({ ...point, spendUsd: point.requests / 80 }))} /></section>
      <section className="panel"><div className="panel-heading"><h3>{t('Breakdown', '细分')}</h3><div className="segmented-control">{(['model', 'key', 'source'] as const).map((item) => <button type="button" className={dimension === item ? 'active' : ''} onClick={() => setDimension(item)} key={item}>{item === 'model' ? t('Model', '模型') : item === 'key' ? 'API Key' : t('Source', '来源')}</button>)}</div></div><div className="breakdown-list">{breakdown.map((item) => <div key={item.label}><span>{item.label}</span><div className="progress"><i style={{ width: `${item.share}%` }} /></div><strong>{item.share.toFixed(1)}%</strong><small>{metric(item.value)} {t('requests', '次请求')}</small></div>)}</div></section>
    </div>
  );
}

export function ApiKeysPage({ locale }: { locale: Locale }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const createTriggerRef = useRef<HTMLButtonElement>(null);
  const [records, setRecords] = useState<ApiKeyRecord[]>(apiKeyRecords);
  const [showCreate, setShowCreate] = useState(false);
  const [secret, setSecret] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const created = createLocalApiKey(String(data.get('name') || 'New key'), String(data.get('scope') || 'Full API'), new Date().toISOString().slice(0, 10));
    setRecords((current) => [created.record, ...current]); setCopied(false); setSecret(created.secret); setShowCreate(false);
  };
  const copySecret = async () => { if (!secret) return; await navigator.clipboard.writeText(secret); setCopied(true); };
  return (
    <div className="dashboard-stack">
      <section className="page-title-row"><div><span className="eyebrow">First-party credentials</span><h2>{t('API keys', 'API 密钥')}</h2><p>{t('Keys are shown once at creation. Never expose them in browser code or client-side logs.', '密钥只在创建时完整显示一次，请勿将其暴露在浏览器代码或客户端日志中。')}</p></div><button ref={createTriggerRef} type="button" className="button primary" onClick={() => setShowCreate(true)}>{t('Create API key', '创建 API Key')}</button></section>
      <section className="panel table-panel"><div className="responsive-table"><table><thead><tr><th>{t('Name', '名称')}</th><th>{t('Key', '密钥')}</th><th>{t('Scope', '权限')}</th><th>{t('Created', '创建时间')}</th><th>{t('Last used', '最后使用')}</th><th>{t('Status', '状态')}</th><th /></tr></thead><tbody>{records.map((record) => <tr key={record.id}><td><button type="button" className="editable-name" title={t('Click to rename', '点击重命名')} onClick={() => { const next = window.prompt(t('Rename key', '重命名密钥'), record.name); if (next?.trim()) setRecords((current) => renameApiKey(current, record.id, next.trim())); }}>{record.name}</button></td><td><code>{record.maskedSecret}</code></td><td>{record.scope}</td><td>{record.createdAt}</td><td>{record.lastUsedAt}</td><td><span className={`status-badge ${record.status === 'active' ? 'success' : 'muted'}`}>{record.status}</span></td><td><div className="table-actions"><button type="button" onClick={() => setRecords((current) => setApiKeyStatus(current, record.id, record.status === 'active' ? 'disabled' : 'active'))}>{record.status === 'active' ? t('Disable', '停用') : t('Enable', '启用')}</button><button type="button" className="danger-text" onClick={() => setConfirmDelete(record.id)}>{t('Delete', '删除')}</button></div>{confirmDelete === record.id ? <div className="inline-confirm"><span>{t('Delete permanently?', '永久删除？')}</span><button type="button" onClick={() => { setRecords((current) => deleteApiKey(current, record.id)); setConfirmDelete(null); }}>{t('Confirm', '确认')}</button><button type="button" onClick={() => setConfirmDelete(null)}>{t('Cancel', '取消')}</button></div> : null}</td></tr>)}</tbody></table></div></section>
      {showCreate ? <Modal title={t('Create API key', '创建 API Key')} returnFocus={createTriggerRef.current} onClose={() => setShowCreate(false)}><form className="modal-form" onSubmit={submit}><label>{t('Name', '名称')}<input name="name" required autoFocus placeholder="Desktop production" /></label><label>{t('Environment', '环境')}<select name="environment"><option>Production</option><option>Development</option></select></label><label>{t('Scope', '权限范围')}<select name="scope"><option>Full API</option><option>Responses only</option><option>Read usage</option></select></label><label>{t('Expiration', '有效期')}<select><option>90 days</option><option>1 year</option><option>{t('No expiration', '永不过期')}</option></select></label><button className="button primary" type="submit">{t('Create key', '创建密钥')}</button></form></Modal> : null}
      {secret ? <Modal title={t('Copy your API key now', '立即复制 API Key')} returnFocus={createTriggerRef.current} onClose={() => { setCopied(false); setSecret(null); }}><div className="secret-reveal"><p>{t('This is the only time the complete key will be displayed.', '这是唯一一次显示完整密钥。')}</p><code>{secret}</code><button type="button" className="button primary" onClick={copySecret}>{copied ? <IconCheck width={16} height={16} /> : <IconCopy width={16} height={16} />}{copied ? t('Copied', '已复制') : t('Copy key', '复制密钥')}</button></div></Modal> : null}
    </div>
  );
}

export function BillingPage({ locale, region }: { locale: Locale; region: Region }) {
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const topupTriggerRef = useRef<HTMLButtonElement>(null);
  const [autoRecharge, setAutoRecharge] = useState(billingAccount.autoRechargeEnabled);
  const [showTopup, setShowTopup] = useState(false);
  const [balance, setBalance] = useState(billingAccount.walletBalanceUsd);
  const [amount, setAmount] = useState(50);
  return (
    <div className="dashboard-stack">
      <section className="page-title-row"><div><span className="eyebrow">Billing</span><h2>{t('Subscription and API wallet', '订阅与 API 余额')}</h2><p>{t('Two products remain separate, including invoices and renewal logic.', '两类产品分别计费，发票与续费逻辑也保持独立。')}</p></div></section>
      <section className="billing-columns">
        <article className="panel billing-card"><div className="billing-icon"><IconCheck width={22} height={22} /></div><span className="eyebrow">{t('Personal subscription', '个人订阅')}</span><h3>LingXi Pro</h3><strong>{formatCurrency(19, region)}<small>/{t('month, billed annually', '月，按年支付')}</small></strong><p>{t(`Renews ${billingAccount.nextRenewal}.`, `${billingAccount.nextRenewal} 自动续费。`)}</p><button type="button" className="button secondary small">{t('Manage plan', '管理套餐')}</button></article>
        <article className="panel billing-card wallet"><div className="billing-icon"><IconWallet width={22} height={22} /></div><span className="eyebrow">API wallet</span><h3>{t('Available balance', '可用余额')}</h3><strong>{formatCurrency(balance, region)}</strong><p>{t(`Auto-recharge ${autoRecharge ? 'on' : 'off'} at ${formatCurrency(15, region)}.`, `余额低于 ${formatCurrency(15, region)} 时自动充值${autoRecharge ? '已开启' : '已关闭'}。`)}</p><div className="billing-actions"><button ref={topupTriggerRef} type="button" className="button primary small" onClick={() => setShowTopup(true)}>{t('Top up', '充值')}</button><label className="switch"><input type="checkbox" checked={autoRecharge} onChange={(event) => setAutoRecharge(event.target.checked)} /><span />{t('Auto', '自动')}</label></div></article>
      </section>
      <section className="panel table-panel"><PanelHeading title={t('Invoices', '发票')} /><div className="responsive-table"><table><thead><tr><th>{t('Description', '说明')}</th><th>{t('Issued', '开具日期')}</th><th>{t('Amount', '金额')}</th><th>{t('Status', '状态')}</th></tr></thead><tbody>{billingAccount.invoices.map((invoice) => <tr key={invoice.id}><td>{invoice.label}</td><td>{invoice.issuedAt}</td><td>{formatCurrency(invoice.amountUsd, region)}</td><td><span className={`status-badge ${invoice.status === 'paid' ? 'success' : 'accent'}`}>{invoice.status}</span></td></tr>)}</tbody></table></div></section>
      <section className="panel table-panel"><PanelHeading title={t('API wallet activity', 'API 余额流水')} /><div className="responsive-table"><table><thead><tr><th>{t('Activity', '活动')}</th><th>{t('Time', '时间')}</th><th>{t('Amount', '金额')}</th></tr></thead><tbody>{billingAccount.transactions.map((transaction) => <tr key={transaction.id}><td>{transaction.label}</td><td>{transaction.at}</td><td className={transaction.amountUsd < 0 ? 'danger-text' : 'positive-text'}>{transaction.amountUsd > 0 ? '+' : ''}{formatCurrency(transaction.amountUsd, region)}</td></tr>)}</tbody></table></div></section>
      {showTopup ? <Modal title={t('Top up API wallet', '充值 API 余额')} returnFocus={topupTriggerRef.current} onClose={() => setShowTopup(false)}><div className="modal-form"><div className="amount-options">{[20, 50, 100, 300].map((value) => <button type="button" className={amount === value ? 'active' : ''} onClick={() => setAmount(value)} key={value}>{formatCurrency(value, region)}</button>)}</div><label>{t('Payment method', '支付方式')}<select><option>{billingAccount.paymentMethod}</option><option>{t('Add payment method', '添加支付方式')}</option></select></label><div className="estimate-total"><span>{t('Amount', '金额')}</span><strong>{formatCurrency(amount, region)}</strong></div><button type="button" className="button primary" onClick={() => { setBalance((current) => current + amount); setShowTopup(false); }}>{t('Prototype top up', '原型充值')}</button><small>{t('Prototype interaction only. No payment is submitted.', '仅为原型交互，不会提交真实支付。')}</small></div></Modal> : null}
    </div>
  );
}

function Modal({ title, children, onClose, returnFocus }: { title: string; children: ReactNode; onClose: () => void; returnFocus?: HTMLElement | null }) {
  const dialogRef = useRef<HTMLElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const previousFocus = returnFocus ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const root = document.getElementById('root');
    root?.setAttribute('inert', '');
    document.body.classList.add('modal-open');

    const focusableSelector = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])';
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(focusableSelector);
    const preferredFocus = dialogRef.current?.querySelector<HTMLElement>('[autofocus], input:not(:disabled)') ?? focusable?.[0];
    preferredFocus?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const items = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(focusableSelector) ?? []);
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      root?.removeAttribute('inert');
      document.body.classList.remove('modal-open');
      previousFocus?.focus();
    };
  }, []);

  return createPortal(
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <section ref={dialogRef} className="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title">
        <div className="modal-head"><h2 id="modal-title">{title}</h2><button type="button" className="icon-button" aria-label="Close" onClick={onClose}><IconClose width={19} height={19} /></button></div>
        {children}
      </section>
    </div>,
    document.body,
  );
}
