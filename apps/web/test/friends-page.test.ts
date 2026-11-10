import assert from 'node:assert/strict';
import type { TestContext } from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { createElement } from 'react';
import { JsxEmit, ModuleKind, ScriptTarget, transpileModule } from 'typescript';
import type { User } from 'firebase/auth';
import * as intent from '../src/friends/add-intent';
import type FriendsPageType from '../src/pages/FriendsPage';
import { FakeFriends, card, pairTeamId, pointer, uid, until } from './support/fakeFriends';
import { FakeTeams, conversation } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { hostNode, reactTestRenderer, scenario, settle, textOf, withDom } from './support/workspacePage';

const require = createRequire(import.meta.url);
const { FriendsWorkspaceView } = loadTeamsModule<typeof import('../src/friends/FriendsWorkspaceView')>(new URL('../src/friends/FriendsWorkspaceView', import.meta.url).pathname);
const { SignInCard } = loadTeamsModule<typeof import('../src/teams/SignInCard')>(new URL('../src/teams/SignInCard', import.meta.url).pathname);
const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-', intentKey = 'lingxi.friends.addIntent.1';
const store = () => { const values = new Map<string, string>(); return { values, getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } }; };

/** The real page, intent helper and workspace UI; only Firebase and the network are replaced. */
function pageFixture() {
  const friends = new FakeFriends(), teams = new FakeTeams(), listeners = new Set<(user: User | null) => void>();
  const source = new URL('../src/pages/FriendsPage.tsx', import.meta.url);
  const compiled = transpileModule(readFileSync(source, 'utf8'), { fileName: source.pathname, compilerOptions: { module: ModuleKind.CommonJS, target: ScriptTarget.ES2020, jsx: JsxEmit.ReactJSX, esModuleInterop: true } });
  const auth = { app: {} }, module = { exports: {} as { default: typeof FriendsPageType } };
  const mocks: Record<string, unknown> = {
    'firebase/auth': { onAuthStateChanged(_auth: unknown, next: (user: User | null) => void) { listeners.add(next); next(null); return () => { listeners.delete(next); }; }, signOut: async () => undefined },
    '../app/firebase': { auth }, '../teams/firestore': { teamsDatabase: () => ({}) }, '../friends/add-intent': intent,
    '../friends/api': { webFriendsApi: () => ({ ...friends.api, close() {} }) }, '../teams/api': { webTeamsApi: () => ({ ...teams.api, close() {} }) },
    '../friends/FriendsWorkspaceView': { FriendsWorkspaceView }, '../teams/SignInCard': { SignInCard },
  };
  new Function('require', 'module', 'exports', compiled.outputText)((id: string) => id in mocks ? mocks[id] : require(id), module, module.exports);
  return { Page: module.exports.default, friends, teams, signIn() { const user = { uid, email: 'fixture@example.invalid' } as User; listeners.forEach(next => next(user)); } };
}

function browserTabs(context: TestContext, persistent: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>) {
  withDom(context);
  const names = ['location', 'history', 'localStorage', 'sessionStorage'] as const;
  const saved = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  context.after(() => { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  return (search: string, pathname = '/friends/add') => {
    const location = { pathname, search, hash: '' };
    Object.defineProperty(globalThis, 'location', { configurable: true, value: location });
    Object.defineProperty(globalThis, 'history', { configurable: true, value: { replaceState() { location.search = ''; location.hash = ''; } } });
    Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: persistent });
    return location;
  };
}
const open = async (Page: typeof FriendsPageType, props: Record<string, unknown> = {}) => {
  const { create, act } = reactTestRenderer(); let tree: any;
  const badges: (string | null)[] = [];
  await act(async () => { tree = create(createElement(Page, { locale: 'en', route: '/friends/add', badge: null, onBadge: (text: string | null) => badges.push(text), ...props }), { createNodeMock: hostNode }); await settle(); });
  return { tree, badges, act, text: () => textOf(tree.toJSON() ?? ''), find: (match: (node: any) => boolean): any[] => tree.root.findAll((node: any) => typeof node.type === 'string' && match(node)) };
};

scenario('a friend link opened signed out is kept bounded, the address bar is cleaned, and nothing is added', async context => {
  const persistent = store(), fixture = pageFixture(), tab = browserTabs(context, persistent);
  const location = tab(`?code=${code}`);
  const page = await open(fixture.Page); context.after(() => page.act(async () => page.tree.unmount()));
  assert.match(page.text(), /Sign in to your LingXi account to add friends and chat\./);
  assert.equal(location.search, '', 'the code is not left in the address bar');
  const saved = JSON.parse(persistent.getItem(intentKey)!) as { code: string; expiresAt: number };
  assert.equal(saved.code, code); assert.ok(saved.expiresAt > Date.now() + 6.9 * 86400000 && saved.expiresAt <= Date.now() + 7 * 86400000);
  assert.deepEqual(fixture.friends.commands, []); assert.equal(fixture.friends.open.cards, 0, 'no listener without an account');
});

