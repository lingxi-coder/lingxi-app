import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test, { after, type TestContext } from 'node:test';
import { createElement, Fragment } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { AGENT_AVATARS, agentAvatarIndex, teamBotAvatarId } from '../src/remote/reuse';
import { renderTeamAvatar } from '../src/teams/teamAvatar';
import { FakeFriends, card, pairTeamId } from './support/fakeFriends';
import { FakeTeams, botCard, conversation, memoryStorage, message, team, until, uid } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, mount, scenario, textOf, withDom } from './support/workspacePage';

// A Bot wears on the Web the avatar it has on the Desktop (`TeamAvatar.tsx` there): the same art picked from the same id by
// the same rule, in the box of the shared Teams views, rounded to 24% of its side. People keep the workspace's initials.
// The fake browser stays for the whole file, so a view that is unmounted after its test still finds its document.
withDom({ after } as unknown as TestContext);
const { TeamsWorkspaceLayout } = loadTeamsModule<typeof import('../src/teams/TeamsWorkspaceView')>(new URL('../src/teams/TeamsWorkspaceView', import.meta.url).pathname);
const { FriendsWorkspaceView } = loadTeamsModule<typeof import('../src/friends/FriendsWorkspaceView')>(new URL('../src/friends/FriendsWorkspaceView', import.meta.url).pathname);

const fixtures = new URL('../../../packages/bridge-client/fixtures/', import.meta.url);
const readFixture = <T>(name: string): T => JSON.parse(readFileSync(new URL(name, fixtures), 'utf8')) as T;
const draw = (identity: Parameters<typeof renderTeamAvatar>[0], size = 28) => renderToStaticMarkup(createElement(Fragment, null, renderTeamAvatar(identity, size)));
const bot = (id: string) => ({ kind: 'bot' as const, id, name: 'Helper' });

test('people are drawn by the workspace, as initials on a circle: nothing is rendered for them', () => {
  assert.equal(renderTeamAvatar({ kind: 'user', id: 'bob', name: 'Bob' }, 28), undefined);
  assert.equal(renderTeamAvatar({ kind: 'user', id: 'bot:owner:host:planner', name: 'Not a Bot' }, 28), undefined, 'by what it is, not by the shape of its id');
});

test('a Bot is its avatar art: both variants of it, picked from its id, in the size asked for, as pictures that say nothing to a screen reader', () => {
  const html = draw(bot('planner'), 32);
  const index = agentAvatarIndex('planner');
  const art = AGENT_AVATARS[index];
  const images = [...html.matchAll(/<img\b[^>]*>/g)].map(match => match[0]);
  assert.equal(images.length, 2);
  assert.match(images[0]!, new RegExp(`^<img class="tw-bot-art" data-art="light" data-agent-avatar="${index}" src="${art.light.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}" width="32" height="32" alt="" draggable="false"/?>$`));
  assert.match(images[1]!, new RegExp(`data-art="dark" data-agent-avatar="${index}" src="${art.dark.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}" width="32" height="32"`));
  assert.notEqual(art.light, art.dark);
  assert.ok(images.every(image => image.includes('alt=""')), 'the name is beside the avatar already');
});

test('a Bot has the same avatar whether the workspace names it by its id or by its key, and that avatar is the one the Desktop draws', () => {
  for (const id of ['planner', 'assistant-fixture', '3f2b8c10-1c0e-4c7f-9d2e-5a1b6e7f8a90']) {
    const expected = agentAvatarIndex(id);
    assert.equal(draw(bot(id)).match(/data-agent-avatar="(\d+)"/)?.[1], String(expected), `${id} by id`);
    assert.equal(draw(bot(`bot:owner-uid:host-1:${id}`)).match(/data-agent-avatar="(\d+)"/)?.[1], String(expected), `${id} by key`);
  }
  assert.equal(AGENT_AVATARS.length, 28, 'the Desktop\'s 28');
});

test('the id a Bot is looked up by follows the vectors of the phones, and the index follows the ones of every client', () => {
  const vectors = readFixture<{ avatars: { teamBotAvatarId: [string, string][]; indexPrefixed: { agentId: string; repeat?: { char: string; count: number }; expect: number }[] } }>('mobile-team-bot-vectors.json');
  assert.ok(vectors.avatars.teamBotAvatarId.length >= 3);
  for (const [input, expected] of vectors.avatars.teamBotAvatarId) assert.equal(teamBotAvatarId(input), expected, input);
  assert.ok(vectors.avatars.indexPrefixed.length >= 8);
  for (const row of vectors.avatars.indexPrefixed) assert.equal(agentAvatarIndex(row.repeat ? row.repeat.char.repeat(row.repeat.count) : row.agentId), row.expect, row.agentId);
  const identities = readFixture<{ id: string; index: number }[]>('agent-avatar-identities.json');
  assert.ok(identities.length >= 30);
  for (const row of identities) assert.equal(agentAvatarIndex(row.id), row.index, JSON.stringify(row.id));
  // The `agent:` prefix is part of the rule: a bare id and the prefixed one are the same agent.
  assert.equal(agentAvatarIndex('agent:planner'), agentAvatarIndex('planner'));
});

