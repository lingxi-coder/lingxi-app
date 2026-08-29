import { Card, MergedBadge, MergedNotice, OverriddenNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { isEditableLayer } from './CustomProviders';
import { rowState } from '../useEngineSettings';
import { ghostButtonStyle } from './ghostButton';

export interface HookEventSummary {
  event: string;
  count: number;
}

export interface HooksPageModel {
  editable: false;
  escapeHatch: 'raw-json';
  events: HookEventSummary[];
}

/**
 * Hooks are read-only on this page by deliberate scope decision (Task 18
 * brief: "a structured editor is out of scope and was ruled so
 * deliberately" — hooks are nested JSON: `{ [eventName]: [{ matcher,
 * hooks: [...] }] }`, and a form editor for that shape would be its own
 * substantial task). `editable`/`escapeHatch` are literal types, not
 * booleans/strings computed from input, so a future edit that tries to make
 * this page writable has to change the TYPE, which a test can catch —
 * pinned per the brief's own test text.
 */
export function hooksPageModel(effective: Record<string, unknown>): HooksPageModel {
  const hooks = effective['hooks'];
  const events: HookEventSummary[] = hooks && typeof hooks === 'object' && !Array.isArray(hooks)
    ? Object.entries(hooks as Record<string, unknown>).map(([event, groups]) => ({
        event,
        count: Array.isArray(groups) ? groups.length : 0,
      }))
    : [];
  return { editable: false, escapeHatch: 'raw-json', events };
}

/**
 * `hooks` is layered per `nav.ts` (it lives in the settings files, unlike
 * MCP) AND a `DeepMerge` key (`engine/src/settings/schema.rs`'s
 * `MERGE_STRATEGIES`) — so what actually runs is a cross-layer combination,
 * not any one layer's own list. This page therefore reads
 * `snapshot.effective` (what's actually active), NOT
 * `snapshot.layers[editingLayer]` — there is nothing to write here, so the
 * "which layer would a write land in" question `CustomProviders`/
 * `Permissions`/`ToolsAgent` all have to answer does not apply; only "what
 * is currently active" does.
 */
export function Hooks({ snapshot, editingLayer, onNavigate, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const model = hooksPageModel(snapshot?.effective ?? {});
  const hooksRowState = snapshot ? rowState(snapshot, 'hooks', editingLayer) : null;

  return (
    <Card title="Hooks（只读）">
      <div style={{ padding: '10px 18px 0', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
        Hooks 是嵌套 JSON，这个页面只做罗列，不提供编辑；需要修改时请跳转到原始 JSON 页面。
      </div>
      {hooksRowState?.kind === 'merged' && (
        <Row title="生效值" badge={<MergedBadge />} align="center"><MergedNotice editingLayer={editingLayer} /></Row>
      )}
      {hooksRowState?.kind === 'overridden' && (
        <Row title="生效值" align="center">
          <OverriddenNotice
            editingLayer={editingLayer}
            effectiveLayer={hooksRowState.by}
            onJump={() => { if (isEditableLayer(hooksRowState.by)) onJumpToLayer(hooksRowState.by); }}
          />
        </Row>
      )}
      {model.events.length === 0 && (
        <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>没有配置任何 hook。</div>
      )}
      {model.events.map((entry) => (
        <Row key={entry.event} align="center" title={entry.event} desc={`${entry.count} 条匹配规则`}>{null}</Row>
      ))}
      <Row title="在 JSON 中编辑" desc="跳转到当前层的原始 JSON 页面。" align="center">
        <button type="button" onClick={() => onNavigate('raw-json')} style={ghostButtonStyle(t)}>打开原始 JSON</button>
      </Row>
    </Card>
  );
}
