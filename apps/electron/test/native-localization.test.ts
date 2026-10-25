import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { nativeCatalog } from '../src/shared/nativeCatalog';
import { configureNativeLocale, nativeLocale, nativeLocaleOverride, nativeTr, onNativeLocaleChange, resolveNativeLocale, setNativeLocale } from '../src/shared/nativeLocale';
import { applicationMenuTemplate } from '../src/main/applicationMenu';
import { persistInterfaceLocale, restoreInterfaceLocale } from '../src/main/localePreferences';
import { HostController } from '../src/main/host';
import { SettingsStore } from '../src/main/settings';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { CH_INTERFACE_LOCALE } from '../src/shared/interfaceLocale';

const locales = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'] as const;
test('native system fallback respects preferred languages and Chinese script overrides', () => {
  assert.equal(resolveNativeLocale(['fr', 'ko-KR']), 'ko');
  assert.equal(resolveNativeLocale(['zh-HK']), 'zh-Hant');
  assert.equal(resolveNativeLocale(['zh-Hans-HK']), 'zh-Hans');
  assert.equal(resolveNativeLocale(['zh-Hant-CN']), 'zh-Hant');
  assert.equal(resolveNativeLocale(['fr']), 'en');
});
test('native copy is complete in five languages and preserves argument positions', () => {
  const placeholders = (value: string) => [...value.matchAll(/%\d+\$[ds]/g)].map(match => match[0]).sort();
  for (const entry of Object.values(nativeCatalog)) for (const locale of locales) {
    assert.ok(entry[locale]?.trim());
    assert.deepEqual(placeholders(entry[locale]!), placeholders(entry.en!));
  }
  for (const locale of locales) {
    setNativeLocale(locale);
    assert.equal(nativeLocale(), locale);
    const title = nativeTr('native_remote_allow', 'Device %2$s 日本語');
    assert.ok(title.includes('Device %2$s 日本語'));
    assert.ok(!title.startsWith('native_'));
    assert.ok(nativeTr('native_remote_code', '042 713').includes('042 713'));
    assert.ok(nativeTr('native_trust_message', '/tmp/%1$s/文件').includes('/tmp/%1$s/文件'));
    const actions: string[] = [];
    const template = applicationMenuTemplate('linux', action => actions.push(action), false);
    assert.equal(template[0]?.label, nativeTr('native_file'));
    assert.equal(template[2]?.label, nativeTr('native_links'));
  }
  setNativeLocale('en');
});
test('native common labels match the canonical catalogs without bundling all mobile copy', () => {
  for (const locale of locales) {
    const canonical = JSON.parse(readFileSync(new URL(`../../../resources/translations/${locale}.json`, import.meta.url), 'utf8'));
    for (const key of ['common_cancel', 'common_delete', 'permission_allow', 'permission_deny']) {
      assert.equal(nativeCatalog[key]![locale], canonical[key]);
    }
  }
});
test('native locale updates notify menus once and reject arbitrary IPC payloads', () => {
  setNativeLocale('en');
  let changes = 0;
  onNativeLocaleChange(() => { changes += 1; });
  setNativeLocale('ja');
  setNativeLocale('ja');
  assert.equal(changes, 1);
  for (const value of ['', 'fr', 'ja-JP', {}, null, 7]) assert.throws(() => setNativeLocale(value), /Invalid interface locale/);
  assert.equal(nativeLocale(), 'ja');
  setNativeLocale('en');
});
test('a system language change rebuilds native menus without creating an explicit override', () => {
  let systemLanguage = 'en-US';
  configureNativeLocale(() => [systemLanguage]);
  setNativeLocale('en', true);
  let menu = applicationMenuTemplate('linux', () => {}, false);
  let rebuilds = 0;
  onNativeLocaleChange(() => { menu = applicationMenuTemplate('linux', () => {}, false); rebuilds += 1; });
  try {
    assert.equal(menu[0]?.label, 'File');
    systemLanguage = 'ja-JP';
    setNativeLocale('ja', true);
    assert.equal(menu[0]?.label, 'ファイル');
    assert.equal(rebuilds, 1);
    assert.equal(nativeLocaleOverride(), null);
  } finally { configureNativeLocale(() => ['en']); setNativeLocale('en'); }
});
test('native interface locale IPC stays guarded and is removed on host disposal', () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-locale-ipc-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (channel: string, handler: (...args: any[]) => any) => handlers.set(channel, handler), removeHandler: (channel: string) => handlers.delete(channel) };
  const manager = { registerIpc() {}, registerWindow() {}, runtimeSummaries: [] };
  const host = new HostController(new SettingsStore(directory), manager as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  try {
    const apply = handlers.get(CH_INTERFACE_LOCALE)!;
    apply({ sender, senderFrame: frame }, 'ja', false);
    assert.equal(nativeLocaleOverride(), 'ja');
    assert.throws(() => apply({ sender, senderFrame: {} }, 'en', false));
    assert.throws(() => apply({ sender: {}, senderFrame: frame }, 'en', false));
    assert.throws(() => apply({ sender, senderFrame: frame }, 'fr', false));
    assert.throws(() => apply({ sender, senderFrame: frame }, 'en', 'false'));
    apply({ sender, senderFrame: frame }, 'en', true);
    assert.equal(nativeLocaleOverride(), null);
    host.dispose();
    assert.equal(handlers.has(CH_INTERFACE_LOCALE), false);
  } finally { setNativeLocale('en'); rmSync(directory, { recursive: true, force: true }); }
  const preload = readFileSync(new URL('../src/preload/index.ts', import.meta.url), 'utf8');
  assert.match(preload, /CH_INTERFACE_LOCALE.*from '\.\.\/shared\/interfaceLocale\.js'/);
  assert.doesNotMatch(preload, /from '\.\.\/shared\/nativeLocale/);
});

test('cold-launch preferences restore overrides and keep follow-system dynamic across restarts', () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-locale-restore-'));
  const file = join(directory, 'interface-language.json');
  let systemLanguage = 'en-US';
  configureNativeLocale(() => [systemLanguage]);
  try {
    setNativeLocale('ja');
    persistInterfaceLocale(file);
    setNativeLocale('en');
    restoreInterfaceLocale(file);
    assert.equal(nativeLocale(), 'ja');
    assert.equal(nativeTr('native_link_unavailable'), 'この LingXi リンクを開けません');
    setNativeLocale('en');
    setNativeLocale('en', true);
    persistInterfaceLocale(file);
    assert.equal(JSON.parse(readFileSync(file, 'utf8')), null);
    systemLanguage = 'ko-KR';
    setNativeLocale('ja');
    restoreInterfaceLocale(file);
    assert.equal(nativeLocaleOverride(), null);
    assert.equal(nativeLocale(), 'ko');
    writeFileSync(file, '{invalid');
    restoreInterfaceLocale(file);
    assert.equal(nativeLocale(), 'ko');
  } finally { configureNativeLocale(() => ['en']); setNativeLocale('en'); rmSync(directory, { recursive: true, force: true }); }
});
