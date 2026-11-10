/** The rail selects a first-level workspace; utility pages retain its sidebar. */

import type { SidebarMode } from '../../shared/settings';

/** The pages of the main area. */
export type AppPage = 'chat' | 'scheduled' | 'apps' | 'changes' | 'bots' | 'teams' | 'friends';

/**
 * Home shows the chat view; Apps, Teams and Friends have their own library pages.
 * Apps and Bots retain a visible chat belonging to their workspace.
 */
export function pageForMode(mode: SidebarMode, current: { page: AppPage; botChat: boolean; appChat?: boolean }): AppPage {
  if (mode === 'home') return 'chat';
  if (mode === 'teams') return 'teams';
  if (mode === 'friends') return 'friends';
  if (mode === 'apps') return current.page === 'chat' && current.appChat ? 'chat' : 'apps';
  return current.page === 'chat' && current.botChat ? 'chat' : 'bots';
}

/**
 * The mode that goes with the page now on screen, for the few pages that name
 * one: the Apps, Teams, Friends and Bots pages select their corresponding workspaces. Every
 * other page keeps the mode as it is. Bots that cannot run here are not a mode.
 */
export function reconcileSidebarMode(mode: SidebarMode, page: AppPage, botsAvailable: boolean): SidebarMode {
  if (page === 'apps') return 'apps';
  if (page === 'teams') return 'teams';
  if (page === 'friends') return 'friends';
  if (page === 'bots') return botsAvailable ? 'bots' : 'home';
  return mode === 'bots' && !botsAvailable ? 'home' : mode;
}

/**
 * The mode to open the window in: the one it was last left in, except that the
 * Bots, where they cannot be used, are not a mode to return to and Home is.
 */
export function restoredSidebarMode(saved: SidebarMode, botsAvailable: boolean): SidebarMode {
  return saved === 'bots' && !botsAvailable ? 'home' : saved;
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
   * The first look at the saved mode. Answers a workspace when the
   * window should open on them, which it does only while it is still where it
   * opened: one that the owner or a link has already moved is left where it is.
   */
  open({ saved, page, mode, botsAvailable }: { saved: SidebarMode; page: AppPage; mode: SidebarMode; botsAvailable: boolean }): Exclude<SidebarMode, 'home'> | undefined {
    if (this.settled) return undefined;
    this.settled = true;
    this.written = saved;
    this.seen = mode;
    if (page !== 'chat' || mode !== 'home') return undefined;
    const restored = restoredSidebarMode(saved, botsAvailable);
    return restored === 'home' ? undefined : restored;
  }

  /** A mode is about to be picked before the saved one was looked at: the saved one is not brought back over the pick. */
  picked(saved: SidebarMode): void {
    if (this.settled) return;
    this.settled = true;
    this.written = saved;
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
 * Follow the settled chat's owner between ordinary projects, Apps and Bots.
 * Teams and Friends stay where the owner put them.
 */
export function modeForSettledChat(mode: SidebarMode, page: AppPage, botChat: boolean, appChat = false): SidebarMode {
  if (page !== 'chat') return mode;
  if (appChat) return 'apps';
  if (mode === 'teams' || mode === 'friends') return mode;
  if (botChat) return 'bots';
  if (mode === 'bots' || mode === 'apps') return 'home';
  return mode;
}
