/**
 * What the sidebar lists, apart from the page the main area shows. The rail's
 * Home, Bots and Teams buttons pick the first; Apps, Scheduled and the changes
 * are pages and leave it alone, so the Bots stay in the sidebar while the Apps
 * are open.
 */

import type { SidebarMode } from '../../shared/settings';

/** The pages of the main area. */
export type AppPage = 'chat' | 'scheduled' | 'apps' | 'changes' | 'bots' | 'teams';

/**
 * Where the main area goes when a mode is picked. Home is the chat view and
 * Teams its own page. Bots is the Bots overview, except that a Bot's chat on
 * screen stays: the sidebar changes around it.
 */
export function pageForMode(mode: SidebarMode, current: { page: AppPage; botChat: boolean }): AppPage {
  if (mode === 'home') return 'chat';
  if (mode === 'teams') return 'teams';
  return current.page === 'chat' && current.botChat ? 'chat' : 'bots';
}

/**
 * The mode that goes with the page now on screen, for the few pages that name
 * one: the Teams page is the Teams mode, and the Bots page the Bots mode. Every
 * other page keeps the mode as it is. Bots that cannot run here are not a mode.
 */
export function reconcileSidebarMode(mode: SidebarMode, page: AppPage, botsAvailable: boolean): SidebarMode {
  if (page === 'teams') return 'teams';
  if (page === 'bots') return botsAvailable ? 'bots' : 'home';
  return mode === 'bots' && !botsAvailable ? 'home' : mode;
}

/**
 * The mode to open the window in. Teams is not brought back yet: its sidebar
 * has nothing of its own to return to, so the window opens on Home.
 */
export function restoredSidebarMode(saved: SidebarMode | undefined, botsAvailable: boolean): SidebarMode {
  return saved === 'bots' && botsAvailable ? 'bots' : 'home';
}

/**
 * Keeps the mode in step with the settings file. The saved mode is looked at
 * once, when the window first knows it can be honored, and after that each
 * change of mode is written once. Nothing is written before the saved mode was
 * looked at, and staying in the mode the window opened in writes nothing, so a
 * window cannot overwrite what it has not honored.
 */
export class SidebarModeSync {
  private settled = false;
  /** What the settings file holds, as far as this window knows. */
  private written: SidebarMode = 'home';
  /** The mode the window was in when it was last looked at; a window starts on Home. */
  private seen: SidebarMode = 'home';

  /**
   * The first look at the saved mode. Answers `'bots'` when the window should
   * open on the Bots, which it does only while it is still where it opened:
   * one that the owner or a link has already moved is left where it is.
   */
  open({ saved, page, mode, botsAvailable }: { saved: SidebarMode | undefined; page: AppPage; mode: SidebarMode; botsAvailable: boolean }): 'bots' | undefined {
    if (this.settled) return undefined;
    this.settled = true;
    this.written = saved ?? 'home';
    this.seen = mode;
    return page === 'chat' && mode === 'home' && restoredSidebarMode(saved, botsAvailable) === 'bots' ? 'bots' : undefined;
  }

  /** A mode is about to be picked before the saved one was looked at: the saved one is not brought back over the pick. */
  picked(saved: SidebarMode | undefined): void {
    if (this.settled) return;
    this.settled = true;
    this.written = saved ?? 'home';
  }

  /** The mode to write to the settings file now that the window is in `mode`, if that is a change and the file does not hold it. */
  toSave(mode: SidebarMode): SidebarMode | undefined {
    if (!this.settled) return undefined;
    const before = this.seen;
    this.seen = mode;
    if (mode === before || mode === this.written) return undefined;
    this.written = mode;
    return mode;
  }
}

/**
 * The mode to move to when the chat on screen has just turned from a project's
 * into a Bot's or back, so that the sidebar lists it. Only the chat view
 * follows, and only between Home and Bots: Teams stays where the owner put it.
 */
export function modeForSettledChat(mode: SidebarMode, page: AppPage, botChat: boolean): SidebarMode {
  if (page !== 'chat') return mode;
  if (botChat && mode === 'home') return 'bots';
  if (!botChat && mode === 'bots') return 'home';
  return mode;
}
