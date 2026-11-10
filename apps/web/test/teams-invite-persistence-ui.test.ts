import assert from 'node:assert/strict';
import type { TestContext } from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { createElement } from 'react';
import { JsxEmit, ModuleKind, ScriptTarget, transpileModule } from 'typescript';
import * as intent from '../src/teams/invite-intent';
import type { User } from 'firebase/auth';
import type TeamsPageType from '../src/pages/TeamsPage';
import { FakeTeams, team, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { hostNode, reactTestRenderer, scenario, settle, textOf, withDom } from './support/workspacePage';

const require = createRequire(import.meta.url);
const { TeamsWorkspaceView } = loadTeamsModule<typeof import('../src/teams/TeamsWorkspaceView')>(new URL('../src/teams/TeamsWorkspaceView', import.meta.url).pathname);
const { SignInCard } = loadTeamsModule<typeof import('../src/teams/SignInCard')>(new URL('../src/teams/SignInCard', import.meta.url).pathname);
const code = 'a'.repeat(32), intentKey = 'lingxi.teams.inviteIntent.1';
const store = () => { const values = new Map<string, string>(); return { values, getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } }; };

/** Keep the real page, invitation helper and workspace UI; only Firebase/network boundaries are mocked. */
function pageFixture() {
  const fake = new FakeTeams('alice'), listeners = new Set<(user: User | null) => void>();
  const source = new URL('../src/pages/TeamsPage.tsx', import.meta.url);
  const compiled = transpileModule(readFileSync(source, 'utf8'), { fileName: source.pathname, compilerOptions: { module: ModuleKind.CommonJS, target: ScriptTarget.ES2020, jsx: JsxEmit.ReactJSX, esModuleInterop: true } });
  const auth = { app: {} }, module = { exports: {} as { default: typeof TeamsPageType } };
  const mocks: Record<string, unknown> = {
    'firebase/auth': { onAuthStateChanged(_auth: unknown, next: (user: User | null) => void) { listeners.add(next); next(null); return () => { listeners.delete(next); }; } },
    '../app/firebase': { auth }, '../teams/firestore': { teamsDatabase: () => ({}) }, '../teams/invite-intent': intent,
    '../teams/api': { webTeamsApi: () => ({ ...fake.api, close() {} }) }, '../friends/api': { webFriendsApi: () => ({ close() {} }) }, '../teams/TeamsWorkspaceView': { TeamsWorkspaceView }, '../teams/SignInCard': { SignInCard },
  };
  new Function('require', 'module', 'exports', compiled.outputText)((id: string) => id in mocks ? mocks[id] : require(id), module, module.exports);
  return { Page: module.exports.default, fake, signIn() { const user = { uid: 'alice', email: 'fixture@example.invalid' } as User; listeners.forEach(next => next(user)); } };
}

function browserTabs(context: TestContext, persistent: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>) {
  withDom(context);
  const names = ['location', 'history', 'localStorage', 'sessionStorage'] as const;
  const saved = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  context.after(() => { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  return (search: string, pathname = '/teams', transient = store()) => {
    const location = { pathname, search, hash: '' };
    Object.defineProperty(globalThis, 'location', { configurable: true, value: location });
    Object.defineProperty(globalThis, 'history', { configurable: true, value: { replaceState() { location.search = ''; location.hash = ''; } } });
    Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: persistent });
    Object.defineProperty(globalThis, 'sessionStorage', { configurable: true, value: transient });
    return location;
  };
}

