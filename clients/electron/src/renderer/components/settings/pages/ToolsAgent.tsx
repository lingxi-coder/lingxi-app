import { useState } from 'react';
import { Card, MergedBadge, MergedNotice, OverriddenNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Toggle } from '../primitives';
import type { PageContentProps } from '../SettingsScreen';
import { isEditableLayer } from './CustomProviders';
import { rowState, type SettingsSnapshot } from '../useEngineSettings';
import { ghostButtonStyle } from './ghostButton';

/**
 * One key's raw value in `editingLayer`'s OWN settings map — never
 * `snapshot.effective` (the cross-layer merge). Same reasoning as
 * `CustomProviders.providersFromLayer`: `update_settings` replaces a key
 * WHOLESALE in one layer's file, so basing a write on the merged view would
 * fork other layers' contributions into whichever layer this page saves.
 * Generic over every field on this page rather than one function per field,
 * since none of them need bespoke parsing beyond a type guard.
 */
function layerValue(snapshot: SettingsSnapshot | null, layer: string, key: string): unknown {
  return snapshot?.layers?.[layer]?.[key];
}

export function boolFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): boolean {
  return layerValue(snapshot, layer, key) === true;
}

export function stringFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): string {
  const value = layerValue(snapshot, layer, key);
  return typeof value === 'string' ? value : '';
}

export function stringArrayFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): string[] {
  const value = layerValue(snapshot, layer, key);
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
}

export function stringMapFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): Record<string, string> {
  const value = layerValue(snapshot, layer, key);
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === 'string') out[k] = v;
  }
  return out;
}

/** `"a, b\nc"` → `['a','b','c']`, blank entries dropped — same shape as `CustomProviders.parseModelsInput`. */
export function parseToolList(text: string): string[] {
  return text.split(/[,\n]/).map((entry) => entry.trim()).filter((entry) => entry.length > 0);
}

