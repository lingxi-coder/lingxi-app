import { lstat } from 'node:fs/promises';
import path from 'node:path';

export function validateGeneratedPath(relative: string): void {
  if (typeof relative !== 'string' || !relative || relative.includes('\\') || path.posix.isAbsolute(relative) || path.posix.normalize(relative) !== relative || relative.split('/').some((part) => part === '..' || part === '.')) {
    throw new Error(`Invalid generated path: ${String(relative)}`);
  }
}

/** Refuse redirects before reading, writing or deleting any owned output. */
export async function preflightGeneratedPaths(output: string, relatives: string[], allowedRoot: string): Promise<void> {
  const root = path.resolve(allowedRoot);
  const directory = path.resolve(output);
  const inside = path.relative(root, directory);
  if (!inside || inside.startsWith('..') || path.isAbsolute(inside)) throw new Error('Generated output must be a dedicated directory within its allowed root');
  for (const relative of relatives) validateGeneratedPath(relative);
  const candidates = new Set<string>([directory]);
  for (const relative of relatives) candidates.add(path.join(directory, relative));
  for (const destination of [...candidates]) {
    let current = destination;
    while (current !== root) {
      candidates.add(current);
      current = path.dirname(current);
    }
  }
  for (const candidate of candidates) {
    try {
      if ((await lstat(candidate)).isSymbolicLink()) throw new Error(`Generated output refuses symbolic links: ${candidate}`);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
  }
}
