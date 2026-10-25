import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useCallback, useEffect, useId, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { Icon } from '../../Icon';
import type { ModelDetailsDto } from '@lingxi/bridge-client';
import { Card } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import { ExtensionHubLayout, useExtensionHubNavigation } from './ExtensionHub';
import { ExtensionGlyph } from './ExtensionHubIcons';
import { adminRecordCommand, nextConfigurationOperationId, noteStyle } from './configurationAdmin';
import {
  FUSION_SCOPES, copiedFusionStrategy, fusionNameError, fusionStrategyKey, parseFusionCatalog,
  type FusionCatalogScope, type FusionGlobals, type FusionScope, type FusionStrategyEntry,
} from './FusionCatalog';


function fusionScopeLabel(scope: FusionScope): string {
  return tr({ user: 'settings_parity_user_layer', project: 'settings_parity_project_layer', local: 'settings_parity_local_layer' }[scope]);
}

/** 一个 `(profile, model)` 对，与引擎 `fusion.panelModels[]` 条目同形。 */
export interface FusionModelChoice {
  readonly profile: string;
  readonly model: string;
}

/** Fusion 硬上限，与引擎 `FUSION_MAX_PANEL` 一致。 */
export const FUSION_MAX_PANEL = 8;
/** Fusion 下限：少于两个模型就不是「多模型合议」。 */
export const FUSION_MIN_PANEL = 2;

export function routeOf(choice: FusionModelChoice): string {
  return `${choice.profile}/${choice.model}`;
}

function isChoice(value: unknown): value is FusionModelChoice {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return typeof record.profile === 'string' && record.profile.trim().length > 0
    && typeof record.model === 'string' && record.model.trim().length > 0;
}

/** 读出某一层自己写的 `fusion` 对象里的panel 和 analyst 角色。格式不对的条目读作「未配置」。 */
export function rolesFromFusionObject(fusion: Record<string, unknown>): {
  panels: FusionModelChoice[];
  analyst: FusionModelChoice | null;
} {
  const raw = fusion.panelModels;
  return {
    panels: Array.isArray(raw) ? raw.filter(isChoice) : [],
    analyst: isChoice(fusion.analystModel) ? fusion.analystModel : null,
  };
}

/** 还缺哪些角色。顺序与引擎 `FusionModelRole::ALL` 一致。 */
export function missingRoles(roles: ReturnType<typeof rolesFromFusionObject>): string[] {
  const missing: string[] = [];
  if (roles.panels.length < FUSION_MIN_PANEL) missing.push(tr('settings_parity_panel_models'));
  if (!roles.analyst) missing.push(tr('settings_parity_analyst'));
  return missing;
}

interface CandidateRow {
  readonly choice: FusionModelChoice;
  readonly label: string;
  readonly providerLabel: string;
  readonly analystCapable: boolean;
}

/** 把 provider catalog 摊平成候选行。没有 provider_id 或 model_id 的行不可寻址，丢弃。 */
export function candidatesFromCatalog(
  catalog: readonly { provider_id: string; provider_label: string; models: readonly ModelDetailsDto[] }[],
): CandidateRow[] {
  const rows: CandidateRow[] = [];
  for (const provider of catalog) {
    for (const model of provider.models) {
      const profile = model.provider_id || provider.provider_id;
      const id = model.model_id;
      if (!profile || !id) continue;
      rows.push({
        choice: { profile, model: id },
        label: model.display_name || id,
        providerLabel: model.provider_label || provider.provider_label || profile,
        // 严格按 `fusion_analyst_capable`，不按 capabilities.structured_output：
        // 后者只是模型自身的属性，而 analyst 还要求 profile 的 codec 能编码
        // `response_format`。旧引擎不带这个字段时读作 false —— 宁可少列一个
        // 可用的 analyst，也不要提供一个只会在花完钱之后失败的选择。
        analystCapable: model.fusion_analyst_capable === true,
      });
    }
  }
  return rows;
}

/** Use the host's credential/configuration status, including custom profiles. */
export function configuredCandidates(
  rows: readonly CandidateRow[],
  credentials: readonly { providerId: string; configured: boolean }[] = [],
): CandidateRow[] {
  const configured = new Set(credentials.filter((entry) => entry.configured).map((entry) => entry.providerId));
  return rows.filter((row) => configured.has(row.choice.profile === 'builtin' ? 'anthropic' : row.choice.profile));
}

