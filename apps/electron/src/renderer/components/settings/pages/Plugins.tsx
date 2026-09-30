import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import { Icon } from '../../Icon';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { EditableLayer, PageContentProps } from '../SettingsScreen';
import { objectFromLayer } from '../layerFields';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import { ExtensionHubTabs, extensionHubStyle, useExtensionHubDialogFocus } from './ExtensionHub';
import {
  adminRecordCommand,
  asRecord,
  asString,
  DomainOperationBanner,
  EmptyDetail,
  Field,
  nextConfigurationOperationId,
  noteStyle,
  parseEventEnvelope,
  prettyJson,
  searchInputStyle,
  secondaryMetaStyle,
  SourcePill,
  textareaStyle,
} from './configurationAdmin';

interface PluginCatalogEnvelope {
  installed?: Array<{
    id: string;
    name: string;
    display_name?: string;
    version?: string;
    path?: string;
    default_enabled?: boolean;
    description?: string;
    dependencies?: string[];
    config_schema_json?: string;
    secret_configured?: Record<string, boolean>;
  }>;
  available?: Array<{
    id: string;
    name: string;
    marketplace: string;
    version?: string;
    description?: string;
    installed?: boolean;
    upgrade_available?: boolean;
  }>;
  marketplaces?: Array<{
    name: string;
    source_json?: string;
    install_location?: string;
    last_updated?: string;
  }>;
  policies_json?: string;
  revisions?: Record<string, string>;
}

type PluginSelection =
  | { kind: 'list' }
  | { kind: 'plugin'; id: string }
  | { kind: 'available'; id: string }
  | { kind: 'marketplace'; name: string }
  | { kind: 'marketplace-add' }
  | { kind: 'policies' };

interface PreparedPluginOperation {
  payload: Record<string, unknown>;
  summary: string;
}

interface PluginConfigField {
  type?: 'string' | 'number' | 'boolean' | 'directory' | 'file';
  title?: string;
  description?: string;
  sensitive?: boolean;
  required?: boolean;
  default?: unknown;
  multiple?: boolean;
  min?: number;
  max?: number;
}

export function pluginDependencyCaveat(): string {
  return '启用会递归启用已安装依赖；停用仍被其它已启用插件依赖的项目会被拒绝。所有生命周期操作先预检，再由用户确认执行。';
}

function callPluginAdmin(bridge: PageContentProps['bridge'], command: unknown) {
  const admin = (bridge as { pluginAdmin?: (payload: unknown) => Promise<unknown> }).pluginAdmin;
  return typeof admin === 'function' ? admin(command as never) : Promise.resolve();
}

function editableMarketplaces(snapshot: PageContentProps['snapshot'], editingLayer: EditableLayer): Record<string, unknown> {
  const layer = snapshot?.layers?.[editingLayer] ?? {};
  if ('extraKnownMarketplaces' in layer) return objectFromLayer(snapshot, editingLayer, 'extraKnownMarketplaces');
  return objectFromLayer(snapshot, editingLayer, 'additionalMarketplaces');
}

