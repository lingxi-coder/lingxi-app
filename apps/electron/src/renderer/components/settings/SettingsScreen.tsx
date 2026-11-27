import { useI18n } from '../../i18n/useI18n';
import { tr } from '../../i18n/locale';
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ComponentType, type CSSProperties, type ReactNode } from 'react';

import type { SettingsLinkRequest } from '../../deepLinkRoutes';
import { dialogFocusTarget } from '../../bridge/bridgeRuntimeState.js';
import type { SettingsSnapshotEvent, UseBridge } from '../../bridge/bridgeTypes.js';
import { useT } from '../../theme/ThemeContext';
import type { ThemeMode } from '../../theme/tokens';
import { Icon } from '../Icon';
import { SETTINGS_NAV, searchNav, navPageLabel, navGroupLabel, type NavPage } from './nav';
import { ExtensionGlyph } from './pages/ExtensionHubIcons';

// renderToStaticMarkup cannot wait on import(). Node tests preload the ESM graph; the packaged renderer still loads a page when it is shown.
function nodeTestPreload(): boolean {
  const proc = (globalThis as { process?: NodeJS.Process }).process;
  return proc?.release?.name === 'node' && !proc.versions?.electron;
}
function bindSettingsPage(exportName: string, load: () => Promise<unknown>) {
  return lazy(() => load().then((module) => ({
    default: (module as Record<string, ComponentType<PageContentProps>>)[exportName]!,
  })));
}
const settingsPageLoaders = {
  Decision: () => import('./pages/Decision'),
  About: () => import('./pages/About'),
  Account: () => import('./pages/Account'),
  Appearance: () => import('./pages/Appearance'),
  Notifications: () => import('./pages/Notifications'),
  CustomProviders: () => import('./pages/CustomProviders'),
  Fusion: () => import('./pages/Fusion'),
  Diagnostics: () => import('./pages/Diagnostics'),
  General: () => import('./pages/General'),
  Hooks: () => import('./pages/Hooks'),
  McpServers: () => import('./pages/McpServers'),
  Permissions: () => import('./pages/Permissions'),
  ComputerUse: () => import('./pages/ComputerUse'),
  Plugins: () => import('./pages/Plugins'),
  ArchivedChats: () => import('./pages/ArchivedChats'),
  Projects: () => import('./pages/Projects'),
  RemoteControl: () => import('./pages/RemoteControl'),
  ProviderCredentials: () => import('./pages/ProviderCredentials'),
  Skills: () => import('./pages/Skills'),
  ToolsAgent: () => import('./pages/ToolsAgent'),
  Voice: () => import('./pages/Voice'),
};
const preloadSettingsPages = nodeTestPreload();
const eagerSettingsPages: Partial<Record<keyof typeof settingsPageLoaders, ComponentType<PageContentProps>>> = preloadSettingsPages
  ? Object.fromEntries(await Promise.all(Object.entries(settingsPageLoaders).map(async ([exportName, load]) => {
      const module = await load() as Record<string, ComponentType<PageContentProps>>;
      return [exportName, module[exportName]!];
    })))
  : {};
const Decision = eagerSettingsPages.Decision ?? bindSettingsPage('Decision', settingsPageLoaders.Decision);
const About = eagerSettingsPages.About ?? bindSettingsPage('About', settingsPageLoaders.About);
const Account = eagerSettingsPages.Account ?? bindSettingsPage('Account', settingsPageLoaders.Account);
const Appearance = eagerSettingsPages.Appearance ?? bindSettingsPage('Appearance', settingsPageLoaders.Appearance);
const Notifications = eagerSettingsPages.Notifications ?? bindSettingsPage('Notifications', settingsPageLoaders.Notifications);
const CustomProviders = eagerSettingsPages.CustomProviders ?? bindSettingsPage('CustomProviders', settingsPageLoaders.CustomProviders);
const Fusion = eagerSettingsPages.Fusion ?? bindSettingsPage('Fusion', settingsPageLoaders.Fusion);
const Diagnostics = eagerSettingsPages.Diagnostics ?? bindSettingsPage('Diagnostics', settingsPageLoaders.Diagnostics);
const General = eagerSettingsPages.General ?? bindSettingsPage('General', settingsPageLoaders.General);
const Hooks = eagerSettingsPages.Hooks ?? bindSettingsPage('Hooks', settingsPageLoaders.Hooks);
const McpServers = eagerSettingsPages.McpServers ?? bindSettingsPage('McpServers', settingsPageLoaders.McpServers);
const Permissions = eagerSettingsPages.Permissions ?? bindSettingsPage('Permissions', settingsPageLoaders.Permissions);
const ComputerUse = eagerSettingsPages.ComputerUse ?? bindSettingsPage('ComputerUse', settingsPageLoaders.ComputerUse);
const Plugins = eagerSettingsPages.Plugins ?? bindSettingsPage('Plugins', settingsPageLoaders.Plugins);
const ArchivedChats = eagerSettingsPages.ArchivedChats ?? bindSettingsPage('ArchivedChats', settingsPageLoaders.ArchivedChats);
const Projects = eagerSettingsPages.Projects ?? bindSettingsPage('Projects', settingsPageLoaders.Projects);
const RemoteControl = eagerSettingsPages.RemoteControl ?? bindSettingsPage('RemoteControl', settingsPageLoaders.RemoteControl);
const ProviderCredentials = eagerSettingsPages.ProviderCredentials ?? bindSettingsPage('ProviderCredentials', settingsPageLoaders.ProviderCredentials);
const Skills = eagerSettingsPages.Skills ?? bindSettingsPage('Skills', settingsPageLoaders.Skills);
const ToolsAgent = eagerSettingsPages.ToolsAgent ?? bindSettingsPage('ToolsAgent', settingsPageLoaders.ToolsAgent);
const Voice = eagerSettingsPages.Voice ?? bindSettingsPage('Voice', settingsPageLoaders.Voice);
import { provenanceDescription, provenanceLabel, type Provenance } from './rows';
import { projectDirFromSnapshot, projectDisplayName } from './useEngineSettings';
import { ExtensionCatalogTabs, extensionHubStyle } from './pages/ExtensionHub';
import { useProjectSettingsBridge } from './projectSettingsBridge';
import { projectKey, useExtensionScopes, type ExtensionScopeNavigation } from './extensionScopes';
import type { SettingsSnapshot } from './useEngineSettings';