test('every one of the 28 avatars has both of its variants as self-contained SVG files', () => {
  assert.equal(new Set(AGENT_AVATARS.flatMap(art => [art.light, art.dark])).size, 56);
  for (const art of AGENT_AVATARS) {
    for (const src of [art.light, art.dark]) {
      const svg = readFileSync(new URL(src), 'utf8');
      assert.match(svg, /<svg\b/);
      assert.doesNotMatch(svg, /<script|<foreignObject|(?:href|src)=["']https?:/i, src);
    }
  }
});

const webCss = readFileSync(new URL('../src/teams/teams-web.css', import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const workspaceCss = readFileSync(new URL('../../../packages/teams/workspace.css', import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const declaration = (css: string, selector: string, property: string): string | undefined => {
  const rule = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].find(match => match[1]!.split(',').map(part => part.trim()).includes(selector));
  return rule && new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*([^;]+)`).exec(rule[2]!)?.[1]?.trim();
};
/** The weight of a selector made of classes, attributes and pseudo-classes: how many of each. */
const specificity = (selector: string): number => (selector.match(/\.[\w-]+|\[[^\]]+\]|:(?!:)[\w-]+/g)?.length ?? 0);

test('a Bot\'s box is rounded to 24% of its side and people keep the circle; the dark and the light art take turns with the theme', () => {
  assert.equal(declaration(webCss, ".teams-web .tw-avatar[data-kind='bot']", 'border-radius'), '24%');
  assert.equal(declaration(workspaceCss, ".tw-avatar[data-kind='bot']", 'border-radius'), '8px', 'the workspace\'s own rule is a fixed size');
  assert.ok(specificity(".teams-web .tw-avatar[data-kind='bot']") > specificity(".tw-avatar[data-kind='bot']"), 'and the Web\'s is the one that applies');
  assert.equal(declaration(workspaceCss, ".tw-avatar[data-kind='user']", 'border-radius'), '50%');
  assert.equal(declaration(webCss, ".teams-web .tw-avatar[data-kind='user']", 'border-radius'), undefined, 'people are left to it');
  // Light art by default, dark art under the site\'s dark theme, never both.
  assert.equal(declaration(webCss, ".tw-bot-art[data-art='dark']", 'display'), 'none');
  assert.equal(declaration(webCss, ":root[data-theme='dark'] .tw-bot-art[data-art='dark']", 'display'), 'block');
  assert.equal(declaration(webCss, ":root[data-theme='dark'] .tw-bot-art[data-art='light']", 'display'), 'none');
  assert.equal(declaration(webCss, '.tw-bot-art', 'width'), '100%');
  assert.equal(declaration(webCss, '.tw-bot-art', 'height'), '100%');
});

// --- in the pages ----------------------------------------------------------------------------------------------

const at = (minute: number) => new Date(2026, 10, 2, 11, minute).toISOString();
const helper = { kind: 'shared_bot' as const, sharedBotId: 'assistant-fixture', displayName: 'Fixture Bot' };
/** The avatar boxes of a page: the Bots' (with the art in them) and the people's (initials), by what each was drawn as. */
const avatars = (page: { find(match: (node: any) => boolean): any[] }) => {
  const boxes = page.find(node => classes(node).includes('tw-avatar'));
  return {
    bots: boxes.filter(node => node.props['data-kind'] === 'bot'),
    people: boxes.filter(node => node.props['data-kind'] === 'user'),
    art: page.find(node => classes(node).includes('tw-bot-art')),
  };
};

scenario('Teams page: a Bot DM and the Bot\'s messages wear the Bot\'s avatar, and people are initials on circles', async context => {
  const fake = new FakeTeams();
  fake.teams = [team('team-aaaa', { name: 'Acme', role: 'owner' })];
  fake.bots = [botCard()];
  fake.conversations.set('team-aaaa', [
    conversation('chan-general', { title: 'general', lastSeq: 1, lastReadSeq: 1, lastMessage: { id: 'message-1', seq: 1, at: at(1), preview: 'One', author: { kind: 'user', uid: 'bob' } } }),
    conversation('dm-bob', { kind: 'dm', title: '', participants: ['user:alice', 'user:bob'], lastSeq: 1, lastReadSeq: 1, lastMessage: { id: 'message-1', seq: 1, at: at(2), preview: 'Hi', author: { kind: 'user', uid: 'bob' } } }),
    conversation('bot-chat', { kind: 'bot_dm', title: 'Fixture Bot', participants: ['user:alice', 'shared_bot:assistant-fixture'], lastSeq: 2, lastReadSeq: 2, lastMessage: { id: 'message-2', seq: 2, at: at(3), preview: 'Hello Alice', author: helper } }),
  ]);
  fake.threads.set('bot-chat', [message(1, { text: 'Hi there', createdAt: at(2), author: { kind: 'user', uid: 'alice' } }), message(2, { text: 'Hello Alice', createdAt: at(3), author: helper })]);
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => workspace.close());
  const snapshot = (): TeamsSnapshot => workspace.getSnapshot();
  await until(() => snapshot().conversations.length === 3 && snapshot().botsLoaded, 'the team to load');
  const page = await mount(workspace, current => createElement(TeamsWorkspaceLayout, { teams: { workspace, snapshot: current, signedIn: true }, zh: false } as never)); context.after(() => page.unmount());
  const index = String(agentAvatarIndex('assistant-fixture'));

  // The sidebar: the Bot DM leads with the Bot, the direct message with the person.
  let drawn = avatars(page);
  assert.ok(drawn.bots.length >= 1, 'the Bot DM row');
  assert.ok(drawn.people.length >= 1, 'the direct message row');
  assert.ok(drawn.bots.every(node => classes(node).includes('tw-avatar-custom')), 'a Bot\'s box holds its art');
  assert.ok(drawn.people.every(node => !classes(node).includes('tw-avatar-custom')), 'a person\'s holds their initials');
  assert.ok(drawn.people.some(node => textOf(node) === 'B'), 'Bob: the first letter of the name');
  assert.deepEqual([...new Set(drawn.art.map(node => node.props['data-agent-avatar']))].map(String), [index], 'the art of the Bot\'s id');
  assert.equal(drawn.art.length, drawn.bots.length * 2, 'both variants of it, in every Bot\'s box');
  assert.deepEqual([...new Set(drawn.art.map(node => node.props['data-art']))].sort(), ['dark', 'light']);
  const sidebarBots = drawn.bots.length;

  // Opening the Bot DM draws the Bot's message with the same avatar, and the person's own never has art.
  workspace.selectConversation('bot-chat');
  await until(() => snapshot().messages.length === 2, 'the thread'); await page.during(() => undefined);
  drawn = avatars(page);
  assert.ok(drawn.bots.length > sidebarBots, 'the Bot\'s message has its avatar beside it');
  assert.deepEqual([...new Set(drawn.art.map(node => node.props['data-agent-avatar']))].map(String), [index], 'the same art everywhere');
  assert.equal(drawn.art.length, drawn.bots.length * 2);
  assert.ok(drawn.art.every(node => node.props.src === AGENT_AVATARS[Number(index)][node.props['data-art'] as 'light' | 'dark']));
  assert.ok(drawn.art.every(node => node.props.alt === '' && node.props.draggable === false));
});

scenario('Friends page: a Bot in a friend\'s chat wears the Bot\'s avatar, the friend and their requests stay initials', async context => {
  const friends = new FakeFriends(), teams = new FakeTeams();
  friends.cards = [card('bob', { handle: 'bobby', lastSeq: 1, lastReadSeq: 1 }), card('erin', { state: 'incoming', note: 'Hi, it is Erin' })];
  teams.bots = [botCard({ name: 'Bob Helper', maintainer: { uid: 'bob', generation: 1 } })];
  teams.conversations.set(pairTeamId('bob'), [
    conversation('friend_chat', { kind: 'dm', title: '', participants: ['user:alice', 'user:bob'], lastSeq: 1, lastReadSeq: 1 }),
    conversation('bot-chat', { kind: 'bot_dm', title: 'Bob Helper', participants: ['user:alice', 'shared_bot:assistant-fixture'], lastSeq: 1, lastReadSeq: 1 }),
  ]);
  teams.threads.set('friend_chat', [message(1, { text: 'Hi Alice' })]);
  teams.threads.set('bot-chat', [message(1, { text: 'Hello from the Bot', author: { ...helper, displayName: 'Bob Helper' } })]);
  const page = await mount({ subscribe: () => () => undefined, getSnapshot: () => 0 }, () => createElement(FriendsWorkspaceView, {
    friends: friends.api, teams: teams.api, uid, zh: false, route: '/friends', chat: 'bob', code: '', onCodeHandled() {}, onBadge() {},
  } as never));
  context.after(() => page.unmount());
  await until(() => page.byLabel('Shared Bots').length > 0, 'the friend chat'); await page.during(() => undefined);

  let drawn = avatars(page);
  assert.equal(drawn.bots.length, 0, 'no Bot in sight yet: the friend, the request and the person who wrote are people');
  assert.ok(drawn.people.length >= 2);
  assert.ok(drawn.people.every(node => !classes(node).includes('tw-avatar-custom')));

  // Message assistant opens the Bot DM that is there, in the friend chat's own Teams view.
  await page.click(page.oneByLabel('Shared Bots'));
  await page.click(page.button('Message assistant'));
  await until(() => page.text().includes('Hello from the Bot'), 'the Bot DM'); await page.during(() => undefined);
  drawn = avatars(page);
  assert.ok(drawn.bots.length >= 1, 'the Bot\'s message has its avatar');
  assert.deepEqual([...new Set(drawn.art.map(node => node.props['data-agent-avatar']))].map(String), [String(agentAvatarIndex('assistant-fixture'))], 'the avatar of the Bot\'s id, as in a Team');
  assert.equal(drawn.art.length, drawn.bots.length * 2);
  assert.ok(drawn.bots.every(node => classes(node).includes('tw-avatar-custom')));
});
