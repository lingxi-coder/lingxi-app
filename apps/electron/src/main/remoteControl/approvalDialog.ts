import { nativeTr } from '../../shared/nativeLocale.js';

/**
 * The native dialog on which the user approves a new device, after comparing
 * the pairing code with the one the device shows. Copy follows the current UI language; device names and pairing codes stay unchanged.
 *
 * Limits keep a stream of connection attempts from becoming a stream of
 * dialogs: one dialog at a time (a couple of further requests may wait their
 * turn, the rest are told `busy`), at most five dialogs per ten minutes, a
 * device the user denied is refused without asking for ten minutes, and an
 * unanswered dialog closes itself after two minutes.
 */

import { systemClock, type Clock, type DevicePlatform } from '@lingxi/bridge-client/remote';

export interface ApprovalRequest {
  deviceId: string;
  deviceName: string;
  platform: DevicePlatform;
  /** Six digits. */
  pairingCode: string;
  /** Aborted when the connection goes away: closes the dialog, or drops the request from the line. */
  signal: AbortSignal;
  /** Checked when the request reaches the front of the line: a device approved in the meantime is not asked about again. */
  alreadyApproved?: () => boolean;
}

export type ApprovalDenial =
  /** The user chose Deny, or the device was denied recently. */
  | 'denied'
  | 'rate_limited'
  | 'busy'
  /** Nobody answered within two minutes. */
  | 'timeout'
  /** The connection ended first, or the dialog could not be shown. */
  | 'cancelled';

export type ApprovalOutcome = { allowed: true } | { allowed: false; reason: ApprovalDenial };

/** The `dialog.showMessageBox` options in use. */
export interface ApprovalMessageBox {
  type: 'question';
  title: string;
  message: string;
  detail: string;
  buttons: string[];
  defaultId: number;
  cancelId: number;
  noLink: true;
  signal: AbortSignal;
}

/** `dialog.showMessageBox`, parented to the main window when there is one. */
export type ShowMessageBox = (options: ApprovalMessageBox) => Promise<{ response: number }>;

export interface ApprovalDialogOptions {
  show: ShowMessageBox;
  clock?: Clock;
}

const ALLOW = 0;
const DENY = 1;
const DIALOG_TIMEOUT_MS = 2 * 60_000;
const RATE_WINDOW_MS = 10 * 60_000;
const MAX_DIALOGS_PER_WINDOW = 5;
const DENIAL_MEMORY_MS = 10 * 60_000;
const MAX_WAITING = 2;
const NAME_SHOWN_CHARS = 64;

const PLATFORM_LABELS: Record<DevicePlatform, string> = { web: 'Web', android: 'Android', ios: 'iOS' };

const denial = (reason: ApprovalDenial): ApprovalOutcome => ({ allowed: false, reason });

