import { useSettingsDraftGuard } from '../useSettingsDraftGuard';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ExtensionGlyph } from './ExtensionHubIcons';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { EditableLayer, PageContentProps } from '../SettingsScreen';
import { objectFromLayer } from '../layerFields';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import { ExtensionHubLayout, ExtensionHubHeader, ExtensionIcon, extensionHubStyle, useExtensionHubDialogFocus, useExtensionHubNavigation } from './ExtensionHub';
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

export function Plugins({ bridge, snapshot, editingLayer, onJumpToLayer, onNavigate, onRegisterNavigationRequest, onClose, onRegisterCloseRequest, initialPluginId, linkRequest }: PageContentProps) {
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

  const appliedLink = useRef<string>();
  const [selection, setSelection] = useState<PluginSelection>(initialPluginId ? { kind: 'plugin', id: initialPluginId } : { kind: 'list' });
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
  const [catalogTab, setCatalogTab] = useState<'public' | 'personal'>('public');
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [pendingHubPage, setPendingHubPage] = useState<{ pageId: string; pluginId?: string } | null>(null);
  const detailPageRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (adminAvailable) void callPluginAdmin(bridge, adminRecordCommand('get_catalog'));
  }, [adminAvailable, bridge.pluginAdmin]);

  useEffect(() => {
    setDraftEnabled(enabledPlugins);
    setDraftConfigs(pluginConfigs);
    setDraftMarketplaces(marketplaces);
  }, [editingLayer, enabledPlugins, marketplaces, pluginConfigs]);

  useEffect(() => {
    if (linkRequest?.kind !== 'plugin' || appliedLink.current === linkRequest.requestId) return;
    if (linkRequest.payload) {
      appliedLink.current = linkRequest.requestId;
      const source = asString(linkRequest.payload.source ?? linkRequest.payload.url);
      const plugin = asString(linkRequest.payload.plugin ?? linkRequest.payload.id);
      if (source) { setMarketplaceSource(source); setSelection({ kind: 'marketplace-add' }); }
      else if (plugin) setSelection({ kind: 'available', id: plugin });
      else setPageError('This plugin import needs a plugin ID or marketplace source.');
    } else if (bridge.pluginCatalogEvent) {
      appliedLink.current = linkRequest.requestId;
      if (pluginIds.includes(linkRequest.id!)) setSelection({ kind: 'plugin', id: linkRequest.id! });
      else if (availablePlugins.some(plugin => plugin.id === linkRequest.id)) setSelection({ kind: 'available', id: linkRequest.id! });
      else { setSelection({ kind: 'list' }); setPageError('This plugin is unavailable on this device.'); }
    }
  }, [linkRequest, bridge.pluginCatalogEvent, pluginIds.join('\n'), availablePlugins]);
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

  const guardedDirty = dirty || Boolean(marketplaceSource.trim()) || preparedOperation !== null;
  const closePrompt = useSettingsDraftGuard({ onClose, onNavigate, onRegisterCloseRequest }, guardedDirty);
  const requestHubNavigation = (pageId: string, pluginId?: string) => {
    if (guardedDirty) {
      setPendingHubPage({ pageId, pluginId });
      return;
    }
    if (pageId === 'plugins') requestSelection(pluginId ? { kind: 'plugin', id: pluginId } : { kind: 'list' });
    else onNavigate(pageId, pluginId);
  };
  useExtensionHubNavigation(onRegisterNavigationRequest, requestHubNavigation);
  const discardAndNavigate = () => {
    if (!pendingHubPage) return;
    setDraftEnabled(enabledPlugins);
    setDraftConfigs(pluginConfigs);
    setDraftMarketplaces(marketplaces);
    setPreparedOperation(null);
    setPendingSelection(null);
    setMarketplaceSource('');
    setSelection(pendingHubPage.pageId === 'plugins' && pendingHubPage.pluginId ? { kind: 'plugin', id: pendingHubPage.pluginId } : { kind: 'list' });
    if (pendingHubPage.pageId !== 'plugins') onNavigate(pendingHubPage.pageId, pendingHubPage.pluginId);
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
      return `${id} ${entry?.name ?? ''} ${entry?.display_name ?? ''} ${entry?.description ?? ''}`.toLowerCase().includes(search.trim().toLowerCase());
    })
    : pluginIds;
  const filteredAvailable = search.trim()
    ? availablePlugins.filter((entry) => `${entry.id} ${entry.name} ${entry.description ?? ''}`.toLowerCase().includes(search.trim().toLowerCase()))
    : availablePlugins;
  const filteredMarketplaces = search.trim() ? marketplaceNames.filter((name) => name.toLowerCase().includes(search.trim().toLowerCase())) : marketplaceNames;

  const persistPayload = JSON.stringify({
    scope: editingLayer,
    enabledPlugins: draftEnabled,
    pluginConfigs: draftConfigs,
    extraKnownMarketplaces: draftMarketplaces,
  });

  const requestSelection = (next: PluginSelection) => {
    if (guardedDirty) {
      setPendingSelection(next);
      return;
    }
    setPreparedOperation(null);
    setSelection(next);
    setAddMenuOpen(false);
  };

  const isPluginDetail = selection.kind === 'plugin' || selection.kind === 'available';
  const dialogRef = useExtensionHubDialogFocus(selection.kind !== 'list' && !isPluginDetail, () => requestSelection({ kind: 'list' }));
  useEffect(() => {
    if (isPluginDetail) detailPageRef.current?.closest('.settings-content-scroll')?.scrollTo({ top: 0 });
  }, [isPluginDetail, selection]);

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
      .then(() => { setPreparedOperation(null); setMarketplaceSource(''); })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : '无法执行插件操作。'))
      .finally(() => setBusy(false));
  };

  const manifest = selectedPlugin
    ? (catalog.installed ?? []).find((entry) => (entry.id || entry.name) === selectedPlugin) ?? null
    : null;
  const configSchema = useMemo(() => parseEventEnvelope<{ fields?: Record<string, PluginConfigField> } | null>(manifest?.config_schema_json, {}) ?? {}, [manifest?.config_schema_json]);
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

  const detailName = selectedPlugin ? manifest?.display_name ?? manifest?.name ?? selectedPlugin : selectedAvailable?.name ?? '';
  const detailDescription = manifest?.description ?? selectedAvailable?.description ?? '';
  const publicGroups = [...new Set(filteredAvailable.map((entry) => entry.marketplace))];
  const relatedSkills = parseEventEnvelope<{ entries?: Array<{ id: string; name: string; description?: string; pluginOwner?: string }> }>(bridge.skillCatalogEvent?.catalog_json, {}).entries?.filter((entry) => entry.pluginOwner === (selectedPlugin ?? selectedAvailable?.id ?? '')) ?? [];
  const refreshCatalog = () => {
    setPageError(null);
    setBusy(true);
    void callPluginAdmin(bridge, adminRecordCommand('get_catalog'))
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : 'Unable to refresh plugins.'))
      .finally(() => setBusy(false));
  };

  return (
    <ExtensionHubLayout>
      {closePrompt}
      {pageError && selection.kind === 'list' && <div role="alert">{pageError}</div>}
      {linkRequest?.payload && <p role="status">Review this plugin import. Use preview and confirm to install or add its marketplace.</p>}
      {!isPluginDetail && <>
        <ExtensionHubHeader title="Plugins" description="Connect plugins to let LingXi work across your tools" search={search} onSearch={setSearch} actions={<>
          <button type="button" onClick={refreshCatalog} disabled={busy} className="extension-hub-icon-button" aria-label="Refresh plugins"><ExtensionGlyph name="refresh" size={16} /></button>
          <button type="button" onClick={() => requestSelection({ kind: 'policies' })} className="extension-hub-icon-button" aria-label="Plugin settings"><ExtensionGlyph name="cog" size={16} /></button>
          <details className="extension-hub-menu" open={addMenuOpen} onToggle={(event) => setAddMenuOpen(event.currentTarget.open)}>
            <summary className="extension-hub-primary">Add <ExtensionGlyph name="chevron" size={13} /></summary>
            <div className="extension-hub-menu-popover">
              <button type="button" onClick={() => { setCatalogTab('public'); setAddMenuOpen(false); }}>Browse available plugins</button>
              <button type="button" onClick={() => requestSelection({ kind: 'marketplace-add' })}>Add a marketplace</button>
            </div>
          </details>
        </>} />
        {navigationPrompt}
        <DomainOperationBanner t={t} operation={pluginOperation} fallbackDomainLabel="Plugins" />
        {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
        <nav className="extension-catalog-tabs" aria-label="Plugin catalog">
          <button type="button" aria-pressed={catalogTab === 'public'} onClick={() => setCatalogTab('public')}>Public</button>
          <button type="button" aria-pressed={catalogTab === 'personal'} onClick={() => setCatalogTab('personal')}>Personal</button>
        </nav>
        {catalogTab === 'public' && publicGroups.map((marketplace) => <section className="extension-catalog-section" key={marketplace}>
          <h2 className="extension-hub-group-title">{marketplace} <ExtensionGlyph name="chevronR" size={14} /></h2>
          <div className="extension-hub-list">
            {filteredAvailable.filter((entry) => entry.marketplace === marketplace).map((entry) => <div key={entry.id} className={`extension-hub-row${entry.installed ? ' is-installed' : ''}`}>
              <ExtensionIcon />
              <button type="button" className="extension-hub-row-copy extension-hub-row-open" onClick={() => requestSelection({ kind: pluginIds.includes(entry.id) ? 'plugin' : 'available', id: entry.id })}>
                <span className="extension-hub-row-title">{entry.name}</span><span className="extension-hub-row-description">{entry.description || entry.name}</span>
              </button>
              <button type="button" className="extension-hub-icon-button" aria-label={`${entry.installed ? 'Manage' : 'Install'} ${entry.name}`} onClick={() => requestSelection({ kind: pluginIds.includes(entry.id) ? 'plugin' : 'available', id: entry.id })}><ExtensionGlyph name={entry.installed ? 'more' : 'plus'} size={20} /></button>
            </div>)}
          </div>
        </section>)}
        {(catalogTab === 'personal' || !publicGroups.length) && <section className="extension-catalog-section">
          <h2 className="extension-hub-group-title">Installed</h2>
          <div className="extension-hub-list">
            {filteredPlugins.map((id) => {
              const entry = (catalog.installed ?? []).find((candidate) => (candidate.id || candidate.name) === id);
              return <div key={id} className="extension-hub-row configuration-entry">
                <ExtensionIcon />
                <button type="button" className="extension-hub-row-copy extension-hub-row-open" onClick={() => requestSelection({ kind: 'plugin', id })}>
                  <span className="extension-hub-row-title">{entry?.display_name ?? entry?.name ?? id}</span><span className="extension-hub-row-description">{entry?.description ?? 'Installed plugin'}</span>
                </button>
                <button type="button" className="extension-hub-icon-button" aria-label={`Configure ${entry?.display_name ?? id}`} onClick={() => requestSelection({ kind: 'plugin', id })}><ExtensionGlyph name="more" size={16} /></button>
              </div>;
            })}
          </div>
          {!filteredPlugins.length && <div className="extension-hub-empty">{search ? 'No plugins match your search.' : 'No plugins installed yet. Add a marketplace to find your next plugin.'}</div>}
        </section>}
        {catalogTab === 'public' && !publicGroups.length && !search && <button type="button" className="extension-see-more" onClick={() => requestSelection({ kind: 'marketplace-add' })}>Add a marketplace to discover more plugins <ExtensionGlyph name="chevronR" size={14} /></button>}
        {marketplaceNames.length > 0 && <details className="extension-hub-marketplaces">
          <summary>Marketplaces · {marketplaceNames.length}</summary>
          <div className="extension-hub-list">{filteredMarketplaces.map((name) => <button key={name} type="button" className="extension-hub-row extension-hub-row-open" onClick={() => requestSelection({ kind: 'marketplace', name })}><ExtensionIcon /><span className="extension-hub-row-copy"><span className="extension-hub-row-title">{name}</span><span className="extension-hub-row-description">{asString(asRecord(draftMarketplaces[name]).source, 'Marketplace source')}</span></span></button>)}</div>
        </details>}
      </>}
        {selection.kind !== 'list' && <div className={isPluginDetail ? "extension-plugin-detail" : "extension-detail-backdrop"} onMouseDown={(event) => { if (!isPluginDetail && event.target === event.currentTarget) requestSelection({ kind: 'list' }); }}>
          <section ref={isPluginDetail ? detailPageRef : dialogRef} tabIndex={-1} className={isPluginDetail ? "extension-plugin-page" : "extension-detail-dialog"} role={isPluginDetail ? undefined : "dialog"} aria-modal={isPluginDetail ? undefined : true} aria-label="Plugin details" style={extensionHubStyle(t)}>
            {isPluginDetail ? <>
              <nav className="extension-breadcrumb" aria-label="Breadcrumb"><button type="button" onClick={() => requestSelection({ kind: 'list' })}>Plugins</button><ExtensionGlyph name="chevronR" size={15} /><span>{detailName}</span></nav>
              <header className="extension-plugin-hero">
                <ExtensionIcon large />
                <div className="extension-plugin-title-row"><h1>{detailName}</h1><div className="extension-hub-toolbar-actions">
                  {selectedPlugin ? <button type="button" className="extension-hub-toggle" role="switch" aria-checked={draftEnabled[selectedPlugin] !== false} aria-label={`${draftEnabled[selectedPlugin] !== false ? 'Disable' : 'Enable'} ${detailName}`} disabled={busy} onClick={() => previewLifecycle({ action: draftEnabled[selectedPlugin] !== false ? 'disable' : 'enable', plugin: selectedPlugin, scope: editingLayer })} /> : <button type="button" disabled={busy || !adminAvailable} className="extension-hub-primary" onClick={() => selectedAvailable && previewLifecycle({ action: selectedAvailable.installed ? 'update' : 'install', plugin: selectedAvailable.id, scope: editingLayer })}><ExtensionGlyph name="plus" size={16} />{selectedAvailable?.installed ? 'Update plugin' : 'Install plugin'}</button>}
                </div></div>
                <p>{detailDescription}</p>
              </header>
            </> : <header className="extension-detail-head">
              <span className="extension-hub-icon"><ExtensionGlyph name="plugin" size={20} color={t.text3} /></span>
              <div className="extension-detail-head-copy">
                <div className="extension-detail-title-line">
                  <span className="extension-detail-title">{selectedPlugin ? manifest?.display_name ?? selectedPlugin : selectedAvailable?.name ?? selectedMarketplace ?? (selection.kind === 'marketplace-add' ? 'Add marketplace' : 'Plugin policies')}</span>
                  <span className="extension-detail-kind">Settings</span>
                </div>
              </div>
              <button type="button" className="extension-hub-icon-button" data-dialog-initial-focus aria-label="Close details" onClick={() => requestSelection({ kind: 'list' })}><ExtensionGlyph name="x" size={16} /></button>
            </header>}
            <div className="extension-detail-content">
            {navigationPrompt}
            <DomainOperationBanner t={t} operation={pluginOperation} fallbackDomainLabel="Plugins" />
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            {secretNotice && <div role="status" style={noteStyle(t, secretNotice.includes('重启') ? 'warn' : 'neutral')}>{secretNotice}</div>}
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                当前配置有未保存修改。请保存，或丢弃后切换。
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { setDraftEnabled(enabledPlugins); setDraftConfigs(pluginConfigs); setDraftMarketplaces(marketplaces); setMarketplaceSource(''); setPreparedOperation(null); setSelection(pendingSelection); setPendingSelection(null); }} style={ghostButtonStyle(t)}>丢弃并切换</button>
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
            {isPluginDetail && <>
              <div className="extension-plugin-showcase"><ExtensionIcon large /><div><strong>{detailName}</strong><p>{detailDescription || 'Extend your workspace with this plugin.'}</p></div></div>
              {relatedSkills.length > 0 && <section><h2 className="extension-detail-section-title">Skills <span>{relatedSkills.length}</span></h2>{relatedSkills.map((skill) => <button type="button" key={skill.id} className="extension-hub-row extension-hub-row-open" onClick={() => requestHubNavigation('skills')}><ExtensionIcon kind="skill" /><span className="extension-hub-row-copy"><span className="extension-hub-row-title">{skill.name}</span><span className="extension-hub-row-description">{skill.description}</span></span><ExtensionGlyph name="chevronR" size={16} /></button>)}</section>}
              <section><h2 className="extension-detail-section-title">Information</h2><dl className="extension-plugin-information">
                <dt>Version</dt><dd>{manifest?.version ?? selectedAvailable?.version ?? '—'}</dd>
                <dt>Source</dt><dd>{selectedAvailable?.marketplace ?? 'Installed locally'}</dd>
                <dt>Status</dt><dd>{selectedPlugin ? draftEnabled[selectedPlugin] === false ? 'Disabled' : 'Enabled' : selectedAvailable?.installed ? 'Installed' : 'Available to install'}</dd>
                {manifest?.path && <><dt>Location</dt><dd>{manifest.path}</dd></>}
              </dl></section>
            </>}
            {selection.kind === 'plugin' && selectedPlugin && (
              <details className="extension-plugin-configuration">
                <summary>Configuration <ExtensionGlyph name="chevron" size={14} /></summary>
                <div className="extension-plugin-config-body">
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
                </div>
              </details>
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
            {dirty && <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
              <button type="button" disabled={!dirty || busy} onClick={preview} style={ghostButtonStyle(t, !dirty || busy)}>预检配置</button>
              <button type="button" disabled={!dirty || busy} onClick={save} style={ghostButtonStyle(t, !dirty || busy)}>保存配置</button>
              <button type="button" disabled={!dirty || busy} onClick={() => { setDraftEnabled(enabledPlugins); setDraftConfigs(pluginConfigs); setDraftMarketplaces(marketplaces); }} style={ghostButtonStyle(t, !dirty || busy, true)}>取消修改</button>
            </div>}
            </div>
          </section>
        </div>}

      {!isPluginDetail && <details className="extension-hub-page extension-hub-secondary-settings" style={extensionHubStyle(t)}>
        <summary>Settings data sources</summary>
        <Card title="来源说明">
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="enabledPlugins" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="pluginConfigs" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <div style={{ padding: '12px 18px', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
            非敏感配置写入当前层的 pluginConfigs；敏感字段仅通过系统 Credential Broker 保存，catalog 只返回 configured 状态。管理员市场策略只读。
          </div>
        </Card>
      </details>}
    </ExtensionHubLayout>
  );
}