scenario('reopening Friends and signing in looks the kept link up and asks; only the confirmation redeems, and the intent ends with it', async context => {
  const persistent = store(), fixture = pageFixture(), tab = browserTabs(context, persistent);
  persistent.setItem(intentKey, JSON.stringify({ code, expiresAt: Date.now() + 86400000 }));
  tab('', '/friends');
  fixture.friends.lookup({ uid: 'sam', displayName: 'Sam', relation: 'none' });
  const page = await open(fixture.Page, { route: '/friends' }); context.after(() => page.act(async () => page.tree.unmount()));
  assert.match(page.text(), /Sign in to your LingXi account/);
  await page.act(async () => { fixture.signIn(); await settle(); });
  await until(() => fixture.friends.commands.length === 1, 'the lookup'); await page.act(async () => { await settle(); });
  assert.deepEqual(fixture.friends.commands, [{ action: 'lookup', code }]);
  assert.match(page.text(), /Add “Sam” as a friend\?/); assert.equal(fixture.friends.executed('redeem').length, 0, 'opening is not adding');
  assert.ok(persistent.getItem(intentKey), 'still pending until answered');
  const add = page.find(node => node.type === 'button' && textOf(node) === 'Add friend' && String(node.props.className).includes('tw-primary')).at(-1)!;
  await page.act(async () => { add.props.onClick(); await settle(); });
  await until(() => fixture.friends.executed('redeem').length === 1, 'the redeem');
  assert.equal((fixture.friends.executed('redeem')[0] as { code: string }).code, code);
  await page.act(async () => { await settle(); });
  assert.equal(persistent.getItem(intentKey), null, 'answered');
});

scenario('a tab that denies storage still opens the link from the address bar, once', async context => {
  const denied = { getItem(): string | null { throw new Error('denied'); }, setItem() { throw new Error('denied'); }, removeItem() { throw new Error('denied'); } };
  const fixture = pageFixture(), tab = browserTabs(context, denied);
  const location = tab(`?code=${code}`);
  const page = await open(fixture.Page); context.after(() => page.act(async () => page.tree.unmount()));
  assert.equal(location.search, `?code=${code}`, 'the URL is the only place the link is kept, so it stays');
  fixture.friends.lookup({ uid: 'sam' });
  await page.act(async () => { fixture.signIn(); await settle(); });
  await until(() => fixture.friends.commands.length === 1, 'the lookup');
  assert.deepEqual(fixture.friends.commands, [{ action: 'lookup', code }]);
});

scenario('a signed-in page runs the three listeners, reports the badge of D14 and clears it when it goes', async context => {
  const persistent = store(), fixture = pageFixture(), tab = browserTabs(context, persistent);
  fixture.friends.cards = [card('dave', { state: 'incoming' }), card('bob', { lastSeq: 4, lastReadSeq: 1, lastMessage: pointer(4) })];
  tab('', '/friends');
  const page = await open(fixture.Page, { route: '/friends' });
  await page.act(async () => { fixture.signIn(); await settle(); });
  await until(() => page.badges.includes('4'), 'the badge');
  assert.deepEqual(fixture.friends.open, { cards: 1, blocks: 1, offers: 1 });
  assert.ok(page.text().includes('Bob') && page.text().includes('Friend requests'));
  fixture.friends.emitCards([card('bob', { lastSeq: 4, lastReadSeq: 4 })]);
  await until(() => page.badges.at(-1) === null, 'the badge to clear');
  await page.act(async () => page.tree.unmount());
  assert.deepEqual(fixture.friends.open, { cards: 0, blocks: 0, offers: 0 }, 'leaving the page releases the listeners');
  assert.equal(page.badges.at(-1), null);
});

scenario('/friends/add opens Add friend and /friends?chat= opens that friend\'s chat, once the account is there', async context => {
  const persistent = store(), fixture = pageFixture(), tab = browserTabs(context, persistent);
  fixture.friends.cards = [card('bob', { lastSeq: 0, lastReadSeq: 0 })];
  tab('', '/friends/add');
  const adding = await open(fixture.Page); context.after(() => adding.act(async () => adding.tree.unmount()));
  await adding.act(async () => { fixture.signIn(); await settle(); });
  await until(() => adding.text().includes('Find someone'), 'the Add friend page');
  await adding.act(async () => adding.tree.unmount());

  const chat = pageFixture(), again = browserTabs(context, persistent);
  chat.friends.cards = [card('bob')];
  chat.teams.conversations.set(pairTeamId('bob'), [conversation('friend_chat', { kind: 'dm', participants: ['user:alice', 'user:bob'] })]);
  again('?chat=bob', '/friends');
  const page = await open(chat.Page, { route: '/friends' }); context.after(() => page.act(async () => page.tree.unmount()));
  await page.act(async () => { chat.signIn(); await settle(); });
  await until(() => page.find(node => node.type === 'textarea').length > 0, 'the chat\'s composer');
  assert.ok(page.find(node => node.type === 'h2').some(node => textOf(node) === 'Bob'), 'with the friend in the header');
});
