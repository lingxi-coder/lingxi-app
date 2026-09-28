import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

let root: string | undefined;

/** Read fixtures from the locked runtime identity, never an adjacent checkout. */
export function runtimePath(...segments: string[]): string {
  root ??= execFileSync('python3', [
    fileURLToPath(new URL('../../../scripts/lib/runtime_source.py', import.meta.url)),
    '--root',
  ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }).trim();
  if (!root) throw new Error('The pinned runtime resolver returned an empty path');
  return join(root, ...segments);
}
