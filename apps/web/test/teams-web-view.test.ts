import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { type TestContext } from 'node:test';
import { createElement } from 'react';
import type { TeamsApi } from '../../../packages/teams';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { FakeFriends, card } from './support/fakeFriends';
import { FakeTeams, botCard, conversation, memoryStorage, message, team, until, uid } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { hostNode, mount, reactTestRenderer, scenario, settle, textOf, withDom } from './support/workspacePage';

// The Web page's own part of the Teams page: the workspace opened for an account, and the layout of the three
// views. The views themselves are covered by teams-workspace-interaction.test.ts and teams-bots-ui.test.ts.
const { TeamsWorkspaceLayout } = loadTeamsModule<typeof import('../src/teams/TeamsWorkspaceView')>(new URL('../src/teams/TeamsWorkspaceView', import.meta.url).pathname);
const { useTeamsWorkspace } = loadTeamsModule<typeof import('../../../packages/teams/useTeamsWorkspace')>('useTeamsWorkspace');
const { SignInCard } = loadTeamsModule<typeof import('../src/teams/SignInCard')>(new URL('../src/teams/SignInCard', import.meta.url).pathname);

const at = (minute: number) => new Date(2026, 10, 2, 11, minute).toISOString();
async function open(context: TestContext) {
  withDom(context);
  const fake = new FakeTeams();
  fake.teams = [team('team-aaaa', { name: 'Acme', role: 'owner' })];
  fake.bots = [botCard()];
  fake.conversations.set('team-aaaa', [conversation('chan-general', { title: 'general', lastSeq: 1, lastReadSeq: 1, lastMessage: { id: 'message-1', seq: 1, at: at(1), preview: 'One', author: { kind: 'user', uid: 'bob' } } })]);
  fake.threads.set('chan-general', [message(1, { text: 'One', createdAt: at(1) })]);
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => workspace.close());
  const snapshot = (): TeamsSnapshot => workspace.getSnapshot();
  await until(() => snapshot().conversations.length === 1 && snapshot().botsLoaded, 'the team to load');
  return { fake, workspace, snapshot };
}
const layout = (workspace: TeamsWorkspace, more: Record<string, unknown> = {}) => (snapshot: TeamsSnapshot) =>
  createElement(TeamsWorkspaceLayout, { teams: { workspace, snapshot, signedIn: true }, locale: 'en', ...more } as never);

