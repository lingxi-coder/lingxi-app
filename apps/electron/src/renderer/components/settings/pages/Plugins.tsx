import { currentLocale, tr, translate, type DesktopLocale } from '../../../i18n/locale';
import { useSettingsDraftGuard } from '../useSettingsDraftGuard';
import { useEffect, useMemo, useRef, useState } from 'react';
import { publicExtensionItems, publicExtensionText, type PublicExtensionItem } from '../../../../shared/publicExtensionDirectory';
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
  | { kind: 'marketplace'; name: string; sourceConflict?: string }
  | { kind: 'marketplace-add'; source?: string }
  | { kind: 'policies' };

const DESKTOP_PLATFORMS = new Set(['macos', 'windows', 'linux']);
const PUBLIC_EXTENSION_CATEGORIES = ['browser', 'development', 'productivity'] as const;

function publicCategoryLabel(category: string): string {
  const key = {
    browser: 'desktop_extension_category_browser',
    development: 'desktop_extension_category_development',
    productivity: 'desktop_extension_category_productivity',
  }[category];
  return key ? tr(key) : category;
}

type PublicDirectoryText = (typeof publicExtensionItems)[number]['description'];

function localizedDirectoryText(value?: PublicDirectoryText): string {
  return value ? publicExtensionText(value, currentLocale()) : '';
}

function publicPluginSearchText(item: (typeof publicExtensionItems)[number]): string {
  return [
    item.name,
    item.publisher,
    publicCategoryLabel(item.category),
    localizedDirectoryText(item.description),
    localizedDirectoryText(item.requirements),
    item.marketplace?.name,
    item.marketplace?.plugin,
    item.sourceUrl,
    item.documentationUrl,
  ].filter(Boolean).join(' ').toLowerCase();
}

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

export function pluginDependencyCaveat(locale: DesktopLocale = currentLocale()): string {
  return translate('desktop_extension_plugin_dependencies', locale);
}

function pluginActionLabel(action: string): string {
  const keys: Record<string, string> = {
    install: 'plugins_action_install', uninstall: 'plugins_action_uninstall', update: 'plugins_action_update',
    enable: 'settings_provider_enable', disable: 'desktop_extension_disable',
    add_marketplace: 'plugins_marketplace_add', remove_marketplace: 'plugins_marketplace_remove', update_marketplace: 'plugins_marketplace_update',
  };
  return keys[action] ? tr(keys[action]!) : action;
}

function validGithubSegment(value: string): boolean {
  return /^[a-z0-9_.-]+$/i.test(value) && value !== '.' && value !== '..';
}

function githubShorthandIdentity(source: string): string | null {
  const match = source.trim().match(/^([a-z0-9_.-]+)\/([a-z0-9_.-]+)(?:\.git)?$/i);
  if (!match || !validGithubSegment(match[1]!) || !validGithubSegment(match[2]!)) return null;
  return `${match[1]!.toLowerCase()}/${match[2]!.replace(/\.git$/i, '').toLowerCase()}`;
}

