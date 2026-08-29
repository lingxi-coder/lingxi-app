import { useEffect, useRef, useState } from 'react';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Toggle } from '../primitives';
import type { EditableLayer, PageContentProps } from '../SettingsScreen';
import { objectFromLayer } from '../layerFields';
import { parseJsonObjectInput } from '../jsonInput';
import { ghostButtonStyle, inputStyle } from './ghostButton';

/**
 * Edits `enabledPlugins` (keyed by `plugin@marketplace` → truthy/config
 * value), `pluginConfigs` (keyed by `plugin@marketplace` → a config object),
 * and `additionalMarketplaces` (keyed by marketplace name → a source
 * declaration). All three are `DeepMerge` settings keys with no dedicated
 * command, so writes go through the generic `update_settings`
 * (`bridge.updateEngineSettings`), same shape as `CustomProviders`.
 *
 * `extraKnownMarketplaces` / `strictKnownMarketplaces` / `blockedMarketplaces`
 * are deliberately NOT on this page: their own doc comments in
 * `engine/src/settings/schema.rs` describe them as the MANAGED counterparts
 * of `additionalMarketplaces` (an admin allow/deny list), not something a
 * desktop user adds to directly — out of scope, noted in the Task 18 report
 * rather than silently added or silently dropped.
 */
export function Plugins({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const enabledPlugins = objectFromLayer(snapshot, editingLayer, 'enabledPlugins');
  const pluginConfigs = objectFromLayer(snapshot, editingLayer, 'pluginConfigs');
  const marketplaces = objectFromLayer(snapshot, editingLayer, 'additionalMarketplaces');

  const [newPluginId, setNewPluginId] = useState('');
  const [newMarketplaceName, setNewMarketplaceName] = useState('');
  const [newMarketplaceSource, setNewMarketplaceSource] = useState('{\n  "source": "https://example.test/marketplace.json"\n}');
  const [saving, setSaving] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Hygiene, not a correctness fix like `PluginConfigRow`'s own effect below:
  // these three are always-blank drafts (never seeded FROM a layer's data —
  // there is no "load an existing marketplace/plugin id into this field to
  // edit it" affordance on this page), so a stale value here cannot fork
  // data across layers the way a SEEDED field could. Clearing them on layer
  // switch just avoids "I typed this for `user`, forgot, switched to
  // `project`, and it's still sitting there."
  useEffect(() => {
    setNewPluginId('');
    setNewMarketplaceName('');
    setNewMarketplaceSource('{\n  "source": "https://example.test/marketplace.json"\n}');
    setSaveError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingLayer]);

  const write = (patch: Record<string, unknown>, key: string) => {
    setSaving(key);
    setSaveError(null);
    void bridge.updateEngineSettings(editingLayer, patch)
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存插件设置。'))
      .finally(() => setSaving(null));
  };

  const pluginNames = Object.keys(enabledPlugins).sort();
  const marketplaceNames = Object.keys(marketplaces).sort();

  return (
    <>
      <Card title="已启用的插件 (enabledPlugins)">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="enabledPlugins" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {pluginNames.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有启用任何插件。</div>
        )}
        {pluginNames.map((id) => (
          <PluginToggleRow
            key={id}
            id={id}
            value={enabledPlugins[id]}
            saving={saving === 'enabledPlugins'}
            onChange={(next) => write({ enabledPlugins: { ...enabledPlugins, [id]: next } }, 'enabledPlugins')}
            onRemove={() => {
              const next = { ...enabledPlugins };
              delete next[id];
              write({ enabledPlugins: next }, 'enabledPlugins');
            }}
          />
        ))}
        <Row title="新增插件" desc="plugin@marketplace" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input value={newPluginId} onChange={(e) => setNewPluginId(e.target.value)} placeholder="my-plugin@my-marketplace" aria-label="新增插件 id" style={{ ...inputStyle(t), width: 240 }} />
            <button
              type="button"
              disabled={saving === 'enabledPlugins' || !newPluginId.trim()}
              onClick={() => { write({ enabledPlugins: { ...enabledPlugins, [newPluginId.trim()]: true } }, 'enabledPlugins'); setNewPluginId(''); }}
              style={ghostButtonStyle(t, saving === 'enabledPlugins' || !newPluginId.trim())}
            >
              添加
            </button>
          </div>
        </Row>
      </Card>

      <Card title="插件配置 (pluginConfigs)">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="pluginConfigs" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {Object.keys(pluginConfigs).length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有插件配置。</div>
        )}
        {Object.entries(pluginConfigs).map(([id, config]) => (
          <PluginConfigRow
            key={id}
            id={id}
            config={config}
            editingLayer={editingLayer}
            saving={saving === 'pluginConfigs'}
            onSave={(next) => write({ pluginConfigs: { ...pluginConfigs, [id]: next } }, 'pluginConfigs')}
            onRemove={() => {
              const next = { ...pluginConfigs };
              delete next[id];
              write({ pluginConfigs: next }, 'pluginConfigs');
            }}
          />
        ))}
      </Card>

      <Card title="市场 (additionalMarketplaces)">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="additionalMarketplaces" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {marketplaceNames.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有额外的市场。</div>
        )}
        {marketplaceNames.map((name) => (
          <Row key={name} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{name}</span>} desc={JSON.stringify(marketplaces[name])}>
            <button
              type="button"
              disabled={saving === 'additionalMarketplaces'}
              onClick={() => {
                const next = { ...marketplaces };
                delete next[name];
                write({ additionalMarketplaces: next }, 'additionalMarketplaces');
              }}
              style={ghostButtonStyle(t, saving === 'additionalMarketplaces', true)}
            >
              移除
            </button>
          </Row>
        ))}
        <Row title="新增市场" align="start">
          <div style={{ display: 'grid', gap: 7 }}>
            <input value={newMarketplaceName} onChange={(e) => setNewMarketplaceName(e.target.value)} placeholder="市场名称" aria-label="新增市场名称" style={inputStyle(t)} />
            <textarea
              value={newMarketplaceSource}
              onChange={(e) => setNewMarketplaceSource(e.target.value)}
              aria-label="新增市场来源 (JSON)"
              rows={4}
              className="mono"
              style={{ ...inputStyle(t), width: 360, resize: 'vertical' }}
            />
            <button
              type="button"
              disabled={saving === 'additionalMarketplaces' || !newMarketplaceName.trim()}
              onClick={() => {
                const parsed = parseJsonObjectInput(newMarketplaceSource);
                if ('error' in parsed) { setSaveError(parsed.error); return; }
                write({ additionalMarketplaces: { ...marketplaces, [newMarketplaceName.trim()]: parsed.config } }, 'additionalMarketplaces');
                setNewMarketplaceName('');
              }}
              style={ghostButtonStyle(t, saving === 'additionalMarketplaces' || !newMarketplaceName.trim())}
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

