import assert from 'node:assert/strict';
import test from 'node:test';
import { FRIEND_CHAT } from '../../../packages/teams/friends';
import type { MessageMutation } from '../../../packages/teams/messages';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { pairTeamId } from './support/fakeFriends';
import { FakeTeams, botCard, conversation, memoryStorage, members, message, team, uid, until } from './support/fakeTeams';

// A friend chat is the Teams workspace pinned to the pair team (D9): same messages, none of the Team around it.
const id = pairTeamId('bob');
function pair(setup: (fake: FakeTeams) => void = () => {}) {
  const fake = new FakeTeams();
  fake.conversations.set(id, [conversation(FRIEND_CHAT, { kind: 'dm', title: '', participants: ['user:alice', 'user:bob'], lastSeq: 2, lastReadSeq: 0 })]);
  fake.threads.set(FRIEND_CHAT, [message(1), message(2)]);
  setup(fake);
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5,
    pair: { team: team(id, { name: 'Bob', memberCount: 2 }), members: members('alice', 'bob'), conversationId: FRIEND_CHAT } });
  return { fake, workspace, snapshot: (): TeamsSnapshot => workspace.getSnapshot() };
}

test('a pair workspace is ready at once, opens its one conversation, never lists or loads detail, and fetches the friendship\'s Bot cards (D21)', async context => {
  const { fake, workspace, snapshot } = pair(); context.after(() => workspace.close());
  assert.equal(snapshot().loading, false, 'the team and its two people are given, not fetched');
  assert.equal(snapshot().selectedTeamId, id); assert.equal(snapshot().team?.name, 'Bob'); assert.deepEqual(snapshot().directory.map(value => value.uid), ['alice', 'bob']);
  await until(() => snapshot().conversation?.id === FRIEND_CHAT && snapshot().messages.length === 2, 'the conversation');
  assert.equal(snapshot().selectedConversationId, FRIEND_CHAT, 'opened without anyone choosing it');
  await workspace.refresh();
  assert.equal(fake.listCalls, 0); assert.deepEqual(fake.detailCalls, []);
  assert.equal(fake.changed.size, 0, 'nothing follows the membership of a team that is not listed');
  const asked = fake.commands.filter(command => !['read', 'mutate', 'create'].includes(command.action));
  assert.deepEqual(asked.map(command => command.action), ['listSharedBots'], 'the Bot cards, and no tasks');
  assert.deepEqual([asked[0]!.teamId, asked[0]!.generation], [id, 1], 'of the pair team');
  assert.deepEqual(snapshot().bots, []); assert.equal(snapshot().botsLoaded, true); assert.equal(snapshot().botsError, '');
  assert.ok(fake.watches.every(watch => watch.scope.teamId === id && watch.scope.generation === 1));
});

test('messages are sent and read in the pair team\'s conversation', async context => {
  const { fake, workspace, snapshot } = pair(); context.after(() => workspace.close());
  await until(() => snapshot().messages.length === 2, 'the thread');
  await snapshot().messaging!.queue('create', 'See you');
  await until(() => fake.commands.some(command => command.action === 'mutate'), 'the send');
  const sent = fake.commands.find((command): command is MessageMutation => command.action === 'mutate')!;
  assert.deepEqual([sent.teamId, sent.generation, sent.conversationId, sent.operation, sent.text], [id, 1, FRIEND_CHAT, 'create', 'See you']);
  assert.equal(await workspace.markRead(), true);
  const read = fake.commands.find(command => command.action === 'read') as { teamId: string; conversationId: string; lastReadSeq: number };
  assert.deepEqual([read.teamId, read.conversationId, read.lastReadSeq], [id, FRIEND_CHAT, 2]);
});

test('the two people can be renamed; a Teams workspace has no such directory to replace', async context => {
  const { workspace, snapshot } = pair(); context.after(() => workspace.close());
  let notified = 0;
  const stop = workspace.subscribe(() => { notified++; }); context.after(stop);
  workspace.setPairMembers([{ uid, displayName: 'Alice A', role: 'member', generation: 1 }, { uid: 'bob', displayName: 'Robert', role: 'member', generation: 1 }]);
  assert.deepEqual(snapshot().directory.map(value => value.displayName), ['Alice A', 'Robert']); assert.ok(notified > 0);

  const fake = new FakeTeams(); fake.teams = [team('team-aaaa')];
  const teams = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 }); context.after(() => teams.close());
  await until(() => teams.getSnapshot().directory.length === 3, 'the team');
  teams.setPairMembers([{ uid, displayName: 'Nobody', role: 'member', generation: 1 }]);
  assert.equal(teams.getSnapshot().directory.length, 3);
});