/**
 * The three settings-file layers a person can actually pick from up here.
 * `device` (Electron-store settings) and `managed` (policy overlay) are real
 * `Provenance` values but neither is an editable destination: `device` never
 * goes through this snapshot at all, and `managed` always wins, so offering
 * it as a tab a person could "write to" would lie about what happens next.
 */
export type EditableLayer = Extract<Provenance, 'user' | 'project' | 'local'>;
const VISIBLE_SETTINGS_LAYERS: EditableLayer[] = ['user', 'project'];

const GROUP_ORDER: NavPage['group'][] = ['个人', '模型与服务', '编码', '高级'];

const LAYER_DISABLED_REASON_ID = 'settings-layer-switcher-disabled-reason';

/**
 * Props were built field-for-field isomorphic to the old settings modal's
 * props (`BetaDesktop.tsx`, retired in Task 20) so that task could swap
 * `<SettingsScreen ... />` into `App.tsx` in place of the modal without
 * touching the call site's shape — same field names, same
 * optionality, same deep-link contract (`initialProviderId` +
 * `pendingModelReference` are what the model picker uses to open Settings on
 * the provider blocking a model; `onClose` is expected to restore focus to
 * whatever opened it, which is the caller's job, not this component's — see
 * the mount effect below for the DIFFERENT focus concern this component DOES
 * own: trapping focus inside the dialog while it's open).
 */
export interface SettingsScreenProps {
  bridge: UseBridge;
  theme: ThemeMode;
  onTheme(value: ThemeMode): void;
  onClose(): void;
  initialPageId?: string;
  initialProviderId?: string;
  linkRequest?: SettingsLinkRequest;
  pendingModelReference?: string;
}

/** Validate a requested settings page before choosing its initial navigation entry. */
export function resolveInitialPage(initialPageId?: string, initialProviderId?: string): string {
  if (initialPageId && SETTINGS_NAV.some((page) => page.id === initialPageId)) return initialPageId;
  if (initialProviderId) return 'provider-credentials';
  return SETTINGS_NAV[0]?.id ?? 'general';
}

/** Whether a settings-file layer tab should be disabled. `user` needs no project; `project` and `local` both live inside a project directory and cannot be edited without one open. */
export function layerDisabled(layer: EditableLayer, hasProject: boolean): boolean {
  return layer !== 'user' && !hasProject;
}

/** The nav grouped by `NavPage.group`, in a fixed display order, filtered to whatever `query` matches (via `nav.ts`'s own `searchNav`). A group with no visible pages is omitted entirely rather than rendered with an empty body. */
export function groupedNav(query: string): Array<{ group: NavPage['group']; pages: NavPage[] }> {
  const matches = query.trim() ? new Set(searchNav(query).map((page) => page.id)) : null;
  return GROUP_ORDER
    .map((group) => ({
      group,
      pages: SETTINGS_NAV.filter((page) => page.group === group && (!matches || matches.has(page.id))),
    }))
    .filter((section) => section.pages.length > 0);
}

/**
 * Turns the wire event's JSON-string fields into the structured shape
 * `useEngineSettings.ts` works with. `ClientEvent::SettingsSnapshot` carries
 * `effective_json` / `provenance_json` as required strings and
 * `files_json` / `active_json` / `locked` / `layers_json` / `merged_keys` as
 * optional ones —
 * the contract crate excludes `serde_json::Value` (not UniFFI-representable),
 * so this parse has to happen somewhere on the TypeScript side, and it
 * happens here rather than inside `useEngineSettings.ts` so that module's
 * six-state machine stays JSON-free and pure. A malformed payload becomes an
 * error result, never a thrown exception — a broken settings file must not
 * take the whole settings screen down with it.
 *
 */
