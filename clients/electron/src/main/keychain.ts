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
 * Store provider secrets as generic-password items instead of encrypting a
 * blob with Electron Safe Storage. Safe Storage's per-app ACL asks for the
 * login password again whenever an ad-hoc packaged helper is launched. The
 * `security` item trusts the security CLI, so the desktop can read the item
 * without spawning a second Electron process or showing a prompt every run.
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
    // Hex keeps the secret out of argv and avoids quoting/newline ambiguity
    // in `security -i`; the keychain stores the decoded bytes.
    const hex = Buffer.from(value, 'utf8').toString('hex');
    const command = [
      'add-generic-password', '-U',
      '-a', shellQuote(accountFor(providerId)),
      '-s', shellQuote(KEYCHAIN_SERVICE),
      '-X', shellQuote(hex),
      // The read path is intentionally a short-lived security CLI process.
      // Trusting that binary avoids Electron Safe Storage's per-launch ACL
      // prompt while keeping the item protected by the user's login keychain.
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
      // Missing items are already cleared. Other errors are surfaced so the
      // UI never reports a successful delete that left a secret behind.
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
