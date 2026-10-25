import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import { useSettingsDraftGuard } from '../useSettingsDraftGuard';
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
import { ProviderRegion } from './ProviderRegion';

/**
 * The nine provider `type` values `settings.providers` accepts. Read from
 * `llm-client/src/provider_settings.rs`'s
 * `SUPPORTED_PROVIDER_TYPES` constant per this task's instruction — NOT
 * retyped from memory — and kept in the engine's own declaration order.
 * the Rust workspace itself is never edited from this side.
 */
// Re-exported for existing import sites (including `settings-providers.test.ts`)
// now that the canonical definition lives in `../rows` (Task 18 fix round 1,
// Minor — every page needing this predicate now shares one copy).
export { isEditableLayer };

export { SUPPORTED_PROVIDER_TYPES, validateCustomProvider } from './customProviderImport';
export type { CustomProviderDraft, CustomProviderModelDraft, SupportedProviderType } from './customProviderImport';
import { validateCustomProvider, validateProfileName, parseProviderImport, validateImportEntry, mergeProviderImport, type CustomProviderDraft, type CustomProviderModelDraft, type ProviderImportEntry, connectionsCarryAuth } from './customProviderImport';
import { trimProviderDraft, editableProviderDraft } from './customProviderDraft';
import { ProviderEditorFields } from './ProviderEditorFields';
import { equalProviderSettings } from '../../../bridge/providerSettingsSave';

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
 * through `bridge.updateProviderSettings(editingLayer, patch)`, this task's
 * addition wrapping the engine's generic `update_settings` wire command
 * (`apps/electron/src/renderer/bridge/useBridge.ts`); no per-key command
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
export function CustomProviders({ bridge, snapshot, editingLayer, onJumpToLayer, onLayerLockChange, onNavigate, onClose, onRegisterNavigationRequest, onRegisterCloseRequest, linkRequest }: PageContentProps) {
  useI18n();
  const t = useT();
  const primaryButtonStyle = { ...ghostButtonStyle(t), background: `color-mix(in srgb, ${t.accent} 85%, #000)`, borderColor: t.accent, color: '#fff', fontWeight: 600 };
  const providers = providersFromLayer(snapshot, editingLayer);
  const routing = routingFromLayer(snapshot, editingLayer);

  const credentialConfigured = (name: string) => bridge.bootstrap?.providerCredentials?.some((entry) => entry.providerId === name && entry.configured) === true;
  const importEntryError = (entry: ProviderImportEntry) => validateImportEntry(entry, { credentialConfigured: credentialConfigured(entry.name) });
  const configuredNames = Object.keys(providers).sort().join('\n');
  useEffect(() => {
    for (const name of configuredNames.split('\n').filter((name) => name && validateProfileName(name) === null)) void bridge.refreshProviderCredential(name).catch(() => {});
  }, [configuredNames, bridge.refreshProviderCredential]);

  const [profileName, setProfileName] = useState('');
  const [originalName, setOriginalName] = useState<string | null>(null);
  const [draft, setDraft] = useState<CustomProviderDraft>({ type: 'openai', models: [{ id: '' }] });
  const [apiKey, setApiKey] = useState('');
  const [editorOpen, setEditorOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState('');
  const [entries, setEntries] = useState<ProviderImportEntry[] | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [importWarning, setImportWarning] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [pendingCredentials, setPendingCredentials] = useState<Record<string, string>>({});
  const generation = useRef(0);
  const fileRef = useRef<HTMLInputElement>(null);

  const [aliasName, setAliasName] = useState('');
  const [aliasTarget, setAliasTarget] = useState('');
  const [maxAttempts, setMaxAttempts] = useState(routing.retry?.maxAttempts?.toString() ?? '');
  const [backoffMs, setBackoffMs] = useState(routing.retry?.backoffMs?.toString() ?? '');
  const [routingError, setRoutingError] = useState<string | null>(null);
  const [routingSaving, setRoutingSaving] = useState(false);
  const busy = saving || routingSaving;

  const providersRowState = snapshot ? rowState(snapshot, 'providers', editingLayer) : null;
  const routingRowState = snapshot ? rowState(snapshot, 'routing', editingLayer) : null;

  const resetForm = () => {
    setProfileName(''); setOriginalName(null); setDraft({ type: 'openai', models: [{ id: '' }] });
    setApiKey(''); setFormError(null); setEditorOpen(false);
  };
  useEffect(() => {
    generation.current += 1;
    resetForm(); setImportOpen(false); setImportText(''); setEntries(null); setPendingCredentials({});
    setSaveError(null); setImportError(null); setAliasName(''); setAliasTarget(''); setRoutingError(null);
    setMaxAttempts(routing.retry?.maxAttempts?.toString() ?? '');
    setBackoffMs(routing.retry?.backoffMs?.toString() ?? '');
    return () => { generation.current += 1; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingLayer, bridge.activeSession?.sessionId]);
  const appliedLink = useRef<string>();
  useEffect(() => {
    if (linkRequest?.kind !== 'provider' || !linkRequest.payload || appliedLink.current === linkRequest.requestId) return;
    appliedLink.current = linkRequest.requestId;
    const text = JSON.stringify(linkRequest.payload, null, 2);
    setImportOpen(true); setImportText(text);
    const result = parseProviderImport(text, providers);
    setEntries(result.diagnostics.some(entry => entry.severity === 'error') ? null : result.entries);
    setImportError(result.diagnostics.filter(entry => entry.severity === 'error').map(entry => entry.message).join('\n') || null);
    setImportWarning(result.diagnostics.filter(entry => entry.severity === 'warning').map(entry => entry.message).join('\n') || null);
  }, [linkRequest]);
  const closePrompt = useSettingsDraftGuard({ onClose, onNavigate, onRegisterCloseRequest, onRegisterNavigationRequest }, Boolean(importOpen && importText.trim()) || editorOpen, true);
  const loadForEdit = (name: string, value: CustomProviderDraft) => {
    setProfileName(name); setOriginalName(name); setDraft(editableProviderDraft(value)); setApiKey('');
    setFormError(null); setEditorOpen(true); setImportOpen(false);
  };
  const persist = async (next: Record<string, CustomProviderDraft> | null, credentials: Record<string, string> = {}, removedProviderId?: string) => {
    if (busy) return false;
    const token = generation.current;
    setSaving(true); onLayerLockChange?.(true); setSaveError(null);
    const configurationChanged = next !== null && !equalProviderSettings(providers, next);
    let configSaved = !configurationChanged;
    const remaining = { ...credentials };
    try {
      if (configurationChanged) await bridge.updateProviderSettings(editingLayer, { providers: next });
      if (generation.current !== token) return false;
      configSaved = true;
      // Continue only in the page/session that initiated this save.
      for (const [name, key] of Object.entries(credentials)) {
        if (generation.current !== token) return false;
        await bridge.setProviderCredential(name, key);
        delete remaining[name];
      }
      if (generation.current !== token) return false;
      if (removedProviderId) await bridge.setModelPickerVisibility(withoutProviderModelPickerVisibility(modelPickerVisibility, removedProviderId));
      if (generation.current === token) {
        resetForm(); setImportOpen(false); setImportText(''); setEntries(null); setPendingCredentials({});
      }
      return true;
    } catch {
      if (generation.current === token) {
        setPendingCredentials(configSaved ? remaining : {});
        setSaveError(configSaved ? removedProviderId ? tr('desktop_settings_a131e886ff') : tr('desktop_settings_df8a4e50ad') : tr('desktop_settings_dce9e3b7a8'));
      }
      return false;
    } finally {
      if (generation.current === token) setSaving(false);
      onLayerLockChange?.(false);
    }
  };
  const handleSaveProvider = () => {
    const name = profileName.trim();
    const normalizedDraft = trimProviderDraft(draft);
    const error = (!originalName ? validateProfileName(name) : null) || validateCustomProvider(normalizedDraft);
    if (error) { setFormError(error); return; }
    if (draft.type !== 'bedrock-claude' && !apiKey.trim() && !draft.apiKeyEnv?.trim() && !connectionsCarryAuth(draft) && !bridge.bootstrap?.providerCredentials?.some((entry) => entry.providerId === name && entry.configured)) { setFormError(tr('desktop_settings_fc019e5232')); return; }
    if (!originalName && providers[name]) { setFormError(tr('desktop_settings_db858f13d7')); return; }
    setFormError(null);
    void persist({ ...providers, [name]: normalizedDraft }, apiKey.trim() ? { [name]: apiKey.trim() } : {});
  };
  const handleRemoveProvider = (name: string) => {
    const next = { ...providers }; delete next[name];
    return persist(next, {}, name);
  };
  const parseImport = () => {
    setImportWarning(null);
    try {
      const result = parseProviderImport(importText, providers);
      setEntries(result.diagnostics.some((entry) => entry.severity === 'error') ? null : result.entries);
      setImportError(result.diagnostics.filter((entry) => entry.severity === 'error').map((entry) => entry.message).join('\n') || null);
      setImportWarning(result.diagnostics.filter((entry) => entry.severity === 'warning').map((entry) => entry.message).join('\n') || null);
    } catch { setImportError(tr('settings_parity_import_invalid')); setEntries(null); }
  };
  const writeRouting = (next: RoutingDraft) => {
    if (busy) return;
    const token = generation.current;
    setRoutingSaving(true); onLayerLockChange?.(true); setRoutingError(null);
    void bridge.updateProviderSettings(editingLayer, { routing: next })
      .catch(() => { if (token === generation.current) setRoutingError(tr('desktop_settings_2174340bd9')); })
      .finally(() => { if (token === generation.current) setRoutingSaving(false); onLayerLockChange?.(false); });
  };

  const handleAddAlias = () => {
    const alias = aliasName.trim();
    const target = aliasTarget.trim();
    if (!alias || !target) { setRoutingError(tr('desktop_settings_de2ae34802')); return; }
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
    if (maxAttempts.trim() && (!Number.isSafeInteger(parsedMaxAttempts) || (parsedMaxAttempts as number) <= 0)) {
      setRoutingError(tr('desktop_settings_a64f420970'));
      return;
    }
    if (backoffMs.trim() && (!Number.isSafeInteger(parsedBackoffMs) || (parsedBackoffMs as number) < 0)) {
      setRoutingError(tr('desktop_settings_7f1ee746ed'));
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
  const draftModelIds = draft.models.map((model) => model.id).filter(Boolean);
  const updateVisibility = (providerId: string, next: ProviderModelPickerVisibility) => {
    if (busy) return;
    void bridge.setModelPickerVisibility({
      ...(modelPickerVisibility ?? {}),
      [providerId]: next,
    });
  };

  return (
    <>
      {closePrompt}
      {!editorOpen && !importOpen && <ProviderRegion bridge={bridge} snapshot={snapshot} editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} onLayerLockChange={onLayerLockChange} />}
      {(editorOpen || importOpen) && <button disabled={busy} style={{ ...ghostButtonStyle(t), marginBottom: 16 }} onClick={() => { resetForm(); setImportOpen(false); setImportText(''); setEntries(null); setImportError(null); }}>{tr('desktop_settings_27e5805bb9')}</button>}
      {!editorOpen && !importOpen && <Card title={tr('desktop_settings_d2b6b0cd42')}>
        {providersRowState?.kind === 'merged' && (
          <Row title={tr('desktop_settings_ad25986d5b')} badge={<MergedBadge />} align="center">
            <MergedNotice editingLayer={editingLayer} />
          </Row>
        )}
        {providersRowState?.kind === 'overridden' && (
          <Row title={tr('desktop_settings_ad25986d5b')} align="center">
            <OverriddenNotice
              editingLayer={editingLayer}
              effectiveLayer={providersRowState.by}
              onJump={() => { if (isEditableLayer(providersRowState.by)) onJumpToLayer(providersRowState.by); }}
            />
          </Row>
        )}
        <div style={{ padding: 18, display: 'flex', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap', alignItems: 'center' }}>
          <span style={{ color: t.text3, fontSize: 12.5 }}>{tr('desktop_settings_custom_providers_count', providerNames.length)}</span>
          <div style={{ display: 'flex', gap: 8 }}>
            <button disabled={busy} style={ghostButtonStyle(t)} onClick={() => { resetForm(); setEditorOpen(true); setImportOpen(false); }}>{tr('desktop_settings_39e0768a3f')}</button>
            <button disabled={busy} style={ghostButtonStyle(t)} onClick={() => { resetForm(); setImportOpen(true); }}>{tr('desktop_settings_8f6e8682a3')}</button>
          </div>
        </div>
        {providerNames.length === 0 && <div style={{ padding: '24px 18px 32px', color: t.text3, textAlign: 'center' }}>{tr('desktop_settings_cb6216aa42')}</div>}
        {providerNames.map((name) => {
          const value = providers[name];
          const validValue = value && typeof value === 'object' && !Array.isArray(value);
          const credential = bridge.bootstrap?.providerCredentials?.find((entry) => entry.providerId === name);
          return <div key={name} style={{ display: 'flex', flexWrap: 'wrap', padding: 18, gap: 14, alignItems: 'center', borderTop: `1px solid ${t.border}` }}>
            <div style={{ flex: '1 1 200px', minWidth: 0, display: 'grid', gap: 6 }}>
              <strong className="mono" style={{ fontSize: 13, overflowWrap: 'anywhere' }}>{name}</strong>
              <span style={{ color: t.text3, fontSize: 12, overflowWrap: 'anywhere' }}>{validValue ? tr('desktop_settings_e33ef5df43', value.type, value.models?.length ?? 0, value.baseUrl ? ` · ${value.baseUrl}` : '') : tr('desktop_settings_58cbac7217')}</span>
              <span style={{ color: t.text3, fontSize: 12, overflowWrap: 'anywhere' }}>{credential?.storageError ? tr('settings_provider_secure_storage_unavailable') : credential?.runtimeOnly ? tr('desktop_settings_77a360870e') : credential?.configured ? (credential.credentialPreview || tr('desktop_settings_faf39dbce4')) : validValue && value.apiKeyEnv ? tr('desktop_settings_39fbedcedd', value.apiKeyEnv) : tr('desktop_settings_bbcaaa9e88')}</span>
            </div>
            <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
              <button disabled={busy} onClick={() => loadForEdit(name, value)} style={ghostButtonStyle(t)}>{tr('settings_title_edit')}</button>
              <button disabled={busy} onClick={() => void handleRemoveProvider(name)} style={ghostButtonStyle(t, busy, true)}>{tr('settings_parity_remove')}</button>
            </div>
          </div>;
        })}
      </Card>}
        {saveError && <div role="alert" style={{ padding: 18, color: t.danger }}>{saveError}
          {Object.keys(pendingCredentials).length > 0 && <button disabled={busy} onClick={() => void persist(null, pendingCredentials)} style={ghostButtonStyle(t)}>{tr('desktop_settings_65f36ab91c')}</button>}
        </div>}
      {editorOpen && <Card title={originalName ? tr('desktop_settings_39e42e9346', originalName) : tr('desktop_settings_7add78b882')}>
        <fieldset disabled={busy} style={{ border: 0, padding: 20, margin: 0, minWidth: 0 }}>
          <ProviderEditorFields hostManagedCredentials={bridge.bootstrap?.credentialBrokerAvailable !== undefined} envOnly={originalName !== null && validateProfileName(originalName) !== null} key={originalName ?? "new"} name={profileName} onName={setProfileName} nameLocked={originalName !== null} draft={draft} onDraft={setDraft} apiKey={apiKey} onApiKey={setApiKey} credentialConfigured={bridge.bootstrap?.providerCredentials?.some((entry) => entry.providerId === profileName && entry.configured)} />
          {formError && <p role="alert" style={{ color: t.danger }}>{formError}</p>}
          <div style={{ display: 'flex', gap: 8, marginTop: 20, flexWrap: 'wrap' }}>
            <button onClick={handleSaveProvider} style={primaryButtonStyle}>{saving ? tr('desktop_settings_ff509c9ba0') : tr('desktop_settings_e6cf84ee3a')}</button>
            <button onClick={resetForm} style={ghostButtonStyle(t)}>{tr('common_cancel')}</button>
            {originalName && validateProfileName(originalName) === null && bridge.bootstrap?.providerCredentials?.some((entry) => entry.providerId === originalName && entry.configured) && <button onClick={() => {
              const token = generation.current;
              setSaving(true); onLayerLockChange?.(true);
              void bridge.clearProviderCredential(originalName).catch(() => { if (token === generation.current) setSaveError(tr('desktop_settings_80c677bc07')); }).finally(() => { if (token === generation.current) setSaving(false); onLayerLockChange?.(false); });
            }} style={ghostButtonStyle(t, false, true)}>{tr('settings_provider_clear_credential')}</button>}
          </div>
        </fieldset>
        {draftProviderId && (
          <Row
            title={tr('desktop_settings_a148f7060d')}
            desc={tr('desktop_settings_2fd722a104')}
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
                      disabled={busy}
                      style={ghostButtonStyle(t)}
                    >
                      {visibility?.showInModelPicker === false ? tr('desktop_settings_ed4cd1fe2f') : tr('desktop_settings_71de9d8dbd')}
                    </button>
                    {candidates.length === 0
                      ? <span style={{ color: t.text4, fontSize: 12.5 }}>{tr('desktop_settings_bbe70ea126')}</span>
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
                            disabled={busy}
                            style={{ ...ghostButtonStyle(t), justifyContent: 'space-between' }}
                          >
                            <span style={{ display: 'grid', gap: 2, textAlign: 'left' }}>
                              <span>{detail?.display_name || modelId}</span>
                              <span className="mono" style={{ color: t.text4, fontSize: 11 }}>
                                {[modelId, detail ? modelCapabilitySummary(detail) : ''].filter(Boolean).join(' · ')}
                              </span>
                            </span>
                            <span>{selected ? tr('settings_provider_show_key') : tr('settings_provider_hide_key')}</span>
                          </button>
                        );
                      })}
                    {visibility?.showInModelPicker !== false && visible.length === 0 && (
                      <span style={{ color: t.warn, fontSize: 12.5 }}>
                        {tr('settings_provider_picker_hidden_empty')}
                      </span>
                    )}
                  </>
                );
              })()}
            </div>
          </Row>
        )}
      </Card>}

      {importOpen && <Card title={tr('desktop_settings_8f6e8682a3')}>
        <fieldset disabled={busy} style={{ border: 0, padding: 20, margin: 0, minWidth: 0 }}>
          <p style={{ color: t.text3, marginTop: 0 }}>{tr('desktop_settings_19476e81da')}</p>
          {!entries ? <>
            <label style={{ display: 'grid', gap: 8 }}>{tr('desktop_settings_b567dbfe1b')}<textarea aria-label={tr('desktop_settings_b567dbfe1b')} value={importText} onChange={(event) => setImportText(event.target.value)} spellCheck={false} rows={10} style={{ ...inputStyle(t), width: '100%', boxSizing: 'border-box', fontFamily: 'monospace', resize: 'vertical' }} /></label>
            <input ref={fileRef} hidden type="file" accept=".json,application/json" onChange={(event) => {
              const file = event.target.files?.[0]; const token = generation.current;
              if (file) void file.text().then((text) => { if (token === generation.current) { setImportText(text); setImportError(null); } }).catch(() => { if (token === generation.current) setImportError(tr('desktop_settings_d892392166')); });
              event.target.value = '';
            }} />
            <div style={{ display: 'flex', gap: 8, marginTop: 12, flexWrap: 'wrap' }}><button style={ghostButtonStyle(t)} onClick={() => { setImportText(JSON.stringify({ providers: { 'my-provider': { type: 'openai', baseUrl: 'https://api.example.com/v1', apiKeyEnv: 'MY_PROVIDER_API_KEY', models: [{ id: 'my-model' }] } } }, null, 2)); setImportError(null); }}>{tr('desktop_settings_9c8caa7e26')}</button><button style={ghostButtonStyle(t)} onClick={() => fileRef.current?.click()}>{tr('desktop_settings_6886fee215')}</button><button disabled={!importText.trim()} style={primaryButtonStyle} onClick={parseImport}>{tr('desktop_settings_6f9dc2f701')}</button><button style={ghostButtonStyle(t)} onClick={() => { setImportOpen(false); setImportText(''); }}>{tr('common_cancel')}</button></div>
          </> : <>
            {entries.map((entry, index) => <section key={index} style={{ padding: '16px 0', borderBottom: `1px solid ${t.border}` }}>
              <label style={{ display: 'flex', gap: 8, marginBottom: 16, fontWeight: 600 }}><input type="checkbox" checked={entry.selected} onChange={(event) => setEntries(entries.map((value, i) => i === index ? { ...value, selected: event.target.checked } : value))} />{entry.name} · {entry.conflict ? tr('desktop_settings_a8ff0154b4') : tr('desktop_settings_0006d696d8')}</label>
              <div hidden={!entry.selected}><ProviderEditorFields hostManagedCredentials={bridge.bootstrap?.credentialBrokerAvailable !== undefined} credentialConfigured={credentialConfigured(entry.name)} name={entry.name} onName={(name) => setEntries(entries.map((value, i) => i === index ? { ...value, name, conflict: Object.prototype.hasOwnProperty.call(providers, name), selected: Object.prototype.hasOwnProperty.call(providers, name) ? false : value.selected } : value))} draft={entry.draft} onDraft={(value) => setEntries(entries.map((item, i) => i === index ? { ...item, draft: value } : item))} apiKey={entry.apiKey ?? ''} onApiKey={(value) => setEntries(entries.map((item, i) => i === index ? { ...item, apiKey: value } : item))} /></div>
              {entry.diagnostics.map((diagnostic, i) => <p key={i} style={{ color: diagnostic.severity === 'error' ? t.danger : t.warn }}>{diagnostic.message}</p>)}
              {entry.selected && importEntryError(entry) && <p role="alert" style={{ color: t.danger }}>{importEntryError(entry)}</p>}
            </section>)}
            <div style={{ display: 'flex', gap: 8, marginTop: 16 }}><button style={ghostButtonStyle(t)} onClick={() => setEntries(null)}>{tr('desktop_settings_1632335e35')}</button><button disabled={!entries.some((entry) => entry.selected) || entries.some((entry) => entry.selected && importEntryError(entry) !== null)} style={primaryButtonStyle} onClick={() => {
              try { const result = mergeProviderImport(providers, entries, { credentialConfigured }); void persist(result.providers, result.credentials); } catch { setImportError(tr('desktop_settings_5380a1b745')); }
            }}>{saving ? tr('desktop_settings_2f58b6eddc') : tr('desktop_settings_2cd60a1b04', entries.filter((entry) => entry.selected).length)}</button></div>
          </>}
          {entries && importWarning && <p style={{ color: t.warn, whiteSpace: 'pre-wrap' }}>{importWarning}</p>}
          {importError && <p role="alert" style={{ color: t.danger, whiteSpace: 'pre-wrap' }}>{importError}</p>}
        </fieldset>
      </Card>}
      <details style={{ marginTop: 24 }}><summary style={{ cursor: 'pointer', color: t.text3, fontWeight: 600, marginBottom: 16 }}>{tr('desktop_settings_0a4c35c853')}</summary>
      <Card title={tr('desktop_settings_c789f347d6')}>
        {routingRowState?.kind === 'merged' && (
          <Row title={tr('desktop_settings_ad25986d5b')} badge={<MergedBadge />} align="center">
            <MergedNotice editingLayer={editingLayer} />
          </Row>
        )}
        {routingRowState?.kind === 'overridden' && (
          <Row title={tr('desktop_settings_ad25986d5b')} align="center">
            <OverriddenNotice
              editingLayer={editingLayer}
              effectiveLayer={routingRowState.by}
              onJump={() => { if (isEditableLayer(routingRowState.by)) onJumpToLayer(routingRowState.by); }}
            />
          </Row>
        )}
        <Row title={tr('desktop_settings_4efe005895')} desc="alias → profile/model" align="start">
          <div style={{ display: 'grid', gap: 7 }}>
            {aliasNames.length === 0 && <span style={{ color: t.text4, fontSize: 12.5 }}>{tr('desktop_settings_5b2c0b3776')}</span>}
            {aliasNames.map((alias) => (
              <div key={alias} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span className="mono" style={{ fontSize: 12 }}>{alias} → {routing.aliases?.[alias]}</span>
                <button type="button" disabled={busy} onClick={() => handleRemoveAlias(alias)} style={ghostButtonStyle(t, routingSaving, true)}>{tr('settings_parity_remove')}</button>
              </div>
            ))}
            <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}>
              <input value={aliasName} onChange={(e) => setAliasName(e.target.value)} placeholder="alias" aria-label={tr('desktop_settings_bea7817bab')} style={inputStyle(t)} />
              <input value={aliasTarget} onChange={(e) => setAliasTarget(e.target.value)} placeholder="profile/model" aria-label={tr('desktop_settings_ad9f1d22f6')} style={inputStyle(t)} />
              <button type="button" disabled={busy} onClick={handleAddAlias} style={ghostButtonStyle(t, routingSaving)}>{tr('workspace_confirm_add')}</button>
            </div>
          </div>
        </Row>
        <Row title={tr('desktop_settings_da4adfe674')} desc="retry.maxAttempts / retry.backoffMs" align="center">
          <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}>
            <input value={maxAttempts} onChange={(e) => setMaxAttempts(e.target.value)} placeholder="maxAttempts" aria-label="maxAttempts" style={{ ...inputStyle(t), width: 110 }} />
            <input value={backoffMs} onChange={(e) => setBackoffMs(e.target.value)} placeholder="backoffMs" aria-label="backoffMs" style={{ ...inputStyle(t), width: 110 }} />
            <button type="button" disabled={busy} onClick={handleSaveRetry} style={ghostButtonStyle(t, routingSaving)}>{routingSaving ? tr('desktop_settings_ff509c9ba0') : tr('settings_save')}</button>
          </div>
        </Row>
        {routingError && (
          <Row title={tr('cron_error_section')} align="center"><span role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{routingError}</span></Row>
        )}
      </Card>
      </details>
    </>
  );
}