export function parseSettingsSnapshot(
  raw: SettingsSnapshotEvent | null | undefined,
): { snapshot: SettingsSnapshot | null; error: string | null } {
  if (!raw) return { snapshot: null, error: null };
  try {
    const effective = JSON.parse(raw.effective_json) as Record<string, unknown>;
    const active = raw.active_json ? JSON.parse(raw.active_json) as Record<string, unknown> : undefined;
    const provenance = JSON.parse(raw.provenance_json) as Record<string, string>;
    const files = raw.files_json ? (JSON.parse(raw.files_json) as SettingsSnapshot['files']) : [];
    const locked = raw.locked ?? [];
    // A producer that predates `layers_json` (additive, §0.10) leaves every
    // layer looking empty rather than throwing — the same "we don't know"
    // default `active`'s own fallback comment argues for, not a crash.
    const layers = raw.layers_json ? (JSON.parse(raw.layers_json) as Record<string, Record<string, unknown>>) : {};
    // A producer that predates `merged_keys` (additive, §0.10) reports no
    // merged keys rather than all of them: "we don't know of any" is the
    // honest reading of an absent field, the same default `layers` takes,
    // and it leaves such a client exactly where it was before this field
    // existed instead of suppressing every provenance badge it can draw.
    const mergedKeys = raw.merged_keys ?? [];
    return {
      snapshot: { effective, active, provenance, files, locked, layers, mergedKeys },
      error: null,
    };
  } catch (cause) {
    return {
      snapshot: null,
      error: cause instanceof Error ? cause.message : 'The settings snapshot could not be parsed.',
    };
  }
}

/**
 * Where a page's real content lives once its task builds it. Task 15 left
 * this empty — every page fell back to the "not wired yet" placeholder.
 * Task 16 registers the five pages whose values live in the Electron store
 * rather than any engine settings layer (they need no engine, which is why
 * `nav.ts` marks all five `needsEngine: false`); Tasks 17-19 register the
 * rest as they land. A page id still absent from this map falls back to
 * `PagePlaceholder`'s "not wired yet" message instead of a blank panel.
 *
 * Task 17 adds the two provider pages: `provider-credentials` (secure
 * credential storage — NOT layered, `nav.ts` marks it `layered: false`) and
 * `custom-providers` (edits `settings.providers` / `settings.routing`,
 * genuinely layered despite sitting outside the 编码 group — see `nav.ts`'s
 * own exception comment).
 *
 * Task 18 adds the six 编码 pages: `permissions` (the three dedicated
 * permission commands, never the generic patch), `tools-agent`
 * (`enabledTools`/`disable*`/`outputStyle`/`modelOverrides`/thinking &
 * vision toggles via the generic patch), `skills` (directory-discovered
 * listing + the one layered row, `syncClaudeAiSkills`), `mcp` (its own
 * three-scope selector, NOT the shell's layer switcher — `nav.ts` marks it
 * `layered: false`), `hooks` (configuration editor + a jump to diagnostics),
 * and `plugins` (`enabledPlugins`/`pluginConfigs`/`additionalMarketplaces`
 * via the generic patch).
 *
 * Task 9 of the desktop-audio-capability plan adds `voice`: the one
 * `SETTINGS_NAV` entry this task's own predecessors (Tasks 15-19) left
 * `implemented: false` on purpose, since the page's contents (Tasks 4-8's
 * voice preferences/capability probe/capture/synthesis modules) did not
 * exist yet. With this, every page declared in `nav.ts` is implemented.
 */
// Exported (not just module-private) so a test can assert, statically, that
// every `implemented: true` `SETTINGS_NAV` entry has a real entry here —
// `Partial<Record<...>>` would otherwise accept a silently-omitted or
// typo'd key with no type error, and the only visible symptom would be the
// `not-wired` placeholder quietly standing in for a page that should exist.
export const PAGE_CONTENT: Partial<Record<string, ComponentType<PageContentProps>>> = {
  decision: Decision,
  general: General,
  account: Account,
  appearance: Appearance,
  notifications: Notifications,
  voice: Voice,
  projects: Projects,
  'remote-control': RemoteControl,
  'archived-chats': ArchivedChats,
  diagnostics: Diagnostics,
  about: About,
  'provider-credentials': ProviderCredentials,
  'custom-providers': CustomProviders,
  permissions: Permissions,
  'computer-use': ComputerUse,
  'tools-agent': ToolsAgent,
  fusion: Fusion,
  skills: Skills,
  mcp: McpServers,
  hooks: Hooks,
  plugins: Plugins,
};

export type SettingsNavigationRequest = (pageId: string, pluginId?: string) => void;

