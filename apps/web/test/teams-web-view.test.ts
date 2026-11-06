import assert from 'node:assert/strict';
import { type TestContext } from 'node:test';
import { createElement } from 'react';
import type { TeamsApi } from '../../../packages/teams';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { FakeTeams, botCard, conversation, memoryStorage, message, team, until, uid } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { mount, reactTestRenderer, scenario, settle, textOf, withDom } from './support/workspacePage';

// The Web page's own part of the Teams page: the workspace opened for an account, and the layout of the three
// views. The views themselves are covered by teams-workspace-interaction.test.ts and teams-bots-ui.test.ts.
const { TeamsWorkspaceLayout } = loadTeamsModule<typeof import('../src/teams/TeamsWorkspaceView')>(new URL('../src/teams/TeamsWorkspaceView', import.meta.url).pathname);
const { useTeamsWorkspace } = loadTeamsModule<typeof import('../../../packages/teams/useTeamsWorkspace')>('useTeamsWorkspace');

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
  createElement(TeamsWorkspaceLayout, { teams: { workspace, snapshot, signedIn: true }, zh: false, ...more } as never);

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
  assert.match(page.text(), /Team assistants/);
  await page.click(page.oneByClass('teams-web-back')); assert.equal(body().props['data-pane'], 'list');
  await page.click(page.find(node => node.props['data-row'] === 'chan-general')[0]);
  assert.equal(snapshot().selectedConversationId, 'chan-general'); assert.equal(body().props['data-pane'], 'conversation');
  await page.click(page.oneByClass('teams-web-back')); assert.equal(body().props['data-pane'], 'list');
  assert.equal(snapshot().selectedConversationId, 'chan-general', 'going back leaves the conversation open for the wide layout');
});

scenario('layout: an invitation from a link opens the join dialog once and joins nothing; no workspace yet is a loading note', async context => {
  const { fake, workspace } = await open(context);
  const code = 'D'.repeat(32);
  let handled = 0;
  const page = await mount(workspace, layout(workspace, { invite: code, onInviteHandled: () => { handled++; } })); context.after(() => page.unmount());
  assert.equal(handled, 1); assert.equal(fake.teamCommands.length, 0, 'opening a link never joins');
  assert.equal(page.find(node => node.type === 'input')[0].props.value, code, 'the code is filled in for the person to confirm');
  const waiting = await mount(workspace, () => createElement(TeamsWorkspaceLayout, { teams: { workspace: undefined, snapshot: undefined, signedIn: true }, zh: true } as never)); context.after(() => waiting.unmount());
  assert.match(waiting.text(), /正在加载/); assert.equal(waiting.byClass('teams-web-body').length, 0);
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
