import fs from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import { join } from 'node:path';

const [root, account, mode] = process.argv.slice(2);
// Keep the prepublication pause alive until the parent sends SIGKILL.
process.on('message', () => {});
const originalLink = fs.link;
if (mode === 'before_publish' || mode === 'publish_failure') fs.link = async (from, to) => {
  if (to === join(root, account)) {
    if (mode === 'publish_failure') throw Object.assign(new Error('publication failed'), { code: 'EIO' });
    process.send({ phase: 'before_publish' });
    await new Promise(() => {});
  }
  return originalLink(from, to);
};
const originalOpen = fs.open;
if (mode === 'write_failure' || mode === 'pause_read') fs.open = async (path, ...args) => {
  const file = await originalOpen(path, ...args);
  if (mode === 'pause_read' && path === join(root, account)) process.send({ phase: 'opened_owner' });
  if (mode === 'write_failure' && String(path).endsWith('.tmp')) file.writeFile = async () => { throw Object.assign(new Error('write failed'), { code: 'EIO' }); };
  return file;
};
const originalStat = fs.lstat;
let paused = false;
if (mode === 'pause_read') fs.lstat = async path => {
  const entry = await originalStat(path);
  if (!paused && path === join(root, account)) {
    paused = true;
    process.send({ phase: 'reading_owner' });
    await new Promise(resolve => process.once('message', resolve));
  }
  return entry;
};
syncBuiltinESMExports();
const { withProviderAccountLock } = await import('../../src/main/provider-accounts.ts');
try {
  await withProviderAccountLock(root, account, async () => {
    process.send({ phase: 'acquired' });
    if (mode === 'held') await new Promise(resolve => process.once('message', resolve));
    const path = join(root, 'counter');
    const before = Number(await fs.readFile(path, 'utf8').catch(() => '0'));
    await new Promise(resolve => setTimeout(resolve, 10));
    await fs.writeFile(path, String(before + 1));
  });
  process.send({ phase: 'done' });
} catch (error) {
  if (!['write_failure', 'publish_failure'].includes(mode)) throw error;
  process.send({ phase: 'failed', code: error.code, remaining: await fs.readdir(root) });
} finally { process.disconnect(); }