scenario('layout: sidebar and conversation side by side, with the details only when asked for, from the header or the team menu', async context => {
  const { workspace } = await open(context);
  const page = await mount(workspace, layout(workspace)); context.after(() => page.unmount());
  assert.equal(page.oneByClass('teams-web-body').props['data-pane'], 'conversation');
  assert.equal(page.byClass('tw-sidebar').length, 1); assert.equal(page.find(node => node.type === 'aside').length, 0, 'the details start closed');
  await page.click(page.oneByClass('tw-switcher'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Team members & settings')[0]);
  const drawer = () => page.find(node => node.type === 'aside');
  assert.equal(drawer().length, 1, 'the team menu opens the details, on the tab they were last on');
  assert.deepEqual(page.find(node => node.props.role === 'tab').map(node => node.props['aria-selected']), [true, false, false]);
  await page.click(page.find(node => node.props.role === 'tab' && textOf(node) === 'Bots')[0]);
  await page.click(page.oneByLabel('Close details')); assert.equal(drawer().length, 0);
  await page.click(page.oneByClass('tw-switcher'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Team members & settings')[0]);
  assert.deepEqual(page.find(node => node.props.role === 'tab').map(node => node.props['aria-selected']), [false, false, true], 'reopened where it was left');
  await page.click(page.oneByLabel('Close details'));
  workspace.selectConversation('chan-general'); await settle(); await page.during(() => undefined);
  const toggle = () => page.find(node => node.type === 'button' && node.props['aria-label'] === 'Team details')[0]!;
  assert.equal(toggle().props['aria-pressed'], false);
  await page.click(toggle()); assert.equal(drawer().length, 1); assert.equal(toggle().props['aria-pressed'], true);
  await page.click(toggle()); assert.equal(drawer().length, 0);
});

scenario('layout: on a phone the sidebar and the conversation take turns, and the conversation has a way back', async context => {
  const { workspace, snapshot } = await open(context);
  const page = await mount(workspace, layout(workspace)); context.after(() => page.unmount());
  const body = () => page.oneByClass('teams-web-body');
  assert.equal(body().props['data-pane'], 'conversation', 'a member starts at the assistant home');
  assert.match(page.text(), /Team Bots/);
  await page.click(page.oneByClass('teams-web-back')); assert.equal(body().props['data-pane'], 'list');
  await page.click(page.find(node => node.props['data-row'] === 'chan-general')[0]);
  assert.equal(snapshot().selectedConversationId, 'chan-general'); assert.equal(body().props['data-pane'], 'conversation');
  await page.click(page.oneByLabel('Team details'));
  assert.equal(page.find(node => node.type === 'aside').length, 1);
  await page.click(page.oneByClass('teams-web-back')); assert.equal(body().props['data-pane'], 'list');
  assert.equal(page.find(node => node.type === 'aside').length, 0, 'the details do not cover the list');
  assert.equal(snapshot().selectedConversationId, 'chan-general', 'going back leaves the conversation open for the wide layout');
});

scenario('layout: an invitation from a link opens the join dialog once and joins nothing; no workspace yet is a loading note', async context => {
  const { fake, workspace } = await open(context);
  const code = 'D'.repeat(32);
  let handled = 0;
  const page = await mount(workspace, layout(workspace, { invite: code, onInviteHandled: () => { handled++; } })); context.after(() => page.unmount());
  assert.equal(handled, 1); assert.equal(fake.teamCommands.length, 0, 'opening a link never joins');
  assert.equal(page.find(node => node.type === 'input')[0].props.value, code, 'the code is filled in for the person to confirm');
  const waiting = await mount(workspace, () => createElement(TeamsWorkspaceLayout, { teams: { workspace: undefined, snapshot: undefined, signedIn: true }, locale: 'zh-Hans' } as never)); context.after(() => waiting.unmount());
  assert.match(waiting.text(), /正在加载/); assert.equal(waiting.byClass('teams-web-body').length, 0);
});

scenario('layout: an owner invites friends from the Invites tab; the Friends workspace lives only while the picker is open and the sender is told once afterwards (D26)', async context => {
  const { fake, workspace } = await open(context);
  const friends = new FakeFriends(); friends.cards = [card('bob'), card('carol'), card('dave')];
  const page = await mount(workspace, layout(workspace, { friends: friends.api })); context.after(() => page.unmount());
  await page.click(page.oneByClass('tw-switcher'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Team members & settings')[0]);
  await page.click(page.find(node => node.props.role === 'tab' && textOf(node) === 'Invites')[0]);
  assert.deepEqual(friends.open, { cards: 0, blocks: 0, offers: 0 }, 'the Teams page runs no Friends listener');
  await page.click(page.button('Invite friends'));
  await until(() => friends.open.cards === 1 && friends.open.offers === 1, 'the picker\'s Friends workspace'); await page.during(() => undefined);
  const dialog = () => page.find(node => node.type === 'dialog')[0]!;
  assert.match(textOf(dialog()), /Invite friends to “Acme”/);
  const names = () => dialog().findAll((node: any) => node.type === 'b').map(textOf);
  assert.deepEqual(names(), ['Carol', 'Dave'], 'Bob is a member of the team the page holds');

  const listCalls = fake.listCalls;
  friends.fail('offerTeam', 'ALREADY_MEMBER');
  await page.click(dialog().findAll((node: any) => node.type === 'button' && textOf(node) === 'Invite')[0]);
  assert.deepEqual(names(), ['Dave'], 'Carol turned out to be a member'); await until(() => fake.listCalls > listCalls, 'the member list to be read again');
  friends.replies.delete('offerTeam');
  await page.click(dialog().findAll((node: any) => node.type === 'button' && textOf(node) === 'Invite')[0]);
  assert.deepEqual(friends.executed('offerTeam').map(command => [(command as { teamId: string }).teamId, (command as { friendUid: string }).friendUid]), [['team-aaaa', 'carol'], ['team-aaaa', 'dave']]);
  await page.click(page.button('Close'));
  assert.equal(page.find(node => node.type === 'dialog').length, 0); assert.deepEqual(friends.open, { cards: 0, blocks: 0, offers: 0 }, 'closing the picker closes the Friends workspace');
  assert.match(page.text(), /Invitation sent to Dave/); assert.doesNotMatch(page.text(), /Invitation sent to Carol/, 'only who was invited');
});

scenario('layout: without a Friends API the Invites tab has no Invite friends button', async context => {
  const { workspace } = await open(context);
  const page = await mount(workspace, layout(workspace)); context.after(() => page.unmount());
  await page.click(page.oneByClass('tw-switcher'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Team members & settings')[0]);
  await page.click(page.find(node => node.props.role === 'tab' && textOf(node) === 'Invites')[0]);
  assert.equal(page.find(node => node.type === 'button' && textOf(node) === 'Invite friends').length, 0); assert.match(page.text(), /Create invite link/);
});

scenario('useTeamsWorkspace opens a workspace for an account and releases it when the account, the API or the page goes', async context => {
  withDom(context);
  const { create, act } = reactTestRenderer();
  const fake = new FakeTeams();
  fake.teams = [team('team-aaaa', { name: 'Acme' })];
  fake.conversations.set('team-aaaa', [conversation('chan-general', { title: 'general' })]);
  const seen: Array<{ workspace?: TeamsWorkspace; snapshot?: TeamsSnapshot; signedIn: boolean }> = [];
  const options = { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 };
  function Probe({ api, account }: { api?: TeamsApi; account?: string }) { seen.push(useTeamsWorkspace(api, account, options)); return null; }
  let tree: any;
  await act(async () => { tree = create(createElement(Probe, {})); await settle(); });
  assert.deepEqual([seen.at(-1)!.workspace, seen.at(-1)!.snapshot, seen.at(-1)!.signedIn], [undefined, undefined, false], 'no account, no workspace');
  await act(async () => { tree.update(createElement(Probe, { api: fake.api, account: uid })); await settle(); });
  await until(() => fake.openWatches > 0 && !!seen.at(-1)!.snapshot, 'the workspace');
  await act(async () => { await settle(); });
  const opened = seen.at(-1)!;
  assert.equal(opened.signedIn, true); assert.equal(opened.workspace!.uid, uid); assert.equal(opened.snapshot!.teams.length, 1);
  await until(() => seen.at(-1)!.snapshot!.conversations.length === 1, 'its conversations'); await act(async () => { await settle(); });
  // Another account gets its own workspace; the first is closed.
  await act(async () => { tree.update(createElement(Probe, { api: fake.api, account: 'carol' })); await settle(); });
  await act(async () => { await settle(); });
  assert.equal(seen.at(-1)!.workspace!.uid, 'carol'); assert.notEqual(seen.at(-1)!.workspace, opened.workspace);
  await act(async () => { tree.update(createElement(Probe, {})); await settle(); });
  assert.equal(seen.at(-1)!.workspace, undefined, 'signing out leaves nothing open');
  await act(async () => { tree.unmount(); await settle(); });
  await until(() => fake.openWatches === 0 && fake.changed.size === 0 && fake.signals.size === 0, 'every listener to be released');
});

// ── The words come from the shared catalog, in the five languages every client speaks (U13) ──

const catalog = (locale: string) => JSON.parse(readFileSync(new URL(`../../../resources/translations/${locale}.json`, import.meta.url), 'utf8')) as Record<string, string>;
/** What a person on each language reads in the layout and on the sign-in card: literal words, so a catalog and a view that share one mistake still fail here. */
const OWN_WORDS = {
  'zh-Hant': { home: '團隊 Bot', back: '‹ 會話列表', direct: '私訊', botChats: 'Bot 對話', menu: '團隊成員與設定', loading: '正在載入…', signIn: '登入靈犀團隊', google: '透過 Google 登入', email: '電子郵件' },
  ja: { home: 'チームの Bot', back: '‹ 会話一覧', direct: 'ダイレクトメッセージ', botChats: 'Bot とのチャット', menu: 'チームのメンバーと設定', loading: '読み込み中…', signIn: 'LingXi チームにサインイン', google: 'Google で続行', email: 'メールアドレス' },
  ko: { home: '팀 Bot', back: '‹ 대화 목록', direct: '개인 메시지', botChats: 'Bot 대화', menu: '팀 멤버 및 설정', loading: '불러오는 중…', signIn: 'LingXi 팀에 로그인', google: 'Google로 계속하기', email: '이메일' },
} as const;

for (const [locale, words] of Object.entries(OWN_WORDS)) {
  scenario(`layout: ${locale} reads its own language, not Simplified Chinese or English`, async context => {
    const { workspace } = await open(context);
    const page = await mount(workspace, layout(workspace, { locale }));
    const text = page.text();
    for (const word of [words.home, words.back, words.direct, words.botChats]) assert.ok(text.includes(word), `${locale} shows "${word}"`);
    assert.ok(page.has(words.back), 'the way back is a button in the language');
    const own = catalog(locale);
    for (const other of ['en', 'zh-Hans']) for (const key of ['teams_bots_title', 'teams_group_direct', 'teams_group_bot_chats', 'tw_back_conversations']) {
      if (catalog(other)[key] !== own[key]) assert.ok(!text.includes(catalog(other)[key]!), `${locale} still shows the ${other} words "${catalog(other)[key]}"`);
    }
    await page.click(page.oneByClass('tw-switcher'));
    assert.equal(page.find(node => node.props.role === 'menuitem' && textOf(node) === words.menu).length, 1, 'the team menu');
    const waiting = await mount(workspace, () => createElement(TeamsWorkspaceLayout, { teams: { workspace: undefined, snapshot: undefined, signedIn: true }, locale } as never));
    assert.equal(waiting.text(), words.loading, 'no workspace yet is a loading note in the language');
    // Unmounted here, while the fake document still exists: its cleanup needs one.
    await waiting.unmount(); await page.unmount();
  });

  scenario(`sign-in card: ${locale} reads its own language`, async context => {
    withDom(context);
    const { create, act } = reactTestRenderer(); let tree: any;
    await act(async () => { tree = create(createElement(SignInCard, { auth: {} as never, locale, note: 'note' }), { createNodeMock: hostNode }); await settle(); });
    context.after(() => act(async () => tree.unmount()));
    const text = textOf({ children: [].concat(tree.toJSON()) });
    for (const word of [words.signIn, words.google, words.email]) assert.ok(text.includes(word), `${locale} shows "${word}"`);
    const english = catalog('en');
    for (const key of ['tw_signin_title', 'account_google', 'account_email', 'account_password']) assert.ok(!text.includes(english[key]!), `${locale} still shows "${english[key]}"`);
  });
}

scenario('layout: the conversation header shares its link; a copy says so, and a failed copy says that instead', async context => {
  const { workspace } = await open(context);
  const saved = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  const copied: string[] = [];
  let fail = false;
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { clipboard: { writeText: async (text: string) => { if (fail) throw new Error('denied'); copied.push(text); } } } });
  context.after(() => { if (saved) Object.defineProperty(globalThis, 'navigator', saved); });
  const page = await mount(workspace, layout(workspace)); context.after(() => page.unmount());
  workspace.selectConversation('chan-general'); await settle(); await page.during(() => undefined);
  await page.click(page.oneByLabel('Share link'));
  assert.deepEqual(copied, ['https://lingxios.net/open/team/chat?conversation=chan-general&team=team-aaaa']);
  assert.equal(textOf(page.oneByClass('tw-link-share-status')), 'Link copied');
  fail = true;
  await page.click(page.oneByLabel('Share link'));
  assert.equal(textOf(page.oneByClass('tw-link-share-status')), 'Couldn’t copy the link');
});
