import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MarketLinkInstaller, marketApplicationId } from '../src/main/local-apps/marketLinkInstaller.ts';
import type { DesktopLocalApp } from '../src/shared/localApps.ts';
import { parseDeepLink } from '../../../packages/deep-links/index.ts';
const row = { id: 'local-1', name: 'Timer', revision: 1, marketOrigin: { appId: 'market-1', releaseId: 'v1', digest: 'a' } } as DesktopLocalApp;
function fixture() {
  const calls: string[] = [];
  const options = {
    list: async (): Promise<DesktopLocalApp[]> => [],
    preview: async (id: string) => { calls.push(`preview:${id}`); return { id, name: 'Timer', summary: 'An offline timer' }; },
    confirm: async () => { calls.push('confirm'); return true; },
    install: async (id: string) => { calls.push(`install:${id}`); return row; },
    installed: () => { calls.push('changed'); },
  };
  return { options, calls };
}
test('browser scheme and HTTPS routes retain only the market application identity', () => {
  for (const link of ['lingxi://market/market-1', 'https://lingxios.net/open/market/market-1']) {
    const target = parseDeepLink(link);
    assert.equal(marketApplicationId(target.route.split('/')[1]), 'market-1');
  }
  for (const value of ['', '../secret', 'a/b', 'https://evil.test/payload.zip', 'a'.repeat(129), null]) assert.throws(() => marketApplicationId(value));
  assert.throws(() => parseDeepLink('lingxi://market/market-1?url=https://evil.test/payload.zip'));
});
test('a link previews, confirms, installs, and notifies the library; duplicate clicks share the operation', async () => {
  const { options, calls } = fixture();
  const installer = new MarketLinkInstaller(options);
  const first = installer.install('market-1');
  assert.equal(installer.install('market-1'), first);
  assert.equal(await first, row);
  assert.deepEqual(calls, ['preview:market-1', 'confirm', 'install:market-1', 'changed']);
});
test('an already installed application is returned without creating or overwriting an installation', async () => {
  const { options, calls } = fixture(); options.list = async () => [row];
  assert.equal(await new MarketLinkInstaller(options).install('market-1'), row);
  assert.deepEqual(calls, []);
});
test('cancel and mismatched preview cannot download or register an application', async () => {
  const { options, calls } = fixture(); options.confirm = async () => false;
  assert.equal(await new MarketLinkInstaller(options).install('market-1'), null);
  assert.deepEqual(calls, ['preview:market-1']);
  options.preview = async () => ({ id: 'another-app', name: 'Wrong', summary: '' });
  await assert.rejects(new MarketLinkInstaller(options).install('market-1'), /invalid application/);
});
test('a failed download reports failure, does not notify success, and allows retry', async () => {
  const { options, calls } = fixture(); let fail = true;
  options.install = async () => { if (fail) throw new Error('download failed'); return row; };
  const installer = new MarketLinkInstaller(options);
  await assert.rejects(installer.install('market-1'), /download failed/);
  assert.ok(!calls.includes('changed'));
  fail = false; assert.equal(await installer.install('market-1'), row);
  assert.equal(calls.filter(value => value === 'changed').length, 1);
});
