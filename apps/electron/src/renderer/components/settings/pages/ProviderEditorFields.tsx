import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useRef, useState } from 'react';
import { useT } from '../../../theme/ThemeContext';
import { SUPPORTED_PROVIDER_TYPES, type CustomProviderDraft, type CustomProviderModelDraft } from './customProviderImport';
import { renameProviderModel, removeProviderModel, canRemoveProviderModel, addProviderConnection, removeProviderConnection, updateProviderConnection, providerConnections } from './customProviderDraft';
import { ghostButtonStyle } from './ghostButton';

/** Shared editor keeps unknown provider/model properties while changing the visible fields. */
export function ProviderEditorFields({ name, onName, nameLocked, draft, onDraft, apiKey, onApiKey, credentialConfigured, envOnly = false, hostManagedCredentials = false }: {
  name: string; onName(value: string): void; nameLocked?: boolean;
  draft: CustomProviderDraft; onDraft(value: CustomProviderDraft): void;
  apiKey: string; onApiKey(value: string): void; credentialConfigured?: boolean; envOnly?: boolean; hostManagedCredentials?: boolean;
}) {
  useI18n();
  const t = useT();
  const pricingIds = useRef(draft.models.map((model) => typeof model.id === 'string' ? model.id.trim() : ''));
  const [authMode, setAuthMode] = useState(envOnly ? 'env' : apiKey.trim() ? 'key' : draft.apiKeyEnv ? 'env' : 'key');
  const connections = providerConnections(draft);
  const input = { width: '100%', minWidth: 0, boxSizing: 'border-box' as const, padding: '9px 11px', borderRadius: 7, border: `1px solid ${t.border}`, background: t.surface, color: t.text, fontFamily: 'inherit', fontSize: 13 };
  const field = { display: 'grid', gap: 7, minWidth: 0, fontSize: 12.5, color: t.text3 };
  return <div style={{ display: 'grid', gap: 16, maxWidth: 860 }}>
    <div style={{ display: 'grid', gap: 14 }}>
      <strong style={{ fontSize: 13 }}>{tr('model_detail_basic')}</strong>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 240px), 1fr))', gap: 14 }}>
        <label style={field}>{tr('desktop_settings_bfecf6eb84')}<input aria-label={tr('desktop_settings_bfecf6eb84')} readOnly={nameLocked} value={name} onChange={(event) => onName(event.target.value)} placeholder={tr('desktop_settings_9510b434c0')} style={input} /></label>
        <label style={field}>{tr('settings_provider_protocol')}<select aria-label={tr('desktop_settings_95e1edbd5f')} value={draft.type} onChange={(event) => onDraft({ ...draft, type: event.target.value })} style={input}>
          {!SUPPORTED_PROVIDER_TYPES.includes(draft.type as typeof SUPPORTED_PROVIDER_TYPES[number]) && <option value="">{tr('settings_provider_choose_protocol')}</option>}
          {SUPPORTED_PROVIDER_TYPES.map((type) => <option value={type} key={type}>{type}</option>)}
        </select></label>
      </div>
      <label style={field}>{connections.length ? tr('desktop_settings_be416cc922') : tr('desktop_settings_cc1781cdb6')}<input aria-label="baseUrl" value={typeof draft.baseUrl === 'string' ? draft.baseUrl : ''} onChange={(event) => onDraft({ ...draft, baseUrl: event.target.value || undefined })} placeholder="https://api.example.com/v1" spellCheck={false} style={input} /></label>
      {draft.type === 'azure-openai' && <label style={field}>{tr('settings_provider_api_version')}<input aria-label="apiVersion" value={typeof draft.apiVersion === 'string' ? draft.apiVersion : ''} onChange={(event) => onDraft({ ...draft, apiVersion: event.target.value })} placeholder="2024-10-21" style={input} /></label>}
      {draft.type === 'bedrock-claude' && <label style={field}>{tr('desktop_settings_aws_region')}<input aria-label="region" value={typeof draft.region === 'string' ? draft.region : ''} onChange={(event) => onDraft({ ...draft, region: event.target.value })} placeholder="us-east-1" style={input} /></label>}
    </div>
    <div style={{ display: 'grid', gap: 12 }}>
      <strong style={{ fontSize: 13 }}>{tr('desktop_settings_d2d648bd1c')}</strong>
      <label style={field}>{tr('desktop_settings_d1ff8f98bc')}<select aria-label={tr('desktop_settings_d1ff8f98bc')} value={authMode} onChange={(event) => {
        setAuthMode(event.target.value);
        if (event.target.value === 'key') onDraft({ ...draft, apiKeyEnv: undefined }); else onApiKey('');
      }} style={input}>{!envOnly && <option value="key">{tr('desktop_settings_af92b1c90b')}</option>}<option value="env" disabled={hostManagedCredentials}>{tr('desktop_settings_ae27b474ea')}</option></select></label>
      {envOnly && <span style={{ color: t.text3, fontSize: 12 }}>{hostManagedCredentials
        ? tr('desktop_settings_6c4934e558')
        : tr('desktop_settings_63ab6c6735')}</span>}
      {authMode === 'key' ? <label style={field}>{tr('settings_provider_api_key')}<input aria-label={tr('settings_provider_api_key')} type="password" autoComplete="new-password" value={apiKey} onChange={(event) => onApiKey(event.target.value)} placeholder={credentialConfigured ? tr('desktop_settings_9aca67d120') : tr('settings_provider_api_key_placeholder')} style={input} /></label>
        : <label style={field}>{tr('desktop_settings_856fddd137')}<input aria-label="apiKeyEnv" disabled={hostManagedCredentials} value={typeof draft.apiKeyEnv === 'string' ? draft.apiKeyEnv : ''} onChange={(event) => onDraft({ ...draft, apiKeyEnv: event.target.value || undefined })} placeholder="MY_PROVIDER_API_KEY" spellCheck={false} style={input} /></label>}
      {!hostManagedCredentials && authMode === 'key' && draft.apiKeyEnv && <span style={{ color: t.text3, fontSize: 12 }}>{tr('desktop_settings_environment_fallback', typeof draft.apiKeyEnv === 'string' ? draft.apiKeyEnv : '')}</span>}
      <span style={{ color: t.text4, fontSize: 12 }}>{hostManagedCredentials
        ? tr('desktop_settings_f65e758fce')
        : tr('desktop_settings_938cc1197f')}</span>
    </div>
    <div style={{ display: 'grid', gap: 12 }}>
      <strong style={{ fontSize: 13 }}>{tr('settings_provider_connections')}</strong>
      <span style={{ color: t.text4, fontSize: 12 }}>
        {tr('desktop_settings_9e7c47bc4c')}</span>
      {connections.length === 0
        ? <span style={{ color: t.text3, fontSize: 12 }}>{tr('settings_provider_connections_single')}</span>
        : connections.map((connection, index) => <div key={index} style={{ display: 'grid', gap: 10, padding: 12, borderRadius: 8, border: `1px solid ${t.border}` }}>
          <div style={{ display: 'flex', alignItems: 'end', gap: 8 }}>
            <label style={{ ...field, flex: 1 }}>{tr('settings_provider_connection_id')} {index + 1}<input aria-label={tr('desktop_settings_d98b428a96', index + 1)} value={typeof connection.id === 'string' ? connection.id : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { id: event.target.value }))} placeholder={tr('desktop_settings_f80cc8f73e')} spellCheck={false} style={input} /></label>
            <button type="button" aria-label={tr('desktop_settings_a5d30b277a', index + 1)} style={ghostButtonStyle(t, false, true)} onClick={() => onDraft(removeProviderConnection(draft, index))}>{tr('settings_parity_remove')}</button>
          </div>
          <label style={field}>{tr('desktop_settings_cc1781cdb6')}<input aria-label={tr('desktop_settings_b36cc73c68', index + 1)} value={typeof connection.baseUrl === 'string' ? connection.baseUrl : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { baseUrl: event.target.value || undefined }))} placeholder="https://api.example.cn/v1" spellCheck={false} style={input} /></label>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 220px), 1fr))', gap: 12 }}>
            <label style={field}>{tr('desktop_settings_1a627ed944')}<select aria-label={tr('desktop_settings_358ef11dee', index + 1)} value={typeof connection.type === 'string' ? connection.type : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { type: event.target.value || undefined }))} style={input}>
              <option value="">{tr('desktop_settings_ab95d61e3b')}{draft.type}</option>
              {SUPPORTED_PROVIDER_TYPES.map((type) => <option value={type} key={type}>{type}</option>)}
            </select></label>
            <label style={field}>{tr('desktop_settings_856fddd137')}<input aria-label={tr('desktop_settings_6b988b728f', index + 1)} disabled={hostManagedCredentials} value={typeof connection.apiKeyEnv === 'string' ? connection.apiKeyEnv : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { apiKeyEnv: event.target.value || undefined }))} placeholder="MY_PROVIDER_CN_API_KEY" spellCheck={false} style={input} /></label>
          </div>
          <span style={{ color: t.text4, fontSize: 11.5 }}>{tr('desktop_settings_connection_credential_default', name || 'provider')}</span>
        </div>)}
      <button type="button" style={{ ...ghostButtonStyle(t), justifySelf: 'start' }} onClick={() => onDraft(addProviderConnection(draft))}>{tr('settings_provider_connection_add')}</button>
    </div>
    <div style={{ display: 'grid', gap: 10 }}>
      <strong style={{ fontSize: 13 }}>{tr('session_details_model')}</strong>
      {draft.models.map((model, index) => <div key={index} style={{ display: 'flex', alignItems: 'end', gap: 8 }}>
        <label style={{ ...field, flex: 1 }}>{tr('settings_provider_model_id')} {index + 1}<input aria-label={tr('desktop_settings_684fd6b4e8', index + 1)} value={typeof model.id === 'string' ? model.id : ''} onChange={(event) => { const next = renameProviderModel(draft, index, event.target.value, pricingIds.current[index]); pricingIds.current[index] = next.pricingId; onDraft(next.draft); }} placeholder={tr('desktop_settings_9265913c97')} style={input} />{Array.isArray(model.aliases) && model.aliases.length ? <span style={{ fontSize: 11 }}>{tr('desktop_settings_cf95352aaf')}{model.aliases.filter((alias) => typeof alias === 'string').join('、')}</span> : null}</label>
        <ContextPolicyField model={model} onChange={next => onDraft({ ...draft, models: draft.models.map((m, i) => i === index ? next : m) })} />
        <button type="button" aria-label={tr('desktop_settings_1b591af7e1', index + 1)} aria-describedby={canRemoveProviderModel(draft, index, pricingIds.current) ? undefined : 'provider-duplicate-model-hint'} disabled={!canRemoveProviderModel(draft, index, pricingIds.current)} style={ghostButtonStyle(t, false, true)} onClick={() => { const next = removeProviderModel(draft, index, pricingIds.current[index], pricingIds.current); pricingIds.current.splice(index, 1); onDraft(next); }}>{tr('settings_parity_remove')}</button>
      </div>)}
      {draft.models.some((_, index) => !canRemoveProviderModel(draft, index, pricingIds.current)) && <span id="provider-duplicate-model-hint" style={{ color: t.warn, fontSize: 12 }}>{tr('desktop_settings_b19f57238b')}</span>}
      <button type="button" style={{ ...ghostButtonStyle(t), justifySelf: 'start' }} onClick={() => { pricingIds.current.push(''); onDraft({ ...draft, models: [...draft.models, { id: '' }] }); }}>{tr('settings_parity_add_model')}</button>
    </div>
  </div>;
}

