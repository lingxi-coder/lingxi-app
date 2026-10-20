import { realpathSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';

/** Only fixed event categories are allowed in uploaded report annotations. */
export function crashCategory(type: string, reason: string): string | null {
  if (reason === 'oom' && type === 'renderer') return 'renderer-oom';
  if (reason !== 'crashed' && reason !== 'abnormal-exit') return null;
  if (type === 'renderer') return 'renderer-crashed';
  if (type === 'GPU') return 'gpu-crashed';
  if (type === 'Utility') return 'utility-crashed';
  return null;
}

/** Explicit acceptance flag is allowed only with a wholly isolated home/data. */
export function isIsolatedCrashProbe(root: string | undefined, tempDirectory: string, userData: string, home: string | undefined, fixedHome: string | undefined): boolean {
  if (!root || !home || !fixedHome) return false;
  const canonical = (path: string) => { try { return realpathSync(path); } catch { return resolve(path); } };
  const directory = canonical(root);
  return dirname(directory) === canonical(tempDirectory)
    && basename(directory).startsWith('lingxi-crashlytics-acceptance-')
    && canonical(userData) === join(directory, 'userData')
    && canonical(home) === join(directory, 'home') && canonical(fixedHome) === canonical(home);
}
