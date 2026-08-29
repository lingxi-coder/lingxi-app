import { useState } from 'react';
import { Card, MergedBadge, MergedNotice, OverriddenNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Toggle } from '../primitives';
import type { PageContentProps } from '../SettingsScreen';
import { isEditableLayer } from './CustomProviders';
import { rowState, type SettingsSnapshot } from '../useEngineSettings';
import { ghostButtonStyle } from './ghostButton';
import { parseJsonObjectInput } from './McpServers';

/**
 * `enabledPlugins` / `pluginConfigs` / `additionalMarketplaces` as
 * `editingLayer`'s OWN raw object — never `snapshot.effective`. All three
 * are `DeepMerge` keys (`engine/src/settings/schema.rs`'s
 * `MERGE_STRATEGIES`), same reasoning as `CustomProviders.providersFromLayer`.
 */
function objectFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): Record<string, unknown> {
  const value = snapshot?.layers?.[layer]?.[key];
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

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

const inputStyle = (t: ReturnType<typeof useT>) => ({
  padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`,
  background: t.surface, color: t.text, fontSize: 12.5, fontFamily: 'inherit',
} as const);

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
        <ProvenanceNotice snapshot={snapshot} keyName="enabledPlugins" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {pluginNames.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有启用任何插件。</div>
        )}
        {pluginNames.map((id) => (
          <Row key={id} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{id}</span>} desc="key 格式为 plugin@marketplace">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <Toggle
                value={enabledPlugins[id] !== false}
                onChange={saving === 'enabledPlugins' ? () => undefined : (v) => write({ enabledPlugins: { ...enabledPlugins, [id]: v } }, 'enabledPlugins')}
              />
              <button
                type="button"
                disabled={saving === 'enabledPlugins'}
                onClick={() => {
                  const next = { ...enabledPlugins };
                  delete next[id];
                  write({ enabledPlugins: next }, 'enabledPlugins');
                }}
                style={ghostButtonStyle(t, saving === 'enabledPlugins', true)}
              >
                移除
              </button>
            </div>
          </Row>
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
        <ProvenanceNotice snapshot={snapshot} keyName="pluginConfigs" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        {Object.keys(pluginConfigs).length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有插件配置。</div>
        )}
        {Object.entries(pluginConfigs).map(([id, config]) => (
          <PluginConfigRow
            key={id}
            id={id}
            config={config}
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
        <ProvenanceNotice snapshot={snapshot} keyName="additionalMarketplaces" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
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

function PluginConfigRow({
  id, config, saving, onSave, onRemove,
}: { id: string; config: unknown; saving: boolean; onSave(next: Record<string, unknown>): void; onRemove(): void }) {
  const t = useT();
  const [text, setText] = useState(JSON.stringify(config, null, 2));
  const [error, setError] = useState<string | null>(null);
  return (
    <Row title={<span className="mono" style={{ fontSize: 12.5 }}>{id}</span>} align="start">
      <div style={{ display: 'grid', gap: 7 }}>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-label={`${id} 配置`}
          rows={4}
          className="mono"
          style={{
            padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`,
            background: t.surface, color: t.text, fontSize: 12, fontFamily: 'inherit', width: 320, resize: 'vertical',
          }}
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
