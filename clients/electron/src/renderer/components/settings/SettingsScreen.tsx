import { useEffect, useMemo, useRef, useState, type ComponentType, type ReactNode } from 'react';

import { dialogFocusTarget, type UseBridge, type SettingsSnapshotEvent } from '../../bridge/useBridge';
import { useT } from '../../theme/ThemeContext';
import type { ThemeMode } from '../../theme/tokens';
import { Icon } from '../Icon';
import { SETTINGS_NAV, searchNav, type NavPage } from './nav';
import { About } from './pages/About';
import { Appearance } from './pages/Appearance';
import { CustomProviders } from './pages/CustomProviders';
import { Diagnostics } from './pages/Diagnostics';
import { General } from './pages/General';
import { Hooks } from './pages/Hooks';
import { McpServers } from './pages/McpServers';
import { Permissions } from './pages/Permissions';
import { Plugins } from './pages/Plugins';
import { Projects } from './pages/Projects';
import { ProviderCredentials } from './pages/ProviderCredentials';
import { Skills } from './pages/Skills';
import { ToolsAgent } from './pages/ToolsAgent';
import { provenanceLabel, type Provenance } from './rows';
import { pendingKeys, type SettingsSnapshot } from './useEngineSettings';

/**
 * The three settings-file layers a person can actually pick from up here.
 * `device` (Electron-store settings) and `managed` (policy overlay) are real
 * `Provenance` values but neither is an editable destination: `device` never
 * goes through this snapshot at all, and `managed` always wins, so offering
 * it as a tab a person could "write to" would lie about what happens next.
 */
export type EditableLayer = Extract<Provenance, 'user' | 'project' | 'local'>;
const EDITABLE_LAYERS: EditableLayer[] = ['user', 'project', 'local'];

const GROUP_ORDER: NavPage['group'][] = ['个人', '模型与服务', '编码', '高级'];

const LAYER_DISABLED_REASON_ID = 'settings-layer-switcher-disabled-reason';
const RESTART_DISABLED_REASON_ID = 'settings-restart-disabled-reason';

/**
 * Props are deliberately isomorphic to the existing `BetaSettingsProps`
 * (`clients/electron/src/renderer/components/BetaDesktop.tsx`) so a later
 * task can swap `<BetaSettings ... />` for `<SettingsScreen ... />` in
 * `App.tsx` without touching the call site's shape — same field names, same
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
  initialProviderId?: string;
  pendingModelReference?: string;
}

/** Only `provider-credentials` has a documented deep link today; this stays a plain equality check rather than a lookup table so a future second deep-link target is an explicit decision, not a silent fallthrough. */
export function resolveInitialPage(initialProviderId?: string): string {
  if (initialProviderId) return 'provider-credentials';
  return SETTINGS_NAV[0]?.id ?? 'general';
}

/** Whether a settings-file layer tab should be disabled. `user` needs no project; `project` and `local` both live inside a project directory and cannot be edited without one open. */
export function layerDisabled(layer: EditableLayer, hasProject: boolean): boolean {
  return layer !== 'user' && !hasProject;
}

/**
 * Why the "restart engine" action in the pending-settings banner should be
 * disabled, or `null` if it should not be. This mirrors only the ONE
 * precondition this component can actually see (`bridge.running`, a turn in
 * flight) plus the absence of a session — it deliberately does NOT try to
 * reproduce every precondition the host enforces (`assertRestartAllowed`,
 * `src/main/host.ts:538-546`: a non-active session, `pendingInteractions >
 * 0`, `hasActiveWork(projectPath)`), since this component has no visibility
 * into most of those. A restart the host rejects for a reason not covered
 * here still surfaces — see `handleRestart`'s catch, rendered as
 * `settings-restart-error` — so an enabled button can never fail silently
 * even though this function's disabling is necessarily incomplete.
 */
export function restartDisabledReason(running: boolean, hasSession: boolean): string | null {
  if (running) return '对话正在进行时无法重启引擎，请等待当前回合结束。';
  if (!hasSession) return '打开一个会话后才能重启引擎。';
  return null;
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
 * A missing `active_json` (an older producer may omit it — it's optional on
 * the wire) defaults `active` to `effective`, NOT to `{}`. An empty object
 * would make `pendingKeys` treat every effective key as newly pending — a
 * maximally loud false "restart to apply" banner manufactured from "we don't
 * know" rather than from an actual difference. Defaulting to `effective`
 * instead means "we don't know of anything pending", which is the honest
 * reading of an absent field.
 */
export function parseSettingsSnapshot(
  raw: SettingsSnapshotEvent | null | undefined,
): { snapshot: SettingsSnapshot | null; error: string | null } {
  if (!raw) return { snapshot: null, error: null };
  try {
    const effective = JSON.parse(raw.effective_json) as Record<string, unknown>;
    const provenance = JSON.parse(raw.provenance_json) as Record<string, string>;
    const files = raw.files_json ? (JSON.parse(raw.files_json) as SettingsSnapshot['files']) : [];
    const active = raw.active_json ? (JSON.parse(raw.active_json) as Record<string, unknown>) : effective;
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
      snapshot: { effective, provenance, files, active, locked, layers, mergedKeys },
      error: null,
    };
  } catch (cause) {
    return {
      snapshot: null,
      error: cause instanceof Error ? cause.message : 'The settings snapshot could not be parsed.',
    };
  }
}

