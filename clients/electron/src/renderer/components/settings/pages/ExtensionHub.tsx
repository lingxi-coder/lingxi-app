import { useEffect, useMemo, useRef } from 'react';
import type { CSSProperties } from 'react';
import { useT } from '../../../theme/ThemeContext';
import type { UseBridge } from '../../../bridge/bridgeTypes.js';

type ExtensionHubTab = 'plugins' | 'mcp' | 'skills';

const DIALOG_FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not(:disabled)',
  'input:not(:disabled):not([type="hidden"])',
  'select:not(:disabled)',
  'textarea:not(:disabled)',
  'summary',
  '[tabindex]:not([tabindex="-1"])',
  '[contenteditable="true"]',
].join(',');

function dialogFocusableElements(dialog: HTMLElement): HTMLElement[] {
  return [...dialog.querySelectorAll<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR)].filter((element) => (
    element.tabIndex >= 0
    && !element.matches(':disabled')
    && element.closest('[hidden], [inert], [aria-hidden="true"]') === null
    && element.getClientRects().length > 0
  ));
}

/** Keeps keyboard focus inside a detail dialog and returns it to the opener. */
export function useExtensionHubDialogFocus(open: boolean, onRequestClose: () => void) {
  const dialogRef = useRef<HTMLElement | null>(null);
  const onRequestCloseRef = useRef(onRequestClose);
  onRequestCloseRef.current = onRequestClose;

  useEffect(() => {
    if (!open) return;

    const dialog = dialogRef.current;
    if (!dialog) return;

    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const initialTarget = dialog.querySelector<HTMLElement>('[data-dialog-initial-focus]')
      ?? dialogFocusableElements(dialog)[0]
      ?? dialog;
    initialTarget.focus({ preventScroll: true });

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        onRequestCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;

      // SettingsScreen also traps Tab at the outer settings dialog. Stop that
      // handler from moving focus out of this nested dialog.
      event.stopPropagation();
      const focusable = dialogFocusableElements(dialog);
      const active = document.activeElement;
      const activeIndex = active instanceof HTMLElement ? focusable.indexOf(active) : -1;
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
      } else if (event.shiftKey && activeIndex <= 0) {
        event.preventDefault();
        focusable[focusable.length - 1]?.focus();
      } else if (!event.shiftKey && (activeIndex < 0 || activeIndex === focusable.length - 1)) {
        event.preventDefault();
        focusable[0]?.focus();
      }
    };

    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('keydown', onKeyDown, true);
      if (previouslyFocused?.isConnected) previouslyFocused.focus({ preventScroll: true });
    };
  }, [open]);

  return dialogRef;
}

export function extensionHubStyle(t: ReturnType<typeof useT>): CSSProperties {
  return {
    '--hub-active': t.surfaceActive,
    '--hub-accent': t.accent,
    '--hub-border': t.border,
    '--hub-hover': t.surfaceHover,
    '--hub-muted': t.text3,
    '--hub-surface': t.surface,
    '--hub-text': t.text,
    '--hub-toggle-off': t.surfaceActive,
  } as CSSProperties;
}

function parseJson(raw: string | null | undefined): Record<string, unknown> {
  if (!raw) return {};
  try {
    const value = JSON.parse(raw) as unknown;
    return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
  } catch {
    return {};
  }
}

function countMcpServers(bridge: UseBridge): number {
  const snapshot = parseJson(bridge.mcpConfigurationSnapshotEvent?.snapshot_json);
  const scopes = Array.isArray(snapshot.scopes) ? snapshot.scopes : [];
  const configured = scopes.reduce((count, scope) => {
    if (!scope || typeof scope !== 'object') return count;
    try {
      const raw = (scope as Record<string, unknown>).raw_json;
      const parsed = parseJson(typeof raw === 'string' ? raw : undefined);
      const servers = parsed.mcpServers;
      return count + (servers && typeof servers === 'object' && !Array.isArray(servers) ? Object.keys(servers).length : 0);
    } catch {
      return count;
    }
  }, 0);
  return configured || bridge.mcpServersEvent?.servers.length || 0;
}

function countSkills(bridge: UseBridge): number {
  const catalog = parseJson(bridge.skillCatalogEvent?.catalog_json);
  if (Array.isArray(catalog.entries)) return catalog.entries.length;
  return bridge.skillsEvent?.skills.length ?? 0;
}

function countPlugins(bridge: UseBridge): number {
  const catalog = parseJson(bridge.pluginCatalogEvent?.catalog_json);
  if (Array.isArray(catalog.installed)) return catalog.installed.length;
  return 0;
}

export function ExtensionHubTabs({
  bridge,
  active,
  onNavigate,
}: {
  bridge: UseBridge;
  active: ExtensionHubTab;
  onNavigate: (pageId: string) => void;
}) {
  const t = useT();
  const counts = useMemo(() => ({
    plugins: countPlugins(bridge),
    mcp: countMcpServers(bridge),
    skills: countSkills(bridge),
  }), [
    bridge.pluginCatalogEvent?.catalog_json,
    bridge.mcpConfigurationSnapshotEvent?.snapshot_json,
    bridge.mcpServersEvent?.servers,
    bridge.skillCatalogEvent?.catalog_json,
    bridge.skillsEvent?.skills,
  ]);

  const tabs: Array<{ id: ExtensionHubTab; label: string }> = [
    { id: 'plugins', label: 'Plugins' },
    { id: 'mcp', label: 'MCPs' },
    { id: 'skills', label: 'Skills' },
  ];

  return (
    <nav className="extension-hub-tabs" aria-label="Plugin settings categories" style={{ '--hub-active': t.surfaceActive, '--hub-border': t.border, '--hub-text': t.text2, '--hub-muted': t.text4 } as CSSProperties}>
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          aria-current={active === tab.id ? 'page' : undefined}
          className={active === tab.id ? 'extension-hub-tab is-active' : 'extension-hub-tab'}
          onClick={() => onNavigate(tab.id)}
        >
          <span>{tab.label}</span>
          <span className="extension-hub-count">{counts[tab.id]}</span>
        </button>
      ))}
    </nav>
  );
}
