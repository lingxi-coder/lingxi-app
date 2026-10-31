/**
 * The app as a login item: opened in the background when the owner logs in to
 * the Mac, so that the Bots can run their routines and take tasks without the
 * owner opening the app first. Free of Electron; the app is handed in.
 *
 * Only an installed app can be a login item. One run from a build folder would
 * register the bare Electron program. The system is the source of truth: the
 * owner can change the item in System Settings, so nothing is kept here and the
 * state is read each time it is asked for.
 */

import type { BotsLoginItemUi } from '../../shared/bots.js';

/** Where the owner allows or removes login items. The window never supplies a URL. */
export const LOGIN_ITEMS_SETTINGS_URL = 'x-apple.systempreferences:com.apple.LoginItems-Settings.extension';

/** What of Electron's `app` the login item uses. */
export interface LoginItemApp {
  readonly isPackaged: boolean;
  /** `status` is there on macOS 13 and up. */
  getLoginItemSettings(options: { type: 'mainAppService' }): { openAtLogin: boolean; wasOpenedAtLogin?: boolean; status?: string };
  setLoginItemSettings(settings: { openAtLogin: boolean; type: 'mainAppService' }): void;
}

export interface LoginItemOptions {
  app: LoginItemApp;
  /** Open a URL with the program that handles it. */
  openExternal(url: string): Promise<void>;
  /** The platform the app runs on; this process's by default. */
  platform?: NodeJS.Platform;
}

export class LoginItem {
  constructor(private readonly options: LoginItemOptions) {}

  private get available(): boolean {
    return (this.options.platform ?? process.platform) === 'darwin' && this.options.app.isPackaged;
  }

  status(): BotsLoginItemUi {
    if (!this.available) return { available: false, enabled: false, needsApproval: false };
    const settings = this.options.app.getLoginItemSettings({ type: 'mainAppService' });
    return { available: true, enabled: settings.openAtLogin, needsApproval: settings.status === 'requires-approval' };
  }

  set(enabled: boolean): void {
    this.assertAvailable();
    this.options.app.setLoginItemSettings({ openAtLogin: enabled, type: 'mainAppService' });
  }

  /** Show the Login Items pane of System Settings. */
  openSettings(): Promise<void> {
    this.assertAvailable();
    return this.options.openExternal(LOGIN_ITEMS_SETTINGS_URL);
  }

  /**
   * Whether the system opened the app because the owner logged in, with the item
   * on, so that the app waits in the background instead of showing a window. A
   * Mac that opens again the apps it had running at logout says "at login" too;
   * that is not this item's doing, unless the item is on as well.
   */
  openedInBackground(): boolean {
    if (!this.available) return false;
    const settings = this.options.app.getLoginItemSettings({ type: 'mainAppService' });
    return settings.wasOpenedAtLogin === true && settings.openAtLogin;
  }

  private assertAvailable(): void {
    if (!this.available) throw new Error('Opening at login is available in the installed app.');
  }
}
