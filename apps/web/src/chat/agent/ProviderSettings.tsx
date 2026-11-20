import { useState, type FormEvent } from 'react';
import presetFile from '../../../../firebase/src/llm-presets.json';
import { isProviderId, type VaultProtocol, type VaultProvider } from '../../../../../packages/llm-vault/contracts';
import { IconArrowRight, IconCheck, IconTrash } from '../../components/Icons';
import type { LlmClient } from './client';
import { LlmError } from './client';
import { errorText } from './errors';

export interface Preset { id: string; label: string; protocol: VaultProtocol; baseUrl: string; defaultModel: string; models: { id: string; name: string }[]; keyUrl: string | null; regions: string[] }
export const PRESETS = (presetFile as { presets: Preset[] }).presets;

const SOURCE: Record<string, [string, string]> = { web: ['Saved on the Web', '网页添加'], desktop: ['From the desktop app', '来自桌面端'], android: ['From Android', '来自 Android'], ios: ['From iOS', '来自 iOS'], unknown: ['Saved', '已保存'] };
const PROTOCOLS: [VaultProtocol, string][] = [['open_ai_chat', 'OpenAI Chat Completions'], ['open_ai_responses', 'OpenAI Responses'], ['anthropic_messages', 'Anthropic Messages'], ['gemini_generate_content', 'Gemini']];

/** `existing` is the record this form changes: a second save after a failed check updates it at its new revision. */
type Form = { kind: 'preset'; preset: Preset; existing?: VaultProvider } | { kind: 'custom'; existing?: VaultProvider };
type Status = { tone: 'busy' | 'ok' | 'error'; text: string } | undefined;

export interface ProviderSettingsProps {
  locale: string;
  client: LlmClient;
  providers: readonly VaultProvider[];
  /** The list changed here; the owner reloads or applies it. */
  onChanged(provider?: VaultProvider): void;
  /** Shown as a dialog: the header offers to close, and a working new provider offers to start chatting. */
  onClose?(): void;
}

