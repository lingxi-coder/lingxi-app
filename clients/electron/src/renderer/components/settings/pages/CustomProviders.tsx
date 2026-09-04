import { useEffect, useState } from 'react';
import { Card, isEditableLayer, MergedBadge, MergedNotice, OverriddenNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { rowState, type SettingsSnapshot } from '../useEngineSettings';
import type { ModelPickerVisibilitySettings, ProviderModelPickerVisibility } from '../../../../shared/settings';
import {
  modelCapabilitySummary,
  providerVisibilityConfig,
  selectedModelIdsForPickerSettings,
} from '../../../bridge/modelCatalog';
import { ghostButtonStyle } from './ghostButton';

/**
 * The nine provider `type` values `settings.providers` accepts. Read from
 * `lingxi-code/llm-client/src/provider_settings.rs`'s
 * `SUPPORTED_PROVIDER_TYPES` constant per this task's instruction — NOT
 * retyped from memory — and kept in the engine's own declaration order.
 * `lingxi-code/` itself is never edited from this side.
 */
// Re-exported for existing import sites (including `settings-providers.test.ts`)
// now that the canonical definition lives in `../rows` (Task 18 fix round 1,
// Minor — every page needing this predicate now shares one copy).
export { isEditableLayer };

export const SUPPORTED_PROVIDER_TYPES = [
  'openai', 'openai-responses', 'anthropic', 'gemini', 'azure-openai',
  'bedrock-claude', 'vertex-claude', 'vertex-gemini', 'foundry-claude',
] as const;

export type SupportedProviderType = (typeof SUPPORTED_PROVIDER_TYPES)[number];

export interface CustomProviderModelDraft {
  id: string;
  aliases?: string[];
}

export interface CustomProviderDraft {
  type: string;
  baseUrl?: string;
  apiKeyEnv?: string;
  models: CustomProviderModelDraft[];
}

/**
 * The write-time gate this task exists for. `settings.providers`' own schema
 * documentation says `models` is required per entry and that an absent or
 * empty list is an error at ENGINE STARTUP, not at save time — so without
 * this check, saving a provider with no models looks like it succeeded and
 * only breaks the next time the engine launches. Returns `null` when the
 * draft may be written.
 */
export function validateCustomProvider(draft: CustomProviderDraft): string | null {
  if (!SUPPORTED_PROVIDER_TYPES.includes(draft.type as SupportedProviderType)) {
    return `unsupported provider type \`${draft.type}\`; supported types are ${SUPPORTED_PROVIDER_TYPES.join(', ')}`;
  }
  if (!draft.models || draft.models.length === 0) {
    return 'this provider needs at least one entry in `models`; an empty list makes the engine fail to start';
  }
  if (draft.models.some((m) => !m.id.trim())) {
    return 'every entry in `models` needs a non-empty `id`';
  }
  return null;
}

/**
 * `settings.providers` as `editingLayer`'s OWN raw value — NOT
 * `snapshot.effective`. This is Task 17 fix round 1: `effective` is the
 * cross-layer MERGED view, and `update_settings` replaces a key WHOLESALE in
 * one layer's file rather than deep-merging
 * (`migrations/src/settings_update.rs`). Basing a write on `effective` would
 * silently fork whichever layer `providers` happened to resolve to into
 * whichever layer gets saved — exactly the bug this fix closes. `layers`
 * (from `ClientEvent::SettingsSnapshot.layers_json`, added for this fix)
 * gives each layer's own unmerged map; a layer that never set the key reads
 * as `{}`, honestly reflecting "this layer owns none of these", not a
 * borrowed view of what some other layer owns.
 */
export function providersFromLayer(snapshot: SettingsSnapshot | null, layer: string): Record<string, CustomProviderDraft> {
  const value = snapshot?.layers?.[layer]?.['providers'];
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, CustomProviderDraft>)
    : {};
}

export interface RoutingRetryDraft {
  maxAttempts?: number;
  backoffMs?: number;
}

export interface RoutingDraft {
  aliases?: Record<string, string>;
  fallback?: Record<string, string[]>;
  retry?: RoutingRetryDraft;
}

/** Same fix as `providersFromLayer` above, for `settings.routing`. */
export function routingFromLayer(snapshot: SettingsSnapshot | null, layer: string): RoutingDraft {
  const value = snapshot?.layers?.[layer]?.['routing'];
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as RoutingDraft) : {};
}