/** `123456` as `123 456`, the way the viewer shows it. */
export function formatPairingCode(code: string): string {
  return code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

function shownName(name: string): string {
  const chars = [...name];
  return chars.length > NAME_SHOWN_CHARS ? `${chars.slice(0, NAME_SHOWN_CHARS).join('')}…` : name;
}

interface Waiting {
  request: ApprovalRequest;
  resolve(outcome: ApprovalOutcome): void;
  onAbort(): void;
}

export class ApprovalDialog {
  private readonly show: ShowMessageBox;
  private readonly clock: Clock;
  private readonly line: Waiting[] = [];
  private readonly shown: number[] = [];
  private readonly deniedUntil = new Map<string, number>();
  private active: AbortController | undefined;
  private disposed = false;

  constructor(options: ApprovalDialogOptions) {
    this.show = options.show;
    this.clock = options.clock ?? systemClock;
  }

  request(request: ApprovalRequest): Promise<ApprovalOutcome> {
    if (this.disposed || request.signal.aborted) return Promise.resolve(denial('cancelled'));
    if (this.recentlyDenied(request.deviceId)) return Promise.resolve(denial('denied'));
    if (this.active !== undefined && this.line.length >= MAX_WAITING) return Promise.resolve(denial('busy'));
    return new Promise<ApprovalOutcome>((resolve) => {
      const waiting: Waiting = {
        request,
        resolve,
        onAbort: () => {
          const index = this.line.indexOf(waiting);
          if (index >= 0) this.line.splice(index, 1);
          resolve(denial('cancelled'));
        },
      };
      request.signal.addEventListener('abort', waiting.onAbort, { once: true });
      this.line.push(waiting);
      this.next();
    });
  }

  /** Close the open dialog and turn everything waiting away. */
  dispose(): void {
    this.disposed = true;
    this.active?.abort();
    for (const waiting of this.line.splice(0)) {
      waiting.request.signal.removeEventListener('abort', waiting.onAbort);
      waiting.resolve(denial('cancelled'));
    }
  }

  private recentlyDenied(deviceId: string): boolean {
    const until = this.deniedUntil.get(deviceId);
    if (until === undefined) return false;
    if (this.clock.now() < until) return true;
    this.deniedUntil.delete(deviceId);
    return false;
  }

  private next(): void {
    if (this.active !== undefined) return;
    const waiting = this.line.shift();
    if (waiting === undefined) return;
    waiting.request.signal.removeEventListener('abort', waiting.onAbort);
    const controller = new AbortController();
    this.active = controller;
    void this.ask(waiting.request, controller).catch(() => denial('cancelled')).then(waiting.resolve).finally(() => {
      this.active = undefined;
      this.next();
    });
  }

  private async ask(request: ApprovalRequest, controller: AbortController): Promise<ApprovalOutcome> {
    if (request.signal.aborted) return denial('cancelled');
    if (this.recentlyDenied(request.deviceId)) return denial('denied');
    if (request.alreadyApproved?.() === true) return { allowed: true };
    const now = this.clock.now();
    while (this.shown.length > 0 && now - this.shown[0]! >= RATE_WINDOW_MS) this.shown.shift();
    if (this.shown.length >= MAX_DIALOGS_PER_WINDOW) return denial('rate_limited');
    this.shown.push(now);

    const abort = (): void => controller.abort();
    request.signal.addEventListener('abort', abort, { once: true });
    let timedOut = false;
    const timer = this.clock.setTimeout(() => { timedOut = true; controller.abort(); }, DIALOG_TIMEOUT_MS);
    try {
      const { response } = await this.show(this.messageBox(request, controller.signal));
      // An answer that raced the abort is not trusted: the connection it was about is gone.
      if (controller.signal.aborted) return denial(timedOut ? 'timeout' : 'cancelled');
      if (response === ALLOW) return { allowed: true };
      this.deniedUntil.set(request.deviceId, this.clock.now() + DENIAL_MEMORY_MS);
      return denial('denied');
    } catch {
      return denial(timedOut ? 'timeout' : 'cancelled');
    } finally {
      this.clock.clearTimeout(timer);
      request.signal.removeEventListener('abort', abort);
    }
  }

  private messageBox(request: ApprovalRequest, signal: AbortSignal): ApprovalMessageBox {
    const name = shownName(request.deviceName);
    return {
      type: 'question',
      title: nativeTr('native_remote_title'),
      message: nativeTr('native_remote_allow', name),
      detail: [
        nativeTr('native_remote_device', name, request.platform === 'web' ? nativeTr('native_remote_browser') : PLATFORM_LABELS[request.platform]),
        nativeTr('native_remote_code', formatPairingCode(request.pairingCode)),
        '',
        nativeTr('native_remote_detail'),
      ].join('\n'),
      buttons: [nativeTr('permission_allow'), nativeTr('permission_deny')],
      defaultId: DENY,
      cancelId: DENY,
      noLink: true,
      signal,
    };
  }
}
