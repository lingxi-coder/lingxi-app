import { execFileSync } from 'node:child_process';

const KEYCHAIN_SERVICE = 'lingxi-code-desktop';
const SECURITY_BIN = '/usr/bin/security';
const SECURITY_TIMEOUT_MS = 3_000;

export interface KeychainCredentialStore {
  readonly available: boolean;
  has(providerId: string): boolean;
  read(providerId: string): string | undefined;
  write(providerId: string, value: string): void;
  clear(providerId: string): void;
}

/**
 * Compatibility reader for credentials written by older Electron releases.
 *
 * New credentials are persisted by the Rust engine. This class stays wired
 * only so launch can read the old `lingxi-code-desktop` generic-password item,
 * pass it to the engine for the current process, and migrate it to the shared
 * CLI/TUI store after the bridge is connected.
 */
export class MacKeychainCredentialStore implements KeychainCredentialStore {
  readonly available = process.platform === 'darwin';

  has(providerId: string): boolean {
    if (!this.available) return false;
    try {
      execFileSync(SECURITY_BIN, ['find-generic-password', '-a', accountFor(providerId), '-s', KEYCHAIN_SERVICE], {
        stdio: ['ignore', 'ignore', 'ignore'],
        timeout: SECURITY_TIMEOUT_MS,
      });
      return true;
    } catch {
      return false;
    }
  }

  read(providerId: string): string | undefined {
    if (!this.available) return undefined;
    try {
      const value = execFileSync(SECURITY_BIN, [
        'find-generic-password', '-a', accountFor(providerId), '-s', KEYCHAIN_SERVICE, '-w',
      ], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
        timeout: SECURITY_TIMEOUT_MS,
      });
      return value.replace(/\r?\n$/, '');
    } catch {
      return undefined;
    }
  }

  write(providerId: string, value: string): void {
    if (!this.available) throw new Error('macOS Keychain is unavailable');
    const hex = Buffer.from(value, 'utf8').toString('hex');
    const command = [
      'add-generic-password', '-U',
      '-a', shellQuote(accountFor(providerId)),
      '-s', shellQuote(KEYCHAIN_SERVICE),
      '-X', shellQuote(hex),
      '-T', shellQuote('/usr/bin/security'),
      '\n',
    ].join(' ');
    try {
      execFileSync(SECURITY_BIN, ['-i'], {
        input: command,
        stdio: ['pipe', 'ignore', 'ignore'],
        timeout: SECURITY_TIMEOUT_MS,
      });
    } catch {
      throw new Error('secure credential storage is unavailable');
    }
  }

  clear(providerId: string): void {
    if (!this.available) return;
    try {
      execFileSync(SECURITY_BIN, [
        'delete-generic-password', '-a', accountFor(providerId), '-s', KEYCHAIN_SERVICE,
      ], {
        stdio: ['ignore', 'ignore', 'ignore'],
        timeout: SECURITY_TIMEOUT_MS,
      });
    } catch {
      if (this.has(providerId)) throw new Error('secure credential storage is unavailable');
    }
  }
}

function accountFor(providerId: string): string {
  return `lingxi-code-desktop-key:${providerId}`;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}
