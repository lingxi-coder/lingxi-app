import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { nativeLocaleOverride, setNativeLocale } from '../shared/nativeLocale.js';

/** Restore before ready so cold-launch dialogs use the saved interface language. */
export function restoreInterfaceLocale(path: string): void {
  try {
    const stored: unknown = JSON.parse(readFileSync(path, 'utf8'));
    if (stored === null) setNativeLocale('en', true);
    else setNativeLocale(stored);
  }
  catch { setNativeLocale('en', true); }
}

export function persistInterfaceLocale(path: string): void {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.tmp`;
  writeFileSync(temporary, JSON.stringify(nativeLocaleOverride()) + '\n', { encoding: 'utf8', mode: 0o600 });
  renameSync(temporary, path);
}
