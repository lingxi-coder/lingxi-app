import { useEffect, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { useT } from '../../../theme/ThemeContext';
import type { UseBridge } from '../../../bridge/bridgeTypes.js';
import { ExtensionGlyph, ExtensionSkillArtwork } from './ExtensionHubIcons';

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
    '--hub-active': t.dark ? t.surfaceActive : '#ededee',
    '--hub-accent': t.accent,
    '--hub-border': t.border,
    '--hub-hover': t.dark ? t.surfaceHover : '#f6f6f6',
    '--hub-muted': t.text3,
    '--hub-subtle': t.dark ? t.text4 : '#b7b7ba',
    '--hub-page': t.stageBg,
    '--hub-sidebar': t.dark ? '#2b2b2d' : '#fafafa',
    '--hub-surface': t.surface,
    '--hub-text': t.text,
    '--hub-toggle-off': t.surfaceActive,
  } as CSSProperties;
}

/** The runtime catalog does not supply artwork, so use a neutral plugin mark. */
export function ExtensionIcon({ kind = 'plugin', large = false }: { kind?: 'plugin' | 'skill' | 'mcp'; large?: boolean }) {
  return <span className={`extension-hub-icon extension-icon-${kind}${large ? ' is-large' : ''}`} aria-hidden="true">
    {kind === 'skill' ? <ExtensionSkillArtwork /> : <ExtensionGlyph name={kind} size={large ? 32 : 24} />}
  </span>;
}

export function ExtensionHubLayout({ bridge, active, onNavigate, onClose, onSelectPlugin, selectedPlugin, scopeControls, children }: {
  bridge: UseBridge;
  active: ExtensionHubTab;
  onNavigate: (pageId: string, pluginId?: string) => void;
  onClose: () => void;
  onSelectPlugin?: (id: string) => void;
  selectedPlugin?: string | null;
  scopeControls?: ReactNode;
  children: ReactNode;
}) {
  const t = useT();
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState('');
  const catalog = parseJson(bridge.pluginCatalogEvent?.catalog_json);
  const installed = (Array.isArray(catalog.installed) ? catalog.installed : []).flatMap((item) => {
    if (!item || typeof item !== 'object') return [];
    const entry = item as Record<string, unknown>;
    const id = typeof entry.id === 'string' ? entry.id : typeof entry.name === 'string' ? entry.name : '';
    const name = typeof entry.display_name === 'string' ? entry.display_name : typeof entry.name === 'string' ? entry.name : id;
    return id && name.toLowerCase().includes(search.trim().toLowerCase()) ? [{ id, name }] : [];
  });
  return <div className="extension-customize" style={extensionHubStyle(t)}>
    <aside className="extension-sidebar">
      <div className="extension-sidebar-heading"><h2>Customize</h2><button type="button" className="extension-hub-icon-button" aria-label="Search installed plugins" aria-expanded={searchOpen} onClick={() => setSearchOpen(!searchOpen)}><ExtensionGlyph name="search" size={16} /></button></div>
      {searchOpen && <input autoFocus className="extension-sidebar-search" aria-label="Filter installed plugins" placeholder="Search installed" value={search} onChange={(event) => setSearch(event.target.value)} />}
      <nav aria-label="Customize">
        {([{ id: 'plugins', label: 'Plugins', icon: 'plugin' }, { id: 'skills', label: 'Skills', icon: 'skills' }, { id: 'mcp', label: 'Tools', icon: 'tools' }] as const).map((tab) => <button key={tab.id} type="button" data-nav-page={tab.id} aria-current={active === tab.id && !selectedPlugin ? 'page' : undefined} className="extension-sidebar-link" onClick={() => onNavigate(tab.id)}><ExtensionGlyph name={tab.icon} /><span>{tab.label}</span></button>)}
      </nav>
      <div className="extension-sidebar-installed"><div className="extension-sidebar-label">Installed</div>
        {installed.map((entry) => <button key={entry.id} type="button" className="extension-sidebar-link" aria-current={selectedPlugin === entry.id ? 'page' : undefined} onClick={() => onSelectPlugin ? onSelectPlugin(entry.id) : onNavigate('plugins', entry.id)}><ExtensionGlyph name="plugin" /><span>{entry.name}</span></button>)}
        {installed.length === 0 && <p className="extension-sidebar-empty">{search ? 'No matching plugins' : 'Your installed plugins appear here.'}</p>}
      </div>
      <div className="extension-sidebar-footer">
        {scopeControls && <details className="extension-scope-controls"><summary>Settings scope <ExtensionGlyph name="chevron" size={12} /></summary>{scopeControls}</details>}
        <button type="button" className="extension-sidebar-link" aria-label="All settings" onClick={() => onNavigate('general')}><ExtensionGlyph name="cog" size={16} /><span>Settings</span></button>
        <button type="button" className="extension-sidebar-link" aria-label="Back to app" onClick={onClose}><ExtensionGlyph name="arrowLeft" size={16} /><span>Back to app</span></button>
      </div>
    </aside>
    <main className="extension-hub-main settings-content-scroll"><div className="extension-hub-page">{children}</div></main>
  </div>;
}

export function ExtensionHubHeader({ title, description, search, onSearch, actions }: {
  title: string; description: string; search: string; onSearch: (value: string) => void; actions: ReactNode;
}) {
  return <header className="extension-hub-header">
    <div className="extension-hub-heading"><h1>{title}</h1><p>{description}</p></div>
    <div className="extension-hub-toolbar"><label className="extension-search-field"><ExtensionGlyph name="search" size={16} /><input value={search} onChange={(event) => onSearch(event.target.value)} placeholder={`Search ${title.toLowerCase()}`} aria-label={`Search ${title.toLowerCase()}`} /></label><div className="extension-hub-toolbar-actions">{actions}</div></div>
  </header>;
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
