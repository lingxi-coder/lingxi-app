/**
 * Whether the system opens the app in the background when the owner logs in, as
 * the Bots page holds it. The system is the source of truth and the owner can
 * change it in System Settings while the window is open, so nothing is kept
 * beyond the last answer: the page reads it again whenever it comes into view.
 */

import type { BotsApi, BotsLoginItemUi } from '../../shared/bots';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

export interface LoginItemState {
  /** Unknown until the host has answered. */
  status: BotsLoginItemUi | undefined;
  /** A change is on its way to the system. */
  busy: boolean;
  /** Why the last read or change failed; empty when there was no reason to give. */
  error: string | undefined;
}

/** What a window without a host to ask has: the app is not one that can be a login item. */
const UNAVAILABLE: BotsLoginItemUi = { available: false, enabled: false, needsApproval: false };

/** Whether the system took the request: it has the item as asked, or waits for the owner to allow it. */
function accepted(enabled: boolean, status: BotsLoginItemUi): boolean {
  return enabled ? status.enabled || status.needsApproval : !status.enabled && !status.needsApproval;
}

export class LoginItemController extends ObservableState<LoginItemState> {
  private request = 0;

  constructor(private readonly api: BotsApi['loginItem'] | undefined) {
    super({ status: api ? undefined : UNAVAILABLE, busy: false, error: undefined });
  }

  /** Read how the system has it. An answer that a later read or change overtook is dropped. */
  async refresh(): Promise<void> {
    if (!this.api) return;
    const request = ++this.request;
    try {
      const status = await this.api.status();
      if (request === this.request) this.update({ status, error: undefined });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error) });
    }
  }

  /** Ask the system to open the app at login, or to stop; what it answers is what the page shows. */
  async setEnabled(enabled: boolean): Promise<void> {
    if (!this.api || this.getSnapshot().busy) return;
    const request = ++this.request;
    this.update({ busy: true, error: undefined });
    let status: BotsLoginItemUi | undefined;
    let error: string | undefined;
    try {
      status = await this.api.set(enabled);
      if (!accepted(enabled, status)) error = '';
    } catch (cause) {
      error = ipcErrorMessage(cause);
    }
    // A read that began meanwhile has the newer word on the status.
    this.update({ busy: false, error, ...(status !== undefined && request === this.request ? { status } : {}) });
  }

  /** Show the owner where the app is allowed to open at login. */
  async openSettings(): Promise<void> {
    if (!this.api) return;
    try { await this.api.openSettings(); }
    catch (error) { this.update({ error: ipcErrorMessage(error) }); }
  }
}

/** What the page shows of the login item. */
export interface LoginItemView {
  /** The switch is on: the system opens the app at login, or will once the owner allows it. */
  checked: boolean;
  disabled: boolean;
  /** Which words go with the switch. */
  note: 'default' | 'unavailable' | 'approval';
  /** Offer to open the place in System Settings where the owner allows it. */
  canOpenSettings: boolean;
}

export function loginItemView(status: BotsLoginItemUi | undefined, busy: boolean): LoginItemView {
  if (status === undefined) return { checked: false, disabled: true, note: 'default', canOpenSettings: false };
  if (!status.available) return { checked: false, disabled: true, note: 'unavailable', canOpenSettings: false };
  return {
    checked: status.enabled || status.needsApproval,
    disabled: busy,
    note: status.needsApproval ? 'approval' : 'default',
    canOpenSettings: status.needsApproval,
  };
}