export interface PageContentProps {
  bridge: UseBridge;
  snapshot: SettingsSnapshot | null;
  editingLayer: EditableLayer;
  extensionScopes?: ExtensionScopeNavigation;
  settingsProject?: string;
  onSelectSettingsProject?: (path: string) => void;
  onLayerLockChange?(locked: boolean): void;
  theme: ThemeMode;
  onTheme(value: ThemeMode): void;
  /** Jumps the shell to another nav page by id — how `General`'s cross-page entries actually navigate, rather than just naming a destination they can't reach. */
  onNavigate(pageId: string, pluginId?: string): void;
  /** Lets a page protect unsaved drafts before the shared sidebar changes pages. */
  onRegisterNavigationRequest?(handler: SettingsNavigationRequest | null): void;
  /** Lets a page protect unsaved drafts before closing the settings surface. */
  onRegisterCloseRequest?(handler: (() => void) | null): void;
  initialPluginId?: string;
  initialProviderId?: string;
  linkRequest?: SettingsLinkRequest;
  pendingModelReference?: string;
  /**
   * Closes the WHOLE settings surface, not just this page. `ProviderCredentials`
   * needs this to reproduce the old settings modal's deep-link behaviour: once a
   * blocking model's provider connects and the pending model applies, the
   * original dialog closed itself and returned focus to the composer rather
   * than leaving the person parked on a settings page they didn't navigate to
   * on purpose.
   */
  onClose(): void;
  /**
   * Switches the shell's layer switcher to `layer` — the shell owns
   * `editingLayer`'s state (this is literally its `setEditingLayer`), so a
   * layered page cannot jump layers on its own. Exists so an
   * `OverriddenNotice`'s "前往该层" affordance actually does something
   * instead of being a dead button — Task 17 fix round 1: a banner offering
   * to jump somewhere and going nowhere is worse than no offer at all.
   */
  onJumpToLayer(layer: EditableLayer): void;
}

function PagePlaceholder({ page, kind }: { page: NavPage; kind: 'not-implemented' | 'not-wired' }) {
  const t = useT();
  const heading = tr(kind === 'not-implemented' ? 'desktop_shell_page_unimplemented' : 'desktop_shell_page_coming', navPageLabel(page));
  const body = kind === 'not-implemented'
    ? tr('desktop_shell_page_unbuilt')
    : tr('desktop_shell_page_unwired');
  return (
    <div data-testid="page-placeholder" data-placeholder-kind={kind} style={{
      marginTop: 8, padding: '40px 24px',
      background: t.surface, border: `0.5px dashed ${t.border}`, borderRadius: 12,
      display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 8, textAlign: 'center',
    }}>
      <div style={{ width: 44, height: 44, borderRadius: 12, background: t.surfaceHover, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        <Icon name={kind === 'not-implemented' ? 'info' : 'cog'} size={22} color={t.text3} stroke={1.5} />
      </div>
      <div style={{ fontSize: 13.5, color: t.text2, fontWeight: 500 }}>{heading}</div>
      <div style={{ fontSize: 12, color: t.text4, maxWidth: 420, lineHeight: 1.6 }}>{body}</div>
    </div>
  );
}

function EngineRequiredEmptyState({ page }: { page: NavPage }) {
  const t = useT();
  return (
    <div data-testid="engine-required-empty-state" style={{
      marginTop: 8, padding: '40px 24px',
      background: t.surface, border: `0.5px solid ${t.border}`, borderRadius: 12,
      display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 8, textAlign: 'center',
    }}>
      <Icon name="server" size={22} color={t.text3} stroke={1.5} />
      <div style={{ fontSize: 13.5, color: t.text2, fontWeight: 500 }}>{tr('desktop_shell_page_engine', navPageLabel(page))}</div>
      <div style={{ fontSize: 12, color: t.text4, maxWidth: 420, lineHeight: 1.6 }}>
        {tr('desktop_shell_engine_required_desc')}
      </div>
    </div>
  );
}

function LayerSwitcher({ value, onChange, hasProject, locked = false }: {
  locked?: boolean;
  value: EditableLayer;
  onChange(layer: EditableLayer): void;
  hasProject: boolean;
}) {
  const t = useT();
  return (
    <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 4 }}>
      <div
        data-testid="layer-switcher"
        role="group"
        aria-label={tr('desktop_shell_storage_scope')}
        className="settings-segmented"
        style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}
      >
        {VISIBLE_SETTINGS_LAYERS.map((layer) => {
          const active = value === layer;
          const disabled = locked || layerDisabled(layer, hasProject);
          return (
            <button
              key={layer}
              type="button"
              data-layer={layer}
              aria-pressed={active}
              disabled={disabled}
              aria-describedby={disabled ? LAYER_DISABLED_REASON_ID : undefined}
              onClick={() => onChange(layer)}
              style={{
                padding: '5px 14px', borderRadius: 7, border: 'none', fontFamily: 'inherit',
                cursor: disabled ? 'not-allowed' : 'pointer',
                background: active ? t.surface : 'transparent',
                color: active ? t.text : disabled ? t.text4 : t.text3,
                fontSize: 12.5, fontWeight: active ? 600 : 500,
              }}
            >
              {provenanceLabel(layer)}
            </button>
          );
        })}
      </div>
      {/* Visible, not just a `title=` tooltip: Chromium does not dispatch the
          pointer events a native tooltip needs on a DISABLED control, and a
          tooltip is invisible to keyboard/screen-reader users regardless. */}
      {!hasProject && (
        <div id={LAYER_DISABLED_REASON_ID} data-testid="layer-switcher-disabled-reason" style={{ fontSize: 11, color: t.text4 }}>
          {tr('desktop_shell_open_project_edit')}
        </div>
      )}
    </div>
  );
}

