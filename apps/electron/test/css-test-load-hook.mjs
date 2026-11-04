import { readFile } from 'node:fs/promises';

export async function load(url, context, nextLoad) {
  if (url.startsWith('file:')) {
    const file = new URL(url);
    if (file.pathname.endsWith('.css') && !file.search && !file.hash) {
      // A missing or unreadable stylesheet is still an error in a test run.
      await readFile(file);
      return { format: 'module', source: 'export {};', shortCircuit: true };
    }
  }
  return nextLoad(url, context);
}