scenario('closing an invitation tab and reopening Teams retains bounded intent until an explicit join', async context => {
  const persistent = store(), fixture = pageFixture(), tab = browserTabs(context, persistent);
  const { create, act } = reactTestRenderer(); let first: any, reopened: any;
  context.after(async () => { await act(async () => { first?.unmount(); reopened?.unmount(); }); });
  tab(`?code=${code}`, '/teams/join');
  await act(async () => { first = create(createElement(fixture.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
  assert.match(textOf(first.toJSON()), /Sign in to review and confirm the invitation/);
  const original = persistent.getItem(intentKey);
  await act(async () => { first.unmount(); first = undefined; });
  tab('');
  await act(async () => { reopened = create(createElement(fixture.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
  assert.match(textOf(reopened.toJSON()), /Sign in to review and confirm the invitation/);
  assert.equal(persistent.getItem(intentKey), original, 'reopening does not renew the invitation lifetime');
  assert.equal(fixture.fake.teamCommands.length, 0);
  await act(async () => { fixture.signIn(); await settle(); });
  assert.equal(reopened.root.findAll((node: any) => node.type === 'input' && node.props.value === code).length, 1);
  assert.equal(fixture.fake.teamCommands.length, 0, 'sign-in and reopening never auto-join');
  fixture.fake.execute = async command => { fixture.fake.teamCommands.push(command); fixture.fake.teams = [team('team-aaaa')]; return { teamId: 'team-aaaa', generation: 1 }; };
  await act(async () => { reopened.root.findAllByType('form')[0].props.onSubmit({ preventDefault() {} }); await settle(); });
  await until(() => fixture.fake.teamCommands.length === 1 && persistent.getItem(intentKey) === null);
  assert.equal(fixture.fake.teamCommands[0]?.action, 'join');
});

scenario('session-only pending invitation is neither read nor migrated into the persistent store', async context => {
  const persistent = store(), transient = store(), fixture = pageFixture(), tab = browserTabs(context, persistent);
  const saved = JSON.stringify({ code, expiresAt: Date.now() + 86400000 }); transient.setItem(intentKey, saved); tab('', '/teams', transient);
  const { create, act } = reactTestRenderer(); let tree: any;
  context.after(async () => { await act(async () => tree?.unmount()); });
  await act(async () => { tree = create(createElement(fixture.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
  assert.doesNotMatch(textOf(tree.toJSON()), /Sign in to review and confirm the invitation/);
  assert.match(textOf(tree.toJSON()), /Create or join a team with your LingXi account/);
  assert.equal(persistent.values.size, 0); assert.equal(transient.getItem(intentKey), saved); assert.equal(fixture.fake.teamCommands.length, 0);
});

scenario('denied persistent storage keeps the canonical invitation URL available for a reload', async context => {
  const denied = { getItem(): string | null { throw new Error('denied'); }, setItem() { throw new Error('denied'); }, removeItem() { throw new Error('denied'); } };
  const fixture = pageFixture(), tab = browserTabs(context, denied), location = tab(`?code=${code}`, '/teams/join');
  const { create, act } = reactTestRenderer(); let tree: any;
  context.after(async () => { await act(async () => tree?.unmount()); });
  await act(async () => { tree = create(createElement(fixture.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
  assert.match(textOf(tree.toJSON()), /Sign in to review and confirm the invitation/);
  assert.equal(location.search, `?code=${code}`, 'the link is stripped only after the intent is retained');
  assert.equal(fixture.fake.teamCommands.length, 0);
});

for (const resolve of ['cancel', 'join'] as const) {
  scenario(`an older tab's ${resolve} preserves the newer tab's invitation`, async context => {
    const persistent = store(), first = pageFixture(), second = pageFixture(), tab = browserTabs(context, persistent);
    const newerCode = 'b'.repeat(32), { create, act } = reactTestRenderer(); let older: any, newer: any;
    context.after(async () => { await act(async () => { older?.unmount(); newer?.unmount(); }); });
    tab(`?code=${code}`, '/teams/join');
    await act(async () => { older = create(createElement(first.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
    await act(async () => { first.signIn(); await settle(); });
    assert.equal(older.root.findAll((node: any) => node.type === 'input' && node.props.value === code).length, 1);
    tab(`?code=${newerCode}`, '/teams/join');
    await act(async () => { newer = create(createElement(second.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
    const saved = persistent.getItem(intentKey); assert.equal(JSON.parse(saved!).code, newerCode);
    if (resolve === 'join') first.fake.execute = async command => { first.fake.teamCommands.push(command); first.fake.teams = [team('team-aaaa')]; return { teamId: 'team-aaaa', generation: 1 }; };
    await act(async () => {
      if (resolve === 'cancel') older.root.findAllByType('button').find((node: any) => textOf(node) === 'Cancel').props.onClick();
      else older.root.findAllByType('form')[0].props.onSubmit({ preventDefault() {} });
      await settle();
    });
    assert.equal(persistent.getItem(intentKey), saved, 'resolving A must preserve the complete newer B record');
    assert.equal(first.fake.teamCommands.length, resolve === 'join' ? 1 : 0);
    await act(async () => { second.signIn(); await settle(); });
    assert.equal(newer.root.findAll((node: any) => node.type === 'input' && node.props.value === newerCode).length, 1);
    assert.equal(second.fake.teamCommands.length, 0, 'the newer invitation still requires explicit confirmation');
  });
}

scenario('closing an unrelated manual join dialog preserves another tab\'s pending invitation', async context => {
  const persistent = store(), first = pageFixture(), second = pageFixture(), tab = browserTabs(context, persistent);
  const { create, act } = reactTestRenderer(); let manual: any, incoming: any;
  context.after(async () => { await act(async () => { manual?.unmount(); incoming?.unmount(); }); });
  tab('');
  await act(async () => { manual = create(createElement(first.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
  await act(async () => { first.signIn(); await settle(); });
  await act(async () => { manual.root.findAllByType('button').find((node: any) => textOf(node) === 'Join with an invite').props.onClick(); await settle(); });
  tab(`?code=${code}`, '/teams/join');
  await act(async () => { incoming = create(createElement(second.Page, { locale: 'en' }), { createNodeMock: hostNode }); await settle(); });
  const saved = persistent.getItem(intentKey); assert.ok(saved);
  await act(async () => { manual.root.findAllByType('button').find((node: any) => textOf(node) === 'Cancel').props.onClick(); await settle(); });
  assert.equal(persistent.getItem(intentKey), saved);
  assert.equal(first.fake.teamCommands.length + second.fake.teamCommands.length, 0);
});