function messageFrom(cause: unknown): string {
  return cause instanceof Error && cause.message ? cause.message : 'The engine could not be restarted.';
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
 * `layered: false`), `hooks` (read-only listing + a jump to `raw-json`),
 * and `plugins` (`enabledPlugins`/`pluginConfigs`/`additionalMarketplaces`
 * via the generic patch).
 */
const PAGE_CONTENT: Partial<Record<string, ComponentType<PageContentProps>>> = {
  general: General,
  appearance: Appearance,
  projects: Projects,
  diagnostics: Diagnostics,
  about: About,
  'provider-credentials': ProviderCredentials,
  'custom-providers': CustomProviders,
  permissions: Permissions,
  'tools-agent': ToolsAgent,
  skills: Skills,
  mcp: McpServers,
  hooks: Hooks,
  plugins: Plugins,
};

export interface PageContentProps {
  bridge: UseBridge;
  snapshot: SettingsSnapshot | null;
  editingLayer: EditableLayer;
  theme: ThemeMode;
  onTheme(value: ThemeMode): void;
  /** Jumps the shell to another nav page by id — how `General`'s cross-page entries actually navigate, rather than just naming a destination they can't reach. */
  onNavigate(pageId: string): void;
  initialProviderId?: string;
  pendingModelReference?: string;
  /**
   * Closes the WHOLE settings surface, not just this page. `ProviderCredentials`
   * needs this to reproduce `BetaSettings`' deep-link behaviour: once a
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
  const heading = kind === 'not-implemented' ? `${page.label}：尚未实现` : `${page.label}：内容即将到来`;
  const body = kind === 'not-implemented'
    ? '这不是空白页或加载中——这个设置页确实还没有被构建，它属于另一项独立的计划，会在那项计划完成后加入。'
    : '这个设置页的信息架构已经确定，但具体内容组件还没有接入这层外壳，会由后续任务补上。';
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
      <div style={{ fontSize: 13.5, color: t.text2, fontWeight: 500 }}>{page.label} 需要正在运行的引擎</div>
      <div style={{ fontSize: 12, color: t.text4, maxWidth: 420, lineHeight: 1.6 }}>
        打开一个项目并等待引擎连接后，这里会显示实际设置。
      </div>
    </div>
  );
}

function LayerSwitcher({ value, onChange, hasProject }: {
  value: EditableLayer;
  onChange(layer: EditableLayer): void;
  hasProject: boolean;
}) {
  const t = useT();
  return (
    <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 4 }}>
      <div
        data-testid="layer-switcher"
        style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}
      >
        {EDITABLE_LAYERS.map((layer) => {
          const active = value === layer;
          const disabled = layerDisabled(layer, hasProject);
          return (
            <button
              key={layer}
              type="button"
              data-layer={layer}
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
          打开一个项目后可编辑
        </div>
      )}
    </div>
  );
}

function PendingSettingsBanner({ pending, disabledReason, onRestart }: {
  pending: string[];
  disabledReason: string | null;
  onRestart(): void;
}) {
  const t = useT();
  if (pending.length === 0) return null;
  return (
    <div
      data-testid="settings-pending-banner"
      role="status"
      style={{
        display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12,
        padding: '10px 18px', background: t.accentBg, borderBottom: `0.5px solid ${t.border}`,
        fontSize: 12.5, color: t.text2, flexShrink: 0,
      }}
    >
      <span>重启引擎以应用（{pending.length} 项）</span>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        {/* Same reasoning as the layer switcher's reason text above: visible
            text a screen reader can reach via `aria-describedby`, not a
            tooltip a disabled button will never dispatch. */}
        {disabledReason && (
          <span id={RESTART_DISABLED_REASON_ID} data-testid="restart-disabled-reason" style={{ fontSize: 11, color: t.text3 }}>
            {disabledReason}
          </span>
        )}
        <button
          type="button"
          disabled={disabledReason !== null}
          aria-describedby={disabledReason ? RESTART_DISABLED_REASON_ID : undefined}
          onClick={onRestart}
          style={{
            padding: '5px 12px', borderRadius: 7, border: `0.5px solid ${t.accentBorder}`,
            background: disabledReason !== null ? t.surfaceActive : t.accent,
            color: disabledReason !== null ? t.text4 : '#fff',
            fontSize: 12, fontWeight: 600, cursor: disabledReason !== null ? 'not-allowed' : 'pointer',
          }}
        >
          重启引擎
        </button>
      </div>
    </div>
  );
}

