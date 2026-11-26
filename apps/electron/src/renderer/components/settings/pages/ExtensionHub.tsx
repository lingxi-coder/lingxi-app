import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { publicExtensionCategories, publicExtensionText, type PublicExtensionCategory, type PublicExtensionItem } from '../../../../shared/publicExtensionDirectory';
import { useI18n } from '../../../i18n/useI18n';
import { Icon } from '../../Icon';
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

const recommendationSymbols = { layout: 'topbar-inspector-open', palette: 'sliders', pencil: 'pencil', cube: 'box', pixels: 'image' } as const;

export function ExtensionIcon({ kind = 'plugin', icon, large = false }: {
  kind?: 'plugin' | 'skill' | 'mcp'; icon?: PublicExtensionItem['icon']; large?: boolean;
}) {
  return <span className={`extension-hub-icon extension-icon-${kind}${large ? ' is-large' : ''}`} aria-hidden="true">
    {icon ? <Icon name={recommendationSymbols[icon]} size={large ? 32 : 22} />
      : kind === 'skill' ? <ExtensionSkillArtwork /> : <ExtensionGlyph name={kind} size={large ? 32 : 24} />}
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

export function ExtensionHubHeader({ title, search, onSearch, actions }: {
  title: string; search: string; onSearch: (value: string) => void; actions: ReactNode;
}) {
  const searchLabel = tr('desktop_shell_search_named', title);
  return <header className="extension-hub-header">
    <div className="extension-hub-toolbar">
      <label className="extension-search-field">
        <ExtensionGlyph name="search" size={16} />
        <input value={search} onChange={(event) => onSearch(event.target.value)} placeholder={searchLabel} aria-label={searchLabel} />
      </label>
      <div className="extension-hub-toolbar-actions">{actions}</div>
    </div>
  </header>;
}

export type ExtensionCatalogTab = 'public' | 'personal';

export function useExtensionCategoryFilter(items: readonly PublicExtensionItem[]) {
  const [value, setValue] = useState<PublicExtensionCategory | 'all'>('all');
  return {
    value, setValue,
    options: publicExtensionCategories.filter(category => items.some(item => item.category === category)),
    items: value === 'all' ? items : items.filter(item => item.category === value),
  };
}

export function ExtensionCatalogTabs({ label, value, onChange, filter, personalLabel = tr('desktop_personal') }: {
  label: string;
  value: ExtensionCatalogTab;
  onChange: (value: ExtensionCatalogTab) => void;
  filter: ReturnType<typeof useExtensionCategoryFilter>;
  personalLabel?: string;
}) {
  return <div className="extension-catalog-controls"><nav className="extension-catalog-tabs" aria-label={label}>
    <button type="button" data-catalog-tab="public" aria-pressed={value === 'public'} onClick={() => onChange('public')}>
      <ExtensionGlyph name="search" size={14} />{tr('extension_directory_recommended')}
    </button>
    <button type="button" data-catalog-tab="personal" aria-pressed={value === 'personal'} onClick={() => onChange('personal')}>
      <ExtensionGlyph name="archive" size={14} />{personalLabel}
    </button>
  </nav>
    {value === 'public' && filter.options.length > 1 && <label className="extension-category-filter">
      {tr('extension_directory_category')}
      <select value={filter.value} onChange={(event) => filter.setValue(event.target.value as typeof filter.value)}>
        <option value="all">{tr('extension_directory_category_all')}</option>
        {filter.options.map(category => <option key={category} value={category}>{tr(`extension_directory_category_${category}`)}</option>)}
      </select>
    </label>}
  </div>;
}

export function ExtensionRecommendationGroups({ items, children }: {
  items: readonly PublicExtensionItem[];
  children: (item: PublicExtensionItem) => ReactNode;
}) {
  return publicExtensionCategories.map((category) => {
    const group = items.filter((item) => item.category === category);
    if (!group.length) return null;
    const title = tr(`extension_directory_category_${category}`);
    const artwork = new URL(`../../../../../../../resources/extension-artwork/drawable-nodpi/extension_${category}.jpg`, import.meta.url).href;
    return <section key={category} className="extension-catalog-section extension-collection" aria-label={title} data-category={category}>
      <h2 className="extension-collection-heading">
        <img src={artwork} width={46} height={46} alt="" draggable={false} />
        {title}<span className="extension-collection-count">{group.length}</span>
      </h2>
      <div className="extension-hub-list extension-collection-grid">{group.map(children)}</div>
    </section>;
  });
}

export function ExtensionRecommendationCard({ item, kind = item.kind, onOpen, status, action }: {
  item: PublicExtensionItem;
  kind?: 'plugin' | 'skill' | 'mcp';
  onOpen: () => void;
  status?: string;
  action: ReactNode;
}) {
  const { locale } = useI18n();
  return <article className="extension-hub-row extension-collection-card">
    <button type="button" id={`extension-card-${item.id}`} className="extension-hub-row-open extension-card-open" onClick={onOpen}
      aria-label={tr('desktop_extension_view_details_named', item.name)}>
      <ExtensionIcon kind={kind} icon={item.icon} />
      <span className="extension-hub-row-title">{item.name}</span>
      <ExtensionGlyph name="chevronR" size={14} />
    </button>
    <p>{publicExtensionText(item.description, locale)}</p>
    <footer>
      <div className="extension-card-meta">
        <span>{item.publisher}{kind === 'skill' && item.skills?.length
          ? ` · ${tr('extension_directory_skills_count', item.skills.length)}` : item.version ? ` · ${item.version}` : ''}</span>
        {status && <span className="extension-card-status">{status}</span>}
      </div>
      {action}
    </footer>
  </article>;
}
