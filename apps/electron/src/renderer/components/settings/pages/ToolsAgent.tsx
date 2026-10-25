import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
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
      <Toggle value={value} onChange={onChange} label={title} disabled={saving} />
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
  useI18n();
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

  const draftOwner = useRef({ layer: editingLayer, initialized: snapshot !== null });
  useEffect(() => {
    if (draftOwner.current.layer === editingLayer && draftOwner.current.initialized) return;
    draftOwner.current = { layer: editingLayer, initialized: snapshot !== null };
    if (!snapshot) return;
    setToolsText(enabledTools.join(', '));
    setOutputStyleText(outputStyle);
    setOverrideFrom('');
    setOverrideTo('');
    setSaveError(null);
  }, [editingLayer, snapshot, enabledTools, outputStyle]);

  const write = (patch: Record<string, unknown>, key: string) => {
    setSaving(key);
    setSaveError(null);
    void bridge.updateEngineSettings(editingLayer, patch)
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : tr('desktop_settings_50fe1f20b6')))
      .finally(() => setSaving(null));
  };

  return (
    <>
      <Card title={tr('desktop_settings_6fac5943aa')}>
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="enabledTools" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        <Row title="enabledTools" desc={tr('desktop_settings_34d18f886d')} align="start">
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
              disabled={!snapshot || saving === 'enabledTools'}
              onClick={() => write({ enabledTools: parseToolList(toolsText) }, 'enabledTools')}
              style={ghostButtonStyle(t, saving === 'enabledTools')}
            >
              {tr('settings_save')}
            </button>
          </div>
        </Row>
      </Card>

      <Card title={tr('desktop_settings_d08bf2421b')}>
        <BoolRow title="disableArtifact" desc={tr('desktop_settings_7ef868c639')} value={disableArtifact} saving={saving === 'disableArtifact'} onChange={(v) => write({ disableArtifact: v }, 'disableArtifact')} />
        <BoolRow title="disableAgentView" desc={tr('desktop_settings_c4142ed999')} value={disableAgentView} saving={saving === 'disableAgentView'} onChange={(v) => write({ disableAgentView: v }, 'disableAgentView')} />
        <BoolRow title="disableAllHooks" desc={tr('desktop_settings_cc32c17384')} value={disableAllHooks} saving={saving === 'disableAllHooks'} onChange={(v) => write({ disableAllHooks: v }, 'disableAllHooks')} />
        <BoolRow title="skipWebFetchPreflight" desc={tr('desktop_settings_4272f3edbc')} value={skipWebFetchPreflight} saving={saving === 'skipWebFetchPreflight'} onChange={(v) => write({ skipWebFetchPreflight: v }, 'skipWebFetchPreflight')} />
      </Card>

      <Card title={tr('desktop_settings_064a31e9f1')}>
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="promptSuggestionEnabled" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        <BoolRow title={tr('desktop_settings_prompt_suggestions')} desc={tr('desktop_settings_9eab68f6d4')} value={snapshot?.layers[editingLayer]?.promptSuggestionEnabled !== false} saving={saving === 'promptSuggestionEnabled'} onChange={(v) => write({ promptSuggestionEnabled: v }, 'promptSuggestionEnabled')} />
        <BoolRow title="alwaysThinkingEnabled" desc={tr('desktop_settings_6e694a1522')} value={alwaysThinkingEnabled} saving={saving === 'alwaysThinkingEnabled'} onChange={(v) => write({ alwaysThinkingEnabled: v }, 'alwaysThinkingEnabled')} />
        <BoolRow title="showThinkingSummaries" desc={tr('desktop_settings_d8c4cb8eb1')} value={showThinkingSummaries} saving={saving === 'showThinkingSummaries'} onChange={(v) => write({ showThinkingSummaries: v }, 'showThinkingSummaries')} />
        <BoolRow title="visionDelegationEnabled" desc={tr('desktop_settings_da3c948583')} value={visionDelegationEnabled} saving={saving === 'visionDelegationEnabled'} onChange={(v) => write({ visionDelegationEnabled: v }, 'visionDelegationEnabled')} />
      </Card>

      <Card title={tr('desktop_settings_8704130f07')}>
        <Row title="outputStyle" desc={tr('desktop_settings_5bf55e39c3')} align="center">
          <div className="settings-inline-fields" style={{ display: 'flex', gap: 7 }}>
            <input value={outputStyleText} onChange={(e) => setOutputStyleText(e.target.value)} placeholder="Explanatory" aria-label="outputStyle" style={inputStyle(t)} />
            <button type="button" disabled={!snapshot || saving === 'outputStyle'} onClick={() => write({ outputStyle: outputStyleText.trim() || null }, 'outputStyle')} style={ghostButtonStyle(t, saving === 'outputStyle')}>{tr('settings_save')}</button>
          </div>
        </Row>
      </Card>

      <Card title={tr('desktop_settings_f496376d26')}>
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="modelOverrides" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {Object.keys(modelOverrides).length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>{tr('desktop_settings_9863316cf9')}</div>
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
              {tr('settings_parity_remove')}
            </button>
          </Row>
        ))}
        <Row title={tr('desktop_settings_90b3bfb6be')} align="center">
          <div className="settings-inline-fields" style={{ display: 'flex', gap: 7 }}>
            <input value={overrideFrom} onChange={(e) => setOverrideFrom(e.target.value)} placeholder={tr('desktop_settings_7106752325')} aria-label={tr('desktop_settings_7106752325')} style={inputStyle(t)} />
            <input value={overrideTo} onChange={(e) => setOverrideTo(e.target.value)} placeholder={tr('desktop_settings_7d37f05964')} aria-label={tr('desktop_settings_7d37f05964')} style={inputStyle(t)} />
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
              {tr('workspace_confirm_add')}
            </button>
          </div>
        </Row>
      </Card>

      <Card title={tr('desktop_settings_459132b9d0')}>
        <Row title={tr('desktop_settings_945680cc85')} desc={tr('desktop_settings_3e7e38b36d')} align="center">
          <button type="button" onClick={() => void bridge.refreshAgents()} style={ghostButtonStyle(t)}>{tr('settings_linux_refresh_button')}</button>
        </Row>
        {bridge.agentCatalog.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>{tr('desktop_settings_7ec40412c0')}</div>
        )}
        {bridge.agentCatalog.map((agent) => (
          <Row
            key={agent.name}
            title={agent.name}
            desc={`${agent.description || tr('desktop_settings_8ae96d747e')}${agent.tools_allowed.length ? ` · tools: ${agent.tools_allowed.join(', ')}` : ''}`}
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
