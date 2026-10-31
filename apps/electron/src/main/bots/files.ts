import { randomUUID } from 'node:crypto';
import { mkdirSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, sep } from 'node:path';

/** Write a private file so that a reader sees the old contents or the new, never half of either. */
export function writeFileAtomic(path: string, contents: string): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${randomUUID()}.tmp`;
  try {
    writeFileSync(temporary, contents, { mode: 0o600, flag: 'wx' });
    renameSync(temporary, path);
  } finally {
    try { unlinkSync(temporary); } catch { /* Renamed, or never created. */ }
  }
}

/** Whether `child` is `parent` or lies inside it. Both must be absolute and normalized. */
export function isInside(child: string, parent: string): boolean {
  if (child === parent) return true;
  return child.startsWith(parent.endsWith(sep) ? parent : `${parent}${sep}`);
}
