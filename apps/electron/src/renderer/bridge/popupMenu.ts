/**
 * How a popup menu of buttons behaves for the keyboard and the pointer: the
 * first item that can be used takes the focus, the arrow keys, Home and End
 * move it, Escape and Tab close the menu, so does a press anywhere else, and
 * closing it hands the focus back to what had it.
 */

type MenuElement = Pick<HTMLElement, 'contains' | 'querySelectorAll'>;
type MenuDocument = Pick<Document, 'addEventListener' | 'removeEventListener' | 'activeElement' | 'body'>;

/** The items a menu's keys move between. */
export const MENU_ITEMS = '[role="menuitem"]:not(:disabled)';

/**
 * Start handling a menu that is on screen. Returns the way to stop, which also
 * gives the focus back to `opener` unless the owner has put it somewhere else.
 * A press on `trigger`, the button that opened the menu, is left to that button,
 * which closes the menu itself.
 */
export function attachPopupMenu({ menu, opener, trigger, onClose, doc = document }: {
  menu: MenuElement | null;
  opener: HTMLElement;
  trigger?: Pick<HTMLElement, 'contains'>;
  onClose(): void;
  doc?: MenuDocument;
}): () => void {
  const items = () => Array.from(menu?.querySelectorAll<HTMLElement>(MENU_ITEMS) ?? []);
  items()[0]?.focus();
  const closeOnOutsidePress = (event: Event) => {
    const pressed = event.target as Node;
    if (!menu?.contains(pressed) && !trigger?.contains(pressed)) onClose();
  };
  const onKey = (event: KeyboardEvent) => {
    if (event.key === 'Escape') { event.preventDefault(); onClose(); return; }
    if (event.key === 'Tab') { onClose(); return; }
    const enabled = items();
    const at = enabled.indexOf(doc.activeElement as HTMLElement);
    const to = event.key === 'ArrowDown' ? at + 1 : event.key === 'ArrowUp' ? Math.max(at, 0) - 1 : event.key === 'Home' ? 0 : event.key === 'End' ? enabled.length - 1 : undefined;
    if (to === undefined || enabled.length === 0) return;
    event.preventDefault();
    enabled[(to + enabled.length) % enabled.length]?.focus();
  };
  doc.addEventListener('pointerdown', closeOnOutsidePress, true);
  doc.addEventListener('keydown', onKey);
  return () => {
    doc.removeEventListener('pointerdown', closeOnOutsidePress, true);
    doc.removeEventListener('keydown', onKey);
    // The item that had the focus goes with the menu, which leaves the focus on the page: give it back.
    if (opener.isConnected && (doc.activeElement === doc.body || doc.activeElement === null)) opener.focus();
  };
}
