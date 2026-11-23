import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from 'react';
import presetFile from '../../../../firebase/src/llm-presets.json';
import { isProviderId, type VaultProtocol, type VaultProvider } from '../../../../../packages/llm-vault/contracts';
import { IconArrowRight, IconCheck, IconTrash } from '../../components/Icons';
import { providerBrandMark } from '../../../../../assets/brand/provider-icons/marks';
import type { LlmClient } from './client';
import { LlmError } from './client';
import { errorText } from './errors';

export interface Preset { id: string; label: string; protocol: VaultProtocol; baseUrl: string; defaultModel: string; models: { id: string; name: string }[]; keyUrl: string | null; regions: string[] }
export const PRESETS = (presetFile as { presets: Preset[] }).presets;

const SOURCE: Record<string, [string, string]> = { web: ['Saved on the Web', '网页添加'], desktop: ['From the desktop app', '来自桌面端'], android: ['From Android', '来自 Android'], ios: ['From iOS', '来自 iOS'], unknown: ['Saved', '已保存'] };
const PROTOCOLS: [VaultProtocol, string][] = [['open_ai_chat', 'OpenAI Chat Completions'], ['open_ai_responses', 'OpenAI Responses'], ['anthropic_messages', 'Anthropic Messages'], ['gemini_generate_content', 'Gemini']];