export function ChoiceSelect({
  value, rows, placeholder, onPick, disabled,
}: {
  value: FusionModelChoice | null;
  rows: readonly CandidateRow[];
  placeholder: string;
  onPick(choice: FusionModelChoice | null): void;
  disabled: boolean;
}) {
  const t = useT();
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();
  const search = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [position, setPosition] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  const [active, setActive] = useState(0);
  const current = value ? routeOf(value) : '';
  const selected = rows.find((row) => routeOf(row.choice) === current);
  const needle = query.trim().toLocaleLowerCase();
  const options = [...(!needle ? [{ key: '', label: placeholder, choice: null as FusionModelChoice | null }] : []),
    ...rows.filter((row) => !needle || `${row.label} ${row.providerLabel} ${routeOf(row.choice)}`.toLocaleLowerCase().includes(needle))
      .map((row) => ({ key: routeOf(row.choice), label: `${row.label} · ${row.providerLabel}`, choice: row.choice }))];
  const close = (focus = true) => { setPosition(null); if (focus) trigger.current?.focus(); };
  const open = () => {
    const rect = trigger.current?.getBoundingClientRect();
    if (!rect) return;
    const below = window.innerHeight - rect.bottom - 12;
    const above = rect.top - 12;
    const height = Math.max(0, Math.min(280, Math.max(below, above)));
    const width = Math.min(Math.max(rect.width, 320), window.innerWidth - 24);
    setQuery('');
    setPosition({ left: Math.max(12, Math.min(rect.left, window.innerWidth - width - 12)),
      top: below >= Math.min(280, above) ? rect.bottom + 4 : Math.max(12, rect.top - height - 4), width, height });
    setActive(Math.max(0, options.findIndex((option) => option.key === current)));
  };
  useEffect(() => {
    if (!position) return;
    search.current?.focus();
    const dismiss = (event: Event) => {
      const target = event.target as Node | null;
      if (target && (list.current?.contains(target) || trigger.current?.contains(target))) return;
      setPosition(null);
    };
    const resize = () => setPosition(null);
    document.addEventListener('pointerdown', dismiss);
    document.addEventListener('scroll', dismiss, true);
    window.addEventListener('resize', resize);
    return () => {
      document.removeEventListener('pointerdown', dismiss);
      document.removeEventListener('scroll', dismiss, true);
      window.removeEventListener('resize', resize);
    };
  }, [position]);
  useEffect(() => {
    if (position) list.current?.querySelector(`[data-option-index="${active}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [active, position, query]);
  useEffect(() => { setPosition(null); }, [disabled, rows]);
  const pick = (index: number) => { const option = options[index]; if (option) onPick(option.choice); close(); };
  return <>
    <button ref={trigger} type="button" disabled={disabled} aria-label={placeholder}
      aria-haspopup="dialog" aria-expanded={position !== null} aria-controls={position ? `${id}-popup` : undefined}
      onClick={() => position ? close() : open()}
      onKeyDown={(event) => { if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); open(); } }}
      style={{ ...inputStyle(t), width: 280, maxWidth: '100%', display: 'flex', alignItems: 'center', gap: 8, textAlign: 'left' }}>
      <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {selected ? `${selected.label} · ${selected.providerLabel}` : current ? tr('desktop_settings_58a5411e7d', current) : placeholder}
      </span><Icon name="chevron" size={13} />
    </button>
    {position && createPortal(<div ref={list} id={`${id}-popup`} className="settings-model-popup" role="dialog" aria-label={placeholder}
      onKeyDown={(event) => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
        else if (event.key === 'Tab') close();
        else if (event.key === 'Enter' || (event.key === ' ' && event.target !== search.current)) { event.preventDefault(); pick(active); }
        else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
          event.preventDefault();
          setActive((index) => event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1
            : Math.max(0, Math.min(options.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1))));
        }
      }}
      style={{ position: 'fixed', zIndex: 100, left: position.left, top: position.top, width: position.width,
        maxHeight: position.height, display: 'flex', flexDirection: 'column', boxSizing: 'border-box',
        padding: 4, borderRadius: 9, border: `1px solid ${t.border}`, background: t.surface, color: t.text,
        colorScheme: t.dark ? 'dark' : 'light', '--settings-accent': t.accent,
        boxShadow: '0 8px 28px rgba(0,0,0,.18)' } as CSSProperties}>
      <input ref={search} value={query} placeholder={tr('desktop_settings_daab6f95a0')} aria-label={tr('composer_search_models_description')}
        role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls={id}
        aria-activedescendant={options.length ? `${id}-${Math.min(active, options.length - 1)}` : undefined}
        onChange={(event) => { setQuery(event.target.value); setActive(0); }}
        style={{ ...inputStyle(t), flexShrink: 0, marginBottom: 4, minWidth: 0 }} />
      <div id={id} role="listbox" aria-label={placeholder} style={{ overflowY: 'auto', minHeight: 0, overscrollBehavior: 'contain' }}>
      {!options.length && <div role="status" style={{ padding: 12, color: t.text3, fontSize: 13 }}>{tr('composer_no_matching_models')}</div>}
      {options.map((option, index) => <div key={option.key} id={`${id}-${index}`} role="option"
        aria-selected={option.key === current} data-option-index={index}
        onMouseDown={(event) => event.preventDefault()} onClick={() => pick(index)}
        style={{ minHeight: 32, display: 'flex', alignItems: 'center', padding: '5px 8px', boxSizing: 'border-box',
          fontSize: 13, borderRadius: 5, background: active === index ? t.surfaceActive : 'transparent', cursor: 'pointer' }}>
        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{option.label}</span>
      </div>)}
      </div>
    </div>, document.body)}
  </>;

}

/** Wrap long controls below labels in narrow settings windows. */
function Row({ title, desc, align = 'start', children }: { title: ReactNode; desc?: ReactNode; align?: 'start' | 'center'; children: ReactNode }) {
  const t = useT();
  return <div style={{ display: 'flex', flexWrap: 'wrap', gap: 16, padding: '16px 18px', alignItems: align === 'center' ? 'center' : 'flex-start', borderTop: `0.5px solid ${t.border}` }}>
    <div style={{ flex: '1 1 180px', minWidth: 0 }}><div style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{title}</div>{desc && <div style={{ fontSize: 12.5, color: t.text3, marginTop: 4, lineHeight: 1.5 }}>{desc}</div>}</div>
    {children && <div style={{ minWidth: 0, maxWidth: '100%', flex: '0 1 auto' }}>{children}</div>}
  </div>;
}

type StrategyDraft = {
  kind: 'strategy'; scope: FusionScope; path: string; revision: string; existing: boolean;
  value: FusionStrategyEntry; original: string;
};
type GlobalsDraft = { kind: 'globals'; scope: FusionScope; path: string; revision: string; value: FusionGlobals; original: string };
type Draft = StrategyDraft | GlobalsDraft;
type PendingWrite = { id: number; closeDraft: boolean; scope: FusionScope; path: string; sessionId?: string };
const SCOPE_DESCRIPTION: Record<FusionScope, string> = {
  user: "desktop_settings_c50e66c0b8",
  project: "desktop_settings_537a20ad62",
  local: "desktop_settings_0b7609f8a5",
};
const NUMBER_FIELDS = [
  ['qualityPanelCount', "desktop_settings_be39534eb7", 2, 8], ['fastPanelCount', "desktop_settings_ce6f24edb5", 2, 8],
  ['maxPanel', "desktop_settings_6c89c6cb02", 2, 8], ['minSuccessfulPanels', "desktop_settings_ad6bf997b2", 2, 8],
  ['panelMaxTurns', "desktop_settings_8325722688", 1, 12], ['panelMaxOutputTokensPerTurn', "desktop_settings_cfdb84d14b", 1],
  ['panelReservedInputTokensPerTurn', "desktop_settings_87ebdd7356", 1], ['maxReservedNanoUsd', "desktop_settings_3697bce124", 0],
  ['analystMaxOutputTokens', "desktop_settings_61a6255398", 1], ['analystMaxTurns', "desktop_settings_343de4a428", 1, 12],
  ['panelIdleTimeoutMs', "desktop_settings_41ae3496ef", 1], ['panelTotalTimeoutMs', "desktop_settings_16855f3a35", 1],
  ['analystTimeoutMs', "desktop_settings_65601afd75", 1], ['totalTimeoutMs', "desktop_settings_68dd88f1ba", 1],
  ['analysisProtocolRetries', "desktop_settings_0db7a8a85f", 0, 1],
] as const;
const IMPLEMENT_FIELDS = [
  ['maxTurns', "desktop_settings_835c5e2cf2", 1, 200], ['panelTimeoutMs', "desktop_settings_a716668b1e", 1],
  ['totalTimeoutMs', "desktop_settings_79b7dc9233", 1], ['verifyTimeoutMs', "desktop_settings_07cd7b7b86", 1],
  ['verifyConcurrency', "desktop_settings_ae7fa2d864", 1, 8], ['retainHours', "desktop_settings_d4da7d2338", 0],
  ['minFreeDiskBytes', "desktop_settings_cd4a9349c6", 0],
] as const;
function ObjectNumberFields({ fields, value, disabled, onChange }: {
  fields: readonly (readonly [string, string, number, number?])[];
  value: Record<string, unknown>; disabled: boolean; onChange(key: string, value: unknown): void;
}) {
  const t = useT();
  return <>{fields.map(([key, label, min, max]) => <Row key={key} title={tr(label)} align="center">
    <input type="number" aria-label={tr(label)} min={min} max={max} step={1}
      placeholder={tr('desktop_settings_9c69cf014b')} value={typeof value[key] === 'number' ? value[key] as number : ''} disabled={disabled}
      onChange={(event) => onChange(key, event.target.value === '' ? undefined : Number(event.target.value))}
      style={{ ...inputStyle(t), width: 160, maxWidth: '100%' }} />
  </Row>)}</>;
}
function OptionalBoolean({ value, label, disabled, onChange }: {
  value: unknown; label: string; disabled: boolean; onChange(value: boolean | undefined): void;
}) {
  const t = useT();
  return <select aria-label={label} disabled={disabled} value={value === true ? 'true' : value === false ? 'false' : ''}
    onChange={(event) => onChange(event.target.value === '' ? undefined : event.target.value === 'true')}
    style={{ ...inputStyle(t), width: 160 }}>
    <option value="">{tr('desktop_settings_7409a60806')}</option><option value="true">{tr('settings_status_on')}</option><option value="false">{tr('settings_status_off')}</option>
  </select>;
}
function withField<T extends object>(value: T, key: string, next: unknown): T {
  const result = { ...value } as Record<string, unknown>;
  if (next === undefined) delete result[key]; else result[key] = next;
  return result as T;
}

export function Fusion({ bridge, onNavigate, onRegisterNavigationRequest, onRegisterCloseRequest, onClose }: PageContentProps) {
  useI18n();
  const t = useT();
  const catalog = useMemo(() => parseFusionCatalog(bridge.fusionCatalogEvent?.catalog_json), [bridge.fusionCatalogEvent?.catalog_json]);
  const [filter, setFilter] = useState<FusionScope | 'all'>('all');
  const [search, setSearch] = useState('');
  const [draft, setDraft] = useState<Draft | null>(null);
  const [pendingAction, setPendingAction] = useState<(() => void) | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pendingPanel, setPendingPanel] = useState<FusionModelChoice | null>(null);
  const [copyScope, setCopyScope] = useState<FusionScope>('user');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [pendingWrite, setPendingWrite] = useState<PendingWrite | null>(null);
  const pendingWriteRef = useRef<PendingWrite | null>(null);
  const currentContext = useRef({ catalog, sessionId: bridge.activeSession?.sessionId });
  currentContext.current = { catalog, sessionId: bridge.activeSession?.sessionId };
  const busy = pendingWrite !== null;
  const dirty = draft !== null && JSON.stringify(draft.value) !== draft.original;
  const candidates = useMemo(() => configuredCandidates(candidatesFromCatalog(bridge.desktop.providerModelCatalog ?? []), bridge.bootstrap?.providerCredentials),
    [bridge.desktop.providerModelCatalog, bridge.bootstrap?.providerCredentials]);
  const analystCandidates = useMemo(() => candidates.filter((row) => row.analystCapable), [candidates]);
  const operation = bridge.configurationOperations?.fusion;
  const finishWrite = useCallback((id: number, failure?: string) => {
    const pending = pendingWriteRef.current;
    if (!pending || pending.id !== id) return;
    pendingWriteRef.current = null;
    setPendingWrite(null);
    if (failure) { setError(failure); return; }
    const context = currentContext.current;
    const sameContext = pending.sessionId === context.sessionId
      && context.catalog.scopes.some((scope) => scope.scope === pending.scope && scope.path === pending.path);
    setConfirmDelete(false);
    if (sameContext) {
      setNotice(tr('desktop_settings_7574a6af27'));
      if (pending.closeDraft) setDraft(null);
    } else {
      setNotice(tr('desktop_settings_7779ab0f7a'));
      setError(tr('desktop_settings_e0b426f69c'));
    }
  }, []);

  useEffect(() => {
    void bridge.fusionAdmin(adminRecordCommand('get_catalog')).catch((cause: unknown) => setError(cause instanceof Error ? cause.message : tr('desktop_settings_1285a14ec7')));
  }, [bridge.fusionAdmin, bridge.activeSession?.sessionId]);
  useEffect(() => {
    if (!pendingWrite || operation?.operation_id !== pendingWrite.id) return;
    if (operation.status === 'failed') finishWrite(pendingWrite.id, operation.message ?? tr('desktop_settings_ccb17b5c6c'));
    else if (operation.status === 'succeeded') finishWrite(pendingWrite.id);
  }, [operation, pendingWrite, finishWrite]);
  const requestAction = (action: () => void) => {
    if (busy) return;
    if (dirty) setPendingAction(() => action);
    else { setError(null); setConfirmDelete(false); action(); }
  };
  useExtensionHubNavigation(onRegisterNavigationRequest, (pageId, pluginId) => requestAction(() => onNavigate(pageId, pluginId)));
  const requestCloseRef = useRef(() => requestAction(onClose));
  requestCloseRef.current = () => requestAction(onClose);
  useEffect(() => {
    onRegisterCloseRequest?.(() => requestCloseRef.current());
    return () => onRegisterCloseRequest?.(null);
  }, [onRegisterCloseRequest]);
  useEffect(() => {
    if (!dirty) return;
    const preventClose = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', preventClose);
    return () => window.removeEventListener('beforeunload', preventClose);
  }, [dirty]);

  const scopeFor = (scope: FusionScope) => catalog.scopes.find((entry) => entry.scope === scope);
  const openStrategy = (scope: FusionCatalogScope, value: FusionStrategyEntry, existing: boolean) => {
    setDraft({ kind: 'strategy', scope: scope.scope, path: scope.path, revision: scope.revision, existing, value: structuredClone(value),
      original: JSON.stringify(existing ? value : { ...value, name: '', config: {} }) });
    setPendingPanel(null); setCopyScope(scope.scope === 'user' ? 'project' : 'user'); setError(null); setNotice(null); setConfirmDelete(false);
  };
  const create = () => {
    const scope = scopeFor(filter === 'all' ? 'user' : filter);
    if (!scope) return;
    openStrategy(scope, { id: crypto.randomUUID(), name: '', config: {} }, false);
  };
  const write = (action: string, scope: FusionScope, revision: string, target?: string, payload?: object, closeDraft = false) => {
    if (busy) return;
    if (draft && draft.scope === scope && draft.path !== scopeFor(scope)?.path) {
      setError(tr('desktop_settings_1ed0e4f978'));
      return;
    }
    const id = nextConfigurationOperationId();
    const pending = { id, closeDraft, scope, path: scopeFor(scope)?.path ?? '', sessionId: bridge.activeSession?.sessionId };
    pendingWriteRef.current = pending;
    setPendingWrite(pending); setError(null); setNotice(null);
    void bridge.fusionAdmin(adminRecordCommand(action, { operation_id: id, scope, revision,
      ...(target ? { target } : {}), ...(payload ? { payload_json: JSON.stringify(payload) } : {}) }))
      .then((receipt) => {
        // The RPC belongs to the initiating session. Its completion still
        // arrives when that session's receipt is no longer shown by useBridge.
        if (receipt?.status === 'succeeded') finishWrite(id);
        else if (receipt?.status === 'failed') finishWrite(id, receipt.message ?? tr('desktop_settings_ccb17b5c6c'));
      })
      .catch((cause: unknown) => finishWrite(id, cause instanceof Error ? cause.message : tr('desktop_settings_2c8675ec00')));
  };
  const updateConfig = (key: string, value: unknown) => setDraft((current) => current?.kind === 'strategy'
    ? { ...current, value: { ...current.value, config: withField(current.value.config, key, value) } } : current);
  const strategyDraft = draft?.kind === 'strategy' ? draft : null;
  const config = strategyDraft?.value.config ?? {};
  const roles = rolesFromFusionObject(config);
  const maxPanel = typeof config.maxPanel === 'number' ? Math.max(2, Math.min(8, config.maxPanel)) : FUSION_MAX_PANEL;
  const panelCandidates = useMemo(() => candidates.filter((row) => !roles.panels.some((choice) => routeOf(choice) === routeOf(row.choice))), [candidates, config.panelModels]);
  const movePanel = (index: number, delta: number) => {
    const next = [...roles.panels]; const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target]!, next[index]!]; updateConfig('panelModels', next);
  };
  const scope = draft ? scopeFor(draft.scope) : filter === 'all' ? undefined : scopeFor(filter);
  const nameError = strategyDraft ? fusionNameError(strategyDraft.value.name, scope, strategyDraft.value.id) : null;
  const save = () => {
    if (!draft) return;
    if (draft.kind === 'globals') { write('set_globals', draft.scope, draft.revision, undefined, { ...draft.value, ...(draft.value.allowed_profiles ? { allowed_profiles: draft.value.allowed_profiles.map((profile) => profile.trim()).filter(Boolean) } : {}) }, true); return; }
    if (nameError) { setError(nameError); return; }
    write('save', draft.scope, draft.revision, undefined, { ...draft.value, name: draft.value.name.trim() }, true);
  };
  const readiness = (value: FusionStrategyEntry): string => {
    const valueRoles = rolesFromFusionObject(value.config);
    const missing = missingRoles(valueRoles);
    if (missing.length) return tr('desktop_settings_1de3c779e4', missing.join('、'));
    if (valueRoles.panels.some((choice) => !candidates.some((row) => routeOf(row.choice) === routeOf(choice)))) return tr('desktop_settings_14ed1a3298');
    if (!analystCandidates.some((row) => routeOf(row.choice) === routeOf(valueRoles.analyst!))) return tr('desktop_settings_cb94209780');
    return tr('settings_provider_status_configured');
  };
  const entries = catalog.scopes.flatMap((entry) => entry.strategies.map((strategy) => ({ scope: entry, strategy })));
  const needle = search.trim().toLocaleLowerCase();
  const filtered = entries.filter(({ scope: entryScope, strategy }) => (filter === 'all' || entryScope.scope === filter)
    && (!needle || `${strategy.name} ${fusionScopeLabel(entryScope.scope)} ${rolesFromFusionObject(strategy.config).panels.map(routeOf).join(' ')}`.toLocaleLowerCase().includes(needle)));
  const globals = scope?.globals ?? catalog.effective_globals;
  const effectiveDefault = entries.find(({ scope: entryScope, strategy }) => catalog.default_strategy?.scope === entryScope.scope && catalog.default_strategy.id === strategy.id);
  const implement = config.implement && typeof config.implement === 'object' && !Array.isArray(config.implement) ? config.implement as Record<string, unknown> : {};

  return <ExtensionHubLayout>
    <header className="extension-hub-header">
      <div className="extension-hub-heading"><p>{tr('desktop_settings_d7d136ebec')}</p></div>
      {!draft && <div className="extension-hub-toolbar">
        <label className="extension-search-field"><ExtensionGlyph name="search" size={16} /><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder={tr('desktop_settings_f1fc1084fa')} aria-label={tr('desktop_settings_c2214803ee')} /></label>
        <div className="extension-hub-toolbar-actions">
          <button type="button" className="extension-hub-icon-button" aria-label={tr('desktop_settings_32cc563514')} disabled={busy} onClick={() => void bridge.fusionAdmin(adminRecordCommand('get_catalog')).catch((cause: unknown) => setError(String(cause)))}><ExtensionGlyph name="refresh" size={16} /></button>
          <button type="button" className="extension-hub-primary" disabled={busy || !catalog.scopes.length} onClick={create}>{tr('desktop_settings_bb090baf35')}<ExtensionGlyph name="plus" size={14} /></button>
        </div>
      </div>}
    </header>
    {pendingAction && <div role="alert" style={noteStyle(t, 'warn')}>
      {tr('desktop_settings_a225fc0a75')}<div style={{ display: 'flex', gap: 8, marginTop: 8 }}><button type="button" style={ghostButtonStyle(t)} onClick={() => { const action = pendingAction; setPendingAction(null); setDraft(null); setError(null); action(); }}>{tr('desktop_settings_5b19cb5d3c')}</button><button type="button" style={ghostButtonStyle(t)} onClick={() => setPendingAction(null)}>{tr('cron_keep_editing')}</button></div>
    </div>}
    {error && <div role="alert" style={noteStyle(t, 'danger')}>{error}</div>}
    {notice && <div role="status" style={{ ...noteStyle(t), marginBottom: 16 }}>{notice}</div>}
    {catalog.diagnostics.map((message, index) => <div key={index} role="alert" style={noteStyle(t, 'warn')}>{message}</div>)}
    {!draft ? <div className="configuration-list" style={{ display: 'grid', gap: 20 }}>
      <div role="group" aria-label={tr('desktop_settings_53bc997d63')} style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
        {(['all', ...FUSION_SCOPES] as const).map((value) => <button key={value} type="button" aria-pressed={filter === value}
          onClick={() => { setFilter(value); setError(null); }} style={{ ...ghostButtonStyle(t), background: filter === value ? t.surfaceActive : 'transparent' }}>{value === 'all' ? tr('drawer_filter_all') : fusionScopeLabel(value)}</button>)}
      </div>
      <div style={{ ...noteStyle(t), margin: 0 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center', flexWrap: 'wrap' }}>
          <span>{filter === 'all' ? tr('desktop_settings_58725d177f') : tr('desktop_settings_58ce30666f', fusionScopeLabel(filter))} {tr('desktop_settings_dd92f506a5')}{globals.enabled === undefined ? tr('desktop_settings_7409a60806') : globals.enabled ? tr('settings_status_on') : tr('settings_status_off')} {tr('desktop_settings_e7c9286a37')}{globals.proactive === undefined ? tr('desktop_settings_7409a60806') : globals.proactive ? tr('settings_status_on') : tr('settings_status_off')}</span>
          {scope && <button type="button" disabled={busy} style={ghostButtonStyle(t, busy)} onClick={() => setDraft({ kind: 'globals', scope: scope.scope, path: scope.path, revision: scope.revision, value: structuredClone(scope.globals), original: JSON.stringify(scope.globals) })}>{tr('desktop_settings_d22d9f369f')}</button>}
        </div>
        <div style={{ marginTop: 6 }}>{filter === 'all' ? tr('desktop_settings_f2451010f4') : tr(SCOPE_DESCRIPTION[filter])}</div>
        <div style={{ marginTop: 6 }}>{tr('desktop_settings_a6ae5dd15c')}{effectiveDefault ? `${effectiveDefault.strategy.name} · ${fusionScopeLabel(effectiveDefault.scope.scope)}` : tr('desktop_settings_e421f058b8')}{tr('desktop_settings_de38f52aa8')}</div>
        {scope?.default_strategy_id && <button type="button" disabled={busy} style={{ ...ghostButtonStyle(t), marginTop: 8 }} onClick={() => write('set_default', scope.scope, scope.revision)}>{tr('desktop_settings_cdc7068287')}</button>}
      </div>
      <div className="extension-hub-list" aria-label={tr('desktop_settings_bb3ef33253')} style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
        {filtered.map(({ scope: entryScope, strategy }) => <div key={fusionStrategyKey(entryScope.scope, strategy.id)} className="extension-hub-row configuration-entry">
          <span className="extension-hub-icon" aria-hidden="true"><Icon name="sparkle" size={22} /></span>
          <button type="button" className="extension-hub-row-copy extension-hub-row-open" disabled={busy} onClick={() => openStrategy(entryScope, strategy, true)}>
            <span className="extension-hub-row-title">{strategy.name}{entryScope.default_strategy_id === strategy.id && <span style={{ fontSize: 11, color: t.text3, marginLeft: 9 }}>{tr('desktop_settings_c19889c484')}</span>}</span>
            <span className="extension-hub-row-description">{rolesFromFusionObject(strategy.config).panels.map(routeOf).join(' · ') || tr('desktop_settings_b8cf8fa1e8')}</span>
            <span style={{ color: t.text3, fontSize: 12, marginTop: 3 }}>{entryScope.admitted ? readiness(strategy) : tr('desktop_settings_b115bc8f90')}</span>
          </button>
          <div className="extension-hub-row-side" style={{ flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <span className="extension-hub-row-scope" style={{ display: 'inline', whiteSpace: 'nowrap' }}>{fusionScopeLabel(entryScope.scope)}</span>
            <button type="button" disabled={busy || entryScope.default_strategy_id === strategy.id} aria-label={tr('desktop_settings_f24a9bdce0', strategy.name, fusionScopeLabel(entryScope.scope))} style={ghostButtonStyle(t, busy || entryScope.default_strategy_id === strategy.id)} onClick={() => write('set_default', entryScope.scope, entryScope.revision, strategy.id)}>{entryScope.default_strategy_id === strategy.id ? tr('settings_badge_default') : tr('settings_provider_set_default')}</button>
          </div>
        </div>)}
        {!filtered.length && <div role="status" style={{ padding: '32px 18px', color: t.text3, fontSize: 13 }}>{bridge.fusionCatalogEvent ? search ? tr('desktop_settings_1ec5382fdc') : tr('desktop_settings_1137ab8f24') : tr('desktop_settings_59fbc25018')}</div>}
      </div>
    </div> : <div className="configuration-detail" style={{ display: 'grid', gap: 20, minWidth: 0 }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap' }}>
        <button type="button" style={ghostButtonStyle(t, busy)} disabled={busy} onClick={() => requestAction(() => setDraft(null))}>{tr('desktop_settings_2cace37f4e')}</button>
        <span style={{ fontSize: 12, color: t.text3 }}>{busy ? tr('audio_settings_saving') : dirty ? tr('desktop_settings_c2277872cd') : tr('desktop_settings_d66ee6681a')}</span>
      </div>
      <div><h3 style={{ margin: 0, fontSize: 20, color: t.text }}>{draft.kind === 'globals' ? tr('desktop_settings_5c7a092907', fusionScopeLabel(draft.scope)) : draft.existing ? draft.value.name : tr('desktop_settings_bb090baf35')}</h3><p style={{ margin: '8px 0 0', color: t.text3, fontSize: 12.5 }}>{tr(SCOPE_DESCRIPTION[draft.scope])}</p>{scope?.path && <p className="mono" style={{ margin: '6px 0 0', color: t.text4, fontSize: 11, overflowWrap: 'anywhere' }}>{scope.path}</p>}</div>
      {draft.kind === 'globals' ? <Card title={tr('desktop_settings_4dde14c665')}>
        {([['enabled', tr('desktop_settings_3e0c173617')], ['proactive', tr('desktop_settings_436ae7193f')], ['slash_cross_provider_default', tr('desktop_settings_7c1663b465')], ['allow_cross_provider_for_agent', tr('desktop_settings_cfac4e0240')]] as const).map(([key, label]) => <Row key={key} title={label} align="center"><OptionalBoolean label={label} value={draft.value[key]} disabled={busy} onChange={(value) => setDraft({ ...draft, value: withField(draft.value, key, value) })} /></Row>)}
        <Row title={tr('desktop_settings_c77395d9bf')} desc={tr('desktop_settings_2758dd1855')} align="start"><textarea aria-label={tr('desktop_settings_c77395d9bf')} value={(draft.value.allowed_profiles ?? []).join('\n')} disabled={busy} rows={4} onChange={(event) => setDraft({ ...draft, value: withField(draft.value, 'allowed_profiles', event.target.value.split('\n')) })} style={{ ...inputStyle(t), width: 280, resize: 'vertical' }} /></Row>
      </Card> : <>
        <Card title={tr('desktop_settings_c747a9ade3')}>
          <Row title={tr('desktop_settings_d44e9b3d3b')} align="center"><input aria-label={tr('desktop_settings_a903683f81')} value={draft.value.name} placeholder={tr('desktop_settings_c85d4e91ed')} maxLength={120} disabled={busy} onChange={(event) => setDraft({ ...draft, value: { ...draft.value, name: event.target.value } })} style={{ ...inputStyle(t), width: 280, maxWidth: '100%' }} /></Row>
          <Row title={tr('cron_scope_label')} desc={draft.existing ? tr('desktop_settings_dc602a32c6') : tr(SCOPE_DESCRIPTION[draft.scope])} align="center"><select aria-label={tr('desktop_settings_d63c88c543')} disabled={busy || draft.existing} value={draft.scope} onChange={(event) => { const next = scopeFor(event.target.value as FusionScope); if (next) setDraft({ ...draft, scope: next.scope, path: next.path, revision: next.revision }); }} style={{ ...inputStyle(t), width: 160 }}>{catalog.scopes.map((entry) => <option key={entry.scope} value={entry.scope}>{fusionScopeLabel(entry.scope)}</option>)}</select></Row>
          <Row title={tr('desktop_settings_328b71b18b')} desc={tr('desktop_settings_a820647e83')} align="center"><select aria-label={tr('desktop_settings_328b71b18b')} disabled={busy} value={typeof config.preset === 'string' ? config.preset : ''} onChange={(event) => updateConfig('preset', event.target.value || undefined)} style={{ ...inputStyle(t), width: 160 }}><option value="">{tr('desktop_settings_9c69cf014b')}</option><option value="quality">{tr('desktop_shell_quality')}</option><option value="fast">{tr('desktop_chat_fast')}</option></select></Row>
        </Card>
        {scope?.diagnostic && <div role="alert" style={noteStyle(t, 'warn')}>{scope.diagnostic}</div>}
        <div role="status" style={noteStyle(t)}>{draft.existing && !scope?.admitted ? tr('desktop_settings_f0ad50b69b') : readiness(draft.value)}{tr('desktop_settings_29e31c06b5')}</div>
        <Card title={tr('settings_parity_panel_models')}>
          <Row title={tr('desktop_settings_5dc5594248')} desc={tr('desktop_settings_e3006c04c4', FUSION_MIN_PANEL, maxPanel)} align="start">{null}</Row>
          {roles.panels.map((choice, index) => <Row key={`${index}:${routeOf(choice)}`} title={<span className="mono" style={{ fontSize: 12.5, overflowWrap: 'anywhere' }}>{index + 1}. {routeOf(choice)}</span>} align="center"><div style={{ display: 'flex', gap: 6 }}><button type="button" disabled={busy || index === 0} aria-label={tr('desktop_settings_f8c7e50aab', routeOf(choice))} style={ghostButtonStyle(t, busy || index === 0)} onClick={() => movePanel(index, -1)}>↑</button><button type="button" disabled={busy || index === roles.panels.length - 1} aria-label={tr('desktop_settings_31e7b4cdbd', routeOf(choice))} style={ghostButtonStyle(t, busy || index === roles.panels.length - 1)} onClick={() => movePanel(index, 1)}>↓</button><button type="button" disabled={busy} aria-label={tr('desktop_settings_fc8c501995', routeOf(choice))} style={ghostButtonStyle(t)} onClick={() => updateConfig('panelModels', roles.panels.filter((_, i) => i !== index))}>{tr('settings_parity_remove')}</button></div></Row>)}
          <Row title={tr('settings_parity_add_panel')} align="center"><div style={{ display: 'flex', gap: 7, flexWrap: 'wrap', maxWidth: '100%' }}><ChoiceSelect value={pendingPanel} rows={panelCandidates} placeholder={tr('settings_parity_choose_model')} onPick={setPendingPanel} disabled={busy || roles.panels.length >= maxPanel} /><button type="button" disabled={busy || !pendingPanel || !panelCandidates.some((row) => routeOf(row.choice) === routeOf(pendingPanel)) || roles.panels.length >= maxPanel} style={ghostButtonStyle(t, busy || !pendingPanel)} onClick={() => { if (!pendingPanel || !panelCandidates.some((row) => routeOf(row.choice) === routeOf(pendingPanel))) return; updateConfig('panelModels', [...roles.panels, pendingPanel]); setPendingPanel(null); }}>{tr('workspace_confirm_add')}</button></div></Row>
        </Card>
        <Card title={tr('settings_parity_analyst')}><Row title={tr('desktop_settings_2ecd1c0f5e')} desc={tr('desktop_settings_9c866d1162')} align="center"><ChoiceSelect value={roles.analyst} rows={analystCandidates} placeholder={tr('desktop_settings_94ab09f1a2')} onPick={(value) => updateConfig('analystModel', value ?? undefined)} disabled={busy} /></Row></Card>
        <details style={{ border: `1px solid ${t.border}`, borderRadius: 12, minWidth: 0 }}><summary style={{ padding: 16, fontSize: 13, color: t.text2, cursor: 'pointer' }}>{tr('desktop_settings_972a97281c')}</summary><p style={{ padding: '0 18px', color: t.text3, fontSize: 12 }}>{tr('desktop_settings_70f762cf88')}</p>
          <ObjectNumberFields fields={NUMBER_FIELDS} value={config} disabled={busy} onChange={updateConfig} />
          <Row title={tr('desktop_settings_cb0c4fe64e')} align="center"><OptionalBoolean label={tr('desktop_settings_cb0c4fe64e')} value={config.partialOk} disabled={busy} onChange={(value) => updateConfig('partialOk', value)} /></Row>
          <Row title={tr('desktop_settings_0887d9f435')} align="center"><OptionalBoolean label={tr('desktop_settings_0887d9f435')} value={config.analystTools} disabled={busy} onChange={(value) => updateConfig('analystTools', value)} /></Row>
          <Row title={tr('desktop_settings_a15ac5fba8')} align="center"><select aria-label={tr('desktop_settings_a15ac5fba8')} value={typeof config.completionPolicy === 'string' ? config.completionPolicy : ''} disabled={busy} onChange={(event) => updateConfig('completionPolicy', event.target.value || undefined)} style={{ ...inputStyle(t), width: 200 }}><option value="">{tr('desktop_settings_9c69cf014b')}</option><option value="wait_all">{tr('desktop_settings_4329a53710')}</option><option value="quorum_after_grace">{tr('desktop_settings_5f10072d24')}</option></select></Row>
          <details style={{ margin: 16 }}><summary style={{ padding: '8px 0', color: t.text2, fontSize: 13, cursor: 'pointer' }}>{tr('desktop_settings_6dcd625041')}</summary>
            <ObjectNumberFields fields={IMPLEMENT_FIELDS} value={implement} disabled={busy} onChange={(key, value) => updateConfig('implement', withField(implement, key, value))} />
            <Row title={tr('desktop_settings_5c705e7de8')} desc={tr('desktop_settings_9e4388f364')} align="start"><textarea aria-label={tr('desktop_settings_5c705e7de8')} rows={4} value={Array.isArray(implement.verifyCommands) ? implement.verifyCommands.join('\n') : ''} disabled={busy} onChange={(event) => updateConfig('implement', withField(implement, 'verifyCommands', event.target.value ? event.target.value.split('\n') : undefined))} style={{ ...inputStyle(t), width: 280, resize: 'vertical' }} /></Row>
          </details>
        </details>
        {draft.existing && <Card title={tr('desktop_settings_b08b84ac3d')}><Row title={tr('desktop_settings_102f86f930')} desc={tr('desktop_settings_07f0680049')} align="center"><div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}><select aria-label={tr('desktop_settings_3ad4931e0b')} value={copyScope} disabled={busy} onChange={(event) => setCopyScope(event.target.value as FusionScope)} style={inputStyle(t)}>{catalog.scopes.filter((entry) => entry.scope !== draft.scope).map((entry) => <option key={entry.scope} value={entry.scope}>{fusionScopeLabel(entry.scope)}</option>)}</select><button type="button" disabled={busy || !scopeFor(copyScope)} style={ghostButtonStyle(t)} onClick={() => { const target = scopeFor(copyScope); if (target) requestAction(() => openStrategy(target, copiedFusionStrategy(scope?.strategies.find((entry) => entry.id === draft.value.id) ?? draft.value, crypto.randomUUID()), false)); }}>{tr('desktop_settings_6fe471d852')}</button></div></Row><Row title={tr('desktop_settings_1190565c3d')} desc={tr('desktop_settings_111605194c')} align="center"><button type="button" disabled={busy} style={ghostButtonStyle(t, busy, true)} onClick={() => setConfirmDelete(true)}>{tr('desktop_settings_1190565c3d')}</button></Row>{confirmDelete && <div role="alert" style={{ ...noteStyle(t, 'warn'), margin: 16 }}>{tr('desktop_settings_delete_strategy_confirm', draft.value.name)}<div style={{ display: 'flex', gap: 8, marginTop: 8 }}><button type="button" disabled={busy} style={ghostButtonStyle(t, busy, true)} onClick={() => write('delete', draft.scope, draft.revision, draft.value.id, undefined, true)}>{tr('desktop_settings_a3ea3c17b4')}</button><button type="button" style={ghostButtonStyle(t)} onClick={() => setConfirmDelete(false)}>{tr('desktop_settings_00213c7f27')}</button></div></div>}</Card>}
      </>}
      <div style={{ display: 'flex', justifyContent: 'flex-end', flexWrap: 'wrap', gap: 8, padding: '4px 0 20px' }}>
        <button type="button" disabled={busy} style={ghostButtonStyle(t, busy)} onClick={() => requestAction(() => setDraft(null))}>{tr('common_cancel')}</button>
        <button type="button" className="extension-hub-primary" disabled={busy || (!dirty && (draft.kind === 'globals' || draft.existing)) || (draft.kind === 'strategy' && !draft.value.name.trim())} onClick={save}>{busy ? tr('audio_settings_saving') : tr('settings_save')}</button>
      </div>
    </div>}
  </ExtensionHubLayout>;
}
