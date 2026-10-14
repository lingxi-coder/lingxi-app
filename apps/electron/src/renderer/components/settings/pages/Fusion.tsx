import { useCallback, useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
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
  FUSION_SCOPES, FUSION_SCOPE_LABELS, copiedFusionStrategy, fusionNameError, fusionNeedsLegacyReplacement, fusionStrategyKey, parseFusionCatalog,
  type FusionCatalogScope, type FusionGlobals, type FusionScope, type FusionStrategyEntry,
} from './FusionCatalog';

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
  if (roles.panels.length < FUSION_MIN_PANEL) missing.push('panel 模型');
  if (!roles.analyst) missing.push('analyst 模型');
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
        {selected ? `${selected.label} · ${selected.providerLabel}` : current ? `${current} （当前不可达）` : placeholder}
      </span><Icon name="chevron" size={13} />
    </button>
    {position && createPortal(<div ref={list} id={`${id}-popup`} role="dialog" aria-label={placeholder}
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
        boxShadow: '0 8px 28px rgba(0,0,0,.18)' }}>
      <input ref={search} value={query} placeholder="搜索模型或 Provider…" aria-label="搜索模型"
        role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls={id}
        aria-activedescendant={options.length ? `${id}-${Math.min(active, options.length - 1)}` : undefined}
        onChange={(event) => { setQuery(event.target.value); setActive(0); }}
        style={{ ...inputStyle(t), flexShrink: 0, marginBottom: 4, minWidth: 0 }} />
      <div id={id} role="listbox" aria-label={placeholder} style={{ overflowY: 'auto', minHeight: 0, overscrollBehavior: 'contain' }}>
      {!options.length && <div role="status" style={{ padding: 12, color: t.text3, fontSize: 13 }}>没有匹配的模型</div>}
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
  value: FusionStrategyEntry; original: string; replaceLegacy: boolean;
};
type GlobalsDraft = { kind: 'globals'; scope: FusionScope; path: string; revision: string; value: FusionGlobals; original: string };
type Draft = StrategyDraft | GlobalsDraft;
type PendingWrite = { id: number; closeDraft: boolean; scope: FusionScope; path: string; sessionId?: string };
const SCOPE_DESCRIPTION: Record<FusionScope, string> = {
  user: '保存在用户设置中，供所有项目使用。',
  project: '保存在项目共享设置中，可随仓库提交。',
  local: '保存在当前项目的本地设置中，不共享给其他人。',
};
const NUMBER_FIELDS = [
  ['qualityPanelCount', 'Quality 模型数', 2, 8], ['fastPanelCount', 'Fast 模型数', 2, 8],
  ['maxPanel', '模型数量上限', 2, 8], ['minSuccessfulPanels', '最少成功模型数', 2, 8],
  ['panelMaxTurns', '每个模型最多执行轮数', 1, 12], ['panelMaxOutputTokensPerTurn', '每轮输出 Token 上限', 1],
  ['panelReservedInputTokensPerTurn', '每轮预留输入 Token', 1], ['maxReservedNanoUsd', '预留费用上限（nano USD）', 0],
  ['analystMaxOutputTokens', 'Analyst 输出 Token 上限', 1], ['analystMaxTurns', 'Analyst 最多执行轮数', 1, 12],
  ['panelIdleTimeoutMs', '模型空闲超时（毫秒）', 1], ['panelTotalTimeoutMs', '模型总超时（毫秒）', 1],
  ['analystTimeoutMs', 'Analyst 超时（毫秒）', 1], ['totalTimeoutMs', '整体超时（毫秒）', 1],
  ['analysisProtocolRetries', 'Analyst 格式重试次数', 0, 1],
] as const;
const IMPLEMENT_FIELDS = [
  ['maxTurns', '实现阶段轮数上限', 1, 200], ['panelTimeoutMs', '模型实现超时（毫秒）', 1],
  ['totalTimeoutMs', '实现阶段总超时（毫秒）', 1], ['verifyTimeoutMs', '单条验证命令超时（毫秒）', 1],
  ['verifyConcurrency', '并行验证数量', 1, 8], ['retainHours', '工作树保留时间（小时）', 0],
  ['minFreeDiskBytes', '最低磁盘可用空间（字节）', 0],
] as const;
function ObjectNumberFields({ fields, value, disabled, onChange }: {
  fields: readonly (readonly [string, string, number, number?])[];
  value: Record<string, unknown>; disabled: boolean; onChange(key: string, value: unknown): void;
}) {
  const t = useT();
  return <>{fields.map(([key, label, min, max]) => <Row key={key} title={label} align="center">
    <input type="number" aria-label={label} min={min} max={max} step={1}
      placeholder="引擎默认" value={typeof value[key] === 'number' ? value[key] as number : ''} disabled={disabled}
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
    <option value="">未指定</option><option value="true">开启</option><option value="false">关闭</option>
  </select>;
}
function withField<T extends object>(value: T, key: string, next: unknown): T {
  const result = { ...value } as Record<string, unknown>;
  if (next === undefined) delete result[key]; else result[key] = next;
  return result as T;
}

export function Fusion({ bridge, onNavigate, onRegisterNavigationRequest, onRegisterCloseRequest, onClose }: PageContentProps) {
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
  const dirty = draft !== null && (JSON.stringify(draft.value) !== draft.original || (draft.kind === 'strategy' && draft.replaceLegacy));
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
      setNotice('Fusion 配置已保存。');
      if (pending.closeDraft) setDraft(null);
    } else {
      setNotice('原会话的 Fusion 配置已保存。');
      setError('当前会话或项目已切换。请返回策略列表重新打开配置。');
    }
  }, []);

  useEffect(() => {
    void bridge.fusionAdmin(adminRecordCommand('get_catalog')).catch((cause: unknown) => setError(cause instanceof Error ? cause.message : '无法读取 Fusion 策略。'));
  }, [bridge.fusionAdmin, bridge.activeSession?.sessionId]);
  useEffect(() => {
    if (!pendingWrite || operation?.operation_id !== pendingWrite.id) return;
    if (operation.status === 'failed') finishWrite(pendingWrite.id, operation.message ?? '保存失败，请刷新后重试。');
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
      original: JSON.stringify(existing ? value : { ...value, name: '', config: {} }), replaceLegacy: false });
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
      setError('当前项目已切换。请返回策略列表重新打开配置；草稿尚未写入。');
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
        else if (receipt?.status === 'failed') finishWrite(id, receipt.message ?? '保存失败，请刷新后重试。');
      })
      .catch((cause: unknown) => finishWrite(id, cause instanceof Error ? cause.message : '无法保存 Fusion 配置。'));
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
  const needsLegacyReplacement = fusionNeedsLegacyReplacement(scope);
  const nameError = strategyDraft ? fusionNameError(strategyDraft.value.name, scope, strategyDraft.value.id) : null;
  const save = () => {
    if (!draft) return;
    if (draft.kind === 'globals') { write('set_globals', draft.scope, draft.revision, undefined, { ...draft.value, ...(draft.value.allowed_profiles ? { allowed_profiles: draft.value.allowed_profiles.map((profile) => profile.trim()).filter(Boolean) } : {}) }, true); return; }
    if (nameError) { setError(nameError); return; }
    write('save', draft.scope, draft.revision, undefined, { ...draft.value, name: draft.value.name.trim(),
      ...(draft.replaceLegacy ? { replace_legacy: true } : {}) }, true);
  };
  const readiness = (value: FusionStrategyEntry): string => {
    const valueRoles = rolesFromFusionObject(value.config);
    const missing = missingRoles(valueRoles);
    if (missing.length) return `待配置 · 缺少 ${missing.join('、')}`;
    if (valueRoles.panels.some((choice) => !candidates.some((row) => routeOf(row.choice) === routeOf(choice)))) return '不可运行 · Panel 模型未连接';
    if (!analystCandidates.some((row) => routeOf(row.choice) === routeOf(valueRoles.analyst!))) return '不可运行 · Analyst 不可用';
    return '已配置';
  };
  const entries = catalog.scopes.flatMap((entry) => entry.strategies.map((strategy) => ({ scope: entry, strategy })));
  const needle = search.trim().toLocaleLowerCase();
  const filtered = entries.filter(({ scope: entryScope, strategy }) => (filter === 'all' || entryScope.scope === filter)
    && (!needle || `${strategy.name} ${FUSION_SCOPE_LABELS[entryScope.scope]} ${rolesFromFusionObject(strategy.config).panels.map(routeOf).join(' ')}`.toLocaleLowerCase().includes(needle)));
  const globals = scope?.globals ?? catalog.effective_globals;
  const effectiveDefault = entries.find(({ scope: entryScope, strategy }) => catalog.default_strategy?.scope === entryScope.scope && catalog.default_strategy.id === strategy.id);
  const implement = config.implement && typeof config.implement === 'object' && !Array.isArray(config.implement) ? config.implement as Record<string, unknown> : {};

  return <ExtensionHubLayout>
    <header className="extension-hub-header">
      <div className="extension-hub-heading"><p>为不同任务配置多模型合议策略。最终回答由当前会话模型完成。</p></div>
      {!draft && <div className="extension-hub-toolbar">
        <label className="extension-search-field"><ExtensionGlyph name="search" size={16} /><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索策略或模型" aria-label="搜索 Fusion 策略" /></label>
        <div className="extension-hub-toolbar-actions">
          <button type="button" className="extension-hub-icon-button" aria-label="刷新 Fusion 策略" disabled={busy} onClick={() => void bridge.fusionAdmin(adminRecordCommand('get_catalog')).catch((cause: unknown) => setError(String(cause)))}><ExtensionGlyph name="refresh" size={16} /></button>
          <button type="button" className="extension-hub-primary" disabled={busy || !catalog.scopes.length} onClick={create}>新建策略 <ExtensionGlyph name="plus" size={14} /></button>
        </div>
      </div>}
    </header>
    {pendingAction && <div role="alert" style={noteStyle(t, 'warn')}>
      当前修改尚未保存。离开将丢弃这些修改。
      <div style={{ display: 'flex', gap: 8, marginTop: 8 }}><button type="button" style={ghostButtonStyle(t)} onClick={() => { const action = pendingAction; setPendingAction(null); setDraft(null); setError(null); action(); }}>丢弃并继续</button><button type="button" style={ghostButtonStyle(t)} onClick={() => setPendingAction(null)}>继续编辑</button></div>
    </div>}
    {error && <div role="alert" style={noteStyle(t, 'danger')}>{error}</div>}
    {notice && <div role="status" style={{ ...noteStyle(t), marginBottom: 16 }}>{notice}</div>}
    {catalog.diagnostics.map((message, index) => <div key={index} role="alert" style={noteStyle(t, 'warn')}>{message}</div>)}
    {!draft ? <div className="configuration-list" style={{ display: 'grid', gap: 20 }}>
      <div role="group" aria-label="Fusion 作用域筛选" style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
        {(['all', ...FUSION_SCOPES] as const).map((value) => <button key={value} type="button" aria-pressed={filter === value}
          onClick={() => { setFilter(value); setError(null); }} style={{ ...ghostButtonStyle(t), background: filter === value ? t.surfaceActive : 'transparent' }}>{value === 'all' ? '全部' : FUSION_SCOPE_LABELS[value]}</button>)}
      </div>
      <div style={{ ...noteStyle(t), margin: 0 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center', flexWrap: 'wrap' }}>
          <span>{filter === 'all' ? '最终生效设置' : `${FUSION_SCOPE_LABELS[filter]}范围的全局设置`} · Agent 合议 {globals.enabled === undefined ? '未指定' : globals.enabled ? '开启' : '关闭'} · 主动合议 {globals.proactive === undefined ? '未指定' : globals.proactive ? '开启' : '关闭'}</span>
          {scope && <button type="button" disabled={busy || fusionNeedsLegacyReplacement(scope)} style={ghostButtonStyle(t, busy || fusionNeedsLegacyReplacement(scope))} onClick={() => setDraft({ kind: 'globals', scope: scope.scope, path: scope.path, revision: scope.revision, value: structuredClone(scope.globals), original: JSON.stringify(scope.globals) })}>编辑全局设置</button>}
        </div>
        <div style={{ marginTop: 6 }}>{filter === 'all' ? '全局限制适用于每个策略；选择具体作用域后可以编辑。' : SCOPE_DESCRIPTION[filter]}</div>
        <div style={{ marginTop: 6 }}>当前默认：{effectiveDefault ? `${effectiveDefault.strategy.name} · ${FUSION_SCOPE_LABELS[effectiveDefault.scope.scope]}` : '未设置，运行前需要选择策略'}。默认优先级：本地 → 项目 → 用户。</div>
        {scope?.default_strategy_id && <button type="button" disabled={busy || fusionNeedsLegacyReplacement(scope)} style={{ ...ghostButtonStyle(t), marginTop: 8 }} onClick={() => write('set_default', scope.scope, scope.revision)}>清除此作用域默认</button>}
      </div>
      <div className="extension-hub-list" aria-label="Fusion 策略列表" style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
        {filtered.map(({ scope: entryScope, strategy }) => <div key={fusionStrategyKey(entryScope.scope, strategy.id)} className="extension-hub-row configuration-entry">
          <span className="extension-hub-icon" aria-hidden="true"><Icon name="sparkle" size={22} /></span>
          <button type="button" className="extension-hub-row-copy extension-hub-row-open" disabled={busy} onClick={() => openStrategy(entryScope, strategy, true)}>
            <span className="extension-hub-row-title">{strategy.name}{entryScope.default_strategy_id === strategy.id && <span style={{ fontSize: 11, color: t.text3, marginLeft: 9 }}>该层默认</span>}</span>
            <span className="extension-hub-row-description">{rolesFromFusionObject(strategy.config).panels.map(routeOf).join(' · ') || '尚未添加 Panel 模型'}</span>
            <span style={{ color: t.text3, fontSize: 12, marginTop: 3 }}>{entryScope.admitted ? readiness(strategy) : '当前会话未启用此作用域'}</span>
          </button>
          <div className="extension-hub-row-side" style={{ flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <span className="extension-hub-row-scope" style={{ display: 'inline', whiteSpace: 'nowrap' }}>{FUSION_SCOPE_LABELS[entryScope.scope]}</span>
            <button type="button" disabled={busy || fusionNeedsLegacyReplacement(entryScope) || entryScope.default_strategy_id === strategy.id} aria-label={`设为默认 ${strategy.name} ${FUSION_SCOPE_LABELS[entryScope.scope]}`} style={ghostButtonStyle(t, busy || entryScope.default_strategy_id === strategy.id)} onClick={() => write('set_default', entryScope.scope, entryScope.revision, strategy.id)}>{entryScope.default_strategy_id === strategy.id ? '默认' : '设为默认'}</button>
          </div>
        </div>)}
        {!filtered.length && <div role="status" style={{ padding: '32px 18px', color: t.text3, fontSize: 13 }}>{bridge.fusionCatalogEvent ? search ? '没有匹配的策略。' : '还没有策略。新建一个策略，选择 Panel 与 Analyst 模型。' : '正在读取 Fusion 策略…'}</div>}
      </div>
    </div> : <div className="configuration-detail" style={{ display: 'grid', gap: 20, minWidth: 0 }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap' }}>
        <button type="button" style={ghostButtonStyle(t, busy)} disabled={busy} onClick={() => requestAction(() => setDraft(null))}>← 返回策略列表</button>
        <span style={{ fontSize: 12, color: t.text3 }}>{busy ? '正在保存…' : dirty ? '有未保存的修改' : '修改后点击保存'}</span>
      </div>
      <div><h3 style={{ margin: 0, fontSize: 20, color: t.text }}>{draft.kind === 'globals' ? `${FUSION_SCOPE_LABELS[draft.scope]}全局设置` : draft.existing ? draft.value.name : '新建策略'}</h3><p style={{ margin: '8px 0 0', color: t.text3, fontSize: 12.5 }}>{SCOPE_DESCRIPTION[draft.scope]}</p>{scope?.path && <p className="mono" style={{ margin: '6px 0 0', color: t.text4, fontSize: 11, overflowWrap: 'anywhere' }}>{scope.path}</p>}</div>
      {draft.kind === 'globals' ? <Card title="适用于所有 Fusion 策略">
        {([['enabled', '允许 Agent 和工作流使用 Fusion'], ['proactive', '主动使用 Fusion'], ['slash_cross_provider_default', '手动调用默认允许跨 Provider'], ['allow_cross_provider_for_agent', '允许 Agent 跨 Provider']] as const).map(([key, label]) => <Row key={key} title={label} align="center"><OptionalBoolean label={label} value={draft.value[key]} disabled={busy} onChange={(value) => setDraft({ ...draft, value: withField(draft.value, key, value) })} /></Row>)}
        <Row title="允许的 Provider" desc="每行一个 Profile ID；留空表示不增加限制。托管策略的限制仍然生效。" align="start"><textarea aria-label="允许的 Provider" value={(draft.value.allowed_profiles ?? []).join('\n')} disabled={busy} rows={4} onChange={(event) => setDraft({ ...draft, value: withField(draft.value, 'allowed_profiles', event.target.value.split('\n')) })} style={{ ...inputStyle(t), width: 280, resize: 'vertical' }} /></Row>
      </Card> : <>
        <Card title="策略信息">
          <Row title="名称" align="center"><input aria-label="策略名称" value={draft.value.name} placeholder="例如：代码审查" maxLength={120} disabled={busy} onChange={(event) => setDraft({ ...draft, value: { ...draft.value, name: event.target.value } })} style={{ ...inputStyle(t), width: 280, maxWidth: '100%' }} /></Row>
          <Row title="作用域" desc={draft.existing ? '已保存策略的作用域固定。使用下方复制功能创建独立策略。' : SCOPE_DESCRIPTION[draft.scope]} align="center"><select aria-label="策略作用域" disabled={busy || draft.existing} value={draft.scope} onChange={(event) => { const next = scopeFor(event.target.value as FusionScope); if (next) setDraft({ ...draft, scope: next.scope, path: next.path, revision: next.revision, replaceLegacy: false }); }} style={{ ...inputStyle(t), width: 160 }}>{catalog.scopes.map((entry) => <option key={entry.scope} value={entry.scope}>{FUSION_SCOPE_LABELS[entry.scope]}</option>)}</select></Row>
          <Row title="默认预设" desc="Quality 侧重质量，Fast 侧重速度；均按 Panel 列表顺序选择模型。" align="center"><select aria-label="默认预设" disabled={busy} value={typeof config.preset === 'string' ? config.preset : ''} onChange={(event) => updateConfig('preset', event.target.value || undefined)} style={{ ...inputStyle(t), width: 160 }}><option value="">引擎默认</option><option value="quality">Quality</option><option value="fast">Fast</option></select></Row>
        </Card>
        {scope?.diagnostic && <div role="alert" style={noteStyle(t, 'warn')}>{scope.diagnostic}{!needsLegacyReplacement && <p style={{ margin: '10px 0 0' }}>修正当前策略后保存，其他策略和此作用域的设置会保留。</p>}{needsLegacyReplacement && <label style={{ display: 'flex', gap: 8, marginTop: 10, alignItems: 'start' }}><input type="checkbox" checked={draft.replaceLegacy} onChange={(event) => setDraft({ ...draft, replaceLegacy: event.target.checked })} />我确认用此新策略替换该作用域的旧 Fusion 配置（旧单份配置不会迁移）。</label>}</div>}
        <div role="status" style={noteStyle(t)}>{draft.existing && !scope?.admitted ? '当前会话未启用此作用域；可以编辑，启用该作用域后才能运行' : readiness(draft.value)}。最终回答继续使用当前会话模型，无需另设合成模型。</div>
        <Card title="Panel 模型">
          <Row title="顺序即优先级" desc={`至少 ${FUSION_MIN_PANEL} 个，最多 ${maxPanel} 个模型。预设取列表中的前 N 项。`} align="start">{null}</Row>
          {roles.panels.map((choice, index) => <Row key={`${index}:${routeOf(choice)}`} title={<span className="mono" style={{ fontSize: 12.5, overflowWrap: 'anywhere' }}>{index + 1}. {routeOf(choice)}</span>} align="center"><div style={{ display: 'flex', gap: 6 }}><button type="button" disabled={busy || index === 0} aria-label={`上移 ${routeOf(choice)}`} style={ghostButtonStyle(t, busy || index === 0)} onClick={() => movePanel(index, -1)}>↑</button><button type="button" disabled={busy || index === roles.panels.length - 1} aria-label={`下移 ${routeOf(choice)}`} style={ghostButtonStyle(t, busy || index === roles.panels.length - 1)} onClick={() => movePanel(index, 1)}>↓</button><button type="button" disabled={busy} aria-label={`移除 ${routeOf(choice)}`} style={ghostButtonStyle(t)} onClick={() => updateConfig('panelModels', roles.panels.filter((_, i) => i !== index))}>移除</button></div></Row>)}
          <Row title="添加 Panel 模型" align="center"><div style={{ display: 'flex', gap: 7, flexWrap: 'wrap', maxWidth: '100%' }}><ChoiceSelect value={pendingPanel} rows={panelCandidates} placeholder="选择模型…" onPick={setPendingPanel} disabled={busy || roles.panels.length >= maxPanel} /><button type="button" disabled={busy || !pendingPanel || !panelCandidates.some((row) => routeOf(row.choice) === routeOf(pendingPanel)) || roles.panels.length >= maxPanel} style={ghostButtonStyle(t, busy || !pendingPanel)} onClick={() => { if (!pendingPanel || !panelCandidates.some((row) => routeOf(row.choice) === routeOf(pendingPanel))) return; updateConfig('panelModels', [...roles.panels, pendingPanel]); setPendingPanel(null); }}>添加</button></div></Row>
        </Card>
        <Card title="Analyst 模型"><Row title="评审 Panel 输出" desc="仅显示已连接且支持 Fusion 结构化评审的模型。" align="center"><ChoiceSelect value={roles.analyst} rows={analystCandidates} placeholder="选择 analyst…" onPick={(value) => updateConfig('analystModel', value ?? undefined)} disabled={busy} /></Row></Card>
        <details style={{ border: `1px solid ${t.border}`, borderRadius: 12, minWidth: 0 }}><summary style={{ padding: 16, fontSize: 13, color: t.text2, cursor: 'pointer' }}>高级执行参数</summary><p style={{ padding: '0 18px', color: t.text3, fontSize: 12 }}>留空使用引擎默认值。每个策略独立保存，不继承其他策略的参数。</p>
          <ObjectNumberFields fields={NUMBER_FIELDS} value={config} disabled={busy} onChange={updateConfig} />
          <Row title="允许部分模型失败" align="center"><OptionalBoolean label="允许部分模型失败" value={config.partialOk} disabled={busy} onChange={(value) => updateConfig('partialOk', value)} /></Row>
          <Row title="Analyst 只读工具" align="center"><OptionalBoolean label="Analyst 只读工具" value={config.analystTools} disabled={busy} onChange={(value) => updateConfig('analystTools', value)} /></Row>
          <Row title="完成条件" align="center"><select aria-label="完成条件" value={typeof config.completionPolicy === 'string' ? config.completionPolicy : ''} disabled={busy} onChange={(event) => updateConfig('completionPolicy', event.target.value || undefined)} style={{ ...inputStyle(t), width: 200 }}><option value="">引擎默认</option><option value="wait_all">等待全部</option><option value="quorum_after_grace">达到成功数量后提前完成</option></select></Row>
          <details style={{ margin: 16 }}><summary style={{ padding: '8px 0', color: t.text2, fontSize: 13, cursor: 'pointer' }}>实现模式</summary>
            <ObjectNumberFields fields={IMPLEMENT_FIELDS} value={implement} disabled={busy} onChange={(key, value) => updateConfig('implement', withField(implement, key, value))} />
            <Row title="验证命令" desc="每行一条命令，依次在各 Panel 工作树中执行。" align="start"><textarea aria-label="验证命令" rows={4} value={Array.isArray(implement.verifyCommands) ? implement.verifyCommands.join('\n') : ''} disabled={busy} onChange={(event) => updateConfig('implement', withField(implement, 'verifyCommands', event.target.value ? event.target.value.split('\n') : undefined))} style={{ ...inputStyle(t), width: 280, resize: 'vertical' }} /></Row>
          </details>
        </details>
        {draft.existing && <Card title="管理策略"><Row title="复制到其他作用域" desc="创建一个新 ID，原策略保持独立。" align="center"><div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}><select aria-label="复制目标作用域" value={copyScope} disabled={busy} onChange={(event) => setCopyScope(event.target.value as FusionScope)} style={inputStyle(t)}>{catalog.scopes.filter((entry) => entry.scope !== draft.scope).map((entry) => <option key={entry.scope} value={entry.scope}>{FUSION_SCOPE_LABELS[entry.scope]}</option>)}</select><button type="button" disabled={busy || !scopeFor(copyScope)} style={ghostButtonStyle(t)} onClick={() => { const target = scopeFor(copyScope); if (target) requestAction(() => openStrategy(target, copiedFusionStrategy(scope?.strategies.find((entry) => entry.id === draft.value.id) ?? draft.value, crypto.randomUUID()), false)); }}>复制策略</button></div></Row><Row title="删除策略" desc="若它是该层默认策略，删除时会同时清除默认引用。" align="center"><button type="button" disabled={busy} style={ghostButtonStyle(t, busy, true)} onClick={() => setConfirmDelete(true)}>删除策略</button></Row>{confirmDelete && <div role="alert" style={{ ...noteStyle(t, 'warn'), margin: 16 }}>确认删除“{draft.value.name}”？<div style={{ display: 'flex', gap: 8, marginTop: 8 }}><button type="button" disabled={busy} style={ghostButtonStyle(t, busy, true)} onClick={() => write('delete', draft.scope, draft.revision, draft.value.id, undefined, true)}>确认删除</button><button type="button" style={ghostButtonStyle(t)} onClick={() => setConfirmDelete(false)}>保留策略</button></div></div>}</Card>}
      </>}
      <div style={{ display: 'flex', justifyContent: 'flex-end', flexWrap: 'wrap', gap: 8, padding: '4px 0 20px' }}>
        <button type="button" disabled={busy} style={ghostButtonStyle(t, busy)} onClick={() => requestAction(() => setDraft(null))}>取消</button>
        <button type="button" className="extension-hub-primary" disabled={busy || (!dirty && (draft.kind === 'globals' || draft.existing)) || (draft.kind === 'strategy' && (!draft.value.name.trim() || (needsLegacyReplacement && !draft.replaceLegacy)))} onClick={save}>{busy ? '正在保存…' : '保存'}</button>
      </div>
    </div>}
  </ExtensionHubLayout>;
}