/** `existing` is the record this form changes: a second save after a failed check updates it at its new revision. */
type Form = { kind: 'preset'; preset: Preset; existing?: VaultProvider } | { kind: 'custom'; existing?: VaultProvider };
type ProviderPopup = { title: string; text: string; ready?: boolean };

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
  const [saving, setSaving] = useState<string>();
  const [checking, setChecking] = useState<string>();
  const [removing, setRemoving] = useState(false);
  const [confirming, setConfirming] = useState<VaultProvider>();
  const [feedback, setFeedback] = useState<ProviderPopup>();
  const byId = new Map(providers.map(provider => [provider.id, provider]));

  const failure = (error: unknown) => errorText(error instanceof LlmError ? error.code : undefined, zh);
  const showError = (text: string) => setFeedback({ title: t('Model services', '模型服务'), text });
  const save = async (input: Parameters<LlmClient['upsertProvider']>[0]) => {
    setSaving(t('Saving…', '正在保存…'));
    try {
      const saved = await client.upsertProvider(input);
      onChanged(saved);
      setForm(current => current && { ...current, existing: saved });
      setSaving(t('Checking…', '正在验证…'));
      const result = saved.webUsable ? await client.testProvider(saved.id) : { ok: true };
      if (result.ok) {
        setForm(undefined);
        setFeedback({ title: saved.label, text: t(`${saved.label} is connected.`, `${saved.label} 已连接。`), ready: true });
      } else {
        showError(`${t('Saved, but the check failed: ', '已保存，但验证未通过：')}${errorText(result.code, zh)}`);
      }
    } catch (error) { showError(failure(error)); }
    finally { setSaving(undefined); }
  };

  const test = async (provider: VaultProvider) => {
    setChecking(provider.id);
    try {
      const result = await client.testProvider(provider.id);
      setFeedback({ title: provider.label, text: result.ok ? t('Connection test succeeded.', '连接测试成功。') : errorText(result.code, zh) });
    } catch (error) { showError(failure(error)); }
    finally { setChecking(undefined); }
  };
  const remove = async (provider: VaultProvider) => {
    setRemoving(true);
    setConfirming(undefined);
    try { await client.deleteProvider(provider.id, provider.revision); onChanged(); }
    catch (error) { showError(failure(error)); }
    finally { setRemoving(false); }
  };
  const info = t('Keys are kept encrypted with your LingXi account and shared with your desktop app. This page never shows a key again.', '密钥加密保存在你的灵犀账号中，并与桌面端共用。保存后，网页不会再显示密钥。');
  const cancel = t('Cancel', '取消');
  const close = t('Close', '关闭');
  const busy = Boolean(saving || checking || removing);

  return <section className="agent-settings" aria-labelledby="agent-settings-title">
    <header className="agent-settings-head">
      <div className="agent-settings-title">
        <h2 id="agent-settings-title">{t('Model services', '模型服务')}</h2>
        <button type="button" className="chat-icon-button" aria-label={t('About model services', '关于模型服务')}
          onClick={() => setFeedback({ title: t('Model services', '模型服务'), text: info })}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
            <circle cx="12" cy="12" r="9" /><path d="M12 11v6m0-10v2" />
          </svg>
        </button>
      </div>
      {onClose ? <button type="button" className="chat-button chat-button-quiet" onClick={onClose}>{t('Close', '关闭')}</button> : null}
    </header>

    {providers.length ? <ul className="agent-provider-list" aria-label={t('Saved services', '已保存的服务')}>
      {providers.map(provider => {
        return <li key={provider.id} className="agent-provider-row">
          <ServiceMark id={provider.presetId ?? ''} label={provider.label} />
          <span className="agent-provider-text">
            <strong>{provider.label}</strong>
            <span>{provider.keyHint} · {(SOURCE[provider.source] ?? SOURCE.unknown!)[zh ? 1 : 0]}{provider.webUsable ? '' : ` · ${t('Desktop only', '仅桌面端可用')}`}</span>
          </span>
          <span className="agent-provider-actions">
            {provider.webUsable ? <button type="button" className="chat-button chat-button-quiet" disabled={busy} onClick={() => void test(provider)}>{checking === provider.id ? t('Checking…', '正在验证…') : t('Check', '验证')}</button> : null}
            {provider.presetId && PRESETS.some(p => p.id === provider.presetId) ? <button type="button" className="chat-button chat-button-quiet"
              disabled={busy} onClick={() => { setForm({ kind: 'preset', preset: PRESETS.find(p => p.id === provider.presetId)!, existing: provider }); }}>{t('Replace key', '更换 key')}</button> : null}
            <button type="button" className="chat-icon-button" disabled={busy} aria-label={t(`Delete ${provider.label}`, `删除 ${provider.label}`)} onClick={() => setConfirming(provider)}><IconTrash width={16} height={16} /></button>
          </span>
        </li>;
      })}
    </ul> : null}

    {form ? <ProviderForm key={form.kind === 'preset' ? form.preset.id : 'custom'} form={form} zh={zh} saving={saving} locked={Boolean(checking || removing)} taken={id => byId.has(id)}
      onCancel={() => setForm(undefined)} onProblem={showError} onSave={input => void save(input)} />
      : <>
        <h3 className="agent-settings-sub">{providers.length ? t('Connect another', '连接更多服务') : t('Connect a model service', '连接一个模型服务')}</h3>
        <ul className="agent-preset-grid">
          {PRESETS.map(preset => {
            const connected = byId.has(preset.id);
            return <li key={preset.id}><button type="button" className="agent-preset" aria-label={`${preset.label}${connected ? t(' (connected)', '（已连接）') : ''}`}
              disabled={busy} onClick={() => setForm({ kind: 'preset', preset, existing: byId.get(preset.id) })}>
              <ServiceMark id={preset.id} label={preset.label} />
              <span>
                <span className="agent-preset-name">{preset.label}</span>
                <span className="agent-preset-sub">{connected ? <><IconCheck width={14} height={14} />{t('Connected', '已连接')}</> : preset.models[0]?.name ?? preset.defaultModel}</span>
              </span>
            </button></li>;
          })}
          <li><button type="button" className="agent-preset agent-preset-custom" disabled={busy} onClick={() => setForm({ kind: 'custom' })}>
            <span className="agent-preset-name">{t('Custom', '自定义')}</span><span className="agent-preset-sub">{t('OpenAI-compatible and more', 'OpenAI 兼容等')}</span>
          </button></li>
        </ul>
      </>}
    {confirming && <ServiceDialog title={t(`Delete ${confirming.label}?`, `删除 ${confirming.label}？`)}
      text={t('Copies pulled to a desktop app stay there.', '已拉取到桌面端的副本仍会保留。')} onDismiss={() => setConfirming(undefined)}>
      <button type="submit" className="chat-button chat-button-quiet">{cancel}</button>
      <button type="button" className="chat-button chat-button-danger" onClick={() => void remove(confirming)}>{t('Delete', '删除')}</button>
    </ServiceDialog>}
    {feedback && !confirming && <ServiceDialog title={feedback.title} text={feedback.text} onDismiss={() => setFeedback(undefined)}>
      <button type="submit" className="chat-button chat-button-quiet">{close}</button>
      {feedback.ready && onClose && <button type="button" className="chat-button" onClick={onClose}>{t('Start chatting', '开始对话')}<IconArrowRight width={16} height={16} /></button>}
    </ServiceDialog>}
  </section>;
}