const inputStyle = (t: ReturnType<typeof useT>) => ({
  padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`,
  background: t.surface, color: t.text, fontSize: 12.5, fontFamily: 'inherit',
} as const);

function ProvenanceNotice({
  snapshot, keyName, editingLayer, onJumpToLayer,
}: { snapshot: SettingsSnapshot | null; keyName: string; editingLayer: PageContentProps['editingLayer']; onJumpToLayer: PageContentProps['onJumpToLayer'] }) {
  const state = snapshot ? rowState(snapshot, keyName, editingLayer) : null;
  if (state?.kind === 'merged') {
    return <Row title="生效层" badge={<MergedBadge />} align="center"><MergedNotice editingLayer={editingLayer} /></Row>;
  }
  if (state?.kind === 'overridden') {
    return (
      <Row title="生效层" align="center">
        <OverriddenNotice
          editingLayer={editingLayer}
          effectiveLayer={state.by}
          onJump={() => { if (isEditableLayer(state.by)) onJumpToLayer(state.by); }}
        />
      </Row>
    );
  }
  return null;
}

function BoolRow({
  title, desc, value, saving, onChange,
}: { title: string; desc: string; value: boolean; saving: boolean; onChange(next: boolean): void }) {
  return (
    <Row title={title} desc={desc} align="center">
      <Toggle value={value} onChange={saving ? () => undefined : onChange} />
    </Row>
  );
}

/**
 * Edits the settings keys that gate tool availability and agent-loop
 * behaviour (`enabledTools`, the `disable*` flags, `outputStyle`,
 * `modelOverrides`, thinking/vision delegation toggles,
 * `skipWebFetchPreflight`). All writes go through the generic
 * `update_settings` command (`bridge.updateEngineSettings`) — none of these
 * keys have a dedicated command the way `permissions` does. Every read
 * comes from `snapshot.layers[editingLayer]`, never `snapshot.effective`,
 * for the same reason `CustomProviders` does — see `layerValue` above.
 */
export function ToolsAgent({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const enabledTools = stringArrayFromLayer(snapshot, editingLayer, 'enabledTools');
  const outputStyle = stringFromLayer(snapshot, editingLayer, 'outputStyle');
  const modelOverrides = stringMapFromLayer(snapshot, editingLayer, 'modelOverrides');
  const disableArtifact = boolFromLayer(snapshot, editingLayer, 'disableArtifact');
  const disableAgentView = boolFromLayer(snapshot, editingLayer, 'disableAgentView');
  const disableAllHooks = boolFromLayer(snapshot, editingLayer, 'disableAllHooks');
  const alwaysThinkingEnabled = boolFromLayer(snapshot, editingLayer, 'alwaysThinkingEnabled');
  const showThinkingSummaries = boolFromLayer(snapshot, editingLayer, 'showThinkingSummaries');
  const visionDelegationEnabled = boolFromLayer(snapshot, editingLayer, 'visionDelegationEnabled');
  const skipWebFetchPreflight = boolFromLayer(snapshot, editingLayer, 'skipWebFetchPreflight');

  const [toolsText, setToolsText] = useState(enabledTools.join(', '));
  const [outputStyleText, setOutputStyleText] = useState(outputStyle);
  const [overrideFrom, setOverrideFrom] = useState('');
  const [overrideTo, setOverrideTo] = useState('');
  const [saving, setSaving] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  const write = (patch: Record<string, unknown>, key: string) => {
    setSaving(key);
    setSaveError(null);
    void bridge.updateEngineSettings(editingLayer, patch)
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存设置。'))
      .finally(() => setSaving(null));
  };

  return (
    <>
      <Card title="启用的工具 (enabledTools)">
        <ProvenanceNotice snapshot={snapshot} keyName="enabledTools" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        <Row title="enabledTools" desc="留空表示不限制；按名称列出工具会把可用工具限制在这个列表内。此键跨层做并集去重，不是覆盖。" align="start">
          <div style={{ display: 'grid', gap: 7 }}>
            <input
              value={toolsText}
              onChange={(e) => setToolsText(e.target.value)}
              placeholder="Bash, Read, Edit, …"
              aria-label="enabledTools"
              style={{ ...inputStyle(t), width: 320 }}
            />
            <button
              type="button"
              disabled={saving === 'enabledTools'}
              onClick={() => write({ enabledTools: parseToolList(toolsText) }, 'enabledTools')}
              style={ghostButtonStyle(t, saving === 'enabledTools')}
            >
              保存
            </button>
          </div>
        </Row>
      </Card>

      <Card title="内建工具开关">
        <BoolRow title="disableArtifact" desc="禁用 Artifact 工具。" value={disableArtifact} saving={saving === 'disableArtifact'} onChange={(v) => write({ disableArtifact: v }, 'disableArtifact')} />
        <BoolRow title="disableAgentView" desc="禁用 Agent 视图工具。" value={disableAgentView} saving={saving === 'disableAgentView'} onChange={(v) => write({ disableAgentView: v }, 'disableAgentView')} />
        <BoolRow title="disableAllHooks" desc="禁用所有 hooks，无论它们在哪一层定义。" value={disableAllHooks} saving={saving === 'disableAllHooks'} onChange={(v) => write({ disableAllHooks: v }, 'disableAllHooks')} />
        <BoolRow title="skipWebFetchPreflight" desc="跳过 WebFetch 的预检请求。" value={skipWebFetchPreflight} saving={saving === 'skipWebFetchPreflight'} onChange={(v) => write({ skipWebFetchPreflight: v }, 'skipWebFetchPreflight')} />
      </Card>

      <Card title="Agent 行为">
        <BoolRow title="alwaysThinkingEnabled" desc="总是启用扩展思考。" value={alwaysThinkingEnabled} saving={saving === 'alwaysThinkingEnabled'} onChange={(v) => write({ alwaysThinkingEnabled: v }, 'alwaysThinkingEnabled')} />
        <BoolRow title="showThinkingSummaries" desc="展示模型思考过程的摘要。" value={showThinkingSummaries} saving={saving === 'showThinkingSummaries'} onChange={(v) => write({ showThinkingSummaries: v }, 'showThinkingSummaries')} />
        <BoolRow title="visionDelegationEnabled" desc="允许把图像理解委派给视觉模型。" value={visionDelegationEnabled} saving={saving === 'visionDelegationEnabled'} onChange={(v) => write({ visionDelegationEnabled: v }, 'visionDelegationEnabled')} />
      </Card>

      <Card title="输出风格 (outputStyle)">
        <Row title="outputStyle" desc="标量字段，后写入的层直接覆盖，不参与合并。" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input value={outputStyleText} onChange={(e) => setOutputStyleText(e.target.value)} placeholder="Explanatory" aria-label="outputStyle" style={inputStyle(t)} />
            <button type="button" disabled={saving === 'outputStyle'} onClick={() => write({ outputStyle: outputStyleText.trim() || null }, 'outputStyle')} style={ghostButtonStyle(t, saving === 'outputStyle')}>保存</button>
          </div>
        </Row>
      </Card>

      <Card title="模型覆盖 (modelOverrides)">
        <ProvenanceNotice snapshot={snapshot} keyName="modelOverrides" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {Object.keys(modelOverrides).length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有模型覆盖。</div>
        )}
        {Object.entries(modelOverrides).map(([from, to]) => (
          <Row key={from} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{from} → {to}</span>}>
            <button
              type="button"
              disabled={saving === 'modelOverrides'}
              onClick={() => {
                const next = { ...modelOverrides };
                delete next[from];
                write({ modelOverrides: next }, 'modelOverrides');
              }}
              style={ghostButtonStyle(t, saving === 'modelOverrides', true)}
            >
              移除
            </button>
          </Row>
        ))}
        <Row title="新增覆盖" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input value={overrideFrom} onChange={(e) => setOverrideFrom(e.target.value)} placeholder="原模型 id" aria-label="原模型 id" style={inputStyle(t)} />
            <input value={overrideTo} onChange={(e) => setOverrideTo(e.target.value)} placeholder="替换为" aria-label="替换为" style={inputStyle(t)} />
            <button
              type="button"
              disabled={saving === 'modelOverrides' || !overrideFrom.trim() || !overrideTo.trim()}
              onClick={() => {
                write({ modelOverrides: { ...modelOverrides, [overrideFrom.trim()]: overrideTo.trim() } }, 'modelOverrides');
                setOverrideFrom('');
                setOverrideTo('');
              }}
              style={ghostButtonStyle(t, saving === 'modelOverrides' || !overrideFrom.trim() || !overrideTo.trim())}
            >
              添加
            </button>
          </div>
        </Row>
      </Card>

      {saveError && <div role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{saveError}</div>}
    </>
  );
}