/** The model services of this account: the ones saved (from any device), and the presets to connect. Keys go in, never out. */
export function ProviderSettings({ locale, client, providers, onChanged, onClose }: ProviderSettingsProps) {
  const zh = locale === 'zh';
  const t = (en: string, cn: string) => zh ? cn : en;
  const [form, setForm] = useState<Form>();
  const [status, setStatus] = useState<Status>();
  const [rowStatus, setRowStatus] = useState<Record<string, Status>>({});
  const [confirming, setConfirming] = useState<string>();
  const [ready, setReady] = useState<VaultProvider>();
  const byId = new Map(providers.map(provider => [provider.id, provider]));

  const failure = (error: unknown) => errorText(error instanceof LlmError ? error.code : undefined, zh);
  const save = async (input: Parameters<LlmClient['upsertProvider']>[0]) => {
    setStatus({ tone: 'busy', text: t('Saving…', '正在保存…') });
    let saved: VaultProvider;
    try { saved = await client.upsertProvider(input); } catch (error) { setStatus({ tone: 'error', text: failure(error) }); return; }
    onChanged(saved);
    setForm(current => current && { ...current, existing: saved });
    setStatus({ tone: 'busy', text: t('Saved. Checking the key…', '已保存，正在验证 key…') });
    try {
      const result = saved.webUsable ? await client.testProvider(saved.id) : { ok: true };
      if (result.ok) { setForm(undefined); setStatus(undefined); setReady(saved); }
      else setStatus({ tone: 'error', text: `${t('Saved, but the check failed: ', '已保存，但验证未通过：')}${errorText(result.code, zh)}` });
    } catch (error) { setStatus({ tone: 'error', text: failure(error) }); }
  };

  const test = async (provider: VaultProvider) => {
    setRowStatus(current => ({ ...current, [provider.id]: { tone: 'busy', text: t('Checking…', '正在验证…') } }));
    try {
      const result = await client.testProvider(provider.id);
      setRowStatus(current => ({ ...current, [provider.id]: result.ok ? { tone: 'ok', text: t('Working', '可用') } : { tone: 'error', text: errorText(result.code, zh) } }));
    } catch (error) { setRowStatus(current => ({ ...current, [provider.id]: { tone: 'error', text: failure(error) } })); }
  };
  const remove = async (provider: VaultProvider) => {
    setConfirming(undefined);
    try { await client.deleteProvider(provider.id, provider.revision); onChanged(); }
    catch (error) { setRowStatus(current => ({ ...current, [provider.id]: { tone: 'error', text: failure(error) } })); }
  };

  return <section className="agent-settings" aria-labelledby="agent-settings-title">
    <header className="agent-settings-head">
      <div>
        <h2 id="agent-settings-title">{t('Model services', '模型服务')}</h2>
        <p>{t('Keys are kept encrypted with your LingXi account and shared with your desktop app. This page never shows a key again.', '密钥加密保存在你的灵犀账号中，并与桌面端共用。保存后，网页不会再显示密钥。')}</p>
      </div>
      {onClose ? <button type="button" className="chat-button chat-button-quiet" onClick={onClose}>{t('Close', '关闭')}</button> : null}
    </header>

    {ready ? <div className="agent-ready" role="status">
      <IconCheck width={18} height={18} /><span>{t(`${ready.label} is connected.`, `${ready.label} 已连接。`)}</span>
      {onClose ? <button type="button" className="chat-button" onClick={onClose}>{t('Start chatting', '开始对话')}<IconArrowRight width={16} height={16} /></button> : null}
    </div> : null}

    {providers.length ? <ul className="agent-provider-list" aria-label={t('Saved services', '已保存的服务')}>
      {providers.map(provider => {
        const row = rowStatus[provider.id];
        return <li key={provider.id} className="agent-provider-row">
          <span className="chat-side-avatar" aria-hidden="true">{provider.label.slice(0, 1).toUpperCase()}</span>
          <span className="agent-provider-text">
            <strong>{provider.label}</strong>
            <span>{provider.keyHint} · {(SOURCE[provider.source] ?? SOURCE.unknown!)[zh ? 1 : 0]}{provider.webUsable ? '' : ` · ${t('Desktop only', '仅桌面端可用')}`}</span>
            {row ? <span className={`agent-status agent-status-${row.tone}`} role={row.tone === 'error' ? 'alert' : 'status'}>{row.text}</span> : null}
          </span>
          <span className="agent-provider-actions">
            {provider.webUsable ? <button type="button" className="chat-button chat-button-quiet" onClick={() => void test(provider)}>{t('Check', '验证')}</button> : null}
            {provider.presetId && PRESETS.some(p => p.id === provider.presetId) ? <button type="button" className="chat-button chat-button-quiet"
              onClick={() => { setReady(undefined); setStatus(undefined); setForm({ kind: 'preset', preset: PRESETS.find(p => p.id === provider.presetId)!, existing: provider }); }}>{t('Replace key', '更换 key')}</button> : null}
            {confirming === provider.id
              ? <button type="button" className="chat-button chat-button-danger" onClick={() => void remove(provider)}>{t('Delete from my account (copies already pulled to a desktop app stay there)', '从账号中删除（已拉取到桌面端的副本仍会保留）')}</button>
              : <button type="button" className="chat-icon-button" aria-label={t(`Delete ${provider.label}`, `删除 ${provider.label}`)} onClick={() => setConfirming(provider.id)}><IconTrash width={16} height={16} /></button>}
          </span>
        </li>;
      })}
    </ul> : null}

    {form ? <ProviderForm key={form.kind === 'preset' ? form.preset.id : 'custom'} form={form} zh={zh} status={status} taken={id => byId.has(id)}
      onCancel={() => { setForm(undefined); setStatus(undefined); }} onSave={input => void save(input)} />
      : <>
        <h3 className="agent-settings-sub">{providers.length ? t('Connect another', '连接更多服务') : t('Connect a model service', '连接一个模型服务')}</h3>
        <ul className="agent-preset-grid">
          {PRESETS.map(preset => {
            const connected = byId.has(preset.id);
            return <li key={preset.id}><button type="button" className="agent-preset" aria-label={`${preset.label}${connected ? t(' (connected)', '（已连接）') : ''}`}
              onClick={() => { setReady(undefined); setStatus(undefined); setForm({ kind: 'preset', preset, existing: byId.get(preset.id) }); }}>
              <span className="agent-preset-name">{preset.label}</span>
              <span className="agent-preset-sub">{connected ? <><IconCheck width={14} height={14} />{t('Connected', '已连接')}</> : preset.models[0]?.name ?? preset.defaultModel}</span>
            </button></li>;
          })}
          <li><button type="button" className="agent-preset agent-preset-custom" onClick={() => { setReady(undefined); setStatus(undefined); setForm({ kind: 'custom' }); }}>
            <span className="agent-preset-name">{t('Custom', '自定义')}</span><span className="agent-preset-sub">{t('OpenAI-compatible and more', 'OpenAI 兼容等')}</span>
          </button></li>
        </ul>
      </>}
  </section>;
}