function ServiceMark({ id, label }: { id: string; label: string }) {
  const mark = providerBrandMark(id);
  return <span className="chat-side-avatar agent-service-mark" aria-hidden="true">
    {mark?.color ? <img src={mark.source} alt="" />
      : mark ? <span style={{ maskImage: `url("${mark.source}")` }} />
      : label.slice(0, 1).toUpperCase()}
  </span>;
}

function ServiceDialog({ title, text, onDismiss, children }: {
  title: string; text: string; onDismiss(): void; children: ReactNode;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const id = useId();
  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    element.showModal();
    return () => element.close();
  }, []);
  return <dialog ref={dialog} className="agent-dialog agent-service-popup" aria-labelledby={`${id}-title`} aria-describedby={`${id}-message`}
    onClose={onDismiss}>
    <h3 id={`${id}-title`}>{title}</h3>
    <p id={`${id}-message`}>{text}</p>
    <form method="dialog" className="agent-form-actions">{children}</form>
  </dialog>;
}

function ProviderForm({ form, zh, saving, locked, taken, onCancel, onProblem, onSave }: {
  form: Form; zh: boolean; saving?: string; locked: boolean; taken(id: string): boolean;
  onCancel(): void; onProblem(message: string): void; onSave: (input: Parameters<LlmClient['upsertProvider']>[0]) => void;
}) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [key, setKey] = useState('');
  const [custom, setCustom] = useState({ id: '', label: '', baseUrl: '', protocol: 'open_ai_chat' as VaultProtocol, models: '' });
  const busy = Boolean(saving || locked);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    const apiKey = key.trim();
    if (form.kind === 'preset') {
      if (!apiKey) { onProblem(t('Paste the API key.', '请粘贴 API key。')); return; }
      // The key leaves this component's state as soon as it is sent.
      setKey('');
      onSave({ action: 'upsert', id: form.preset.id, presetId: form.preset.id, apiKey, ...(form.existing ? { expectedRevision: form.existing.revision } : {}) });
      return;
    }
    const models = custom.models.split(/[\s,]+/u).filter(Boolean).slice(0, 100);
    const existing = form.existing?.id === custom.id ? form.existing : undefined;
    if (!isProviderId(custom.id)) { onProblem(t('The id uses lowercase letters, digits, dots, dashes and underscores.', 'ID 只能使用小写字母、数字、点、连字符和下划线。')); return; }
    if (PRESETS.some(preset => preset.id === custom.id)) { onProblem(t('This id belongs to a listed service; connect it from the list, or pick another id.', '这个 ID 属于列表中的服务，请从列表连接，或换一个 ID。')); return; }
    if (taken(custom.id) && !existing) { onProblem(t('A service with this id already exists.', '已有同样 ID 的服务。')); return; }
    if (!/^https:\/\/[^/\s]+\.[^/\s]+/u.test(custom.baseUrl.trim())) { onProblem(t('The base URL must start with https://.', '服务地址必须以 https:// 开头。')); return; }
    if (!models.length) { onProblem(t('Add at least one model id.', '至少填写一个模型 ID。')); return; }
    if (!apiKey) { onProblem(t('Paste the API key.', '请粘贴 API key。')); return; }
    setKey('');
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
      <input type="password" value={key} onChange={e => setKey(e.target.value)} autoComplete="off" spellCheck={false} placeholder={form.kind === 'preset' && form.existing ? form.existing.keyHint : 'sk-…'} required />
    </label>
    {form.kind === 'preset' && form.preset.keyUrl ? <a className="agent-form-link" href={form.preset.keyUrl} target="_blank" rel="noopener noreferrer">{t(`Get a ${form.preset.label} API key`, `获取 ${form.preset.label} API key`)}<IconArrowRight width={14} height={14} /></a> : null}
    <div className="agent-form-actions">
      <button type="button" className="chat-button chat-button-quiet" onClick={onCancel} disabled={busy}>{t('Cancel', '取消')}</button>
      <button type="submit" className="chat-button" disabled={busy}>{saving ?? t('Save', '保存')}</button>
    </div>
  </form>;
}