export function Plugins({ bridge, snapshot, editingLayer, onJumpToLayer, onNavigate }: PageContentProps) {
  const t = useT();
  const adminAvailable = typeof (bridge as { pluginAdmin?: unknown }).pluginAdmin === 'function';
  const catalog = useMemo(() => parseEventEnvelope<PluginCatalogEnvelope>(bridge.pluginCatalogEvent?.catalog_json, { installed: [], revisions: {} }), [bridge.pluginCatalogEvent?.catalog_json]);
  const pluginOperation = bridge.configurationOperations?.plugin ?? null;
  const enabledPlugins = useMemo(() => objectFromLayer(snapshot, editingLayer, 'enabledPlugins'), [editingLayer, snapshot]);
  const pluginConfigs = useMemo(() => objectFromLayer(snapshot, editingLayer, 'pluginConfigs'), [editingLayer, snapshot]);
  const marketplaces = useMemo(() => editableMarketplaces(snapshot, editingLayer), [editingLayer, snapshot]);
  const revision = catalog.revisions?.[editingLayer] ?? '';
  const pluginIds = [...new Set([...Object.keys(enabledPlugins), ...Object.keys(pluginConfigs), ...(catalog.installed ?? []).map((entry) => entry.id || entry.name)])].sort();
  const availablePlugins = catalog.available ?? [];
  const marketplaceNames = [...new Set([...Object.keys(marketplaces), ...(catalog.marketplaces ?? []).map((entry) => entry.name)])].sort();

  const [selection, setSelection] = useState<PluginSelection>({ kind: 'list' });
  const [search, setSearch] = useState('');
  const [draftEnabled, setDraftEnabled] = useState(enabledPlugins);
  const [draftConfigs, setDraftConfigs] = useState(pluginConfigs);
  const [draftMarketplaces, setDraftMarketplaces] = useState(marketplaces);
  const [configText, setConfigText] = useState('{}');
  const [pageError, setPageError] = useState<string | null>(null);
  const [preparedOperation, setPreparedOperation] = useState<PreparedPluginOperation | null>(null);
  const [pendingSelection, setPendingSelection] = useState<PluginSelection | null>(null);
  const [marketplaceSource, setMarketplaceSource] = useState('');
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string>>({});
  const [secretNotice, setSecretNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showAvailable, setShowAvailable] = useState(false);
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [pendingHubPage, setPendingHubPage] = useState<string | null>(null);

  useEffect(() => {
    if (adminAvailable) void callPluginAdmin(bridge, adminRecordCommand('get_catalog'));
  }, [adminAvailable, bridge.pluginAdmin]);

  useEffect(() => {
    setDraftEnabled(enabledPlugins);
    setDraftConfigs(pluginConfigs);
    setDraftMarketplaces(marketplaces);
  }, [editingLayer, enabledPlugins, marketplaces, pluginConfigs]);

  const selectedPlugin = selection.kind === 'plugin' ? selection.id : null;
  const selectedAvailable = selection.kind === 'available'
    ? availablePlugins.find((entry) => entry.id === selection.id) ?? null
    : null;
  const selectedMarketplace = selection.kind === 'marketplace' ? selection.name : null;

  useEffect(() => {
    if (selectedPlugin) setConfigText(prettyJson(asRecord(draftConfigs[selectedPlugin])));
    else if (selectedMarketplace) setConfigText(prettyJson(asRecord(draftMarketplaces[selectedMarketplace])));
  }, [draftConfigs, draftMarketplaces, selectedMarketplace, selectedPlugin]);

  const dirty = JSON.stringify(draftEnabled) !== JSON.stringify(enabledPlugins)
    || JSON.stringify(draftConfigs) !== JSON.stringify(pluginConfigs)
    || JSON.stringify(draftMarketplaces) !== JSON.stringify(marketplaces);

  const requestHubNavigation = (pageId: string) => {
    if (dirty) {
      setPendingHubPage(pageId);
      return;
    }
    onNavigate(pageId);
  };
  const discardAndNavigate = () => {
    if (!pendingHubPage) return;
    setDraftEnabled(enabledPlugins);
    setDraftConfigs(pluginConfigs);
    setDraftMarketplaces(marketplaces);
    setPreparedOperation(null);
    setPendingSelection(null);
    setSelection({ kind: 'list' });
    onNavigate(pendingHubPage);
    setPendingHubPage(null);
  };
  const navigationPrompt = pendingHubPage && (
    <div role="alert" style={noteStyle(t, 'warn')}>
      Unsaved plugin changes. Discard them before switching tabs?
      <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <button type="button" onClick={discardAndNavigate} style={ghostButtonStyle(t)}>Discard and switch</button>
        <button type="button" onClick={() => setPendingHubPage(null)} style={ghostButtonStyle(t, false, true)}>Continue editing</button>
      </div>
    </div>
  );

  const filteredPlugins = search.trim()
    ? pluginIds.filter((id) => {
      const entry = (catalog.installed ?? []).find((candidate) => (candidate.id || candidate.name) === id);
      return `${id} ${entry?.display_name ?? ''} ${entry?.description ?? ''}`.toLowerCase().includes(search.trim().toLowerCase());
    })
    : pluginIds;
  const filteredAvailable = search.trim()
    ? availablePlugins.filter((entry) => `${entry.id} ${entry.description ?? ''}`.toLowerCase().includes(search.trim().toLowerCase()))
    : availablePlugins;
  const filteredMarketplaces = search.trim() ? marketplaceNames.filter((name) => name.toLowerCase().includes(search.trim().toLowerCase())) : marketplaceNames;

  const persistPayload = JSON.stringify({
    scope: editingLayer,
    enabledPlugins: draftEnabled,
    pluginConfigs: draftConfigs,
    extraKnownMarketplaces: draftMarketplaces,
  });

  const requestSelection = (next: PluginSelection) => {
    if (dirty) {
      setPendingSelection(next);
      return;
    }
    setPreparedOperation(null);
    setSelection(next);
    setAddMenuOpen(false);
  };

  const dialogRef = useExtensionHubDialogFocus(selection.kind !== 'list', () => requestSelection({ kind: 'list' }));

  const preview = () => {
    setPageError(null);
    void callPluginAdmin(bridge, adminRecordCommand('preview_operation', {
      operation_id: nextConfigurationOperationId(),
      scope: editingLayer,
      revision,
      payload_json: JSON.stringify({
        action: 'save_config',
        scope: editingLayer,
        enabledPlugins: draftEnabled,
        pluginConfigs: draftConfigs,
        extraKnownMarketplaces: draftMarketplaces,
      }),
    }))
      .then((result: unknown) => {
        const details = result && typeof result === 'object' && 'details_json' in result ? asString((result as Record<string, unknown>).details_json) : '';
        setPreparedOperation({
          payload: { action: 'save_config', scope: editingLayer },
          summary: details || '设置预检已完成。',
        });
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法预检插件变更。'));
  };

  const save = () => {
    if (!revision) {
      setPageError('当前缺少 revision，无法保存。');
      return;
    }
    setPageError(null);
    setBusy(true);
    void callPluginAdmin(bridge, adminRecordCommand('save_config', {
      operation_id: nextConfigurationOperationId(),
      scope: editingLayer,
      revision,
      payload_json: persistPayload,
    }))
      .then(() => setPreparedOperation(null))
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法保存插件设置。'))
      .finally(() => setBusy(false));
  };

  const previewLifecycle = (payload: Record<string, unknown>) => {
    setPageError(null);
    setBusy(true);
    void callPluginAdmin(bridge, adminRecordCommand('preview_operation', {
      operation_id: nextConfigurationOperationId(),
      scope: editingLayer,
      payload_json: JSON.stringify(payload),
    }))
      .then((result: unknown) => {
        const details = result && typeof result === 'object' && 'details_json' in result
          ? asString((result as Record<string, unknown>).details_json)
          : '';
        setPreparedOperation({ payload, summary: details || `已预检 ${asString(payload.action)}。` });
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法预检插件操作。'))
      .finally(() => setBusy(false));
  };

  const confirmLifecycle = () => {
    if (!preparedOperation || preparedOperation.payload.action === 'save_config' || !revision) return;
    setPageError(null);
    setBusy(true);
    void callPluginAdmin(bridge, adminRecordCommand('apply_operation', {
      operation_id: nextConfigurationOperationId(),
      scope: editingLayer,
      revision,
      payload_json: JSON.stringify({ ...preparedOperation.payload, scope: editingLayer, confirmed: true }),
    }))
      .then(() => setPreparedOperation(null))
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法执行插件操作。'))
      .finally(() => setBusy(false));
  };

  const manifest = selectedPlugin
    ? (catalog.installed ?? []).find((entry) => (entry.id || entry.name) === selectedPlugin) ?? null
    : null;
  const configSchema = useMemo(() => parseEventEnvelope<{ fields?: Record<string, PluginConfigField> }>(manifest?.config_schema_json, {}), [manifest?.config_schema_json]);
  const selectedConfig = selectedPlugin ? asRecord(draftConfigs[selectedPlugin]) : {};
  const selectedOptions = asRecord(selectedConfig.options);
  const updateOption = (key: string, value: unknown) => {
    if (!selectedPlugin) return;
    setDraftConfigs((current) => {
      const pluginConfig = asRecord(current[selectedPlugin]);
      const options = { ...asRecord(pluginConfig.options) };
      if (value === undefined) delete options[key];
      else options[key] = value;
      return { ...current, [selectedPlugin]: { ...pluginConfig, options } };
    });
  };

  return (
    <>
      <div className="extension-hub-page" style={extensionHubStyle(t)}>
        <ExtensionHubTabs bridge={bridge} active="plugins" onNavigate={requestHubNavigation} />
        <div className="extension-hub-toolbar">
          <input className="extension-hub-search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search plugins" aria-label="搜索插件与市场" style={searchInputStyle(t)} />
          <div className="extension-hub-toolbar-actions">
            <button type="button" onClick={() => setShowAvailable((value) => !value)} style={ghostButtonStyle(t)}>Browse directory</button>
            <div className="extension-hub-menu">
              <button type="button" onClick={() => setAddMenuOpen((value) => !value)} aria-expanded={addMenuOpen} style={{ ...ghostButtonStyle(t), background: t.text, color: t.surface, borderColor: t.text }}>Add <Icon name="chevron" size={12} /></button>
              {addMenuOpen && <div className="extension-hub-menu-popover">
                <button type="button" onClick={() => { setShowAvailable(true); setAddMenuOpen(false); }}>Browse available plugins</button>
                <button type="button" onClick={() => requestSelection({ kind: 'marketplace-add' })}>Add a marketplace</button>
                <button type="button" onClick={() => requestSelection({ kind: 'policies' })}>View policies</button>
              </div>}
            </div>
          </div>
        </div>

        {selection.kind === 'list' && navigationPrompt}

        {selection.kind === 'list' && <>
          <DomainOperationBanner t={t} operation={pluginOperation} fallbackDomainLabel="Plugins" />
          {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
          {secretNotice && <div role="status" style={noteStyle(t, secretNotice.includes('重启') ? 'warn' : 'neutral')}>{secretNotice}</div>}
          {preparedOperation && (
            <div style={noteStyle(t, 'warn')}>
              <div>{preparedOperation.summary}</div>
              {preparedOperation.payload.action !== 'save_config' && (
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" disabled={busy} onClick={confirmLifecycle} style={ghostButtonStyle(t, busy)}>确认执行</button>
                  <button type="button" disabled={busy} onClick={() => setPreparedOperation(null)} style={ghostButtonStyle(t, busy, true)}>取消</button>
                </div>
              )}
            </div>
          )}
        </>}

        <details className="extension-hub-caveat" data-testid="plugin-dependency-caveat">
          <summary>Before changing plugin status</summary>
          <div style={{ ...noteStyle(t, 'warn'), marginTop: 8 }}>{pluginDependencyCaveat()}</div>
        </details>

        <div className="extension-hub-group-title">Installed plugins</div>
        <div className="extension-hub-list">
          {filteredPlugins.map((id) => {
            const entry = (catalog.installed ?? []).find((candidate) => (candidate.id || candidate.name) === id);
            const enabled = draftEnabled[id] !== false;
            return (
              <div key={id} className="extension-hub-row configuration-entry">
                <span className="extension-hub-icon" style={{ '--hub-icon-bg': t.surfaceHover } as CSSProperties}><Icon name="puzzle" size={19} color={t.text3} /></span>
                <button type="button" className="extension-hub-row-copy extension-hub-row-open" onClick={() => requestSelection({ kind: 'plugin', id })}>
                  <span className="extension-hub-row-title">{entry?.display_name ?? id}</span>
                  <span className="extension-hub-row-description">{entry?.description ?? id}</span>
                </button>
                <div className="extension-hub-row-side">
                  <span className="extension-hub-row-scope">{editingLayer === 'user' ? 'Personal' : editingLayer === 'project' ? 'Project' : 'Local'}</span>
                  <button
                    type="button"
                    className="extension-hub-toggle"
                    role="switch"
                    aria-checked={enabled}
                    aria-label={`${enabled ? 'Disable' : 'Enable'} ${entry?.display_name ?? id}`}
                    disabled={busy}
                    onClick={() => previewLifecycle({ action: enabled ? 'disable' : 'enable', plugin: id, scope: editingLayer })}
                  />
                  <button type="button" className="extension-hub-icon-button" aria-label={`Configure ${entry?.display_name ?? id}`} onClick={() => requestSelection({ kind: 'plugin', id })}>
                    <Icon name="more" size={16} />
                  </button>
                </div>
              </div>
            );
          })}
          {filteredPlugins.length === 0 && <div style={{ padding: '24px 18px', color: t.text3, fontSize: 13 }}>No installed plugins match this search.</div>}
        </div>

        {showAvailable && <>
          <div className="extension-hub-group-title">Available and updates</div>
          <div className="extension-hub-list">
            {filteredAvailable.map((entry) => (
              <button key={entry.id} type="button" className="extension-hub-row extension-hub-row-open" onClick={() => requestSelection({ kind: 'available', id: entry.id })}>
                <span className="extension-hub-icon"><Icon name="puzzle" size={19} color={t.text3} /></span>
                <span className="extension-hub-row-copy">
                  <span className="extension-hub-row-title">{entry.name}</span>
                  <span className="extension-hub-row-description">{entry.description ?? entry.id}</span>
                </span>
                <span className="extension-hub-row-side">{entry.upgrade_available ? 'Update available' : entry.installed ? 'Installed' : entry.marketplace}</span>
              </button>
            ))}
            {filteredAvailable.length === 0 && <div style={{ padding: '20px 18px', color: t.text3, fontSize: 13 }}>No available plugins match this search.</div>}
          </div>
        </>}

        {marketplaceNames.length > 0 && <details className="extension-hub-marketplaces">
          <summary>Marketplaces · {marketplaceNames.length}</summary>
          <div className="extension-hub-list">
            {filteredMarketplaces.map((name) => (
              <button key={name} type="button" className="extension-hub-row extension-hub-row-open" onClick={() => requestSelection({ kind: 'marketplace', name })}>
                <span className="extension-hub-icon"><Icon name="box" size={18} color={t.text3} /></span>
                <span className="extension-hub-row-copy"><span className="extension-hub-row-title">{name}</span><span className="extension-hub-row-description">{asString(asRecord(draftMarketplaces[name]).source, 'Marketplace source')}</span></span>
              </button>
            ))}
          </div>
        </details>}

        {selection.kind !== 'list' && <div className="extension-detail-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) requestSelection({ kind: 'list' }); }}>
          <section ref={dialogRef} tabIndex={-1} className="extension-detail-dialog" role="dialog" aria-modal="true" aria-label="Plugin details" style={extensionHubStyle(t)}>
            <header className="extension-detail-head">
              <span className="extension-hub-icon"><Icon name="puzzle" size={20} color={t.text3} /></span>
              <div className="extension-detail-head-copy">
                <div className="extension-detail-title-line">
                  <span className="extension-detail-title">{selectedPlugin ? manifest?.display_name ?? selectedPlugin : selectedAvailable?.name ?? selectedMarketplace ?? (selection.kind === 'marketplace-add' ? 'Add marketplace' : 'Plugin policies')}</span>
                  <span className="extension-detail-kind">{selection.kind === 'plugin' ? 'Plugin' : selection.kind === 'available' ? 'Directory' : 'Settings'}</span>
                </div>
              </div>
              {selectedPlugin && <button
                type="button"
                className="extension-hub-toggle"
                role="switch"
                aria-checked={draftEnabled[selectedPlugin] !== false}
                aria-label={`${draftEnabled[selectedPlugin] !== false ? 'Disable' : 'Enable'} ${manifest?.display_name ?? selectedPlugin}`}
                disabled={busy}
                onClick={() => previewLifecycle({ action: draftEnabled[selectedPlugin] !== false ? 'disable' : 'enable', plugin: selectedPlugin, scope: editingLayer })}
              />}
              <button type="button" className="extension-hub-icon-button" data-dialog-initial-focus aria-label="Close details" onClick={() => requestSelection({ kind: 'list' })}><Icon name="x" size={17} /></button>
            </header>
            <div className="extension-detail-content">
            {navigationPrompt}
            <DomainOperationBanner t={t} operation={pluginOperation} fallbackDomainLabel="Plugins" />
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            {secretNotice && <div role="status" style={noteStyle(t, secretNotice.includes('重启') ? 'warn' : 'neutral')}>{secretNotice}</div>}
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                当前配置有未保存修改。请保存，或丢弃后切换。
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { setDraftEnabled(enabledPlugins); setDraftConfigs(pluginConfigs); setDraftMarketplaces(marketplaces); setSelection(pendingSelection); setPendingSelection(null); }} style={ghostButtonStyle(t)}>丢弃并切换</button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>继续编辑</button>
                </div>
              </div>
            )}
            {preparedOperation && (
              <div style={noteStyle(t, 'warn')}>
                <div>{preparedOperation.summary}</div>
                {preparedOperation.payload.action !== 'save_config' && (
                  <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                    <button type="button" disabled={busy} onClick={confirmLifecycle} style={ghostButtonStyle(t, busy)}>确认执行</button>
                    <button type="button" disabled={busy} onClick={() => setPreparedOperation(null)} style={ghostButtonStyle(t, busy, true)}>取消</button>
                  </div>
                )}
              </div>
            )}
            {selection.kind === 'plugin' && selectedPlugin && (
              <>
                <div>
                  <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 6 }}>
                    <SourcePill t={t} label={editingLayer} />
                    {manifest?.version && <SourcePill t={t} label={manifest.version} />}
                  </div>
                  {manifest?.path && <div className="mono" style={secondaryMetaStyle(t)}>{manifest.path}</div>}
                </div>
                {manifest?.description && <div style={noteStyle(t)}>{manifest.description}</div>}
                <Row title="运行状态" desc={pluginDependencyCaveat()} align="center">
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: draftEnabled[selectedPlugin] === false ? 'enable' : 'disable', plugin: selectedPlugin, scope: editingLayer })} style={ghostButtonStyle(t, busy)}>
                    预检{draftEnabled[selectedPlugin] === false ? '启用' : '停用'}
                  </button>
                </Row>
                {(manifest?.dependencies?.length ?? 0) > 0 && (
                  <div style={noteStyle(t)}>依赖：{manifest?.dependencies?.join('、')}</div>
                )}
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'update', plugin: selectedPlugin, scope: editingLayer })} style={ghostButtonStyle(t, busy)}>预检升级</button>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'uninstall', plugin: selectedPlugin, scope: editingLayer })} style={ghostButtonStyle(t, busy, true)}>预检卸载</button>
                </div>
                {Object.entries(configSchema.fields ?? {}).filter(([, field]) => !field.sensitive).map(([key, field]) => {
                  const currentValue = selectedOptions[key] ?? field.default;
                  const label = `${field.title ?? key}${field.required ? ' *' : ''}`;
                  if (field.multiple) {
                    const values = Array.isArray(currentValue) ? currentValue : [];
                    return (
                      <Field key={key} t={t} label={label}>
                        <textarea
                          value={values.map(String).join('\n')}
                          onChange={(event) => {
                            const values = event.target.value
                              .split('\n')
                              .map((value) => value.trim())
                              .filter(Boolean)
                              .map((value) => field.type === 'number' ? Number(value) : field.type === 'boolean' ? value === 'true' : value);
                            updateOption(key, values);
                          }}
                          rows={4}
                          style={textareaStyle(t, 4)}
                          aria-label={`${key} option`}
                          placeholder="每行一个值"
                        />
                        {field.description && <div style={secondaryMetaStyle(t)}>{field.description}</div>}
                      </Field>
                    );
                  }
                  if (field.type === 'boolean') {
                    return (
                      <Field key={key} t={t} label={label}>
                        <select value={currentValue === undefined ? '' : currentValue ? 'true' : 'false'} onChange={(event) => updateOption(key, event.target.value === '' ? undefined : event.target.value === 'true')} style={inputStyle(t)} aria-label={`${key} option`}>
                          <option value="">未设置</option>
                          <option value="true">开启</option>
                          <option value="false">关闭</option>
                        </select>
                        {field.description && <div style={secondaryMetaStyle(t)}>{field.description}</div>}
                      </Field>
                    );
                  }
                  return (
                    <Field key={key} t={t} label={label}>
                      <input
                        type={field.type === 'number' ? 'number' : 'text'}
                        value={currentValue === undefined ? '' : String(currentValue)}
                        min={field.min}
                        max={field.max}
                        onChange={(event) => updateOption(key, event.target.value === '' ? undefined : field.type === 'number' ? Number(event.target.value) : event.target.value)}
                        style={inputStyle(t)}
                        aria-label={`${key} option`}
                        placeholder={field.type === 'directory' ? '/path/to/directory' : field.type === 'file' ? '/path/to/file' : undefined}
                      />
                      {field.description && <div style={secondaryMetaStyle(t)}>{field.description}</div>}
                    </Field>
                  );
                })}
                <Field t={t} label="pluginConfigs JSON">
                  <textarea
                    value={configText}
                    onChange={(event) => {
                      setConfigText(event.target.value);
                      try {
                        const parsed = JSON.parse(event.target.value) as unknown;
                        if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
                          setDraftConfigs((current) => ({ ...current, [selectedPlugin]: parsed }));
                        }
                      } catch {}
                    }}
                    rows={14}
                    style={textareaStyle(t, 14)}
                    aria-label={`${selectedPlugin} 配置`}
                  />
                </Field>
                {Object.entries(configSchema.fields ?? {}).filter(([, field]) => field.sensitive).map(([key, field]) => (
                  <Field key={key} t={t} label={field.title ?? key}>
                    <div style={{ display: 'grid', gap: 6 }}>
                      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                        <input type="password" value={secretDrafts[key] ?? ''} onChange={(event) => setSecretDrafts((current) => ({ ...current, [key]: event.target.value }))} placeholder={manifest?.secret_configured?.[key] ? '已配置（输入新值可替换）' : '输入敏感值'} style={{ flex: 1, minWidth: 0 }} aria-label={`${key} secret`} />
                        <button type="button" disabled={!secretDrafts[key] || busy} onClick={() => { const secret = secretDrafts[key]; if (!secret) return; setBusy(true); setSecretNotice(null); void bridge.setPluginSecret(selectedPlugin, key, secret).then((metadata) => { setSecretDrafts((current) => ({ ...current, [key]: '' })); setSecretNotice(metadata.restartRequired ? '敏感配置已安全保存；当前有活动回合，重启后生效。' : '敏感配置已安全保存并应用。'); return callPluginAdmin(bridge, adminRecordCommand('get_catalog')); }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法保存敏感配置。')).finally(() => setBusy(false)); }} style={ghostButtonStyle(t, !secretDrafts[key] || busy)}>安全保存</button>
                        {manifest?.secret_configured?.[key] && <button type="button" disabled={busy} onClick={() => { setBusy(true); setSecretNotice(null); void bridge.clearPluginSecret(selectedPlugin, key).then((metadata) => { setSecretNotice(metadata.restartRequired ? '敏感配置已清除；当前有活动回合，重启后生效。' : '敏感配置已清除并应用。'); return callPluginAdmin(bridge, adminRecordCommand('get_catalog')); }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法清除敏感配置。')).finally(() => setBusy(false)); }} style={ghostButtonStyle(t, busy, true)}>清除</button>}
                        {manifest?.secret_configured?.[key] && <SourcePill t={t} label="configured" tone="success" />}
                      </div>
                      <div style={secondaryMetaStyle(t)}>{field.description ?? ''}{field.required ? ' · 必填' : ''}</div>
                    </div>
                  </Field>
                ))}
              </>
            )}
            {selection.kind === 'available' && selectedAvailable && (
              <>
                <div>
                  <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 6 }}>
                    <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>{selectedAvailable.name}</div>
                    <SourcePill t={t} label={selectedAvailable.marketplace} />
                    {selectedAvailable.version && <SourcePill t={t} label={selectedAvailable.version} />}
                  </div>
                  <div className="mono" style={secondaryMetaStyle(t)}>{selectedAvailable.id}</div>
                </div>
                {selectedAvailable.description && <div style={noteStyle(t)}>{selectedAvailable.description}</div>}
                <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: selectedAvailable.installed ? 'update' : 'install', plugin: selectedAvailable.id, scope: editingLayer })} style={ghostButtonStyle(t, busy)}>
                  预检{selectedAvailable.installed ? '升级' : '安装'}
                </button>
              </>
            )}
            {selection.kind === 'marketplace' && selectedMarketplace && (
              <>
                <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 6 }}>
                  <div style={{ fontSize: 16, fontWeight: 700, color: t.text }}>{selectedMarketplace}</div>
                  <SourcePill t={t} label={editingLayer} />
                </div>
                <Field t={t} label="Marketplace JSON">
                  <textarea
                    value={configText}
                    onChange={(event) => {
                      setConfigText(event.target.value);
                      try {
                        const parsed = JSON.parse(event.target.value) as unknown;
                        if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
                          setDraftMarketplaces((current) => ({ ...current, [selectedMarketplace]: parsed }));
                        }
                      } catch {}
                    }}
                    rows={12}
                    style={textareaStyle(t, 12)}
                    aria-label="marketplace-json"
                  />
                </Field>
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'marketplace_update', name: selectedMarketplace, scope: editingLayer })} style={ghostButtonStyle(t, busy)}>预检更新</button>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'marketplace_remove', name: selectedMarketplace, scope: editingLayer })} style={ghostButtonStyle(t, busy, true)}>预检移除</button>
                </div>
              </>
            )}
            {selection.kind === 'marketplace-add' && (
              <>
                <EmptyDetail t={t} title="添加 Marketplace" body="支持本地目录、owner/repo、Git URL 或 HTTPS marketplace.json。策略会在执行前校验。" />
                <Field t={t} label="来源">
                  <input value={marketplaceSource} onChange={(event) => setMarketplaceSource(event.target.value)} placeholder="owner/repo 或 https://…" aria-label="marketplace-source" style={{ width: '100%' }} />
                </Field>
                <button type="button" disabled={!marketplaceSource.trim() || busy} onClick={() => previewLifecycle({ action: 'marketplace_add', source: marketplaceSource.trim(), scope: editingLayer })} style={ghostButtonStyle(t, !marketplaceSource.trim() || busy)}>预检添加</button>
              </>
            )}
            {selection.kind === 'policies' && (
              <>
                <EmptyDetail t={t} title="Policies" body="管理员策略仍保持只读展示。" />
                <Field t={t} label="Policies JSON">
                  <textarea value={catalog.policies_json ?? '{}'} readOnly rows={12} style={textareaStyle(t, 12)} aria-label="plugin-policies-json" />
                </Field>
              </>
            )}
            <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
              <button type="button" disabled={!dirty || busy} onClick={preview} style={ghostButtonStyle(t, !dirty || busy)}>预检配置</button>
              <button type="button" disabled={!dirty || busy} onClick={save} style={ghostButtonStyle(t, !dirty || busy)}>保存配置</button>
              <button type="button" disabled={!dirty || busy} onClick={() => { setDraftEnabled(enabledPlugins); setDraftConfigs(pluginConfigs); setDraftMarketplaces(marketplaces); }} style={ghostButtonStyle(t, !dirty || busy, true)}>取消修改</button>
            </div>
            </div>
          </section>
        </div>}
      </div>

      <details className="extension-hub-page extension-hub-secondary-settings" style={extensionHubStyle(t)}>
        <summary>Settings data sources</summary>
        <Card title="来源说明">
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="enabledPlugins" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="pluginConfigs" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <div style={{ padding: '12px 18px', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
            非敏感配置写入当前层的 pluginConfigs；敏感字段仅通过系统 Credential Broker 保存，catalog 只返回 configured 状态。管理员市场策略只读。
          </div>
        </Card>
      </details>
    </>
  );
}
