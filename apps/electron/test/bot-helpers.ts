import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { BotStore } from '../src/main/bots/store.js';

/**
 * A computer for the Bots to live on: a home folder with the default
 * configuration home in it, an app data folder (its path has a space, as the
 * real one does on macOS) with some private entries, and folders to bind.
 */
export function botFixture(t: { after(fn: () => void): void }, options: { ids?: boolean } = {}) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-bots-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = join(root, 'home');
  const defaultConfigHome = join(home, '.lingxi');
  const userData = join(root, 'Application Support', 'LingXi');
  mkdirSync(join(defaultConfigHome, 'skills'), { recursive: true });
  mkdirSync(join(userData, 'logs'), { recursive: true });
  mkdirSync(join(userData, 'bridge-runtime'), { recursive: true });
  writeFileSync(join(userData, 'settings.v1.json'), '{}');
  writeFileSync(join(defaultConfigHome, 'settings.json'), '{"theme":"owner"}');
  let sequence = 0;
  const store = new BotStore({
    userData, home, defaultConfigHome,
    ...(options.ids ? {} : { newId: () => `Bot${String(++sequence).padStart(19, '0')}` }),
  });
  const folder = (name: string): string => {
    const path = join(root, 'work', name);
    mkdirSync(path, { recursive: true });
    return realpathSync(path);
  };
  return { root, home, defaultConfigHome, userData, store, folder };
}

export const SESSION = '11111111-2222-4333-8444-555555555555';

/** What a `BotsUi` is given where the test has nothing to do with tasks. */
export const noTasks = { forOwner: () => ({ tasks: [], truncated: false, active: { received: 0, sent: 0 } }), cancelForOwner: () => undefined };