/** Canonicalize only a root GitHub remote. Reject credentials, refs, queries and subpaths. */
function githubRemoteIdentity(source: string): string | null {
  const value = source.trim();
  if (value.includes('?') || value.includes('#')) return null;
  const scp = value.match(/^git@github\.com:([a-z0-9_.-]+)\/([a-z0-9_.-]+)(?:\.git)?$/i);
  if (scp && validGithubSegment(scp[1]!) && validGithubSegment(scp[2]!)) {
    return `${scp[1]!.toLowerCase()}/${scp[2]!.replace(/\.git$/i, '').toLowerCase()}`;
  }
  if (value.includes('\\')) return null;
  const rawRemote = value.match(/^(?:https|ssh):\/\/[^/?#]*(\/[^?#]*)?$/i);
  if (!rawRemote) return null;
  const rawPath = rawRemote[1] ?? '';
  if (rawPath.includes('%')) return null;
  const rawPathSegments = rawPath.replace(/^\/+|\/+$/g, '').split('/').filter(Boolean);
  if (rawPathSegments.some((segment) => segment === '.' || segment === '..')) return null;

  try {
    const url = new URL(value);
    const protocol = url.protocol.toLowerCase();
    if (url.hostname.toLowerCase() !== 'github.com'
      || url.search !== ''
      || url.hash !== '') return null;
    if (protocol === 'https:') {
      if (value.includes('@') || url.username !== '' || url.password !== '' || (url.port !== '' && url.port !== '443')) return null;
    } else if (protocol === 'ssh:') {
      if ((url.username !== '' && url.username !== 'git') || url.password !== '' || (url.port !== '' && url.port !== '22')) return null;
      const authority = value.match(/^ssh:\/\/([^/]+)/i)?.[1] ?? '';
      const userInfo = authority.includes('@') ? authority.slice(0, authority.indexOf('@')) : null;
      if (userInfo !== null && userInfo !== 'git') return null;
    } else {
      return null;
    }
    const path = url.pathname.replace(/^\/+|\/+$/g, '');
    const parts = path.split('/');
    if (parts.length !== 2) return null;
    const owner = parts[0]!;
    const repo = parts[1]!.replace(/\.git$/i, '');
    if (!validGithubSegment(owner) || !validGithubSegment(repo)) return null;
    return `${owner.toLowerCase()}/${repo.toLowerCase()}`;
  } catch {
    return null;
  }
}

/** `marketplace_add` accepts shorthand or a root remote; the stored DTO keeps its discriminator. */
function githubRepositoryIdentity(source: string): string | null {
  return githubShorthandIdentity(source) ?? githubRemoteIdentity(source);
}

function marketplaceSourceMatches(source: string, sourceJson?: string): boolean {
  if (!sourceJson) return false;
  try {
    const sourceRecord = JSON.parse(sourceJson) as Record<string, unknown>;
    const expectedRepo = githubRepositoryIdentity(source);
    if (!expectedRepo || typeof sourceRecord.source !== 'string') return false;
    const keys = Object.keys(sourceRecord).sort().join(',');
    if (sourceRecord.source === 'github' && keys === 'repo,source' && typeof sourceRecord.repo === 'string') {
      return githubShorthandIdentity(sourceRecord.repo) === expectedRepo;
    }
    if (sourceRecord.source === 'git' && keys === 'source,url' && typeof sourceRecord.url === 'string') {
      return githubRemoteIdentity(sourceRecord.url) === expectedRepo;
    }
    return false;
  } catch {
    return false;
  }
}

export function findRegisteredPublicMarketplace(
  marketplace: PublicExtensionItem['marketplace'],
  registry: readonly { name: string; source_json?: string }[],
) {
  if (!marketplace) return undefined;
  return registry.find((entry) => (
    marketplaceSourceMatches(marketplace.source, entry.source_json)
  ));
}

export function findConflictingPublicMarketplace(
  marketplace: PublicExtensionItem['marketplace'],
  registry: readonly { name: string; source_json?: string }[],
) {
  if (!marketplace) return undefined;
  return registry.find((entry) => (
    entry.name === marketplace.name && !marketplaceSourceMatches(marketplace.source, entry.source_json)
  ));
}

export type PublicMarketplaceSourceAction =
  | { kind: 'registered'; marketplace: { name: string; source_json?: string } }
  | { kind: 'source-conflict'; marketplace: { name: string; source_json?: string } }
  | { kind: 'add' };

export function decidePublicMarketplaceSourceAction(
  marketplace: NonNullable<PublicExtensionItem['marketplace']>,
  registry: readonly { name: string; source_json?: string }[],
): PublicMarketplaceSourceAction {
  const conflict = findConflictingPublicMarketplace(marketplace, registry);
  if (conflict) return { kind: 'source-conflict', marketplace: conflict };
  const registered = findRegisteredPublicMarketplace(marketplace, registry);
  if (registered) return { kind: 'registered', marketplace: registered };
  return { kind: 'add' };
}

export function publicMarketplacePluginTargetId(
  marketplace: PublicExtensionItem['marketplace'],
  sourceAction: PublicMarketplaceSourceAction | null,
): string | null {
  if (!marketplace?.plugin) return null;
  const registeredName = sourceAction?.kind === 'registered' ? sourceAction.marketplace.name : marketplace.name;
  return `${marketplace.plugin}@${registeredName}`;
}

function callPluginAdmin(bridge: PageContentProps['bridge'], command: unknown) {
  const admin = (bridge as { pluginAdmin?: (payload: unknown) => Promise<unknown> }).pluginAdmin;
  return typeof admin === 'function' ? admin(command as never) : Promise.resolve();
}

export function pluginCredentialCleanupFromOperation(result: unknown): { pluginId: string; lastScopeRemoved: boolean } | undefined {
  if (!result || typeof result !== 'object' || !('details_json' in result)) return undefined;
  const details = (result as { details_json?: unknown }).details_json;
  if (typeof details !== 'string') return undefined;
  try {
    const parsed = JSON.parse(details) as { credentialCleanup?: unknown };
    const cleanup = parsed?.credentialCleanup;
    if (!cleanup || typeof cleanup !== 'object' || Array.isArray(cleanup)) return undefined;
    const value = cleanup as { pluginId?: unknown; lastScopeRemoved?: unknown };
    if (typeof value.pluginId !== 'string' || !value.pluginId || typeof value.lastScopeRemoved !== 'boolean') return undefined;
    return { pluginId: value.pluginId, lastScopeRemoved: value.lastScopeRemoved };
  } catch {
    return undefined;
  }
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
  const publicDirectoryPlugins = useMemo(() => publicExtensionItems.filter((item) => (
    item.kind === 'plugin' && item.platforms.some((platform) => DESKTOP_PLATFORMS.has(platform))
  )), []);
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
  const [secretNotice, setSecretNotice] = useState<{ text: string; restartRequired: boolean } | null>(null);
  const [pluginSecretCleanupNotice, setPluginSecretCleanupNotice] = useState<{
    kind: 'done' | 'restart' | 'retained' | 'failed' | 'deferred' | 'unavailable';
    pluginId?: string;
    restartRequired?: boolean;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [catalogTab, setCatalogTab] = useState<'public' | 'personal'>('public');
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [pendingHubPage, setPendingHubPage] = useState<{ pageId: string; pluginId?: string } | null>(null);
  const detailPageRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (selection.kind === 'marketplace-add' && selection.source) setMarketplaceSource(selection.source);
  }, [selection]);

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
      else setPageError(tr('desktop_extension_plugin_import_missing'));
    } else if (bridge.pluginCatalogEvent) {
      appliedLink.current = linkRequest.requestId;
      if (pluginIds.includes(linkRequest.id!)) setSelection({ kind: 'plugin', id: linkRequest.id! });
      else if (availablePlugins.some(plugin => plugin.id === linkRequest.id)) setSelection({ kind: 'available', id: linkRequest.id! });
      else { setSelection({ kind: 'list' }); setPageError(tr('desktop_extension_plugin_unavailable')); }
    }
  }, [linkRequest, bridge.pluginCatalogEvent, pluginIds.join('\n'), availablePlugins]);
  const selectedPlugin = selection.kind === 'plugin' ? selection.id : null;
  const selectedAvailable = selection.kind === 'available'
    ? availablePlugins.find((entry) => entry.id === selection.id) ?? null
    : null;
  const selectedMarketplace = selection.kind === 'marketplace' ? selection.name : null;
  const selectedMarketplaceConflict = selection.kind === 'marketplace' ? selection.sourceConflict : undefined;
  const selectedMarketplaceConflictRow = selection.kind === 'marketplace' && selection.sourceConflict
    ? findConflictingPublicMarketplace({ name: selection.name, source: selection.sourceConflict }, catalog.marketplaces ?? [])
    : undefined;

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
      {tr('desktop_extension_unsaved_plugin')} <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
        <button type="button" onClick={discardAndNavigate} style={ghostButtonStyle(t)}>{tr('desktop_extension_discard_switch')}</button>
        <button type="button" onClick={() => setPendingHubPage(null)} style={ghostButtonStyle(t, false, true)}>{tr('desktop_extension_continue_editing')}</button>
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
  const filteredPublicDirectory = search.trim()
    ? publicDirectoryPlugins.filter((entry) => publicPluginSearchText(entry).includes(search.trim().toLowerCase()))
    : publicDirectoryPlugins;
  const publicDirectoryGroups = PUBLIC_EXTENSION_CATEGORIES
    .map((category) => ({ category, entries: filteredPublicDirectory.filter((entry) => entry.category === category) }))
    .filter((group) => group.entries.length > 0);
  const hasPublicResults = filteredPublicDirectory.length > 0 || filteredAvailable.length > 0;

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
          summary: details || tr('desktop_extension_preview_complete'),
        });
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_plugin_preview_failed')));
  };

  const save = () => {
    if (!revision) {
      setPageError(tr('desktop_extension_missing_revision'));
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
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_plugin_save_failed')))
      .finally(() => setBusy(false));
  };

  const attemptPluginSecretCleanup = async (pluginId: string) => {
    const cleanup = (bridge as { cleanupPluginSecretsAfterUninstall?: (id: string, lastScopeRemoved: true) => Promise<{
      status: 'cleared' | 'retained' | 'deferred' | 'partial';
      restartRequired?: boolean;
    }> }).cleanupPluginSecretsAfterUninstall;
    if (typeof cleanup !== 'function') {
      setPluginSecretCleanupNotice({ kind: 'failed', pluginId });
      return;
    }
    try {
      const result = await cleanup(pluginId, true);
      if (result.status === 'cleared') {
        setPluginSecretCleanupNotice({ kind: result.restartRequired ? 'restart' : 'done' });
      } else if (result.status === 'retained') {
        setPluginSecretCleanupNotice({ kind: 'retained' });
      } else {
        setPluginSecretCleanupNotice({
          kind: result.status === 'deferred' ? 'deferred' : 'failed',
          pluginId,
          ...(result.restartRequired ? { restartRequired: true } : {}),
        });
      }
    } catch {
      setPluginSecretCleanupNotice({ kind: 'failed', pluginId });
    }
  };

  const retryPluginSecretCleanup = async () => {
    const pluginId = pluginSecretCleanupNotice?.pluginId;
    if (!pluginId || busy) return;
    setBusy(true);
    await attemptPluginSecretCleanup(pluginId);
    setBusy(false);
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
        setPreparedOperation({ payload, summary: details || tr('desktop_extension_operation_preview', pluginActionLabel(asString(payload.action))) });
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_operation_preview_failed')))
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
      .then(async (result: unknown) => {
        if (preparedOperation.payload.action === 'uninstall') {
          const cleanup = pluginCredentialCleanupFromOperation(result);
          if (cleanup?.lastScopeRemoved) await attemptPluginSecretCleanup(cleanup.pluginId);
          else if (cleanup) setPluginSecretCleanupNotice({ kind: 'retained' });
          else setPluginSecretCleanupNotice({ kind: 'unavailable' });
        }
        const marketplaceAdded = preparedOperation.payload.action === 'marketplace_add';
        const conflictMarketplaceRemoved = preparedOperation.payload.action === 'marketplace_remove' && Boolean(selectedMarketplaceConflict);
        setPreparedOperation(null);
        setMarketplaceSource('');
        if (marketplaceAdded || conflictMarketplaceRemoved) {
          setSelection({ kind: 'list' });
          setCatalogTab('public');
          return callPluginAdmin(bridge, adminRecordCommand('get_catalog'));
        }
      })
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_operation_failed')))
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
      .catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_refresh_plugins_failed')))
      .finally(() => setBusy(false));
  };
  const openPublicDirectoryItem = (item: (typeof publicExtensionItems)[number]) => {
    const marketplace = item.marketplace;
    if (!marketplace) return;

    const sourceAction = decidePublicMarketplaceSourceAction(marketplace, catalog.marketplaces ?? []);
    if (sourceAction.kind === 'source-conflict') {
      requestSelection({ kind: 'marketplace', name: sourceAction.marketplace.name, sourceConflict: marketplace.source });
      return;
    }

    const pluginId = publicMarketplacePluginTargetId(marketplace, sourceAction);
    if (pluginId && pluginIds.includes(pluginId)) {
      requestSelection({ kind: 'plugin', id: pluginId });
      return;
    }
    const available = pluginId ? availablePlugins.find((entry) => entry.id === pluginId) : undefined;
    if (available) {
      requestSelection({ kind: 'available', id: available.id });
      return;
    }

    if (sourceAction.kind === 'registered') {
      refreshCatalog();
      requestSelection({ kind: 'marketplace', name: sourceAction.marketplace.name });
      return;
    }
    requestSelection({ kind: 'marketplace-add', source: marketplace.source });
  };

  return (
    <ExtensionHubLayout>
      {closePrompt}
      {pluginSecretCleanupNotice && <div
        role={pluginSecretCleanupNotice.kind === 'failed' || pluginSecretCleanupNotice.kind === 'deferred' || pluginSecretCleanupNotice.kind === 'unavailable' ? 'alert' : 'status'}
        style={noteStyle(t, pluginSecretCleanupNotice.kind === 'failed' || pluginSecretCleanupNotice.kind === 'deferred' || pluginSecretCleanupNotice.kind === 'unavailable' ? 'danger' : 'neutral')}
      >
        <span>{tr({
          done: 'desktop_extension_plugin_secret_cleanup_done',
          restart: 'desktop_extension_plugin_secret_cleanup_restart',
          retained: 'desktop_extension_plugin_secret_cleanup_retained',
          failed: 'desktop_extension_plugin_secret_cleanup_failed',
          deferred: 'desktop_extension_plugin_secret_cleanup_deferred',
          unavailable: 'desktop_extension_plugin_secret_cleanup_unavailable',
        }[pluginSecretCleanupNotice.kind])}</span>
        {pluginSecretCleanupNotice.restartRequired && <span>{tr('desktop_extension_plugin_secret_cleanup_restart_pending')}</span>}
        {(pluginSecretCleanupNotice.kind === 'failed' || pluginSecretCleanupNotice.kind === 'deferred') && pluginSecretCleanupNotice.pluginId && (
          <button type="button" disabled={busy} onClick={() => { void retryPluginSecretCleanup(); }} style={ghostButtonStyle(t, busy)}>
            {tr('desktop_extension_plugin_secret_cleanup_retry')}
          </button>
        )}
      </div>}
      {pageError && selection.kind === 'list' && <div role="alert">{pageError}</div>}
      {linkRequest?.payload && <p role="status">{tr('desktop_extension_plugin_import')}</p>}
      {!isPluginDetail && <>
        <ExtensionHubHeader title={tr('desktop_extension_plugins')} description={tr('desktop_extension_plugin_description')} search={search} onSearch={setSearch} actions={<>
          <button type="button" onClick={refreshCatalog} disabled={busy} className="extension-hub-icon-button" aria-label={tr('desktop_extension_refresh_plugins')}><ExtensionGlyph name="refresh" size={16} /></button>
          <button type="button" onClick={() => requestSelection({ kind: 'policies' })} className="extension-hub-icon-button" aria-label={tr('desktop_extension_plugin_settings')}><ExtensionGlyph name="cog" size={16} /></button>
          <details className="extension-hub-menu" open={addMenuOpen} onToggle={(event) => setAddMenuOpen(event.currentTarget.open)}>
            <summary className="extension-hub-primary">{tr('workspace_confirm_add')} <ExtensionGlyph name="chevron" size={13} /></summary>
            <div className="extension-hub-menu-popover">
              <button type="button" onClick={() => { setCatalogTab('public'); setAddMenuOpen(false); }}>{tr('desktop_extension_browse_plugins')}</button>
              <button type="button" onClick={() => requestSelection({ kind: 'marketplace-add' })}>{tr('desktop_extension_add_marketplace')}</button>
            </div>
          </details>
        </>} />
        {navigationPrompt}
        <DomainOperationBanner t={t} operation={pluginOperation} fallbackDomainLabel={tr('desktop_extension_plugins')} />
        {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
        <nav className="extension-catalog-tabs" aria-label={tr('desktop_extension_plugin_catalog')}>
          <button type="button" aria-pressed={catalogTab === 'public'} onClick={() => setCatalogTab('public')}>{tr('desktop_extension_public')}</button>
          <button type="button" aria-pressed={catalogTab === 'personal'} onClick={() => setCatalogTab('personal')}>{tr('desktop_personal')}</button>
        </nav>
        {catalogTab === 'public' && publicDirectoryGroups.map((group) => <section className="extension-catalog-section" key={`featured-${group.category}`}>
          <h2 className="extension-hub-group-title">{tr('desktop_extension_featured_sources')} · {publicCategoryLabel(group.category)}</h2>
          <div className="extension-hub-list">
            {group.entries.map((entry) => {
              const marketplace = entry.marketplace;
              const sourceAction = marketplace ? decidePublicMarketplaceSourceAction(marketplace, catalog.marketplaces ?? []) : null;
              const pluginId = publicMarketplacePluginTargetId(marketplace, sourceAction);
              const pluginInstalled = pluginId ? pluginIds.includes(pluginId) : false;
              const pluginAvailable = pluginId ? availablePlugins.some((available) => available.id === pluginId) : false;
              const registeredMarketplace = sourceAction?.kind === 'registered';
              const sourceConflict = sourceAction?.kind === 'source-conflict';
              return <div key={entry.id} className="extension-hub-row">
                <ExtensionIcon />
                <div className="extension-hub-row-copy">
                  <span className="extension-hub-row-title">{entry.name}</span>
                  <span className="extension-hub-row-description">{localizedDirectoryText(entry.description)}</span>
                  <span className="extension-hub-row-description">{entry.publisher}{sourceConflict ? ` · ${tr('desktop_extension_marketplace_source_conflict_short')}` : registeredMarketplace && !pluginAvailable ? ` · ${tr('desktop_extension_marketplace_added')}` : ''}</span>
                  {entry.requirements && <span className="extension-hub-row-description">{tr('desktop_extension_requirements')}: {localizedDirectoryText(entry.requirements)}</span>}
                </div>
                <div className="extension-hub-row-side">
                  {marketplace ? <button
                    type="button"
                    className="extension-hub-icon-button"
                    aria-label={tr(sourceConflict ? 'desktop_extension_resolve_marketplace_conflict_named' : pluginInstalled ? 'desktop_extension_manage_named' : pluginAvailable ? 'desktop_extension_install_named' : registeredMarketplace ? 'desktop_extension_browse_marketplace_named' : 'desktop_extension_add_marketplace_named', entry.name)}
                    onClick={() => openPublicDirectoryItem(entry)}
                  ><ExtensionGlyph name={sourceConflict || pluginInstalled || (registeredMarketplace && !pluginAvailable) ? 'more' : 'plus'} size={20} /></button> : <>
                    <a className="extension-hub-icon-button" href={entry.documentationUrl} target="_blank" rel="noopener noreferrer" aria-label={tr('desktop_extension_open_documentation_named', entry.name)} title={tr('desktop_extension_open_documentation')}><ExtensionGlyph name="chevronR" size={16} /></a>
                    <a className="extension-hub-icon-button" href={entry.sourceUrl} target="_blank" rel="noopener noreferrer" aria-label={tr('desktop_extension_open_source_named', entry.name)} title={tr('desktop_extension_open_source')}><ExtensionGlyph name="plugin" size={16} /></a>
                  </>}
                </div>
              </div>;
            })}
          </div>
        </section>)}
        {catalogTab === 'public' && publicGroups.map((marketplace) => <section className="extension-catalog-section" key={marketplace}>
          <h2 className="extension-hub-group-title">{marketplace} <ExtensionGlyph name="chevronR" size={14} /></h2>
          <div className="extension-hub-list">
            {filteredAvailable.filter((entry) => entry.marketplace === marketplace).map((entry) => <div key={entry.id} className={`extension-hub-row${entry.installed ? ' is-installed' : ''}`}>
              <ExtensionIcon />
              <button type="button" className="extension-hub-row-copy extension-hub-row-open" onClick={() => requestSelection({ kind: pluginIds.includes(entry.id) ? 'plugin' : 'available', id: entry.id })}>
                <span className="extension-hub-row-title">{entry.name}</span><span className="extension-hub-row-description">{entry.description || entry.name}</span>
              </button>
              <button type="button" className="extension-hub-icon-button" aria-label={tr(entry.installed ? 'desktop_extension_manage_named' : 'desktop_extension_install_named', entry.name)} onClick={() => requestSelection({ kind: pluginIds.includes(entry.id) ? 'plugin' : 'available', id: entry.id })}><ExtensionGlyph name={entry.installed ? 'more' : 'plus'} size={20} /></button>
            </div>)}
          </div>
        </section>)}
        {catalogTab === 'personal' && <section className="extension-catalog-section">
          <h2 className="extension-hub-group-title">{tr('apps_installed')}</h2>
          <div className="extension-hub-list">
            {filteredPlugins.map((id) => {
              const entry = (catalog.installed ?? []).find((candidate) => (candidate.id || candidate.name) === id);
              return <div key={id} className="extension-hub-row configuration-entry">
                <ExtensionIcon />
                <button type="button" className="extension-hub-row-copy extension-hub-row-open" onClick={() => requestSelection({ kind: 'plugin', id })}>
                  <span className="extension-hub-row-title">{entry?.display_name ?? entry?.name ?? id}</span><span className="extension-hub-row-description">{entry?.description ?? tr('desktop_extension_installed_plugin')}</span>
                </button>
                <button type="button" className="extension-hub-icon-button" aria-label={tr('desktop_extension_configure_named', entry?.display_name ?? id)} onClick={() => requestSelection({ kind: 'plugin', id })}><ExtensionGlyph name="more" size={16} /></button>
              </div>;
            })}
          </div>
          {!filteredPlugins.length && <div className="extension-hub-empty">{search ? tr('desktop_extension_no_plugins_search') : tr('desktop_extension_no_plugins')}</div>}
        </section>}
        {catalogTab === 'public' && search && !hasPublicResults && <div className="extension-hub-empty">{tr('desktop_extension_no_public_plugins_search')}</div>}
        {catalogTab === 'public' && !hasPublicResults && !search && <button type="button" className="extension-see-more" onClick={() => requestSelection({ kind: 'marketplace-add' })}>{tr('desktop_extension_discover_plugins')} <ExtensionGlyph name="chevronR" size={14} /></button>}
        {marketplaceNames.length > 0 && <details className="extension-hub-marketplaces">
          <summary>{tr('desktop_extension_marketplaces_count', marketplaceNames.length)}</summary>
          <div className="extension-hub-list">{filteredMarketplaces.map((name) => <button key={name} type="button" className="extension-hub-row extension-hub-row-open" onClick={() => requestSelection({ kind: 'marketplace', name })}><ExtensionIcon /><span className="extension-hub-row-copy"><span className="extension-hub-row-title">{name}</span><span className="extension-hub-row-description">{asString(asRecord(draftMarketplaces[name]).source, tr('settings_parity_marketplace_source'))}</span></span></button>)}</div>
        </details>}
      </>}
        {selection.kind !== 'list' && <div className={isPluginDetail ? "extension-plugin-detail" : "extension-detail-backdrop"} onMouseDown={(event) => { if (!isPluginDetail && event.target === event.currentTarget) requestSelection({ kind: 'list' }); }}>
          <section ref={isPluginDetail ? detailPageRef : dialogRef} tabIndex={-1} className={isPluginDetail ? "extension-plugin-page" : "extension-detail-dialog"} role={isPluginDetail ? undefined : "dialog"} aria-modal={isPluginDetail ? undefined : true} aria-label={tr('plugins_detail_title')} style={extensionHubStyle(t)}>
            {isPluginDetail ? <>
              <nav className="extension-breadcrumb" aria-label={tr('desktop_extension_breadcrumb')}><button type="button" onClick={() => requestSelection({ kind: 'list' })}>{tr('desktop_extension_plugins')}</button><ExtensionGlyph name="chevronR" size={15} /><span>{detailName}</span></nav>
              <header className="extension-plugin-hero">
                <ExtensionIcon large />
                <div className="extension-plugin-title-row"><h1>{detailName}</h1><div className="extension-hub-toolbar-actions">
                  {selectedPlugin ? <button type="button" className="extension-hub-toggle" role="switch" aria-checked={draftEnabled[selectedPlugin] !== false} aria-label={tr(draftEnabled[selectedPlugin] !== false ? 'desktop_extension_disable_named' : 'desktop_extension_enable_named', detailName)} disabled={busy} onClick={() => previewLifecycle({ action: draftEnabled[selectedPlugin] !== false ? 'disable' : 'enable', plugin: selectedPlugin, scope: editingLayer })} /> : <button type="button" disabled={busy || !adminAvailable} className="extension-hub-primary" onClick={() => selectedAvailable && previewLifecycle({ action: selectedAvailable.installed ? 'update' : 'install', plugin: selectedAvailable.id, scope: editingLayer })}><ExtensionGlyph name="plus" size={16} />{selectedAvailable?.installed ? tr('desktop_extension_update_plugin') : tr('desktop_extension_install_plugin')}</button>}
                </div></div>
                <p>{detailDescription}</p>
              </header>
            </> : <header className="extension-detail-head">
              <span className="extension-hub-icon"><ExtensionGlyph name="plugin" size={20} color={t.text3} /></span>
              <div className="extension-detail-head-copy">
                <div className="extension-detail-title-line">
                  <span className="extension-detail-title">{selectedPlugin ? manifest?.display_name ?? selectedPlugin : selectedAvailable?.name ?? selectedMarketplace ?? (selection.kind === 'marketplace-add' ? tr('plugins_marketplace_add') : tr('desktop_extension_plugin_policies'))}</span>
                  <span className="extension-detail-kind">{tr('settings_title_main')}</span>
                </div>
              </div>
              <button type="button" className="extension-hub-icon-button" data-dialog-initial-focus aria-label={tr('desktop_extension_close_details')} onClick={() => requestSelection({ kind: 'list' })}><ExtensionGlyph name="x" size={16} /></button>
            </header>}
            <div className="extension-detail-content">
            {navigationPrompt}
            <DomainOperationBanner t={t} operation={pluginOperation} fallbackDomainLabel={tr('desktop_extension_plugins')} />
            {pageError && <div role="alert" style={noteStyle(t, 'danger')}>{pageError}</div>}
            {secretNotice && <div role="status" style={noteStyle(t, secretNotice.restartRequired ? 'warn' : 'neutral')}>{secretNotice.text}</div>}
            {pendingSelection && (
              <div style={noteStyle(t, 'warn')}>
                {tr('desktop_extension_unsaved_configuration')} <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button type="button" onClick={() => { setDraftEnabled(enabledPlugins); setDraftConfigs(pluginConfigs); setDraftMarketplaces(marketplaces); setMarketplaceSource(''); setPreparedOperation(null); setSelection(pendingSelection); setPendingSelection(null); }} style={ghostButtonStyle(t)}>{tr('desktop_extension_discard_switch')}</button>
                  <button type="button" onClick={() => setPendingSelection(null)} style={ghostButtonStyle(t, false, true)}>{tr('desktop_extension_continue_editing')}</button>
                </div>
              </div>
            )}
            {preparedOperation && (
              <div style={noteStyle(t, 'warn')}>
                <div>{preparedOperation.summary}</div>
                {preparedOperation.payload.action !== 'save_config' && (
                  <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                    <button type="button" disabled={busy} onClick={confirmLifecycle} style={ghostButtonStyle(t, busy)}>{tr('desktop_extension_confirm_operation')}</button>
                    <button type="button" disabled={busy} onClick={() => setPreparedOperation(null)} style={ghostButtonStyle(t, busy, true)}>{tr('common_cancel')}</button>
                  </div>
                )}
              </div>
            )}
            {isPluginDetail && <>
              <div className="extension-plugin-showcase"><ExtensionIcon large /><div><strong>{detailName}</strong><p>{detailDescription || tr('desktop_extension_plugin_fallback')}</p></div></div>
              {relatedSkills.length > 0 && <section><h2 className="extension-detail-section-title">{tr('settings_title_skills')} <span>{relatedSkills.length}</span></h2>{relatedSkills.map((skill) => <button type="button" key={skill.id} className="extension-hub-row extension-hub-row-open" onClick={() => requestHubNavigation('skills')}><ExtensionIcon kind="skill" /><span className="extension-hub-row-copy"><span className="extension-hub-row-title">{skill.name}</span><span className="extension-hub-row-description">{skill.description}</span></span><ExtensionGlyph name="chevronR" size={16} /></button>)}</section>}
              <section><h2 className="extension-detail-section-title">{tr('desktop_extension_information')}</h2><dl className="extension-plugin-information">
                <dt>{tr('settings_linux_version')}</dt><dd>{manifest?.version ?? selectedAvailable?.version ?? '—'}</dd>
                <dt>{tr('audio_source_title')}</dt><dd>{selectedAvailable?.marketplace ?? tr('desktop_extension_installed_local')}</dd>
                <dt>{tr('session_details_status')}</dt><dd>{selectedPlugin ? draftEnabled[selectedPlugin] === false ? tr('settings_badge_disabled') : tr('plugins_enabled') : selectedAvailable?.installed ? tr('apps_installed') : tr('desktop_extension_available_install')}</dd>
                {manifest?.path && <><dt>{tr('desktop_extension_location')}</dt><dd>{manifest.path}</dd></>}
              </dl></section>
            </>}
            {selection.kind === 'plugin' && selectedPlugin && (
              <details className="extension-plugin-configuration">
                <summary>{tr('desktop_extension_configuration')} <ExtensionGlyph name="chevron" size={14} /></summary>
                <div className="extension-plugin-config-body">
                <Row title={tr('desktop_extension_runtime_status')} desc={pluginDependencyCaveat()} align="center">
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: draftEnabled[selectedPlugin] === false ? 'enable' : 'disable', plugin: selectedPlugin, scope: editingLayer })} style={ghostButtonStyle(t, busy)}>
                    {tr(draftEnabled[selectedPlugin] === false ? 'desktop_extension_preview_enable' : 'desktop_extension_preview_disable')}
                  </button>
                </Row>
                {(manifest?.dependencies?.length ?? 0) > 0 && (
                  <div style={noteStyle(t)}>{tr('desktop_extension_dependencies')}{manifest?.dependencies?.join('、')}</div>
                )}
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'update', plugin: selectedPlugin, scope: editingLayer })} style={ghostButtonStyle(t, busy)}>{tr('desktop_extension_preview_upgrade')}</button>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'uninstall', plugin: selectedPlugin, scope: editingLayer })} style={ghostButtonStyle(t, busy, true)}>{tr('desktop_extension_preview_uninstall')}</button>
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
                          placeholder={tr('desktop_extension_one_per_line')}
                        />
                        {field.description && <div style={secondaryMetaStyle(t)}>{field.description}</div>}
                      </Field>
                    );
                  }
                  if (field.type === 'boolean') {
                    return (
                      <Field key={key} t={t} label={label}>
                        <select value={currentValue === undefined ? '' : currentValue ? 'true' : 'false'} onChange={(event) => updateOption(key, event.target.value === '' ? undefined : event.target.value === 'true')} style={inputStyle(t)} aria-label={`${key} option`}>
                          <option value="">{tr('common_unset')}</option>
                          <option value="true">{tr('settings_status_on')}</option>
                          <option value="false">{tr('settings_status_off')}</option>
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
                    aria-label={tr('desktop_extension_configuration_named', selectedPlugin)}
                  />
                </Field>
                {Object.entries(configSchema.fields ?? {}).filter(([, field]) => field.sensitive).map(([key, field]) => (
                  <Field key={key} t={t} label={field.title ?? key}>
                    <div style={{ display: 'grid', gap: 6 }}>
                      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                        <input type="password" value={secretDrafts[key] ?? ''} onChange={(event) => setSecretDrafts((current) => ({ ...current, [key]: event.target.value }))} placeholder={manifest?.secret_configured?.[key] ? tr('desktop_extension_secret_configured') : tr('desktop_extension_secret_placeholder')} style={{ flex: 1, minWidth: 0 }} aria-label={tr('desktop_extension_secret_named', field.title ?? key)} />
                        <button type="button" disabled={!secretDrafts[key] || busy} onClick={() => { const secret = secretDrafts[key]; if (!secret) return; setBusy(true); setSecretNotice(null); void bridge.setPluginSecret(selectedPlugin, key, secret).then((metadata) => { setSecretDrafts((current) => ({ ...current, [key]: '' })); setSecretNotice({ text: metadata.restartRequired ? tr('desktop_extension_secret_saved_restart') : tr('desktop_extension_secret_saved'), restartRequired: Boolean(metadata.restartRequired) }); return callPluginAdmin(bridge, adminRecordCommand('get_catalog')); }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_secret_save_failed'))).finally(() => setBusy(false)); }} style={ghostButtonStyle(t, !secretDrafts[key] || busy)}>{tr('desktop_extension_secure_save')}</button>
                        {manifest?.secret_configured?.[key] && <button type="button" disabled={busy} onClick={() => { setBusy(true); setSecretNotice(null); void bridge.clearPluginSecret(selectedPlugin, key).then((metadata) => { setSecretNotice({ text: metadata.restartRequired ? tr('desktop_extension_secret_cleared_restart') : tr('desktop_extension_secret_cleared'), restartRequired: Boolean(metadata.restartRequired) }); return callPluginAdmin(bridge, adminRecordCommand('get_catalog')); }).catch((cause: unknown) => setPageError(cause instanceof Error ? cause.message : tr('desktop_extension_secret_clear_failed'))).finally(() => setBusy(false)); }} style={ghostButtonStyle(t, busy, true)}>{tr('settings_provider_clear_key')}</button>}
                        {manifest?.secret_configured?.[key] && <SourcePill t={t} label={tr('settings_parity_secret_configured')} tone="success" />}
                      </div>
                      <div style={secondaryMetaStyle(t)}>{field.description ?? ''}{field.required ? ` · ${tr('desktop_extension_required')}` : ''}</div>
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
                {selectedMarketplaceConflictRow && selectedMarketplaceConflict && <div role="alert" style={noteStyle(t, 'warn')}>
                  {tr('desktop_extension_marketplace_source_conflict', selectedMarketplace, selectedMarketplaceConflictRow.source_json ?? tr('settings_parity_marketplace_source'), selectedMarketplaceConflict)}
                </div>}
                <Field t={t} label={tr('desktop_extension_marketplace_json')}>
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
                  <button type="button" disabled={busy || Boolean(selectedMarketplaceConflictRow)} onClick={() => previewLifecycle({ action: 'marketplace_update', name: selectedMarketplace, scope: editingLayer })} style={ghostButtonStyle(t, busy || Boolean(selectedMarketplaceConflictRow))}>{tr('desktop_extension_preview_update')}</button>
                  <button type="button" disabled={busy} onClick={() => previewLifecycle({ action: 'marketplace_remove', name: selectedMarketplace, scope: editingLayer })} style={ghostButtonStyle(t, busy, true)}>{tr('desktop_extension_preview_remove')}</button>
                </div>
              </>
            )}
            {selection.kind === 'marketplace-add' && (
              <>
                <EmptyDetail t={t} title={tr('plugins_marketplace_add')} body={tr('desktop_extension_add_marketplace_hint')} />
                <Field t={t} label={tr('audio_source_title')}>
                  <input value={marketplaceSource} onChange={(event) => setMarketplaceSource(event.target.value)} placeholder={tr('desktop_extension_marketplace_placeholder')} aria-label={tr('settings_parity_marketplace_source')} style={{ width: '100%' }} />
                </Field>
                <button type="button" disabled={!marketplaceSource.trim() || busy} onClick={() => previewLifecycle({ action: 'marketplace_add', source: marketplaceSource.trim(), scope: editingLayer })} style={ghostButtonStyle(t, !marketplaceSource.trim() || busy)}>{tr('desktop_extension_preview_add')}</button>
              </>
            )}
            {selection.kind === 'policies' && (
              <>
                <EmptyDetail t={t} title={tr('desktop_shell_policies')} body={tr('desktop_extension_policies_readonly')} />
                <Field t={t} label={tr('desktop_extension_policies_json')}>
                  <textarea value={catalog.policies_json ?? '{}'} readOnly rows={12} style={textareaStyle(t, 12)} aria-label="plugin-policies-json" />
                </Field>
              </>
            )}
            {dirty && <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
              <button type="button" disabled={!dirty || busy} onClick={preview} style={ghostButtonStyle(t, !dirty || busy)}>{tr('desktop_extension_preview_configuration')}</button>
              <button type="button" disabled={!dirty || busy || Boolean(selectedMarketplaceConflictRow)} onClick={save} style={ghostButtonStyle(t, !dirty || busy || Boolean(selectedMarketplaceConflictRow))}>{tr('desktop_extension_save_configuration')}</button>
              <button type="button" disabled={!dirty || busy} onClick={() => { setDraftEnabled(enabledPlugins); setDraftConfigs(pluginConfigs); setDraftMarketplaces(marketplaces); }} style={ghostButtonStyle(t, !dirty || busy, true)}>{tr('desktop_extension_cancel_changes')}</button>
            </div>}
            </div>
          </section>
        </div>}

      {!isPluginDetail && <details className="extension-hub-page extension-hub-secondary-settings" style={extensionHubStyle(t)}>
        <summary>{tr('desktop_extension_settings_sources')}</summary>
        <Card title={tr('desktop_extension_source_description')}>
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="enabledPlugins" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="pluginConfigs" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
          <div style={{ padding: '12px 18px', fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
            {tr('desktop_extension_secret_storage_description')}</div>
        </Card>
      </details>}
    </ExtensionHubLayout>
  );
}
