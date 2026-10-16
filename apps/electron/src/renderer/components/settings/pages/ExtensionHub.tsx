import { useEffect, useRef } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps, SettingsNavigationRequest } from '../SettingsScreen';
import { ExtensionGlyph, ExtensionSkillArtwork } from './ExtensionHubIcons';

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
    '--hub-accent-fill': `color-mix(in srgb, ${t.accent} 82%, #000)`,
    '--hub-border': t.border,
    '--hub-danger': t.danger,
    '--hub-hover': t.surfaceHover,
    '--hub-muted': t.text3,
    '--hub-on-accent': '#fff',
    '--hub-subtle': t.text4,
    '--hub-page': t.windowBg,
    '--hub-sidebar': t.sidebarBg,
    '--hub-surface': t.surface,
    '--hub-text': t.text,
    '--hub-text-secondary': t.text2,
    '--hub-toggle-off': t.surfaceActive,
  } as CSSProperties;
}

/** The runtime catalog does not supply artwork, so use a neutral plugin mark. */
export function ExtensionIcon({ kind = 'plugin', large = false }: { kind?: 'plugin' | 'skill' | 'mcp'; large?: boolean }) {
  return <span className={`extension-hub-icon extension-icon-${kind}${large ? ' is-large' : ''}`} aria-hidden="true">
    {kind === 'skill' ? <ExtensionSkillArtwork /> : <ExtensionGlyph name={kind} size={large ? 32 : 24} />}
  </span>;
}

/** Routes the shared settings navigation through the current page's draft guard. */
export function useExtensionHubNavigation(
  register: PageContentProps['onRegisterNavigationRequest'],
  handler: SettingsNavigationRequest,
) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    if (!register) return;
    register((pageId, pluginId) => handlerRef.current(pageId, pluginId));
    return () => register(null);
  }, [register]);
}

export function ExtensionHubLayout({ children }: { children: ReactNode }) {
  const t = useT();
  return <div className="extension-hub-main" style={extensionHubStyle(t)}><div className="extension-hub-page">{children}</div></div>;
}

export function ExtensionHubHeader({ title, description, search, onSearch, actions }: {
  title: string; description: string; search: string; onSearch: (value: string) => void; actions: ReactNode;
}) {
  return <header className="extension-hub-header">
    <div className="extension-hub-heading"><p>{description}</p></div>
    <div className="extension-hub-toolbar"><label className="extension-search-field"><ExtensionGlyph name="search" size={16} /><input value={search} onChange={(event) => onSearch(event.target.value)} placeholder={`Search ${title.toLowerCase()}`} aria-label={`Search ${title.toLowerCase()}`} /></label><div className="extension-hub-toolbar-actions">{actions}</div></div>
  </header>;
}