function ProviderForm({ form, zh, status, taken, onCancel, onSave }: { form: Form; zh: boolean; status: Status; taken(id: string): boolean; onCancel(): void; onSave: (input: Parameters<LlmClient['upsertProvider']>[0]) => void }) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [key, setKey] = useState('');
  const [custom, setCustom] = useState({ id: '', label: '', baseUrl: '', protocol: 'open_ai_chat' as VaultProtocol, models: '' });
  const [problem, setProblem] = useState('');
  const busy = status?.tone === 'busy';
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const apiKey = key.trim();
    if (form.kind === 'preset') {
      if (!apiKey) { setProblem(t('Paste the API key.', '请粘贴 API key。')); return; }
      // The key leaves this component's state as soon as it is sent.
      setKey('');
      onSave({ action: 'upsert', id: form.preset.id, presetId: form.preset.id, apiKey, ...(form.existing ? { expectedRevision: form.existing.revision } : {}) });
      return;
    }
    const models = custom.models.split(/[\s,]+/u).filter(Boolean).slice(0, 100);
    const existing = form.existing?.id === custom.id ? form.existing : undefined;
    if (!isProviderId(custom.id)) { setProblem(t('The id uses lowercase letters, digits, dots, dashes and underscores.', 'ID 只能使用小写字母、数字、点、连字符和下划线。')); return; }
    if (PRESETS.some(preset => preset.id === custom.id)) { setProblem(t('This id belongs to a listed service; connect it from the list, or pick another id.', '这个 ID 属于列表中的服务，请从列表连接，或换一个 ID。')); return; }
    if (taken(custom.id) && !existing) { setProblem(t('A service with this id already exists.', '已有同样 ID 的服务。')); return; }
    if (!/^https:\/\/[^/\s]+\.[^/\s]+/u.test(custom.baseUrl.trim())) { setProblem(t('The base URL must start with https://.', '服务地址必须以 https:// 开头。')); return; }
    if (!models.length) { setProblem(t('Add at least one model id.', '至少填写一个模型 ID。')); return; }
    if (!apiKey) { setProblem(t('Paste the API key.', '请粘贴 API key。')); return; }
    setKey(''); setProblem('');
    onSave({ action: 'upsert', id: custom.id, label: custom.label.trim() || custom.id, protocol: custom.protocol, baseUrl: custom.baseUrl.trim(),
      models: models.map(id => ({ id })), defaultModel: models[0], apiKey, ...(existing ? { expectedRevision: existing.revision } : {}) });
  };
  const title = form.kind === 'preset' ? form.preset.label : t('Custom service', '自定义服务');
  return <form className="agent-form" onSubmit={submit} aria-label={title}>
    <h3>{form.kind === 'preset' && form.existing ? t(`Replace the ${title} key`, `更换 ${title} 的 key`) : t(`Connect ${title}`, `连接 ${title}`)}</h3>
    {form.kind === 'custom' ? <div className="agent-form-grid">
      <label>{t('ID', 'ID')}<input value={custom.id} onChange={e => setCustom({ ...custom, id: e.target.value.trim().toLowerCase() })} placeholder="my-gateway" autoComplete="off" spellCheck={false} required /></label>
      <label>{t('Name', '名称')}<input value={custom.label} onChange={e => setCustom({ ...custom, label: e.target.value })} placeholder={t('My gateway', '我的网关')} maxLength={80} /></label>
      <label className="agent-form-wide">{t('Base URL', '服务地址')}<input value={custom.baseUrl} onChange={e => setCustom({ ...custom, baseUrl: e.target.value })} placeholder="https://api.example.com/v1" inputMode="url" autoComplete="off" spellCheck={false} required /></label>
      <label>{t('Protocol', '协议')}<select value={custom.protocol} onChange={e => setCustom({ ...custom, protocol: e.target.value as VaultProtocol })}>{PROTOCOLS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
      <label>{t('Model ids', '模型 ID')}<input value={custom.models} onChange={e => setCustom({ ...custom, models: e.target.value })} placeholder="model-a, model-b" autoComplete="off" spellCheck={false} required /></label>
    </div> : null}
    <label className="agent-form-key">{t('API key', 'API key')}
      <input type="password" value={key} onChange={e => { setKey(e.target.value); setProblem(''); }} autoComplete="off" spellCheck={false} placeholder={form.kind === 'preset' && form.existing ? form.existing.keyHint : 'sk-…'} required />
    </label>
    {form.kind === 'preset' && form.preset.keyUrl ? <a className="agent-form-link" href={form.preset.keyUrl} target="_blank" rel="noopener noreferrer">{t(`Get a ${form.preset.label} API key`, `获取 ${form.preset.label} API key`)}<IconArrowRight width={14} height={14} /></a> : null}
    {problem ? <p className="agent-status agent-status-error" role="alert">{problem}</p> : status ? <p className={`agent-status agent-status-${status.tone}`} role={status.tone === 'error' ? 'alert' : 'status'}>{status.text}</p> : null}
    <div className="agent-form-actions">
      <button type="button" className="chat-button chat-button-quiet" onClick={onCancel} disabled={busy}>{t('Cancel', '取消')}</button>
      <button type="submit" className="chat-button" disabled={busy}>{t('Save', '保存')}</button>
    </div>
  </form>;
}
