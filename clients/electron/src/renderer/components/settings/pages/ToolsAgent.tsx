import { useEffect, useState } from 'react';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Toggle } from '../primitives';
import type { PageContentProps } from '../SettingsScreen';
import { boolFromLayer, stringArrayFromLayer, stringFromLayer, stringMapFromLayer } from '../layerFields';
import { ghostButtonStyle, inputStyle } from './ghostButton';

/** `"a, b\nc"` → `['a','b','c']`, blank entries dropped — same shape as `CustomProviders.parseModelsInput`. */
export function parseToolList(text: string): string[] {
  return text.split(/[,\n]/).map((entry) => entry.trim()).filter((entry) => entry.length > 0);
}

// Re-exported so callers (and `settings-coding-pages.test.ts`) that used to
// import these FROM this file keep working now that the canonical
// definitions live in `../layerFields` (Task 18 fix round 1, Minor — five
// pages had grown their own copy of this exact gate).
export { boolFromLayer, stringArrayFromLayer, stringFromLayer, stringMapFromLayer };

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
 * for the same reason `CustomProviders` does — see `../layerFields`.
 *
 * **Task 18 fix round 1, Critical**: `toolsText`/`outputStyleText` are
 * text-editor state SEEDED from the editing layer's value — exactly the
 * shape `CustomProviders.tsx` already carries a fix and a comment for
 * (Task 17 fix round 1), except there `useState` only ever reads its
 * initializer ONCE. Switching `editingLayer` re-runs this component with a
 * new `enabledTools`/`outputStyle` (correctly re-derived from the new
 * layer), but a re-render does NOT re-run `useState`'s initializer, and
 * `SettingsScreen.tsx` mounts this component with no `key` — so the OLD
 * layer's text just sits there while the card now claims to be editing a
 * DIFFERENT layer. Saving then writes the stale text into the new layer:
 * for `enabledTools` (`ConcatDedup`) that's a value now duplicated across
 * two layers with no single delete that removes it. The effect below
 * re-seeds both fields whenever `editingLayer` changes, mirroring
 * `CustomProviders`' own `useEffect([editingLayer])` exactly.
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

  // The Critical fix: re-seed every draft from the NEWLY selected layer's
  // own value whenever `editingLayer` changes, so a save can never carry a
  // previous layer's text into the one now selected. Deliberately NOT
  // depending on `enabledTools`/`outputStyle` themselves — those are
  // recomputed on every snapshot refresh (including right after THIS page's
  // own save), and resetting the draft then would fight typing/clobber an
  // in-flight edit on the layer the user is actually still on.
  useEffect(() => {
    setToolsText(enabledTools.join(', '));
    setOutputStyleText(outputStyle);
    setOverrideFrom('');
    setOverrideTo('');
    setSaveError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingLayer]);

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
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="enabledTools" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
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
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="modelOverrides" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
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

      <Card title="运行时 Agent Catalog">
        <Row title="刷新 Catalog" desc="展示当前引擎已加载的 agent 定义。" align="center">
          <button type="button" onClick={() => void bridge.refreshAgents()} style={ghostButtonStyle(t)}>刷新</button>
        </Row>
        {bridge.agentCatalog.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>当前 runtime 没有加载任何 agent。</div>
        )}
        {bridge.agentCatalog.map((agent) => (
          <Row
            key={agent.name}
            title={agent.name}
            desc={`${agent.description || '无描述'}${agent.tools_allowed.length ? ` · tools: ${agent.tools_allowed.join(', ')}` : ''}`}
            align="center"
          >
            {null}
          </Row>
        ))}
      </Card>

      {saveError && <div role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{saveError}</div>}
    </>
  );
}