export function withoutProviderModelPickerVisibility(
  settings: ModelPickerVisibilitySettings | undefined,
  providerId: string,
): ModelPickerVisibilitySettings {
  if (!settings?.[providerId]) return settings ?? {};
  const next = { ...settings };
  delete next[providerId];
  return next;
}

export function visibleCustomProviderModelIds(
  candidates: readonly string[],
  visibility: ProviderModelPickerVisibility | undefined,
): string[] {
  return selectedModelIdsForPickerSettings(candidates, visibility);
}

/** `"gpt-x, gpt-y"` / one-per-line → `[{id:'gpt-x'},{id:'gpt-y'}]`, blank entries dropped. Pure so the parsing itself is testable independent of any form state. */
export function parseModelsInput(text: string): CustomProviderModelDraft[] {
  return text
    .split(/[,\n]/)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0)
    .map((id) => ({ id }));
}

const inputStyle = (t: ReturnType<typeof useT>) => ({
  padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`,
  background: t.surface, color: t.text, fontSize: 12.5, fontFamily: 'inherit',
} as const);

/**
 * Edits `settings.providers` and `settings.routing`. Both are new UI — the
 * old settings modal had no equivalent, there is nothing to lift here. Writes go
 * through `bridge.updateEngineSettings(editingLayer, patch)`, this task's
 * addition wrapping the engine's generic `update_settings` wire command
 * (`clients/electron/src/renderer/bridge/useBridge.ts`); no per-key command
 * exists for either settings key, unlike `permissions` or workspace
 * directories.
 *
 * This is a genuinely `layered` page outside the 编码 group — see `nav.ts`'s
 * own comment on why that's not a contradiction. `rowState`/`OverriddenNotice`
 * (Task 13/Task 12) surface whether a write to the CURRENTLY selected layer
 * would actually take effect, the same way any other layered page would;
 * `OverriddenNotice`'s "前往该层" jumps the shell's layer switcher there via
 * `onJumpToLayer` (Task 17 fix round 1 — it used to be wired to a no-op).
 *
 * Task 17 fix round 1 also moved what this page reads/writes from
 * `snapshot.effective` to `snapshot.layers[editingLayer]` — see
 * `providersFromLayer`/`routingFromLayer` above for why `effective` (a
 * cross-layer merge) was the wrong source for a page that writes one layer
 * WHOLESALE.
 */
export function CustomProviders({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const providers = providersFromLayer(snapshot, editingLayer);
  const routing = routingFromLayer(snapshot, editingLayer);

  const [profileName, setProfileName] = useState('');
  const [type, setType] = useState<string>(SUPPORTED_PROVIDER_TYPES[0]);
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKeyEnv, setApiKeyEnv] = useState('');
  const [modelsText, setModelsText] = useState('');
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [aliasName, setAliasName] = useState('');
  const [aliasTarget, setAliasTarget] = useState('');
  const [maxAttempts, setMaxAttempts] = useState(routing.retry?.maxAttempts?.toString() ?? '');
  const [backoffMs, setBackoffMs] = useState(routing.retry?.backoffMs?.toString() ?? '');
  const [routingError, setRoutingError] = useState<string | null>(null);
  const [routingSaving, setRoutingSaving] = useState(false);

  const providersRowState = snapshot ? rowState(snapshot, 'providers', editingLayer) : null;
  const routingRowState = snapshot ? rowState(snapshot, 'routing', editingLayer) : null;

  const resetForm = () => {
    setProfileName('');
    setType(SUPPORTED_PROVIDER_TYPES[0]);
    setBaseUrl('');
    setApiKeyEnv('');
    setModelsText('');
    setFormError(null);
  };

  // Switching the edited layer changes what `providers`/`routing` above
  // resolve to entirely (a different layer's own map). A form still holding
  // a draft loaded from the PREVIOUS layer (via `loadForEdit`, or typed
  // retry values) must not silently land in the newly selected layer when
  // saved — that would be the exact cross-layer-fork bug this fix exists to
  // prevent, just triggered by the layer switcher instead of a stale read.
  useEffect(() => {
    resetForm();
    setAliasName('');
    setAliasTarget('');
    setRoutingError(null);
    setMaxAttempts(routing.retry?.maxAttempts?.toString() ?? '');
    setBackoffMs(routing.retry?.backoffMs?.toString() ?? '');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingLayer]);

  const loadForEdit = (name: string, draft: CustomProviderDraft) => {
    setProfileName(name);
    setType(draft.type);
    setBaseUrl(draft.baseUrl ?? '');
    setApiKeyEnv(draft.apiKeyEnv ?? '');
    setModelsText(draft.models.map((m) => m.id).join(', '));
    setFormError(null);
  };

  const writeProviders = (next: Record<string, CustomProviderDraft>) => {
    setSaving(true);
    setSaveError(null);
    void bridge.updateEngineSettings(editingLayer, { providers: next })
      .then(() => resetForm())
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存 Provider 设置。'))
      .finally(() => setSaving(false));
  };

  const handleSaveProvider = () => {
    const name = profileName.trim();
    if (!name) { setFormError('需要一个 Profile 名称。'); return; }
    const draft: CustomProviderDraft = {
      type,
      baseUrl: baseUrl.trim() || undefined,
      apiKeyEnv: apiKeyEnv.trim() || undefined,
      models: parseModelsInput(modelsText),
    };
    const error = validateCustomProvider(draft);
    if (error) { setFormError(error); return; }
    setFormError(null);
    writeProviders({ ...providers, [name]: draft });
  };

  const handleRemoveProvider = (name: string) => {
    const next = { ...providers };
    delete next[name];
    setSaving(true);
    setSaveError(null);
    void Promise.all([
      bridge.updateEngineSettings(editingLayer, { providers: next }),
      bridge.setModelPickerVisibility(withoutProviderModelPickerVisibility(modelPickerVisibility, name)),
    ])
      .then(() => {
        if (profileName.trim() === name) resetForm();
      })
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存 Provider 设置。'))
      .finally(() => setSaving(false));
  };

  const writeRouting = (next: RoutingDraft) => {
    setRoutingSaving(true);
    setRoutingError(null);
    void bridge.updateEngineSettings(editingLayer, { routing: next })
      .catch((cause) => setRoutingError(cause instanceof Error ? cause.message : '无法保存路由设置。'))
      .finally(() => setRoutingSaving(false));
  };

  const handleAddAlias = () => {
    const alias = aliasName.trim();
    const target = aliasTarget.trim();
    if (!alias || !target) { setRoutingError('别名和目标都不能为空。'); return; }
    setRoutingError(null);
    const nextAliases = { ...(routing.aliases ?? {}), [alias]: target };
    writeRouting({ ...routing, aliases: nextAliases });
    setAliasName('');
    setAliasTarget('');
  };

  const handleRemoveAlias = (alias: string) => {
    const nextAliases = { ...(routing.aliases ?? {}) };
    delete nextAliases[alias];
    writeRouting({ ...routing, aliases: nextAliases });
  };

  const handleSaveRetry = () => {
    const parsedMaxAttempts = maxAttempts.trim() ? Number(maxAttempts) : undefined;
    const parsedBackoffMs = backoffMs.trim() ? Number(backoffMs) : undefined;
    if (maxAttempts.trim() && (!Number.isFinite(parsedMaxAttempts) || (parsedMaxAttempts as number) <= 0)) {
      setRoutingError('retry.maxAttempts 必须是正整数。');
      return;
    }
    if (backoffMs.trim() && (!Number.isFinite(parsedBackoffMs) || (parsedBackoffMs as number) < 0)) {
      setRoutingError('retry.backoffMs 必须是非负整数。');
      return;
    }
    setRoutingError(null);
    writeRouting({
      ...routing,
      retry: (parsedMaxAttempts === undefined && parsedBackoffMs === undefined)
        ? undefined
        : { maxAttempts: parsedMaxAttempts, backoffMs: parsedBackoffMs },
    });
  };

  const providerNames = Object.keys(providers).sort();
  const aliasNames = Object.keys(routing.aliases ?? {}).sort();
  const modelPickerVisibility = bridge.bootstrap?.settings.modelPickerVisibility;
  const providerModelCatalog = bridge.desktop.providerModelCatalog ?? [];
  const catalogByProviderId = new Map(
    providerModelCatalog.map((provider) => [provider.provider_id, provider] as const),
  );
  const draftProviderId = profileName.trim();
  const draftModelIds = parseModelsInput(modelsText).map((model) => model.id);
  const updateVisibility = (providerId: string, next: ProviderModelPickerVisibility) => {
    void bridge.setModelPickerVisibility({
      ...(modelPickerVisibility ?? {}),
      [providerId]: next,
    });
  };

  return (
    <>
      <Card title="自定义 Provider">
        {providersRowState?.kind === 'merged' && (
          <Row title="生效层" badge={<MergedBadge />} align="center">
            <MergedNotice editingLayer={editingLayer} />
          </Row>
        )}
        {providersRowState?.kind === 'overridden' && (
          <Row title="生效层" align="center">
            <OverriddenNotice
              editingLayer={editingLayer}
              effectiveLayer={providersRowState.by}
              onJump={() => { if (isEditableLayer(providersRowState.by)) onJumpToLayer(providersRowState.by); }}
            />
          </Row>
        )}
        {providerNames.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有自定义 Provider。</div>
        )}
        {providerNames.map((name) => {
          const draft = providers[name];
          return (
            <Row
              key={name}
              align="center"
              title={<span className="mono" style={{ fontSize: 13 }}>{name}</span>}
              desc={`${draft.type} · ${draft.models?.length ?? 0} 个模型${draft.baseUrl ? ` · ${draft.baseUrl}` : ''}`}
            >
              <div style={{ display: 'flex', gap: 7 }}>
                <button type="button" onClick={() => loadForEdit(name, draft)} style={ghostButtonStyle(t)}>编辑</button>
                <button type="button" onClick={() => handleRemoveProvider(name)} style={ghostButtonStyle(t, false, true)}>移除</button>
              </div>
            </Row>
          );
        })}

        <Row title={profileName && providers[profileName] ? `编辑 ${profileName}` : '新增 Provider'} desc="Profile 名称、类型、baseUrl、apiKeyEnv 与至少一个模型 id。" align="start">
          <div style={{ display: 'grid', gap: 7 }}>
            <input value={profileName} onChange={(e) => setProfileName(e.target.value)} placeholder="Profile 名称" aria-label="Profile 名称" style={inputStyle(t)} />
            <select value={type} onChange={(e) => setType(e.target.value)} aria-label="Provider 类型" style={inputStyle(t)}>
              {SUPPORTED_PROVIDER_TYPES.map((value) => <option key={value} value={value}>{value}</option>)}
            </select>
            <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="baseUrl（可选）" aria-label="baseUrl" style={inputStyle(t)} />
            <input value={apiKeyEnv} onChange={(e) => setApiKeyEnv(e.target.value)} placeholder="apiKeyEnv（可选）" aria-label="apiKeyEnv" style={inputStyle(t)} />
            <input value={modelsText} onChange={(e) => setModelsText(e.target.value)} placeholder="模型 id，用逗号分隔" aria-label="模型列表" style={inputStyle(t)} />
            {formError && <span role="alert" style={{ color: t.danger, fontSize: 12 }}>{formError}</span>}
            <div style={{ display: 'flex', gap: 7 }}>
              <button type="button" disabled={saving} onClick={handleSaveProvider} style={ghostButtonStyle(t, saving)}>{saving ? '保存中…' : '保存'}</button>
              <button type="button" disabled={saving} onClick={resetForm} style={ghostButtonStyle(t, saving)}>清空</button>
            </div>
            {saveError && <span role="alert" style={{ color: t.danger, fontSize: 12 }}>{saveError}</span>}
          </div>
        </Row>
        {draftProviderId && (
          <Row
            title="对话模型列表"
            desc="仅影响 Desktop 对话模型选择器的显示，不修改引擎 provider 配置。"
            align="start"
          >
            <div style={{ display: 'grid', gap: 7, minWidth: 260 }}>
              {(() => {
                const catalog = catalogByProviderId.get(draftProviderId);
                const candidates = [...new Set([
                  ...(catalog?.models.map((model) => model.model_id) ?? []),
                  ...draftModelIds,
                ])];
                const visibility = providerVisibilityConfig(modelPickerVisibility, draftProviderId);
                const visible = visibleCustomProviderModelIds(candidates, visibility);
                const detailsByModelId = new Map(
                  (catalog?.models ?? []).map((model) => [model.model_id, model] as const),
                );
                return (
                  <>
                    <button
                      type="button"
                      onClick={() => updateVisibility(draftProviderId, {
                        showInModelPicker: visibility?.showInModelPicker === false,
                        visibleModelIds: visibility?.visibleModelIds,
                      })}
                      style={ghostButtonStyle(t)}
                    >
                      {visibility?.showInModelPicker === false ? '当前已隐藏 Provider' : '当前显示 Provider'}
                    </button>
                    {candidates.length === 0
                      ? <span style={{ color: t.text4, fontSize: 12.5 }}>先填写至少一个模型 id，目录到达后这里会显示可见性选择。</span>
                      : candidates.map((modelId) => {
                        const selected = visible.includes(modelId);
                        const detail = detailsByModelId.get(modelId);
                        return (
                          <button
                            key={modelId}
                            type="button"
                            onClick={() => {
                              const base = visibility?.visibleModelIds ?? candidates;
                              const next = base.includes(modelId)
                                ? base.filter((entry) => entry !== modelId)
                                : [...base, modelId];
                              updateVisibility(draftProviderId, {
                                showInModelPicker: visibility?.showInModelPicker,
                                visibleModelIds: [...new Set(next)],
                              });
                            }}
                            style={{ ...ghostButtonStyle(t), justifyContent: 'space-between' }}
                          >
                            <span style={{ display: 'grid', gap: 2, textAlign: 'left' }}>
                              <span>{detail?.display_name || modelId}</span>
                              <span className="mono" style={{ color: t.text4, fontSize: 11 }}>
                                {[modelId, detail ? modelCapabilitySummary(detail) : ''].filter(Boolean).join(' · ')}
                              </span>
                            </span>
                            <span>{selected ? '显示' : '隐藏'}</span>
                          </button>
                        );
                      })}
                    {visibility?.showInModelPicker !== false && visible.length === 0 && (
                      <span style={{ color: t.warn, fontSize: 12.5 }}>
                        当前没有可显示模型，这个 Provider 会从对话模型列表隐藏。
                      </span>
                    )}
                  </>
                );
              })()}
            </div>
          </Row>
        )}
      </Card>

      <Card title="路由 (routing)">
        {routingRowState?.kind === 'merged' && (
          <Row title="生效层" badge={<MergedBadge />} align="center">
            <MergedNotice editingLayer={editingLayer} />
          </Row>
        )}
        {routingRowState?.kind === 'overridden' && (
          <Row title="生效层" align="center">
            <OverriddenNotice
              editingLayer={editingLayer}
              effectiveLayer={routingRowState.by}
              onJump={() => { if (isEditableLayer(routingRowState.by)) onJumpToLayer(routingRowState.by); }}
            />
          </Row>
        )}
        <Row title="别名 (aliases)" desc="alias → profile/model" align="start">
          <div style={{ display: 'grid', gap: 7 }}>
            {aliasNames.length === 0 && <span style={{ color: t.text4, fontSize: 12.5 }}>还没有别名。</span>}
            {aliasNames.map((alias) => (
              <div key={alias} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span className="mono" style={{ fontSize: 12 }}>{alias} → {routing.aliases?.[alias]}</span>
                <button type="button" onClick={() => handleRemoveAlias(alias)} style={ghostButtonStyle(t, false, true)}>移除</button>
              </div>
            ))}
            <div style={{ display: 'flex', gap: 7 }}>
              <input value={aliasName} onChange={(e) => setAliasName(e.target.value)} placeholder="alias" aria-label="alias 名称" style={inputStyle(t)} />
              <input value={aliasTarget} onChange={(e) => setAliasTarget(e.target.value)} placeholder="profile/model" aria-label="alias 目标" style={inputStyle(t)} />
              <button type="button" disabled={routingSaving} onClick={handleAddAlias} style={ghostButtonStyle(t, routingSaving)}>添加</button>
            </div>
          </div>
        </Row>
        <Row title="重试 (retry)" desc="retry.maxAttempts / retry.backoffMs" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input value={maxAttempts} onChange={(e) => setMaxAttempts(e.target.value)} placeholder="maxAttempts" aria-label="maxAttempts" style={{ ...inputStyle(t), width: 110 }} />
            <input value={backoffMs} onChange={(e) => setBackoffMs(e.target.value)} placeholder="backoffMs" aria-label="backoffMs" style={{ ...inputStyle(t), width: 110 }} />
            <button type="button" disabled={routingSaving} onClick={handleSaveRetry} style={ghostButtonStyle(t, routingSaving)}>{routingSaving ? '保存中…' : '保存'}</button>
          </div>
        </Row>
        {routingError && (
          <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{routingError}</span></Row>
        )}
      </Card>
    </>
  );
}
