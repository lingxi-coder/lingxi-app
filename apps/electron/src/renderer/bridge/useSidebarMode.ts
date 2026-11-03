import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { PublicSettings, SidebarMode, SidebarPreferences } from '../../shared/settings';
import { reconcileSidebarMode, SidebarModeSync, type AppPage } from './sidebarMode';

/**
 * The sidebar mode of the window: what the sidebar lists, kept in step with the
 * page, saved when it changes, and brought back when the window opens.
 */
export function useSidebarMode({ page, setPage, botsAvailable, botsKnown, settings, save }: {
  page: AppPage;
  setPage(page: AppPage): void;
  /** Bots can be used on this computer. */
  botsAvailable: boolean;
  /** The host has said whether they can; until then a saved Bots mode cannot be brought back. */
  botsKnown: boolean;
  /** Absent until the host has handed the window its settings. */
  settings: Pick<PublicSettings, 'sidebar'> | undefined;
  save(preferences: Partial<SidebarPreferences>): Promise<void>;
}): { mode: SidebarMode; setMode(mode: SidebarMode): void } {
  const [mode, setModeState] = useState<SidebarMode>('home');
  const [sync] = useState(() => new SidebarModeSync());
  const saved = settings?.sidebar?.mode;
  const savedRef = useRef(saved);
  savedRef.current = saved;
  // A different `save` is no reason to write the mode again.
  const saveRef = useRef(save);
  saveRef.current = save;

  const setMode = useCallback((next: SidebarMode) => {
    sync.picked(savedRef.current);
    setModeState(next);
  }, [sync]);

  useEffect(() => {
    if (settings === undefined || !botsKnown) return;
    if (sync.open({ saved, page, mode, botsAvailable }) === 'bots') {
      setModeState('bots');
      setPage('bots');
    }
  }, [sync, settings, botsKnown, botsAvailable, saved, page, mode, setPage]);

  useLayoutEffect(() => {
    const next = reconcileSidebarMode(mode, page, botsAvailable);
    if (next !== mode) setMode(next);
  }, [mode, page, botsAvailable, setMode]);

  useEffect(() => {
    const next = sync.toSave(mode);
    if (next !== undefined) void saveRef.current({ mode: next });
  }, [mode, sync]);

  return { mode, setMode };
}