function NavIcon({ name }: { name: string }) {
  const t = useT();
  return <Icon name={name} size={15} color="currentColor" stroke={1.7} style={{ color: t.text3 }} />;
}

export function SettingsScreen({
  bridge, theme, onTheme, onClose, initialProviderId, pendingModelReference,
}: SettingsScreenProps) {
  const t = useT();
  const [page, setPage] = useState<string>(() => resolveInitialPage(initialProviderId));
  const [query, setQuery] = useState('');
  const [editingLayer, setEditingLayer] = useState<EditableLayer>('user');
  const [restartError, setRestartError] = useState<string | null>(null);

  const panelRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  // A ref, not a dependency, so the mount effect below (which must run its
  // capture-focus/attach-listener logic exactly once) always calls the
  // LATEST `onClose` without needing to re-run when the prop identity changes.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const activeSessionId = bridge.activeSession?.sessionId;
  const hasProject = Boolean(bridge.bootstrap?.workspace?.path);
  const engineReady = bridge.connected && Boolean(activeSessionId);
  const activePage = SETTINGS_NAV.find((candidate) => candidate.id === page) ?? SETTINGS_NAV[0];
  const nav = useMemo(() => groupedNav(query), [query]);

  // Editing a layer that just became unavailable (the project closed) falls
  // back to `user` — the one layer that is always editable — rather than
  // leaving the switcher pointed at a now-disabled tab.
  useEffect(() => {
    if (layerDisabled(editingLayer, hasProject)) setEditingLayer('user');
  }, [editingLayer, hasProject]);

  // The shell owns the settings-snapshot lifecycle so every page and the
  // pending banner read the one parse of it, rather than each page task
  // re-fetching and re-parsing the same wire event independently.
  //
  // Gated on `bridge.connected` and NOT `bridge.sessionLoading`: `command()`
  // (the thing `refreshSettingsSnapshot` wraps) silently no-ops while a
  // session is loading, and `activeSession` is already populated with the
  // PENDING session during that window — so a naive `[activeSessionId]`
  // dependency fires once, sends nothing, and never fires again once loading
  // finishes (neither dependency changes), leaving the snapshot — and so the
  // pending banner — permanently null. Depending on `bridge.connected` too
  // makes this effect re-run exactly when loading finishes and the session
  // is actually ready to answer, which is also what makes a post-restart
  // refresh happen automatically: a real restart cycles `connected` through
  // `false` before `true` again, re-firing this effect with no separate
  // "refresh after restart" call needed (see `handleRestart` below).
  useEffect(() => {
    if (!activeSessionId || !bridge.connected || bridge.sessionLoading) return;
    void bridge.refreshSettingsSnapshot().catch(() => undefined);
  }, [activeSessionId, bridge.connected, bridge.sessionLoading, bridge.refreshSettingsSnapshot]);

  // Focus management for a dialog that claims `aria-modal="true"`: claiming
  // it while leaving focus (and Tab) free to wander the background would be
  // its own overclaim — assistive tech is told the background is inert while
  // Tab still walks it. This captures whatever had focus before mount,
  // moves focus onto the close button, traps Tab inside `panelRef`'s
  // focusable descendants (wrapping via the same `dialogFocusTarget` helper
  // `BetaSettings` uses), and restores focus to whatever had it on unmount.
  // This is a DIFFERENT concern from `SettingsScreenProps.onClose` /
  // `SettingsRoute.restoreFocus` in `App.tsx`: that restores focus to the
  // opener once the WHOLE settings surface closes; this is about focus while
  // it's open. Both are needed.
  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = [...(panelRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])',
      ) ?? [])];
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
    () => parseSettingsSnapshot(bridge.settingsSnapshotEvent),
    [bridge.settingsSnapshotEvent],
  );
  const pending = snapshot ? pendingKeys(snapshot) : [];
  const restartReason = restartDisabledReason(bridge.running, Boolean(activeSessionId));

  const handleRestart = () => {
    setRestartError(null);
    // No explicit "refresh the snapshot after this" chain: a successful
    // restart cycles `bridge.connected` through `false`→`true`, which the
    // effect above already treats as a reason to re-fetch. Chaining it here
    // too would just re-race the same no-op-while-loading guard that effect
    // exists to fix.
    void bridge.restartBridge().catch((cause) => setRestartError(messageFrom(cause)));
  };

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
  } else {
    canWriteHere = true;
    const Component = PAGE_CONTENT[activePage.id];
    body = Component
      ? (
        <Component
          bridge={bridge}
          snapshot={snapshot}
          editingLayer={editingLayer}
          theme={theme}
          onTheme={onTheme}
          onNavigate={setPage}
          initialProviderId={initialProviderId}
          pendingModelReference={pendingModelReference}
          onClose={onClose}
          onJumpToLayer={setEditingLayer}
        />
      )
      : <PagePlaceholder page={activePage} kind="not-wired" />;
  }
  const showLayerSwitcher = activePage.layered && canWriteHere;

  return (
    <div
      ref={panelRef}
      role="dialog"
      aria-modal="true"
      aria-label="设置"
      tabIndex={-1}
      style={{
        position: 'absolute', inset: 0, zIndex: 60, display: 'flex', flexDirection: 'column',
        background: t.windowBg, color: t.text,
      }}
    >
      <PendingSettingsBanner pending={pending} disabledReason={restartReason} onRestart={handleRestart} />
      {restartError && (
        <div data-testid="settings-restart-error" role="alert" style={{
          padding: '10px 18px', background: t.danger, color: '#fff', fontSize: 12.5, flexShrink: 0,
        }}>
          重启失败：{restartError}
        </div>
      )}
      {snapshotError && (
        <div data-testid="settings-snapshot-error" role="alert" style={{
          padding: '10px 18px', background: t.danger, color: '#fff', fontSize: 12.5, flexShrink: 0,
        }}>
          设置读取失败：{snapshotError}
        </div>
      )}
      <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
        <nav style={{
          width: 240, flexShrink: 0, display: 'flex', flexDirection: 'column',
          borderRight: `0.5px solid ${t.border}`, background: t.sidebarBg, overflowY: 'auto',
        }}>
          <div style={{ padding: '14px 14px 8px' }}>
            <div style={{
              display: 'flex', alignItems: 'center', gap: 8, padding: '6px 10px', borderRadius: 8,
              background: t.surface, border: `0.5px solid ${t.border}`,
            }}>
              <Icon name="search" size={14} color={t.text3} stroke={1.8} />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="搜索设置"
                aria-label="搜索设置"
                style={{
                  flex: 1, minWidth: 0, border: 'none', outline: 'none', background: 'transparent',
                  color: t.text, fontSize: 12.5, fontFamily: 'inherit',
                }}
              />
            </div>
          </div>
          <div style={{ flex: 1, overflowY: 'auto', padding: '4px 10px 14px' }}>
            {nav.length === 0 && (
              <div data-testid="nav-empty-state" style={{ padding: '18px 6px', color: t.text4, fontSize: 12 }}>
                没有匹配的设置项
              </div>
            )}
            {nav.map((section) => (
              <div key={section.group} style={{ marginBottom: 14 }}>
                <div style={{ padding: '10px 8px 4px', fontSize: 11, fontWeight: 600, color: t.text4, textTransform: 'uppercase', letterSpacing: 0.4 }}>
                  {section.group}
                </div>
                {section.pages.map((navPage) => {
                  const active = navPage.id === activePage.id;
                  return (
                    <button
                      key={navPage.id}
                      type="button"
                      data-nav-page={navPage.id}
                      onClick={() => setPage(navPage.id)}
                      style={{
                        width: '100%', display: 'flex', alignItems: 'center', gap: 9,
                        padding: '7px 8px', borderRadius: 7, border: 'none', textAlign: 'left',
                        background: active ? t.surfaceActive : 'transparent',
                        color: active ? t.text : t.text2,
                        fontSize: 13, fontWeight: active ? 600 : 500, fontFamily: 'inherit', cursor: 'pointer',
                      }}
                    >
                      <NavIcon name={navPage.icon} />
                      <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{navPage.label}</span>
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

        <div style={{ flex: 1, minWidth: 0, overflowY: 'auto', padding: '20px 32px 40px' }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 16, marginBottom: 18 }}>
            <div style={{ fontSize: 18, fontWeight: 600 }}>{activePage.label}</div>
            {showLayerSwitcher && (
              <LayerSwitcher value={editingLayer} onChange={setEditingLayer} hasProject={hasProject} />
            )}
          </div>
          {body}
        </div>

        <button
          ref={closeRef}
          type="button"
          aria-label="Close settings"
          onClick={onClose}
          style={{
            position: 'absolute', top: 14, right: 18, width: 28, height: 28, borderRadius: 8,
            border: `0.5px solid ${t.border}`, background: t.surface, color: t.text3,
            display: 'flex', alignItems: 'center', justifyContent: 'center', cursor: 'pointer',
          }}
        >
          <Icon name="x" size={14} stroke={1.8} />
        </button>
      </div>
    </div>
  );
}
