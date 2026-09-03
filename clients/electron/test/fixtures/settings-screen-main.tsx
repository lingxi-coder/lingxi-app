import { StrictMode, useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import '../../src/renderer/global.css';

import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import type { SettingsSnapshotEvent } from '../../src/renderer/bridge/useBridge';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

// Module-scope (not `useCallback`) so these are referentially stable across
// Fixture re-renders with zero effort — none of them close over any
// component state, they only exist so the Task 16 pages have something
// callable instead of `undefined`.
async function noopAsyncVoid(): Promise<void> {}
async function noopAsyncNull(): Promise<null> { return null; }
async function noopAsyncArray(): Promise<never[]> { return []; }

function Fixture() {
  const fixtureTheme = new URLSearchParams(window.location.search).get('theme') === 'dark' ? 'dark' : 'light';
  const [open, setOpen] = useState(true);
  const [hasProject, setHasProject] = useState(false);
  const [connected, setConnected] = useState(true);
  const [sessionLoading, setSessionLoading] = useState(false);
  const [settingsSnapshotEvent, setSettingsSnapshotEvent] = useState<SettingsSnapshotEvent | null>(null);
  const [refreshCalls, setRefreshCalls] = useState(0);
  const [closeCalls, setCloseCalls] = useState(0);
  const [lastPermissionRuleCall, setLastPermissionRuleCall] = useState<unknown>(null);
  const [lastEngineSettingsPatch, setLastEngineSettingsPatch] = useState<unknown>(null);

  // `useCallback` with empty deps keeps these referentially stable across
  // Fixture re-renders — `SettingsScreen` depends on `bridge.refreshSettingsSnapshot`'s
  // identity in an effect, and a fresh function every render would refire it forever.
  const refreshSettingsSnapshot = useCallback(async () => { setRefreshCalls((n) => n + 1); }, []);
  // Task 18 fix round 1, Important: records its call so a scenario can
  // prove `Permissions.tsx` dispatches through `capturePermissionEdit`'s
  // exact shape end-to-end, not just that the pure function itself
  // returns the right thing in isolation.
  const updatePermissionRules = useCallback(async (destination: string, behavior: string, add: string[], remove: string[]) => {
    setLastPermissionRuleCall({ destination, behavior, add, remove });
  }, []);
  // Task 18 fix round 2: records its call so a scenario can prove WHICH
  // layer a write actually targets and WHAT value it carries — the
  // `PluginToggleRow` ref-across-layer-switch regression this round fixes
  // is only visible by inspecting the patch's VALUE, not just that a write
  // happened.
  const updateEngineSettings = useCallback(async (destination: string, patch: Record<string, unknown>) => {
    setLastEngineSettingsPatch({ destination, patch });
  }, []);

  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      // `settings`/`diagnostics`/`runtimes`/`versions` below only matter to
      // the Task 16 pages (General/Projects/Diagnostics/About), which the
      // "general" (default) and "diagnostics" page ids in these scenarios
      // now render for real instead of a placeholder — a bootstrap this
      // thin would otherwise throw reading `.settings.theme` etc.
      settings: { version: 1 as const, projects: [] as string[], pinnedSessions: [] as never[] },
      workspace: hasProject ? { path: '/test/project', trusted: true } : { trusted: false },
      runtimes: [] as never[],
      diagnostics: [] as never[],
      versions: { app: 'test-app', electron: 'test-electron' },
    },
    desktop: {
      sessions: [],
      activeSessionId: 'session-a',
      models: [],
      modelDetails: [],
      currentModel: 'openai/gpt-5.4',
      conversationControls: null,
      fastMode: false,
      permissionMode: 'default',
      slashCommands: [],
      tasks: {},
      taskOutput: {},
      status: {
        session_id: 'session-a',
        model: 'openai/gpt-5.4',
        n_messages: 2,
        total_cost_usd: 0.125,
        input_tokens: 120,
        output_tokens: 45,
        n_mcp_connected: 1,
        n_mcp_total: 2,
        n_hooks: 1,
        n_agents: 1,
        started_at: '2026-09-02T00:00:00Z',
        cwd: '/test/project',
        status_line: 'ready',
      },
      doctor: {
        checks: [{ name: 'bridge', status: { type: 'pass' } }],
        summary: { passed: 1, warnings: 0, failed: 0 },
      },
      auth: { type: 'signed_out' as const },
      hooks: [],
      agents: [],
      lastCost: null,
      lastCompaction: null,
      lastPermissionResolution: null,
      lastApiRetry: null,
    },
    authState: { type: 'signed_out' as const },
    hooksCatalog: [],
    agentCatalog: [],
    cost: null,
    lastCompaction: null,
    retryState: null,
    connected: connected && !sessionLoading,
    sessionLoading,
    running: false,
    settingsSnapshotEvent,
    // Task 18 pages: `McpServers`/`Skills` call these on MOUNT (not just on
    // a button click), so — unlike the write-side commands below, which
    // only fire on an explicit click these scenarios never make — they must
    // exist here or selecting either page throws through the render.
    mcpServersEvent: null,
    skillsEvent: null,
    skillCatalogEvent: {
      type: 'skill_catalog' as const,
      catalog_json: JSON.stringify({
        entries: [
          { id: '/test/home/.lingxi/skills/release-notes', name: 'release-notes', source: 'user', rootDir: '/test/home/.lingxi/skills', directory: '/test/home/.lingxi/skills/release-notes', writable: true, description: 'Generate polished release notes.' },
          { id: '/test/project/.lingxi/skills/review', name: 'review', source: 'project', rootDir: '/test/project/.lingxi/skills', directory: '/test/project/.lingxi/skills/review', writable: true, description: 'Review the current project.' },
          { id: '<plugin:design>', name: 'design', source: 'plugin', rootDir: '', directory: '<plugin:design>', writable: false, readonlyReason: 'Provided by plugin design@acme.' },
        ],
        trash: [],
        sync_claude_ai_note: 'Stored only. Claude.ai cloud sync is not wired on desktop.',
      }),
    },
    skillDocumentEvent: null,
    mcpConfigurationSnapshotEvent: {
      type: 'mcp_configuration_snapshot' as const,
      snapshot_json: JSON.stringify({
        scopes: [
          { scope: 'user', path: '/test/home/.lingxi.json', revision_sha256: 'a'.repeat(64), raw_json: JSON.stringify({ mcpServers: { context7: { type: 'http', url: 'https://mcp.context7.com/mcp', timeout: 30000, alwaysLoad: true } } }) },
          { scope: 'local', path: '/test/project/.lingxi/settings.local.json', revision_sha256: 'b'.repeat(64), raw_json: '{}' },
          { scope: 'project', path: '/test/project/.mcp.json', revision_sha256: 'c'.repeat(64), raw_json: JSON.stringify({ mcpServers: { project_tools: { command: 'npx', args: ['-y', '@acme/project-tools'] } } }) },
        ],
        runtime_servers: [{ name: 'context7', transport: 'http', status: { type: 'connected' } }],
        approval: { enabled_servers: ['project_tools'], disabled_servers: [], enable_all_project_servers: false, revision_sha256: 'd'.repeat(64) },
      }),
    },
    pluginCatalogEvent: {
      type: 'plugin_catalog' as const,
      catalog_json: JSON.stringify({
        installed: [{ id: 'secure@acme', name: 'secure', display_name: 'Secure Tools', version: '1.2.0', path: '/test/home/.lingxi/plugins/cache/acme/secure/1.2.0', description: 'Project automation with secure configuration.', dependencies: ['shared@acme'], config_schema_json: JSON.stringify({ fields: { TOKEN: { type: 'string', title: 'API token', description: 'Stored in the system credential manager.', sensitive: true, required: true }, REGION: { type: 'string', title: 'Region', description: 'Service region.' } } }), secret_configured: { TOKEN: true } }],
        available: [{ id: 'review@acme', name: 'review', marketplace: 'acme', version: '2.0.0', description: 'Automated review workflows.', installed: false, upgrade_available: false }],
        marketplaces: [{ name: 'acme', source_json: '"acme/plugins"', install_location: '/test/marketplaces/acme' }],
        policies_json: JSON.stringify({ strictKnownMarketplaces: false, allowedMarketplaces: ['acme'] }, null, 2),
        revisions: { user: 'a'.repeat(64), project: 'b'.repeat(64), local: 'c'.repeat(64) },
      }),
    },
    configurationOperations: {},
    refreshMcpServers: noopAsyncVoid,
    refreshSkills: noopAsyncVoid,
    refreshAuth: noopAsyncVoid,
    refreshHooks: noopAsyncVoid,
    refreshAgents: noopAsyncVoid,
    restartBridge: noopAsyncVoid,
    refreshSettingsSnapshot,
    refresh: noopAsyncVoid,
    refreshDiagnostics: noopAsyncArray,
    copyDiagnostics: noopAsyncVoid,
    exportDiagnostics: noopAsyncNull,
    addProject: noopAsyncNull,
    removeProject: noopAsyncVoid,
    activateProject: noopAsyncNull,
    setThemePreference: noopAsyncVoid,
    // `voice` (Task 9 of the desktop-audio-capability plan) joined the
    // `page-content` scenario's list alongside the Task 18 pages — same
    // reasoning as the "write-side commands" block below: no scenario here
    // clicks a voice-preference control yet, but rendering the page reads
    // `bridge.openSystemSettings`/`setVoicePreferences` off this object, so
    // they need to exist or selecting "voice" throws through the render.
    openSystemSettings: noopAsyncVoid,
    setVoicePreferences: noopAsyncVoid,
    // The voice page's microphone row now asks the MAIN process for the OS
    // grant (there is no honest renderer-side source — see
    // `shared/microphoneAccess.ts`). This stub has no main process behind it,
    // so it answers the same thing production answers when the host is
    // unreachable: "cannot determine".
    microphonePermission: async () => 'unavailable' as const,
    sessionRuntimeStatus: () => undefined,
    login: noopAsyncVoid,
    logout: noopAsyncVoid,
    forceCompact: noopAsyncVoid,
    clearSession: noopAsyncVoid,
    // Write-side commands for Task 18's pages — none of these scenarios
    // click a save/add/remove button on them, but they are here so a future
    // scenario that does doesn't have to rediscover this same crash.
    updateEngineSettings,
    updatePermissionRules,
    setDefaultPermissionMode: noopAsyncVoid,
    updateWorkspaceDirectories: noopAsyncVoid,
    upsertMcpServer: noopAsyncVoid,
    removeMcpServer: noopAsyncVoid,
    skillAdmin: noopAsyncVoid,
    mcpAdmin: noopAsyncVoid,
    pluginAdmin: noopAsyncVoid,
    setPluginSecret: async () => ({ pluginId: 'secure@acme', key: 'TOKEN', configured: true, masked: '••••••••', restartRequired: false }),
    clearPluginSecret: async () => ({ pluginId: 'secure@acme', key: 'TOKEN', configured: false, restartRequired: false }),
    runSlashCommand: noopAsyncVoid,
  };

  useEffect(() => {
    window.__settingsScreenTest = {
      selectPage: (id: string) => {
        (document.querySelector(`[data-nav-page="${id}"]`) as HTMLButtonElement | null)?.click();
      },
      setHasProject,
      setConnected,
      setSessionLoading,
      setSnapshot: (effective: Record<string, unknown>, active: Record<string, unknown>) => {
        setSettingsSnapshotEvent({
          type: 'settings_snapshot',
          effective_json: JSON.stringify(effective),
          provenance_json: JSON.stringify(Object.fromEntries(Object.keys(effective).map((key) => [key, 'user']))),
          active_json: JSON.stringify(active),
        } as SettingsSnapshotEvent);
      },
      // Task 18 fix round 1 (Critical regression test): each layer's OWN
      // raw map, keyed exactly like the wire's `layers_json` — lets a
      // scenario put a DIFFERENT value under the same settings key in two
      // layers, the precondition for reproducing "switching layers doesn't
      // re-seed a page's own draft state" (`ToolsAgent`/`Plugins` both had
      // this bug). `effective` is a naive last-object-wins shallow merge
      // across the given layers — good enough for these scenarios, which
      // only ever care about one key at a time.
      setLayeredSnapshot: (layers: Record<string, Record<string, unknown>>) => {
        const effective: Record<string, unknown> = {};
        for (const layerValues of Object.values(layers)) Object.assign(effective, layerValues);
        setSettingsSnapshotEvent({
          type: 'settings_snapshot',
          effective_json: JSON.stringify(effective),
          provenance_json: JSON.stringify(Object.fromEntries(Object.keys(effective).map((key) => [key, 'user']))),
          active_json: JSON.stringify(effective),
          layers_json: JSON.stringify(layers),
        } as SettingsSnapshotEvent);
      },
      clickLayerTab: (layer: string) => {
        (document.querySelector(`[data-layer="${layer}"]`) as HTMLButtonElement | null)?.click();
      },
      // React 16+ tracks whether an input's `value` was set through its own
      // patched setter to decide whether to fire its synthetic `onChange` —
      // a plain `el.value = x` followed by a raw `dispatchEvent` is silently
      // ignored. Going through the underlying native setter first is the
      // standard workaround.
      setFieldValue: (selector: string, value: string) => {
        const field = document.querySelector(selector) as HTMLInputElement | HTMLTextAreaElement | null;
        if (!field) return;
        const proto = field.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
        setter?.call(field, value);
        field.dispatchEvent(new Event('input', { bubbles: true }));
      },
      getFieldValue: (selector: string) => (document.querySelector(selector) as HTMLInputElement | HTMLTextAreaElement | null)?.value ?? null,
      // Task 18 fix round 2: proves a REMOUNT happened (the DOM node itself
      // was destroyed and recreated), as distinct from an ordinary
      // prop/value update on the SAME node — which is exactly the
      // distinction `key={editingLayer}` (`SettingsScreen.tsx`) exists to
      // draw. An expando property survives React patching an existing DOM
      // node's attributes/value; it does NOT survive React tearing the node
      // down and creating a fresh one for a changed `key`. Deliberately NOT
      // focus-based: clicking the layer-switcher tab to change layers would
      // itself move `document.activeElement` to the tab button regardless
      // of whether the PAGE remounted, so focus can't discriminate here.
      markElement: (selector: string, value: string) => {
        const el = document.querySelector(selector) as (Element & { __lingxiTestMarker?: string }) | null;
        if (el) el.__lingxiTestMarker = value;
      },
      readElementMarker: (selector: string) => {
        const el = document.querySelector(selector) as (Element & { __lingxiTestMarker?: string }) | null;
        return el ? (el.__lingxiTestMarker ?? null) : 'element-not-found';
      },
      setMalformedSnapshot: () => {
        setSettingsSnapshotEvent({
          type: 'settings_snapshot',
          effective_json: '{not json',
          provenance_json: '{}',
        } as SettingsSnapshotEvent);
      },
      close: () => setCloseCalls((n) => n + 1),
      openSettings: () => setOpen(true),
      closeSettings: () => setOpen(false),
      focusOpener: () => (document.getElementById('opener') as HTMLButtonElement | null)?.focus(),
      state: () => ({
        hasLayerSwitcher: Boolean(document.querySelector('[data-testid="layer-switcher"]')),
        userDisabled: (document.querySelector('[data-layer="user"]') as HTMLButtonElement | null)?.disabled ?? null,
        projectDisabled: (document.querySelector('[data-layer="project"]') as HTMLButtonElement | null)?.disabled ?? null,
        localDisabled: (document.querySelector('[data-layer="local"]') as HTMLButtonElement | null)?.disabled ?? null,
        layerSwitcherReasonText: document.querySelector('[data-testid="layer-switcher-disabled-reason"]')?.textContent ?? null,
        hasBanner: Boolean(document.querySelector('[data-testid="settings-pending-banner"]')),
        placeholderKind: document.querySelector('[data-testid="page-placeholder"]')?.getAttribute('data-placeholder-kind') ?? null,
        hasSnapshotError: Boolean(document.querySelector('[data-testid="settings-snapshot-error"]')),
        activeElementAriaLabel: document.activeElement instanceof HTMLElement ? document.activeElement.getAttribute('aria-label') : null,
        activeElementId: document.activeElement instanceof HTMLElement ? document.activeElement.id : null,
        dialogPresent: Boolean(document.querySelector('[role="dialog"]')),
        refreshCalls,
        closeCalls,
        lastPermissionRuleCall,
        lastEngineSettingsPatch,
      }),
    };
    return () => { delete window.__settingsScreenTest; };
  }, [refreshCalls, closeCalls, lastPermissionRuleCall, lastEngineSettingsPatch]);

  return (
    <Theme.Provider value={tokens(fixtureTheme === 'dark')}>
      <div>
        <button type="button" id="opener">Open settings</button>
        {open && (
          <SettingsScreen
            bridge={bridge as never}
            theme={fixtureTheme}
            onTheme={() => {}}
            onClose={() => { setCloseCalls((n) => n + 1); setOpen(false); }}
          />
        )}
      </div>
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __settingsScreenTest?: {
      selectPage(id: string): void;
      setHasProject(value: boolean): void;
      setConnected(value: boolean): void;
      setSessionLoading(value: boolean): void;
      setSnapshot(effective: Record<string, unknown>, active: Record<string, unknown>): void;
      setLayeredSnapshot(layers: Record<string, Record<string, unknown>>): void;
      clickLayerTab(layer: string): void;
      setFieldValue(selector: string, value: string): void;
      getFieldValue(selector: string): string | null;
      markElement(selector: string, value: string): void;
      readElementMarker(selector: string): string | null;
      setMalformedSnapshot(): void;
      close(): void;
      openSettings(): void;
      closeSettings(): void;
      focusOpener(): void;
      state(): {
        hasLayerSwitcher: boolean;
        userDisabled: boolean | null;
        projectDisabled: boolean | null;
        localDisabled: boolean | null;
        layerSwitcherReasonText: string | null;
        hasBanner: boolean;
        placeholderKind: string | null;
        hasSnapshotError: boolean;
        activeElementAriaLabel: string | null;
        activeElementId: string | null;
        dialogPresent: boolean;
        refreshCalls: number;
        closeCalls: number;
        lastPermissionRuleCall: unknown;
        lastEngineSettingsPatch: unknown;
      };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
