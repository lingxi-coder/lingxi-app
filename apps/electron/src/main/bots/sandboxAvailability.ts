/**
 * Whether this computer can confine the shell commands of a Bot to its folders. The engine makes the same check
 * when it starts with the sandbox required; making it here first tells the owner before a Bot is made or
 * started, instead of after an engine that would not start. Free of Electron.
 *
 * macOS needs `sandbox-exec`, Linux and WSL2 need `bwrap`; WSL1 and every other system, Windows included, cannot
 * (the engine's dependency check says the same). A missing `socat` is no reason: it only narrows which hosts
 * the commands may reach.
 */

import { accessSync, constants, readFileSync, statSync } from 'node:fs';
import { delimiter, join } from 'node:path';

import { BOT_SANDBOX_UNAVAILABLE_MESSAGE, type BotSandboxAvailability } from '../../shared/bots.js';

/** What of the system the check reads. */
export interface SandboxHost {
  /** Whether the program is found on the `PATH`. */
  onPath(program: string): boolean;
  /** The text of `/proc/version`; absent where there is none to read. */
  procVersion(): string | undefined;
}

export const systemSandboxHost: SandboxHost = {
  onPath(program) {
    for (const directory of (process.env['PATH'] ?? '').split(delimiter)) {
      if (directory === '') continue;
      const file = join(directory, program);
      try {
        if (!statSync(file).isFile()) continue;
        accessSync(file, constants.X_OK);
        return true;
      } catch { /* not here */ }
    }
    return false;
  },
  procVersion() {
    try { return readFileSync('/proc/version', 'utf8'); } catch { return undefined; }
  },
};

/** The engine's reading of `/proc/version`: a WSL kernel is WSL2 when it says `microsoft-standard` or `WSL2`, and else WSL1 when it says `Microsoft`. */
function isWsl1(procVersion: string | undefined): boolean {
  if (procVersion === undefined) return false;
  return !procVersion.includes('microsoft-standard') && !procVersion.includes('WSL2') && procVersion.includes('Microsoft');
}

/** The keys of the texts that say why, in the Bots catalog. */
export const SANDBOX_REASON = {
  noSandboxExec: 'bots_sandbox_why_no_sandbox_exec',
  noBwrap: 'bots_sandbox_why_no_bwrap',
  wsl1: 'bots_sandbox_why_wsl1',
  platform: 'bots_sandbox_why_platform',
} as const;

export function sandboxAvailability(platform: NodeJS.Platform = process.platform, host: SandboxHost = systemSandboxHost): BotSandboxAvailability {
  if (platform === 'darwin') {
    return host.onPath('sandbox-exec') ? { available: true } : { available: false, reason: SANDBOX_REASON.noSandboxExec };
  }
  if (platform === 'linux') {
    if (isWsl1(host.procVersion())) return { available: false, reason: SANDBOX_REASON.wsl1 };
    return host.onPath('bwrap') ? { available: true } : { available: false, reason: SANDBOX_REASON.noBwrap };
  }
  return { available: false, reason: SANDBOX_REASON.platform };
}

/** A Bot's sandbox is on, and this computer cannot provide one: the Bot cannot be made or started as it is. */
export class BotSandboxUnavailableError extends Error {
  constructor() {
    super(BOT_SANDBOX_UNAVAILABLE_MESSAGE);
    this.name = 'BotSandboxUnavailableError';
  }
}
