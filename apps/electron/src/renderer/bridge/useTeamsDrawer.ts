import { useCallback, useEffect, useRef, useState } from 'react';
import type { DrawerTab } from '../../../../../packages/teams/ui';

/** How long the width must stay as it is before it is written to the settings file: a drag changes it on every pointer move. */
const SAVE_AFTER_MS = 400;

export interface TeamsDrawer {
  open: boolean;
  tab: DrawerTab;
  /** The width to draw at: the one chosen in this window, else the saved one, else none (the panel's own default). */
  width: number | undefined;
  toggle(): void;
  /** Open on a tab; without one, on the tab it was last on. */
  show(tab?: DrawerTab): void;
  close(): void;
  setTab(tab: DrawerTab): void;
  setWidth(width: number): void;
}

/**
 * The Teams details panel of the window: whether it is open, on which tab, and how wide. Only the width outlives the
 * window. It is saved once it has stayed as it is for a moment (a drag would otherwise write the file on every move),
 * and not at all when it is what the file holds already.
 */
export function useTeamsDrawer({ savedWidth, save }: { savedWidth: number | undefined; save(width: number): void }): TeamsDrawer {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<DrawerTab>('members');
  const [chosen, setChosen] = useState<number>();
  const saved = useRef(savedWidth);
  saved.current = savedWidth;
  const write = useRef(save);
  write.current = save;
  const waiting = useRef<{ timer: ReturnType<typeof setTimeout>; width: number }>();

  const flush = useCallback(() => {
    const pending = waiting.current;
    if (!pending) return;
    clearTimeout(pending.timer);
    waiting.current = undefined;
    if (pending.width !== saved.current) write.current(pending.width);
  }, []);
  const setWidth = useCallback((width: number) => {
    setChosen(width);
    if (waiting.current) clearTimeout(waiting.current.timer);
    waiting.current = { width, timer: setTimeout(flush, SAVE_AFTER_MS) };
  }, [flush]);
  // A width chosen just before the window goes away is still saved.
  useEffect(() => {
    window.addEventListener('pagehide', flush);
    return () => { window.removeEventListener('pagehide', flush); flush(); };
  }, [flush]);

  const toggle = useCallback(() => setOpen((current) => !current), []);
  const show = useCallback((next?: DrawerTab) => { setOpen(true); if (next) setTab(next); }, []);
  const close = useCallback(() => setOpen(false), []);
  return { open, tab, width: chosen ?? savedWidth, toggle, show, close, setTab, setWidth };
}
