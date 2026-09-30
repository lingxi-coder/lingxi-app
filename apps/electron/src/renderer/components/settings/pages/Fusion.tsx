import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Icon } from '../../Icon';
import type { ModelDetailsDto } from '@lingxi/bridge-client';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Toggle } from '../primitives';
import type { PageContentProps } from '../SettingsScreen';
import { objectFromLayer } from '../layerFields';
import { ghostButtonStyle, inputStyle } from './ghostButton';

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

/** 读出某一层自己写的 `fusion` 对象里的三个模型角色。格式不对的条目读作「未配置」。 */
export function rolesFromFusionObject(fusion: Record<string, unknown>): {
  panels: FusionModelChoice[];
  analyst: FusionModelChoice | null;
  synthesizer: FusionModelChoice | null;
} {
  const raw = fusion.panelModels;
  return {
    panels: Array.isArray(raw) ? raw.filter(isChoice) : [],
    analyst: isChoice(fusion.analystModel) ? fusion.analystModel : null,
    synthesizer: isChoice(fusion.synthesizerModel) ? fusion.synthesizerModel : null,
  };
}

/** 还缺哪些角色。顺序与引擎 `FusionModelRole::ALL` 一致。 */
export function missingRoles(roles: ReturnType<typeof rolesFromFusionObject>): string[] {
  const missing: string[] = [];
  if (roles.panels.length < FUSION_MIN_PANEL) missing.push('panel 模型');
  if (!roles.analyst) missing.push('analyst 模型');
  if (!roles.synthesizer) missing.push('synthesizer 模型');
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

/**
 * 编辑 `fusion` 设置里的三个模型角色（`panelModels` / `analystModel` /
 * `synthesizerModel`）以及主开关 `fusion.enabled`。
 *
 * Fusion 没有自动选择回落：引擎要求三个角色都被显式配置，否则运行前就以
 * `FusionError::NotConfigured` 拒绝。所以这一页的职责不是「微调」，而是把
 * 配置补齐——顶部的缺失提示就是这个意思。
 *
 * 写入走通用的 `update_settings`。它是**整键替换**，所以每次写入都必须基于
 * 「当前层自己的 `fusion` 对象」做浅合并，否则同层里的 `fusion.maxPanel` 等
 * 其它键会被这一页悄悄抹掉（与 `CustomProviders` 的 Task 17 缺陷同形）。
 */
export function Fusion({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const fusion = objectFromLayer(snapshot, editingLayer, 'fusion');
  const roles = rolesFromFusionObject(fusion);
  const enabled = fusion.enabled === true;
  const maxPanel = typeof fusion.maxPanel === 'number' ? fusion.maxPanel : FUSION_MAX_PANEL;

  const [saving, setSaving] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [pendingPanel, setPendingPanel] = useState<FusionModelChoice | null>(null);

  const candidates = useMemo(
    () => configuredCandidates(candidatesFromCatalog(bridge.desktop.providerModelCatalog ?? []), bridge.bootstrap?.providerCredentials),
    [bridge.desktop.providerModelCatalog, bridge.bootstrap?.providerCredentials],
  );
  const analystCandidates = useMemo(
    () => candidates.filter((row) => row.analystCapable),
    [candidates],
  );
  const missing = missingRoles(roles);
  const pendingPanelAvailable = pendingPanel !== null && candidates.some((row) => routeOf(row.choice) === routeOf(pendingPanel));

  /** 浅合并进当前层自己的 `fusion` 对象后整键写回。 */
  const write = (patch: Record<string, unknown>, key: string) => {
    setSaving(key);
    setSaveError(null);
    void bridge.updateEngineSettings(editingLayer, { fusion: { ...fusion, ...patch } })
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存设置。'))
      .finally(() => setSaving(null));
  };

  const writePanels = (next: FusionModelChoice[], key: string) => {
    write({ panelModels: next.length > 0 ? next : undefined }, key);
  };

  const move = (index: number, delta: number) => {
    const next = [...roles.panels];
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    writePanels(next, 'panelModels');
  };

  return (
    <>
      {missing.length > 0 && (
        <div
          role="status"
          style={{
            marginBottom: 20, padding: '11px 14px', borderRadius: 10,
            border: `0.5px solid ${t.border}`, background: t.surfaceActive,
            color: t.text2, fontSize: 12.5, lineHeight: 1.6,
          }}
        >
          Fusion 还不能运行：缺少 {missing.join('、')}。
          三个角色都配好之前，<span className="mono">/fusion</span> 会在发出任何请求之前就被拒绝。
        </div>
      )}

      <Card title="主开关 (fusion.enabled)">
        <Row
          title="fusion.enabled"
          desc="开启后 Fusion 才会出现在 Agent 列表里、并允许工作流调用 fusion()。/fusion 斜杠命令与这个开关无关，它始终是逐次显式调用。"
          align="center"
        >
          <Toggle
            value={enabled}
            onChange={saving === 'enabled' ? () => undefined : (v) => write({ enabled: v }, 'enabled')}
            label="fusion.enabled"
          />
        </Row>
      </Card>

      <Card title="Panel 模型 (fusion.panelModels)">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="fusion" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        <Row
          title="顺序即优先级"
          desc={`quality / fast 预设取这个列表的前 N 项，所以顺序有意义。至少 ${FUSION_MIN_PANEL} 个，最多 ${maxPanel} 个（fusion.maxPanel）。`}
          align="start"
        >
          {null}
        </Row>
        {roles.panels.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有选择任何 panel 模型。</div>
        )}
        {roles.panels.map((choice, index) => (
          <Row
            key={routeOf(choice)}
            align="center"
            title={<span className="mono" style={{ fontSize: 12.5 }}>{index + 1}. {routeOf(choice)}</span>}
          >
            <div style={{ display: 'flex', gap: 7 }}>
              <button type="button" disabled={saving !== null || index === 0} onClick={() => move(index, -1)} style={ghostButtonStyle(t, saving !== null || index === 0)} aria-label={`上移 ${routeOf(choice)}`}>↑</button>
              <button type="button" disabled={saving !== null || index === roles.panels.length - 1} onClick={() => move(index, 1)} style={ghostButtonStyle(t, saving !== null || index === roles.panels.length - 1)} aria-label={`下移 ${routeOf(choice)}`}>↓</button>
              <button
                type="button"
                disabled={saving !== null}
                onClick={() => writePanels(roles.panels.filter((_, i) => i !== index), 'panelModels')}
                style={ghostButtonStyle(t, saving !== null, true)}
              >
                移除
              </button>
            </div>
          </Row>
        ))}
        <Row title="添加 panel 模型" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <ChoiceSelect
              value={pendingPanel}
              rows={candidates.filter((row) => !roles.panels.some((picked) => routeOf(picked) === routeOf(row.choice)))}
              placeholder="选择模型…"
              onPick={setPendingPanel}
              disabled={saving !== null || roles.panels.length >= maxPanel}
            />
            <button
              type="button"
              disabled={saving !== null || !pendingPanelAvailable || roles.panels.length >= maxPanel}
              onClick={() => {
                if (!pendingPanel || !pendingPanelAvailable) return;
                writePanels([...roles.panels, pendingPanel], 'panelModels');
                setPendingPanel(null);
              }}
              style={ghostButtonStyle(t, saving !== null || !pendingPanelAvailable || roles.panels.length >= maxPanel)}
            >
              添加
            </button>
          </div>
        </Row>
        {roles.panels.length >= maxPanel && (
          <div style={{ padding: '10px 18px', color: t.text4, fontSize: 12 }}>
            已达 fusion.maxPanel（{maxPanel}）。要再加就先移除一个，或调高 maxPanel。
          </div>
        )}
      </Card>

      <Card title="Analyst 模型 (fusion.analystModel)">
        <Row
          title="评审并打分的模型"
          desc="必须支持结构化输出（JSON schema）。下拉里只列出真正能在自己 provider 上编码 response_format 的路由——否则失败会发生在所有 panel 花完钱之后。"
          align="center"
        >
          <ChoiceSelect
            value={roles.analyst}
            rows={analystCandidates}
            placeholder="选择 analyst…"
            onPick={(choice) => write({ analystModel: choice ?? undefined }, 'analystModel')}
            disabled={saving !== null}
          />
        </Row>
        {analystCandidates.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>
            当前已连接的 provider 里没有能做结构化输出的模型。先连一个可以的 provider。
          </div>
        )}
      </Card>

      <Card title="Synthesizer 模型 (fusion.synthesizerModel)">
        <Row
          title="合成最终答案的模型"
          desc="读 analyst 的分析并写出你看到的那份答案。通常就选你平时对话用的模型。"
          align="center"
        >
          <ChoiceSelect
            value={roles.synthesizer}
            rows={candidates}
            placeholder="选择 synthesizer…"
            onPick={(choice) => write({ synthesizerModel: choice ?? undefined }, 'synthesizerModel')}
            disabled={saving !== null}
          />
        </Row>
      </Card>

      {saveError && <div role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{saveError}</div>}
    </>
  );
}