/** Settings owns the chosen directory. Its file path stays visible even for projects with identical names. */
function LayerContext({ bridge, layer, projectDir, locked, onProject }: {
  onProject: (path: string) => void;
  bridge: UseBridge;
  layer: EditableLayer;
  projectDir: string | null;
  locked: boolean;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const projects = bridge.bootstrap?.settings.projects ?? [];
  const needsProject = layer !== 'user';
  function handleSwitch(path: string) {
    onProject(path);
    setOpen(false);
  }

  return (
    <div data-testid="layer-context" style={{ marginTop: 10, display: 'flex', flexDirection: 'column', gap: 6 }}>
      <div data-testid="layer-description" style={{ fontSize: 12, color: t.text3, lineHeight: 1.6 }}>
        {provenanceDescription(layer)}
      </div>

      {needsProject && projectDir && (
        <div data-testid="layer-project" style={{
          display: 'flex', alignItems: 'center', gap: 9, padding: '7px 10px',
          borderRadius: 8, background: t.surface, border: `0.5px solid ${t.border}`,
        }}>
          <Icon name="folder" size={14} color={t.text3} stroke={1.7} />
          <div style={{ minWidth: 0, flex: 1 }}>
            <div data-testid="layer-project-name" style={{
              fontSize: 12.5, fontWeight: 600, color: t.text,
              overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
            }}>{projectDisplayName(projectDir)}</div>
            {/* 名字会重复（两个项目都叫 `app`），所以完整路径永远跟着一起显示。 */}
            <div className="mono" data-testid="layer-project-path" style={{
              fontSize: 10.5, color: t.text4,
              overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
            }}>{projectDir}</div>
          </div>
          <button
            type="button"
            data-testid="layer-project-switch"
            aria-expanded={open}
            disabled={locked}
            onClick={() => setOpen((previous) => !previous)}
            style={{
              padding: '4px 10px', borderRadius: 6, fontSize: 11.5, fontFamily: 'inherit',
              border: `0.5px solid ${t.border}`, background: t.surfaceHover,
              color: locked ? t.text4 : t.text2,
              cursor: locked ? 'not-allowed' : 'pointer',
            }}
          >{tr('settings_parity_switch_project')}</button>
        </div>
      )}

      {/* 快照还没到的时候不猜一个项目名：宁可说「还不知道」，也不要指着 B 写 A。 */}
      {needsProject && !projectDir && (
        <div data-testid="layer-project-unknown" style={{ fontSize: 11.5, color: t.text4 }}>
          {tr('settings_parity_project_unknown')}
          {projects.length > 0 && <button type="button" data-testid="layer-project-switch" disabled={locked}
            aria-expanded={open} onClick={() => setOpen(!open)}>{tr('settings_parity_switch_project')}</button>}
        </div>
      )}

      {open && (
        <div data-testid="layer-project-picker" style={{
          display: 'flex', flexDirection: 'column', gap: 2, padding: 6,
          borderRadius: 8, background: t.surface, border: `0.5px solid ${t.border}`,
        }}>
          {projects.length === 0 && (
            <div style={{ fontSize: 11.5, color: t.text4, padding: '4px 6px' }}>
              {tr('desktop_shell_add_project_hint')}
            </div>
          )}
          {projects.map((path) => (
            <button
              key={path}
              type="button"
              data-project-option={path}
              disabled={locked}
              onClick={() => handleSwitch(path)}
              style={{
                display: 'flex', alignItems: 'center', gap: 8, width: '100%',
                padding: '6px 8px', borderRadius: 6, border: 'none', textAlign: 'left',
                background: path === projectDir ? t.surfaceActive : 'transparent',
                color: t.text2, fontFamily: 'inherit', fontSize: 12,
                cursor: locked ? 'default' : 'pointer',
              }}
            >
              <span style={{ fontWeight: 600, flexShrink: 0 }}>{projectDisplayName(path)}</span>
              {path === projectDir && (
                <span style={{ fontSize: 10, padding: '1px 6px', borderRadius: 5, background: t.surfaceHover, color: t.text3, fontWeight: 600 }}>
                  {tr('chat_agent_group_current')}
                </span>
              )}
              <span className="mono" style={{
                flex: 1, minWidth: 0, fontSize: 10.5, color: t.text4,
                overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', textAlign: 'right',
              }}>{path}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function NavIcon({ name, pageId }: { name: string; pageId: string }) {
  const t = useT();
  const extensionGlyph = pageId === 'plugins' ? 'plugin' : pageId === 'skills' ? 'skills' : pageId === 'mcp' ? 'mcp' : pageId === 'tools-agent' ? 'tools' : null;
  if (extensionGlyph) return <ExtensionGlyph name={extensionGlyph} size={15} color={t.text3} />;
  return <Icon name={name} size={15} color="currentColor" stroke={1.7} style={{ color: t.text3 }} />;
}

export function SettingsScreen({
  bridge: liveBridge, theme, onTheme, onClose, initialPageId, initialProviderId, pendingModelReference, linkRequest,
}: SettingsScreenProps) {
  const { locale } = useI18n();
  const t = useT();
  const [page, setPage] = useState<string>(() => resolveInitialPage(initialPageId, initialProviderId));
  const [pageLinkRequest, setPageLinkRequest] = useState(linkRequest);
  const [initialPluginId, setInitialPluginId] = useState<string>();
  const navigatePage = useCallback((pageId: string, pluginId?: string) => {
    setPageLinkRequest(undefined);
    setInitialPluginId(pluginId);
    setPage(pageId);
  }, []);
  const closeRequestRef = useRef<(() => void) | null>(null);
  const registerCloseRequest = useCallback((handler: (() => void) | null) => { closeRequestRef.current = handler; }, []);
  const requestClose = () => { if (closeRequestRef.current) closeRequestRef.current(); else onClose(); };
  const navigationRequestRef = useRef<SettingsNavigationRequest | null>(null);
  const registerNavigationRequest = useCallback((handler: SettingsNavigationRequest | null) => {
    navigationRequestRef.current = handler;
  }, []);
  const requestNavigatePage = (pageId: string, pluginId?: string) => {
    if (navigationRequestRef.current) navigationRequestRef.current(pageId, pluginId);
    else navigatePage(pageId, pluginId);
  };
  const [query, setQuery] = useState('');
  const [editingLayer, setEditingLayer] = useState<EditableLayer>(linkRequest?.scope === 'project' || linkRequest?.scope === 'local' ? linkRequest.scope : 'user');
  const [layerLockCount, setLayerLockCount] = useState(0);
  const [snapshotRequestError, setSnapshotRequestError] = useState<{ owner: string | undefined; message: string } | null>(null);
  const snapshotRequestGeneration = useRef(0);
  const layerLocked = layerLockCount > 0;

  const panelRef = useRef<HTMLDivElement>(null);
  const contentScrollRef = useRef<HTMLDivElement>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  // A ref, not a dependency, so the mount effect below (which must run its
  // capture-focus/attach-listener logic exactly once) always calls the
  // LATEST `onClose` without needing to re-run when the prop identity changes.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const activePage = SETTINGS_NAV.find((candidate) => candidate.id === page) ?? SETTINGS_NAV[0];
  const isExtensionPage = page === 'skills' || page === 'plugins' || page === 'mcp';
  const projectPage = activePage.layered || isExtensionPage;
  const [selectedProject, setSelectedProject] = useState<string>();
  const registered = liveBridge.bootstrap?.settings.projects ?? [];
  const liveSnapshot = useMemo(() => parseSettingsSnapshot(liveBridge.settingsSnapshotEvent), [liveBridge.settingsSnapshotEvent]);
  const liveDir = projectDirFromSnapshot(liveSnapshot.snapshot);
  const liveReady = liveBridge.connected && !liveBridge.sessionLoading && Boolean(liveBridge.activeSession?.sessionId);
  const target = projectPage ? selectedProject ?? (!liveReady ? registered[0] : undefined) : undefined;
  const detached = target && !(liveReady && liveDir && projectKey(liveDir) === projectKey(target)) ? target : undefined;
  const bridge = useProjectSettingsBridge(liveBridge, detached);
  const settingsOwner = detached ? `settings:${detached}` : liveBridge.activeSession?.sessionId;
  const hasProject = Boolean(selectedProject || liveBridge.bootstrap?.workspace?.path || registered.length);
  const engineReady = Boolean(detached || (bridge.connected && settingsOwner));
  const nav = useMemo(() => groupedNav(query), [query, locale]);

  useEffect(() => {
    contentScrollRef.current?.scrollTo(0, 0);
  }, [page]);

  // Editing a layer that just became unavailable (the project closed) falls
  // back to `user` — the one layer that is always editable — rather than
  // leaving the switcher pointed at a now-disabled tab.
  useEffect(() => {
    if (layerDisabled(editingLayer, hasProject)) setEditingLayer('user');
  }, [editingLayer, hasProject]);

  // Refresh when a chat connection becomes ready or the settings directory changes.
  // A late error belongs only to the owner that requested it.
  const refreshSnapshot = useCallback(async () => {
    const generation = ++snapshotRequestGeneration.current;
    setSnapshotRequestError(null);
    try { await bridge.refreshSettingsSnapshot(); }
    catch (cause) {
      if (generation === snapshotRequestGeneration.current) {
        setSnapshotRequestError({ owner: settingsOwner, message: cause instanceof Error ? cause.message : tr('desktop_shell_settings_error') });
      }
    }
  }, [settingsOwner, bridge.refreshSettingsSnapshot]);
  useEffect(() => {
    if (!settingsOwner || !bridge.connected || bridge.sessionLoading) return;
    void refreshSnapshot();
    return () => { snapshotRequestGeneration.current += 1; };
  }, [settingsOwner, bridge.connected, bridge.sessionLoading, refreshSnapshot]);

  // Focus management for a dialog that claims `aria-modal="true"`: claiming
  // it while leaving focus (and Tab) free to wander the background would be
  // its own overclaim — assistive tech is told the background is inert while
  // Tab still walks it. This captures whatever had focus before mount,
  // moves focus onto the back button, traps Tab inside `panelRef`'s
  // focusable descendants (wrapping via the same `dialogFocusTarget` helper
  // the old settings modal used), and restores focus to whatever had it on unmount.
  // This is a DIFFERENT concern from `SettingsScreenProps.onClose` /
  // `SettingsRoute.restoreFocus` in `App.tsx`: that restores focus to the
  // opener once the WHOLE settings surface closes; this is about focus while
  // it's open. Both are needed.
  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    backRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      // A native modal owns focus and Escape until its task is dismissed.
      if (panelRef.current?.querySelector('dialog[open]')) return;
      if (event.key === 'Escape') {
        if (panelRef.current?.querySelector('[popover=auto]:popover-open')) return;
        event.preventDefault();
        if (closeRequestRef.current) closeRequestRef.current();
        else onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = [...(panelRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, a[href]',
      ) ?? [])].filter((element) => element.getClientRects().length > 0 && !element.closest('[inert], [aria-hidden="true"]'));
      const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const target = dialogFocusTarget(focusable, active, event.shiftKey);
      if (target) {
        event.preventDefault();
        target.focus();
      } else if (focusable.length === 0) {
        event.preventDefault();
        panelRef.current?.focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      previouslyFocused?.focus();
    };
  }, []);

  const { snapshot, error: snapshotError } = useMemo(
    () => detached ? parseSettingsSnapshot(bridge.settingsSnapshotEvent) : liveSnapshot,
    [detached, bridge.settingsSnapshotEvent, liveSnapshot],
  );
  // The snapshot identifies the directory that will receive project writes.
  const projectDir = projectDirFromSnapshot(snapshot);
  const requestError = snapshotRequestError && snapshotRequestError.owner === settingsOwner ? snapshotRequestError.message : null;
  const settingsReadError = snapshotError ?? requestError;

  const extensionScopes = useExtensionScopes(registered, page, selectedProject ?? projectDir ?? liveBridge.bootstrap?.workspace?.path ?? null,
    editingLayer, setEditingLayer, setSelectedProject, () => {
    setPageLinkRequest(undefined);
    setInitialPluginId(undefined);
  });
  let body: ReactNode;
  // Whether the body actually has something a layer switcher could target.
  // `false` for a page that will never exist here (not implemented) or one
  // whose data this shell cannot even read right now (no engine) — showing
  // the switcher there would offer a write target for a page with nothing to
  // write. `true` even for the "not wired yet" placeholder: the SETTINGS
  // exist and are readable/writable (the engine is ready), only this
  // shell's own editing UI for them hasn't been built yet — a temporary
  // packaging gap, not an absence of anything to write.
  let canWriteHere = false;
  if (!activePage.implemented) {
    body = <PagePlaceholder page={activePage} kind="not-implemented" />;
  } else if (activePage.needsEngine && !engineReady) {
    body = <EngineRequiredEmptyState page={activePage} />;
  } else if ((activePage.layered || detached) && !snapshot) {
    canWriteHere = true;
    const status = settingsReadError
      ? <div role="alert">{tr('desktop_shell_settings_unread')}<button type="button" onClick={() => void refreshSnapshot()}>{tr('common_retry')}</button></div>
      : <div role="status" data-testid="settings-snapshot-loading">{tr('desktop_shell_settings_loading')}</div>;
    body = detached && isExtensionPage ? <div style={extensionHubStyle(t)}>
      <ExtensionCatalogTabs label={navPageLabel(activePage)} scopes={extensionScopes} value={extensionScopes.view ?? 'public'} onChange={extensionScopes.setView} />
      {status}
    </div> : status;
  } else {
    canWriteHere = true;
    const Component = PAGE_CONTENT[activePage.id];
    body = Component
      ? (
        // Drafts belong to both the session and the file layer. A new owner
        // gets a fresh page; ordinary snapshot refreshes keep unsaved input.
        <Suspense fallback={null}>
          <Component
            key={`${settingsOwner ?? 'no-session'}:${editingLayer}`}
            bridge={bridge}
            snapshot={snapshot}
            editingLayer={editingLayer}
            extensionScopes={isExtensionPage ? extensionScopes : undefined}
            settingsProject={selectedProject}
            onSelectSettingsProject={extensionScopes.selectProject}
            theme={theme}
            onTheme={onTheme}
            onNavigate={navigatePage}
            onRegisterNavigationRequest={registerNavigationRequest}
            onRegisterCloseRequest={registerCloseRequest}
            linkRequest={pageLinkRequest}
            initialPluginId={initialPluginId}
            initialProviderId={initialProviderId}
            pendingModelReference={pendingModelReference}
            onClose={onClose}
            onJumpToLayer={(layer) => { if (!layerLocked) setEditingLayer(layer); }}
            onLayerLockChange={(locked) => setLayerLockCount((count) => Math.max(0, count + (locked ? 1 : -1)))}
          />
        </Suspense>
      )
      : <PagePlaceholder page={activePage} kind="not-wired" />;
  }
  const showLayerSwitcher = !isExtensionPage && activePage.layered && canWriteHere;
  const pageTitle = navPageLabel(activePage);

  return (
    <div
      ref={panelRef}
      // Exclude the sidebar/titlebar drag regions still mounted behind settings.
      className="settings-screen no-drag"
      data-macos={globalThis.window?.lingxi?.platform === 'darwin' || undefined}
      role="dialog"
      aria-modal="true"
      aria-label={tr('settings_title_main')}
      tabIndex={-1}
      style={{
        position: 'absolute', inset: 0, zIndex: 60, display: 'flex', flexDirection: 'column',
        background: t.windowBg, color: t.text,
        colorScheme: t.dark ? 'dark' : 'light',
        '--settings-accent': t.accent,
        '--settings-text': t.text,
      } as CSSProperties}
    >
      <div className="settings-window-drag-region drag-region" aria-hidden="true" />
      <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
        <nav aria-label={tr('desktop_settings_categories')} style={{
          width: 240, flexShrink: 0, display: 'flex', flexDirection: 'column',
          borderRight: `0.5px solid ${t.border}`, background: t.sidebarBg, overflow: 'hidden',
        }}>
          <div className="settings-sidebar-controls">
            <button ref={backRef} type="button" aria-label={tr('desktop_back')} onClick={requestClose}
              className="settings-back-button"
              style={{ minHeight: 36, display: 'flex', alignItems: 'center', gap: 9,
                marginBottom: 8, padding: '7px 10px', border: 0, borderRadius: 8, background: t.surfaceActive,
                color: t.text, font: 'inherit', fontSize: 14, textAlign: 'left', cursor: 'pointer' }}>
              <Icon name="arrowLeft" size={17} stroke={1.8} />
              <span>{tr('desktop_back')}</span>
            </button>
            <div className="settings-search" style={{
              display: 'flex', alignItems: 'center', gap: 8, padding: '6px 10px', borderRadius: 8,
              background: t.surface, border: `0.5px solid ${t.border}`,
            }}>
              <Icon name="search" size={14} color={t.text3} stroke={1.8} />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={tr('desktop_settings_search')}
                aria-label={tr('desktop_settings_search')}
                style={{
                  flex: 1, minWidth: 0, border: 'none', outline: 'none', background: 'transparent',
                  color: t.text, fontSize: 12.5, fontFamily: 'inherit',
                }}
              />
            </div>
          </div>
          <div className="settings-nav-scroll settings-scroll-area">
            {nav.length === 0 && (
              <div data-testid="nav-empty-state" style={{ padding: '18px 6px', color: t.text4, fontSize: 12 }}>
                {tr('desktop_shell_settings_no_match')}
              </div>
            )}
            {nav.map((section) => (
              <div key={section.group} style={{ marginBottom: 14 }}>
                <div style={{ padding: '10px 8px 4px', fontSize: 11, fontWeight: 600, color: t.text4, textTransform: 'uppercase', letterSpacing: 0.4 }}>
                  {navGroupLabel(section.group)}
                </div>
                {section.pages.map((navPage) => {
                  const active = navPage.id === activePage.id;
                  return (
                    <button
                      key={navPage.id}
                      type="button"
                      data-nav-page={navPage.id}
                      aria-current={active ? 'page' : undefined}
                      onClick={() => requestNavigatePage(navPage.id)}
                      style={{
                        width: '100%', display: 'flex', alignItems: 'center', gap: 9,
                        padding: '7px 8px', borderRadius: 7, border: 'none', textAlign: 'left',
                        background: active ? t.surfaceActive : 'transparent',
                        color: active ? t.text : t.text2,
                        fontSize: 13, fontWeight: active ? 600 : 500, fontFamily: 'inherit', cursor: 'pointer',
                      }}
                    >
                      <NavIcon name={navPage.icon} pageId={navPage.id} />
                      <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{navPageLabel(navPage)}</span>
                      {!navPage.implemented && (
                        <span style={{ fontSize: 9.5, color: t.text4, fontWeight: 600 }}>WIP</span>
                      )}
                    </button>
                  );
                })}
              </div>
            ))}
          </div>
        </nav>

        <div className="settings-content-viewport">
          <div ref={contentScrollRef} className="settings-content-scroll settings-scroll-area">
            <div className={page === 'computer-use' ? 'settings-content-inner settings-computer-use-content' : 'settings-content-inner'}>
              {page !== 'computer-use' && page !== 'provider-credentials' && <div className="settings-page-header" style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'flex-start', justifyContent: 'space-between', gap: 16, marginBottom: isExtensionPage ? 10 : 18 }}>
                <div>
                  <div className="settings-page-title" style={{ fontSize: 20, fontWeight: 550, letterSpacing: '-0.025em' }}>{pageTitle}</div>
                </div>
                {showLayerSwitcher && (
                  <LayerSwitcher value={editingLayer} onChange={setEditingLayer} hasProject={hasProject} locked={layerLocked} />
                )}
              </div>}
              {showLayerSwitcher && (
                <LayerContext bridge={bridge} layer={editingLayer} projectDir={selectedProject ?? projectDir} locked={layerLocked} onProject={setSelectedProject} />
              )}
              {settingsReadError && (
                <div data-testid="settings-snapshot-error" role="alert" style={{
                  padding: '10px 14px', marginBottom: 18, background: t.surface, color: t.danger,
                  border: `1px solid ${t.border}`, borderRadius: 8, fontSize: 12.5, overflowWrap: 'anywhere',
                }}>
                  {tr('desktop_shell_settings_failed', settingsReadError)}
                </div>
              )}
              {body}
            </div>
          </div>
        </div>


      </div>
    </div>
  );
}