test('a directory the server confirmed without the conversation leaves no conversation, which is how the chat learns it ended', async context => {
  const { fake, workspace, snapshot } = pair(); context.after(() => workspace.close());
  await until(() => snapshot().conversation?.id === FRIEND_CHAT, 'the conversation');
  fake.conversations.set(id, []);
  for (const watch of fake.open(id)) fake.deliver(watch);
  await until(() => snapshot().conversation === undefined, 'the conversation to go');
  assert.equal(snapshot().messagingConnecting, false); assert.equal(snapshot().offline, false);
});

test('closing releases the pair team\'s listeners', async () => {
  const { fake, workspace, snapshot } = pair();
  await until(() => snapshot().messages.length === 2 && fake.openWatches > 0, 'the thread');
  workspace.close();
  assert.equal(fake.openWatches, 0);
});

// ── The friendship's Bots (D21, D22, D25) ──

test('the Bot cards are the pair team\'s: the friend\'s and the viewer\'s own, with the failure code kept for the chat to read', async context => {
  const friendsBot = botCard({ maintainer: { uid: 'bob', generation: 1 } });
  const { fake, workspace, snapshot } = pair(f => { f.bots = [friendsBot, botCard({ sharedBotId: 'mine', maintainer: { uid: 'alice', generation: 1 }, state: 'paused' })]; });
  context.after(() => workspace.close());
  await until(() => snapshot().botsLoaded, 'the Bot cards');
  assert.deepEqual(snapshot().bots.map(bot => bot.sharedBotId), ['assistant-fixture', 'mine']); assert.equal(snapshot().botsUnverified, false); assert.equal(snapshot().botsError, '');

  fake.messages = async () => { throw new Error('MEMBERSHIP_REQUIRED'); };
  await workspace.refreshBots();
  assert.equal(snapshot().botsUnverified, true); assert.equal(snapshot().botsError, 'MEMBERSHIP_REQUIRED', 'the code the chat reads an ended friendship from');
  assert.deepEqual(snapshot().bots.map(bot => bot.sharedBotId), [], 'what could not be verified is not shown');
});

test('Message opens the viewer\'s Bot DM on the pair team: a new one as createConversation bot_dm, the existing one when there is one, and Back selects the friend chat', async context => {
  const card = botCard({ maintainer: { uid: 'bob', generation: 1 } });
  const { fake, workspace, snapshot } = pair(f => { f.bots = [card]; });
  context.after(() => workspace.close());
  await until(() => snapshot().botsLoaded && snapshot().conversation?.id === FRIEND_CHAT, 'the chat and its Bots');
  assert.equal(await workspace.openBotDm(card), true);
  await until(() => fake.commands.some(command => command.action === 'createConversation'), 'the creation');
  const created = fake.commands.find(command => command.action === 'createConversation')!;
  assert.ok(created.action === 'createConversation');
  assert.deepEqual([created.teamId, created.generation], [id, 1]);
  assert.deepEqual({ ...created.payload, conversationId: '' }, { conversationId: '', kind: 'bot_dm', title: card.name, participantUids: [uid], sharedBotIds: [card.sharedBotId] });
  assert.equal(snapshot().selectedConversationId, created.payload.conversationId, 'the new Bot DM is shown while it is confirmed');

  // The server confirms it: the directory has the Bot DM beside the friend chat.
  fake.conversations.set(id, [...fake.conversations.get(id)!, conversation(created.payload.conversationId, { kind: 'bot_dm', title: card.name, participants: ['user:alice', `shared_bot:${card.sharedBotId}`] })]);
  for (const watch of fake.open(id)) fake.deliver(watch);
  await until(() => snapshot().conversation?.id === created.payload.conversationId, 'the Bot DM');
  workspace.selectConversation(FRIEND_CHAT);
  await until(() => snapshot().conversation?.id === FRIEND_CHAT, 'the way back');
  assert.equal(await workspace.openBotDm(card), true);
  assert.equal(snapshot().selectedConversationId, created.payload.conversationId, 'the same Bot DM again');
  assert.equal(fake.commands.filter(command => command.action === 'createConversation').length, 1, 'and no second one');

  assert.equal(await workspace.openBotDm({ ...card, permitted: false }), false); assert.equal(snapshot().actionError, 'BOT_USE_REQUIRED');
  await assert.rejects(workspace.startBotDm({ ...card, state: 'paused' }), /BOT_USE_REQUIRED/);
});
