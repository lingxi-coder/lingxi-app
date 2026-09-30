import { useRef, useState } from 'react';
import { useT } from '../../../theme/ThemeContext';
import { SUPPORTED_PROVIDER_TYPES, type CustomProviderDraft } from './customProviderImport';
import { renameProviderModel, removeProviderModel, canRemoveProviderModel, addProviderConnection, removeProviderConnection, updateProviderConnection, providerConnections } from './customProviderDraft';
import { ghostButtonStyle } from './ghostButton';

/** Shared editor keeps unknown provider/model properties while changing the visible fields. */
export function ProviderEditorFields({ name, onName, nameLocked, draft, onDraft, apiKey, onApiKey, credentialConfigured, envOnly = false }: {
  name: string; onName(value: string): void; nameLocked?: boolean;
  draft: CustomProviderDraft; onDraft(value: CustomProviderDraft): void;
  apiKey: string; onApiKey(value: string): void; credentialConfigured?: boolean; envOnly?: boolean;
}) {
  const t = useT();
  const pricingIds = useRef(draft.models.map((model) => typeof model.id === 'string' ? model.id.trim() : ''));
  const [authMode, setAuthMode] = useState(envOnly ? 'env' : apiKey.trim() ? 'key' : draft.apiKeyEnv ? 'env' : 'key');
  const connections = providerConnections(draft);
  const input = { width: '100%', minWidth: 0, boxSizing: 'border-box' as const, padding: '9px 11px', borderRadius: 7, border: `1px solid ${t.border}`, background: t.surface, color: t.text, fontFamily: 'inherit', fontSize: 13 };
  const field = { display: 'grid', gap: 7, minWidth: 0, fontSize: 12.5, color: t.text3 };
  return <div style={{ display: 'grid', gap: 16, maxWidth: 860 }}>
    <div style={{ display: 'grid', gap: 14 }}>
      <strong style={{ fontSize: 13 }}>基本信息</strong>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 240px), 1fr))', gap: 14 }}>
        <label style={field}>Profile 名称<input aria-label="Profile 名称" readOnly={nameLocked} value={name} onChange={(event) => onName(event.target.value)} placeholder="例如 my-provider" style={input} /></label>
        <label style={field}>协议<select aria-label="Provider 类型" value={draft.type} onChange={(event) => onDraft({ ...draft, type: event.target.value })} style={input}>
          {!SUPPORTED_PROVIDER_TYPES.includes(draft.type as typeof SUPPORTED_PROVIDER_TYPES[number]) && <option value="">请选择协议</option>}
          {SUPPORTED_PROVIDER_TYPES.map((type) => <option value={type} key={type}>{type}</option>)}
        </select></label>
      </div>
      <label style={field}>{connections.length ? '服务地址 · Base URL（默认值，可被下方各连接覆盖）' : '服务地址 · Base URL'}<input aria-label="baseUrl" value={typeof draft.baseUrl === 'string' ? draft.baseUrl : ''} onChange={(event) => onDraft({ ...draft, baseUrl: event.target.value || undefined })} placeholder="https://api.example.com/v1" spellCheck={false} style={input} /></label>
      {draft.type === 'azure-openai' && <label style={field}>API 版本<input aria-label="apiVersion" value={typeof draft.apiVersion === 'string' ? draft.apiVersion : ''} onChange={(event) => onDraft({ ...draft, apiVersion: event.target.value })} placeholder="2024-10-21" style={input} /></label>}
      {draft.type === 'bedrock-claude' && <label style={field}>AWS Region<input aria-label="region" value={typeof draft.region === 'string' ? draft.region : ''} onChange={(event) => onDraft({ ...draft, region: event.target.value })} placeholder="us-east-1" style={input} /></label>}
    </div>
    <div style={{ display: 'grid', gap: 12 }}>
      <strong style={{ fontSize: 13 }}>认证</strong>
      <label style={field}>认证方式<select aria-label="认证方式" value={authMode} onChange={(event) => {
        setAuthMode(event.target.value);
        if (event.target.value === 'key') onDraft({ ...draft, apiKeyEnv: undefined }); else onApiKey('');
      }} style={input}>{!envOnly && <option value="key">直接填写 API Key</option>}<option value="env">环境变量</option></select></label>
      {envOnly && <span style={{ color: t.text3, fontSize: 12 }}>此旧版 Profile ID 仅支持环境变量认证，名称保持不变。</span>}
      {authMode === 'key' ? <label style={field}>API Key<input aria-label="API Key" type="password" autoComplete="new-password" value={apiKey} onChange={(event) => onApiKey(event.target.value)} placeholder={credentialConfigured ? '•••••••• 已保存，留空保持不变' : '输入 API Key'} style={input} /></label>
        : <label style={field}>环境变量名称<input aria-label="apiKeyEnv" value={typeof draft.apiKeyEnv === 'string' ? draft.apiKeyEnv : ''} onChange={(event) => onDraft({ ...draft, apiKeyEnv: event.target.value || undefined })} placeholder="MY_PROVIDER_API_KEY" spellCheck={false} style={input} /></label>}
      {authMode === 'key' && draft.apiKeyEnv && <span style={{ color: t.text3, fontSize: 12 }}>API Key 将优先使用，环境变量 {typeof draft.apiKeyEnv === 'string' ? draft.apiKeyEnv : ''} 保留为回退。</span>}
      <span style={{ color: t.text4, fontSize: 12 }}>凭据按 Profile 名称保存在当前设备，跨配置层共享。已保存凭据优先于环境变量。</span>
    </div>
    <div style={{ display: 'grid', gap: 12 }}>
      <strong style={{ fontSize: 13 }}>连接方式</strong>
      <span style={{ color: t.text4, fontSize: 12 }}>
        同一个服务商可以有多种连接方式（国内 / 国外地址，或多把 Key）。每个连接可以单独设置地址、协议和认证，未填写的字段沿用上方设置。
        请求会按顺序使用，前一个失败时自动切换到下一个。
      </span>
      {connections.length === 0
        ? <span style={{ color: t.text3, fontSize: 12 }}>当前只有一种连接方式，使用上方填写的地址与认证。</span>
        : connections.map((connection, index) => <div key={index} style={{ display: 'grid', gap: 10, padding: 12, borderRadius: 8, border: `1px solid ${t.border}` }}>
          <div style={{ display: 'flex', alignItems: 'end', gap: 8 }}>
            <label style={{ ...field, flex: 1 }}>连接 ID {index + 1}<input aria-label={`连接 ID ${index + 1}`} value={typeof connection.id === 'string' ? connection.id : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { id: event.target.value }))} placeholder="例如 cn 或 intl" spellCheck={false} style={input} /></label>
            <button type="button" aria-label={`移除连接 ${index + 1}`} style={ghostButtonStyle(t, false, true)} onClick={() => onDraft(removeProviderConnection(draft, index))}>移除</button>
          </div>
          <label style={field}>服务地址 · Base URL<input aria-label={`连接 ${index + 1} baseUrl`} value={typeof connection.baseUrl === 'string' ? connection.baseUrl : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { baseUrl: event.target.value || undefined }))} placeholder="https://api.example.cn/v1" spellCheck={false} style={input} /></label>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 220px), 1fr))', gap: 12 }}>
            <label style={field}>协议（留空沿用上方）<select aria-label={`连接 ${index + 1} 协议`} value={typeof connection.type === 'string' ? connection.type : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { type: event.target.value || undefined }))} style={input}>
              <option value="">沿用 {draft.type}</option>
              {SUPPORTED_PROVIDER_TYPES.map((type) => <option value={type} key={type}>{type}</option>)}
            </select></label>
            <label style={field}>环境变量名称<input aria-label={`连接 ${index + 1} apiKeyEnv`} value={typeof connection.apiKeyEnv === 'string' ? connection.apiKeyEnv : ''} onChange={(event) => onDraft(updateProviderConnection(draft, index, { apiKeyEnv: event.target.value || undefined }))} placeholder="MY_PROVIDER_CN_API_KEY" spellCheck={false} style={input} /></label>
          </div>
          <span style={{ color: t.text4, fontSize: 11.5 }}>默认使用「{name || 'provider'}」上保存的凭据；需要不同的 Key 时在该连接上填写 credentialIds。</span>
        </div>)}
      <button type="button" style={{ ...ghostButtonStyle(t), justifySelf: 'start' }} onClick={() => onDraft(addProviderConnection(draft))}>＋ 添加连接方式</button>
    </div>
    <div style={{ display: 'grid', gap: 10 }}>
      <strong style={{ fontSize: 13 }}>模型</strong>
      {draft.models.map((model, index) => <div key={index} style={{ display: 'flex', alignItems: 'end', gap: 8 }}>
        <label style={{ ...field, flex: 1 }}>模型 ID {index + 1}<input aria-label={`模型 ID ${index + 1}`} value={typeof model.id === 'string' ? model.id : ''} onChange={(event) => { const next = renameProviderModel(draft, index, event.target.value, pricingIds.current[index]); pricingIds.current[index] = next.pricingId; onDraft(next.draft); }} placeholder="例如 gpt-4.1" style={input} />{Array.isArray(model.aliases) && model.aliases.length ? <span style={{ fontSize: 11 }}>别名：{model.aliases.filter((alias) => typeof alias === 'string').join('、')}</span> : null}</label>
        <button type="button" aria-label={`移除模型 ${index + 1}`} aria-describedby={canRemoveProviderModel(draft, index, pricingIds.current) ? undefined : 'provider-duplicate-model-hint'} disabled={!canRemoveProviderModel(draft, index, pricingIds.current)} style={ghostButtonStyle(t, false, true)} onClick={() => { const next = removeProviderModel(draft, index, pricingIds.current[index], pricingIds.current); pricingIds.current.splice(index, 1); onDraft(next); }}>移除</button>
      </div>)}
      {draft.models.some((_, index) => !canRemoveProviderModel(draft, index, pricingIds.current)) && <span id="provider-duplicate-model-hint" style={{ color: t.warn, fontSize: 12 }}>请先修正重复模型 ID，再移除被引用的模型；也可以直接移除正在编辑的重复项。</span>}
      <button type="button" style={{ ...ghostButtonStyle(t), justifySelf: 'start' }} onClick={() => { pricingIds.current.push(''); onDraft({ ...draft, models: [...draft.models, { id: '' }] }); }}>＋ 添加模型</button>
    </div>
  </div>;
}