/**
 * Task 18 fix round 1, Minor: `enabledPlugins[id]` is not always a plain
 * boolean — it can be a config-carrying object. The old inline toggle
 * (`enabledPlugins[id] !== false` for "on", writing a hardcoded `true` for
 * "on" and `false` for "off") treated any truthy value as indistinguishable
 * from `true`, so switching a config-object entry off and back on replaced
 * the object with the literal `true`, discarding it. This component
 * remembers the last truthy value it saw (a `ref`, not `state` — the
 * remembered value must survive the "off" render, where `value` itself is
 * `false` and so cannot be read back from props) and restores exactly that
 * value when toggled back on, rather than always writing `true`.
 */
function PluginToggleRow({
  id, value, saving, onChange, onRemove,
}: { id: string; value: unknown; saving: boolean; onChange(next: unknown): void; onRemove(): void }) {
  const t = useT();
  const lastEnabledValue = useRef<unknown>(value !== false ? value : true);
  if (value !== false) lastEnabledValue.current = value;
  const enabled = value !== false;
  return (
    <Row align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{id}</span>} desc="key 格式为 plugin@marketplace">
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <Toggle
          value={enabled}
          onChange={saving ? () => undefined : (next) => onChange(next ? lastEnabledValue.current : false)}
        />
        <button type="button" disabled={saving} onClick={onRemove} style={ghostButtonStyle(t, saving, true)}>
          移除
        </button>
      </div>
    </Row>
  );
}

function PluginConfigRow({
  id, config, editingLayer, saving, onSave, onRemove,
}: {
  id: string; config: unknown; editingLayer: EditableLayer; saving: boolean;
  onSave(next: Record<string, unknown>): void; onRemove(): void;
}) {
  const t = useT();
  const [text, setText] = useState(JSON.stringify(config, null, 2));
  const [error, setError] = useState<string | null>(null);

  // Task 18 fix round 1, Critical: this row is keyed by PLUGIN ID
  // (`Plugins`' `.map` above), not by layer, and `useState`'s initializer
  // only runs on first mount — so switching `editingLayer` while a row for
  // the SAME id exists in both layers reused the existing component
  // instance with the PREVIOUS layer's text still in the textarea, even
  // though `config` (the prop) had already changed to the new layer's
  // value. Saving from there would write the stale layer's config into the
  // newly selected layer — the identical bug `ToolsAgent.tsx` carries a fix
  // and a comment for, here triggered by two layers happening to share a
  // plugin id instead of by `enabledTools`. Re-seeding on `editingLayer`
  // change (not on every `config` change, which also fires right after
  // THIS row's own successful save) closes it the same way.
  useEffect(() => {
    setText(JSON.stringify(config, null, 2));
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingLayer]);

  return (
    <Row title={<span className="mono" style={{ fontSize: 12.5 }}>{id}</span>} align="start">
      <div style={{ display: 'grid', gap: 7 }}>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-label={`${id} 配置`}
          rows={4}
          className="mono"
          style={{ ...inputStyle(t), width: 320, resize: 'vertical' }}
        />
        {error && <span role="alert" style={{ color: t.danger, fontSize: 12 }}>{error}</span>}
        <div style={{ display: 'flex', gap: 7 }}>
          <button
            type="button"
            disabled={saving}
            onClick={() => {
              const parsed = parseJsonObjectInput(text);
              if ('error' in parsed) { setError(parsed.error); return; }
              setError(null);
              onSave(parsed.config);
            }}
            style={ghostButtonStyle(t, saving)}
          >
            保存
          </button>
          <button type="button" disabled={saving} onClick={onRemove} style={ghostButtonStyle(t, saving, true)}>移除</button>
        </div>
      </div>
    </Row>
  );
}
