import assert from 'node:assert/strict';
import test from 'node:test';
import { FRIEND_CHAT } from '../../../packages/teams/friends';
import type { MessageMutation } from '../../../packages/teams/messages';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { pairTeamId } from './support/fakeFriends';
import { FakeTeams, conversation, memoryStorage, members, message, team, uid, until } from './support/fakeTeams';

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

test('a pair workspace is ready at once, opens its one conversation, and never lists, loads detail, or fetches Bot cards', async context => {
  const { fake, workspace, snapshot } = pair(); context.after(() => workspace.close());
  assert.equal(snapshot().loading, false, 'the team and its two people are given, not fetched');
  assert.equal(snapshot().selectedTeamId, id); assert.equal(snapshot().team?.name, 'Bob'); assert.deepEqual(snapshot().directory.map(value => value.uid), ['alice', 'bob']);
  await until(() => snapshot().conversation?.id === FRIEND_CHAT && snapshot().messages.length === 2, 'the conversation');
  assert.equal(snapshot().selectedConversationId, FRIEND_CHAT, 'opened without anyone choosing it');
  await workspace.refresh();
  assert.equal(fake.listCalls, 0); assert.deepEqual(fake.detailCalls, []);
  assert.equal(fake.changed.size, 0, 'nothing follows the membership of a team that is not listed');
  assert.deepEqual(fake.commands.filter(command => !['read', 'mutate', 'create'].includes(command.action)), [], 'no Bot cards, no tasks');
  assert.deepEqual(snapshot().bots, []);
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