function ContextPolicyField({ model, onChange }: { model: CustomProviderModelDraft; onChange(model: CustomProviderModelDraft): void }) {
  const metadata = model.metadata && typeof model.metadata === 'object' && !Array.isArray(model.metadata) ? model.metadata as Record<string, unknown> : {};
  const context = metadata.context && typeof metadata.context === 'object' && !Array.isArray(metadata.context) ? metadata.context as Record<string, unknown> : {};
  const application = context.application && typeof context.application === 'object' ? context.application as Record<string, unknown> : {};
  const mode = typeof application.mode === 'string' ? application.mode : 'auto';
  const update = (application: Record<string, unknown>) => onChange({ ...model, metadata: { ...metadata, context: { ...context, application } } });
  return <label style={{ display: 'grid', gap: 7, fontSize: 12.5 }}>{tr('model_detail_context_policy')}<select aria-label={tr('desktop_settings_e85920d46b', String(model.id))} value={mode} onChange={e => update(e.target.value === 'custom' ? { mode: 'custom', input_tokens: '' } : { mode: e.target.value })}>
      <option value="auto">{tr('desktop_settings_5298ba6e24')}</option><option value="long">{tr('composer_context_policy_long')}</option><option value="custom">{tr('desktop_settings_11659082d5')}</option>
    </select>
    {mode === 'custom' && <input aria-label={tr('desktop_settings_2d4cb78cb0', String(model.id))} type="number" min="1" step="1" value={typeof application.input_tokens === 'number' ? application.input_tokens : ''} onChange={e => update({ mode: 'custom', input_tokens: e.target.value === '' ? '' : Number(e.target.value) })} />}
  </label>;
}
