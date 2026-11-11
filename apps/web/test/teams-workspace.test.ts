import assert from 'node:assert/strict';
import test from 'node:test';
import {
  TeamsWorkspace, compareConversations, conversationLabel, conversationParticipants, conversationPreview, markdownPlainText, pointerPreview, defaultConversationTitle, findBotDm, findDm, groupConversations, unreadCount,
  type TeamsSnapshot,
} from '../../../packages/teams/workspace';
import type { TeamConversation } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, members, message, settle, team, uid, until } from './support/fakeTeams';

// ── Pure helpers ──

const at = (minute: number) => `2026-11-02T10:${String(minute).padStart(2, '0')}:00.000Z`;
const last = (minute: number, more: Partial<NonNullable<TeamConversation['lastMessage']>> = {}) => ({ id: `m-${minute}`, seq: minute, at: at(minute), preview: 'Hi', author: { kind: 'user' as const, uid: 'bob' }, ...more });

test('unreadCount is max(0, lastSeq - lastReadSeq)', () => {
  assert.equal(unreadCount({ lastSeq: 7, lastReadSeq: 4 }), 3);
  assert.equal(unreadCount({ lastSeq: 4, lastReadSeq: 4 }), 0);
  assert.equal(unreadCount({ lastSeq: 2, lastReadSeq: 9 }), 0);
});

test('groupConversations: kinds map to sections and each section is newest first', () => {
  const rows = [
    conversation('chan-old', { kind: 'channel', lastMessage: last(1) }), conversation('chan-new', { kind: 'channel', lastMessage: last(9) }),
    conversation('dm', { kind: 'dm', lastMessage: last(5) }), conversation('group', { kind: 'group', lastMessage: last(7) }),
    conversation('bot', { kind: 'bot_dm', lastMessage: last(3) }),
  ];
  const groups = groupConversations(rows, uid);
  assert.deepEqual(groups.channels.map(value => value.id), ['chan-new', 'chan-old']);
  assert.deepEqual(groups.direct.map(value => value.id), ['group', 'dm']);
  assert.deepEqual(groups.botDirect.map(value => value.id), ['bot']);
});

test('groupConversations: rows without a time follow, by lastSeq then title', () => {
  const rows = [
    conversation('c', { title: 'Zulu', lastSeq: 4 }), conversation('a', { title: 'Alpha', lastSeq: 4 }), conversation('b', { title: 'Beta', lastSeq: 9 }),
    conversation('timed', { lastMessage: last(2), lastSeq: 1 }), conversation('null-time', { title: 'Null', lastMessage: { ...last(8), at: null }, lastSeq: 6 }),
  ];
  assert.deepEqual(groupConversations(rows, uid).channels.map(value => value.id), ['timed', 'b', 'null-time', 'a', 'c']);
  assert.equal(compareConversations(rows[3]!, rows[3]!), 0);
});

test('groupConversations: Bot DMs the viewer only reads as the Bot owner follow their own', () => {
  const key = botCard().sharedBotId;
  const rows = [
    conversation('others', { kind: 'bot_dm', participants: ['user:bob', `shared_bot:${key}`], lastMessage: last(9) }),
    conversation('mine', { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${key}`], lastMessage: last(1) }),
  ];
  assert.deepEqual(groupConversations(rows, uid).botDirect.map(value => value.id), ['mine', 'others']);
});

test('conversationLabel: channels are titles; dm and group fall back to the other participants', () => {
  const people = members('alice', 'bob', 'carol');
  assert.equal(conversationLabel(conversation('x', { kind: 'channel', title: ' general ' }), people, uid), 'general');
  assert.equal(conversationLabel(conversation('x', { kind: 'channel', title: '' }), people, uid, { zh: true }), '频道');
  const dm = (title: string) => conversation('d', { kind: 'dm', title, participants: ['user:alice', 'user:bob'] });
  assert.equal(conversationLabel(dm('Project sync'), people, uid), 'Project sync');
  assert.equal(conversationLabel(dm(''), people, uid), 'Bob');
  assert.equal(conversationLabel(dm('Alice, Bob'), people, uid), 'Bob', 'a title made of the participants\' names is generated, not chosen');
  assert.equal(conversationLabel(dm('Bob, Alice'), people, uid), 'Bob');
  const group = (title: string) => conversation('g', { kind: 'group', title, participants: ['user:alice', 'user:bob', 'user:carol'] });
  assert.equal(conversationLabel(group('Alice, Bob, Carol'), people, uid), 'Bob, Carol');
  assert.equal(conversationLabel(group('Alice, Bob, Carol'), people, uid, { zh: true }), 'Bob、Carol');
  assert.equal(conversationLabel(group('Launch crew'), people, uid), 'Launch crew');
  assert.equal(conversationLabel(group('  '), people, uid), 'Bob, Carol');
  assert.equal(conversationLabel(dm(''), [], uid), 'Member', 'people the directory does not list');
  assert.equal(conversationLabel(conversation('solo', { kind: 'dm', title: '', participants: ['user:alice'] }), people, uid), 'Direct message');
  assert.equal(conversationLabel(conversation('solo', { kind: 'group', title: '', participants: [] }), people, uid, { zh: true }), '群聊');
});

test('conversationLabel: a Bot DM is the Bot\'s name, with the person for the Bot owner reading someone else\'s', () => {
  const card = botCard({ name: 'Scribe' }), people = members('alice', 'bob');
  const own = conversation('b', { kind: 'bot_dm', title: 'Stored title', participants: ['user:alice', `shared_bot:${card.sharedBotId}`] });
  assert.equal(conversationLabel(own, people, uid, { bots: [card] }), 'Scribe');
  assert.equal(conversationLabel(own, people, uid), 'Stored title', 'no card: the stored title');
  assert.equal(conversationLabel({ ...own, title: '' }, people, uid), 'Bot');
  const others = conversation('b2', { kind: 'bot_dm', title: 'Scribe', participants: ['user:bob', `shared_bot:${card.sharedBotId}`] });
  assert.equal(conversationLabel(others, people, uid, { bots: [card] }), 'Scribe · Bob');
  assert.equal(conversationLabel({ ...others, }, people, uid, { bots: [card] }), 'Scribe · Bob', 'the Bot is found in the participants too');
});

test('conversationParticipants lists other people, then Bots, then the viewer, with the local Bot id for avatars', () => {
  const card = botCard({ name: 'Scribe' }), people = members('alice', 'bob');
  const list = conversationParticipants(conversation('g', { kind: 'group', participants: ['user:alice', `shared_bot:${card.sharedBotId}`, 'user:bob', 'shared_bot:other-assistant'] }), people, [card], uid);
  assert.deepEqual(list.map(value => [value.kind, value.id, value.name, value.self]), [
    ['user', 'bob', 'Bob', false], ['bot', card.sharedBotId, 'Scribe', false], ['bot', 'other-assistant', 'Bot', false], ['user', 'alice', 'Alice', true],
  ]);
});

test('findBotDm finds only the viewer\'s own DM with that Bot', () => {
  const key = botCard().sharedBotId;
  const rows = [conversation('theirs', { kind: 'bot_dm', participants: ['user:bob', `shared_bot:${key}`] }), conversation('mine', { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${key}`] }), conversation('group', { kind: 'group', participants: ['user:alice', `shared_bot:${key}`] })];
  assert.equal(findBotDm(rows, uid, key)?.id, 'mine');
  assert.equal(findBotDm(rows, uid, 'absent-shared-id'), undefined);
});

test('conversationPreview follows the lastMessage rules', () => {
  const row = (lastMessage?: TeamConversation['lastMessage'], kind: TeamConversation['kind'] = 'dm') => conversation('p', { kind, ...(lastMessage ? { lastMessage } : {}) });
  assert.equal(conversationPreview(row(), uid, false), 'No messages yet');
  assert.equal(conversationPreview(row(), uid, true), '暂无消息');
  assert.equal(conversationPreview(row(last(1)), uid, false), 'Hi');
  assert.equal(conversationPreview(row(last(1, { author: { kind: 'user', uid: 'alice' } })), uid, false), 'You: Hi');
  assert.equal(conversationPreview(row(last(1, { author: { kind: 'user', uid: 'alice' } })), uid, true), '你：Hi');
  assert.equal(conversationPreview(row(last(1, { preview: '', recalled: true })), uid, false), 'Message recalled');
  assert.equal(conversationPreview(row(last(1, { preview: '', recalled: true })), uid, true), '消息已撤回');
  assert.equal(conversationPreview(row(last(1, { preview: '', recalled: true, author: { kind: 'user', uid: 'alice' } })), uid, false), 'You: Message recalled');
  assert.equal(conversationPreview(row(last(1, { preview: '' })), uid, false), '[Non-text message]');
  assert.equal(conversationPreview(row(last(1, { preview: '' })), uid, true), '[非文本消息]');
  assert.equal(conversationPreview(row(last(1), 'group'), uid, false, () => 'Bob'), 'Bob: Hi', 'groups name the sender');
  assert.equal(conversationPreview(row(last(1), 'channel'), uid, false, () => 'Bob'), 'Bob: Hi', 'so do channels');
  assert.equal(conversationPreview(row(last(1), 'bot_dm'), uid, false, () => 'Bob'), 'Hi', 'a Bot dm does not');
  assert.equal(conversationPreview(row(last(1), 'dm'), uid, false, () => 'Bob'), 'Hi', 'a dm does not');
});

test('markdownPlainText flattens Markdown to one plain line', () => {
  const vectors: [string, string][] = [
    ['# QA Follow-Up Check\n\n**Goal: close the open items.**\n\n- Scope: defects', 'QA Follow-Up Check Goal: close the open items. Scope: defects'],
    ['**G', 'G'],
    ['- [ ] Confirm the list\n- [x] Done', 'Confirm the list Done'],
    ['| Item | Owner |\n|---|---|\n| Confirm scope | QA 2 |', 'Item Owner Confirm scope QA 2'],
    ['See [the doc](https://example.com/a) and `code`', 'See the doc and code'],
    ['```ts\nconst a = 1;\n```', 'const a = 1;'],
    ['2 * 3 * 4 and snake_case_name', '2 * 3 * 4 and snake_case_name'],
    ['1. first\n2. second', 'first second'],
    ['> quoted _text_', 'quoted text'],
    ['![chart](https://x/y.png)', 'chart'],
    ['---', ''],
    ['', ''],
  ];
  for (const [input, expected] of vectors) assert.equal(markdownPlainText(input), expected, JSON.stringify(input));
});

test('only a shared Bot message preview is flattened; a person\'s and a recalled message stay as stored', () => {
  const bot = { kind: 'shared_bot' as const, sharedBotId: 'bot-aaaaaaaa', displayName: 'Fixture Bot' };
  const markdown = '# QA Follow-Up Check **Goal**';
  assert.equal(pointerPreview(last(1, { preview: markdown, author: bot }), uid, false, () => 'Fixture Bot'), 'Fixture Bot: QA Follow-Up Check Goal');
  assert.equal(pointerPreview(last(1, { preview: markdown }), uid, false, () => 'Bob'), `Bob: ${markdown}`);
  assert.equal(pointerPreview(last(1, { preview: '2 * 3 * 4 # keep' }), uid, false), '2 * 3 * 4 # keep');
  assert.equal(pointerPreview(last(1, { preview: '**G…', author: bot }), uid, false), 'G…');
  assert.equal(pointerPreview(last(1, { preview: '---', author: bot }), uid, true), '[非文本消息]', 'a preview with no text left falls back like an empty one');
  assert.equal(pointerPreview(last(1, { preview: '# gone', recalled: true, author: bot }), uid, false), 'Message recalled');
  const row = conversation('p', { kind: 'bot_dm', lastMessage: last(1, { preview: '**Done**', author: bot }) });
  assert.equal(conversationPreview(row, uid, false), 'Done');
});

// ── Workspace ──

function workspaceFixture(setup: (fake: FakeTeams) => void = () => {}, options: ConstructorParameters<typeof TeamsWorkspace>[2] = {}) {
  const fake = new FakeTeams();
  fake.teams = [team('team-aaaa'), team('team-bbbb', { generation: 2 })];
  fake.conversations.set('team-aaaa', [conversation('chan-a', { lastSeq: 5, lastReadSeq: 2, lastMessage: last(5) }), conversation('dm-a', { kind: 'dm', lastSeq: 1, lastReadSeq: 1 })]);
  fake.conversations.set('team-bbbb', [conversation('chan-b', { lastSeq: 4, lastReadSeq: 2, lastMessage: last(4) })]);
  fake.threads.set('chan-a', [message(1), message(2), message(3), message(4), message(5)]);
  fake.bots = [botCard()];
  setup(fake);
  const storage = memoryStorage();
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: () => storage, environment: new EventTarget(), refreshDelay: 5, ...options });
  return { fake, workspace, storage, snapshot: (): TeamsSnapshot => workspace.getSnapshot() };
}

test('the first team opens with its detail, controller, conversations and Bot cards', async context => {
  const { workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  assert.equal(snapshot().loading, true);
  await until(() => snapshot().conversations.length === 2 && snapshot().bots.length === 1, 'the first team to load');
  const current = snapshot();
  assert.equal(current.loading, false); assert.equal(current.selectedTeamId, 'team-aaaa'); assert.equal(current.team?.name, 'Team team-aaaa');
  assert.equal(current.detail?.team.id, 'team-aaaa'); assert.deepEqual(current.teams.map(value => value.id), ['team-aaaa', 'team-bbbb']);
  assert.ok(current.messaging); assert.equal(current.selectedConversationId, ''); assert.equal(current.botsUnverified, false);
  assert.deepEqual(current.directory.map(value => value.uid), ['alice', 'bob', 'owner']);
});

test('initialTeamId picks the team to open when it is listed', async context => {
  const { workspace, snapshot } = workspaceFixture(() => {}, { initialTeamId: 'team-bbbb' }); context.after(() => workspace.close());
  await until(() => snapshot().selectedTeamId === 'team-bbbb' && !!snapshot().messaging, 'team B');
  assert.equal(snapshot().team?.generation, 2);
});

test('switching teams closes the previous controller and hands it a directory-only unread listener', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().conversations.length === 2, 'team A');
  workspace.selectConversation('chan-a'); await until(() => snapshot().messages.length === 5, 'the open thread');
  const first = snapshot().messaging!;
  assert.equal(fake.open('team-aaaa', 'chan-a').length, 1);
  workspace.selectTeam('team-bbbb');
  await until(() => snapshot().conversations.some(value => value.id === 'chan-b'), 'team B');
  assert.notEqual(snapshot().messaging, first);
  await assert.rejects(first.queue('create', 'late'), /INVALID_INPUT/, 'the previous controller is closed');
  assert.equal(fake.open('team-aaaa', 'chan-a').length, 0, 'its message listener is released');
  assert.equal(fake.open('team-bbbb').length, 1, 'team B has one listener: its controller');
  await until(() => fake.open('team-aaaa', '').length === 1, 'team A\'s unread listener');
  assert.equal(snapshot().selectedTeamId, 'team-bbbb'); assert.equal(snapshot().team?.generation, 2);
  assert.equal(snapshot().detail?.team.id, 'team-bbbb');
  // Back to A: the conversation that was open is open again, and B is counted by a listener.
  workspace.selectTeam('team-aaaa');
  await until(() => snapshot().selectedConversationId === 'chan-a' && snapshot().messages.length === 5, 'the remembered conversation');
  assert.equal(fake.open('team-aaaa', '').length, 0, 'the selected team does not also keep an unread listener');
  assert.ok(fake.open('team-bbbb', '').length === 1);
});

test('unread: the selected team counts its open directory, the others a directory-only listener', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().unreadByTeam['team-aaaa'] === 3 && snapshot().unreadByTeam['team-bbbb'] === 2, 'both counts');
  assert.equal(snapshot().totalUnread, 5);
  const [light] = fake.open('team-bbbb');
  assert.equal(light!.conversationId, '', 'no message query for a team that is not open');
  fake.emit(light!, { kind: 'read', conversationId: 'chan-b', lastReadSeq: 4 });
  assert.equal(snapshot().unreadByTeam['team-bbbb'], 0); assert.equal(snapshot().totalUnread, 3);
  fake.emit(light!, { kind: 'read', conversationId: 'chan-b', lastReadSeq: 1, fromCache: true });
  assert.equal(snapshot().unreadByTeam['team-bbbb'], 0, 'read marks never step back');
  fake.emit(light!, { kind: 'read', conversationId: 'unknown', lastReadSeq: 9 });
  fake.conversations.set('team-bbbb', [conversation('chan-b', { lastSeq: 9, lastReadSeq: 2 }), conversation('other', { lastSeq: 3, lastReadSeq: 0 })]);
  fake.deliver(light!);
  assert.equal(snapshot().unreadByTeam['team-bbbb'], 10);
  // Snapshots of another account or generation are ignored.
  light!.next({ uid: 'mallory', ...light!.scope, kind: 'directory', fromCache: false, conversations: [] });
  light!.next({ uid, teamId: 'team-bbbb', generation: 99, kind: 'directory', fromCache: false, conversations: [] });
  assert.equal(snapshot().unreadByTeam['team-bbbb'], 10);
  // A listener error stops that team's count; the next refresh starts it again.
  fake.emit(light!, { kind: 'error', error: 'MEMBERSHIP_REQUIRED' });
  assert.equal(light!.open, false); assert.equal('team-bbbb' in snapshot().unreadByTeam, false);
  await workspace.refresh();
  await until(() => fake.open('team-bbbb').length === 1 && 'team-bbbb' in snapshot().unreadByTeam, 'the restarted listener');
});

test('unread: at most maxWatchedTeams other teams get a listener', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.teams = ['team-aaaa', 'team-bbbb', 'team-cccc', 'team-dddd'].map(id => team(id)); }, { maxWatchedTeams: 2 });
  context.after(() => workspace.close());
  await until(() => !snapshot().loading && !!snapshot().messaging, 'load'); await settle();
  assert.deepEqual(['team-bbbb', 'team-cccc', 'team-dddd'].map(id => fake.open(id).length), [1, 1, 0]);
});

test('execute runs one command at a time and refetches afterwards', async context => {
  let release!: (value: { teamId?: string }) => void;
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.execute = command => { value.teamCommands.push(command); return new Promise(resolve => { release = resolve; }); }; });
  context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  const listed = fake.listCalls;
  const first = workspace.execute({ action: 'create', teamId: 'team-cccc', name: 'Third' });
  assert.equal(snapshot().busy, true);
  assert.equal(await workspace.execute({ action: 'create', teamId: 'team-dddd', name: 'Fourth' }), false);
  assert.deepEqual(fake.teamCommands.map(value => value.action), ['create']);
  fake.teams.push(team('team-cccc', { generation: 3 }));
  release({ teamId: 'team-cccc' });
  assert.equal(await first, true);
  assert.equal(snapshot().busy, false); assert.ok(fake.listCalls > listed);
  assert.equal(snapshot().selectedTeamId, 'team-cccc', 'the created team opens');
  assert.equal(snapshot().team?.generation, 3);
});

test('execute: an invite shows its link; a failure keeps its code', async context => {
  const code = 'A'.repeat(32);
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.execute = async command => { value.teamCommands.push(command); if (command.action === 'join') throw new Error('INVITE_UNAVAILABLE'); return { code }; }; });
  context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  assert.equal(await workspace.execute({ action: 'invite', teamId: 'team-aaaa', requestId: 'request-1', role: 'member', maxUses: 1, ttlSeconds: 3600 }), true);
  assert.equal(snapshot().inviteLink, `https://lingxios.net/open/team/join?code=${code}`);
  assert.equal(await workspace.execute({ action: 'join', code, requestId: 'request-2' }), false);
  assert.equal(snapshot().error, 'INVITE_UNAVAILABLE'); assert.equal(snapshot().busy, false);
  assert.equal(fake.teamCommands.length, 2);
});

test('execute: a result that arrives after an invalidation is not applied', async context => {
  let release!: (value: { code?: string; teamId?: string }) => void;
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.execute = () => new Promise(resolve => { release = resolve; }); });
  context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  const pending = workspace.execute({ action: 'invite', teamId: 'team-aaaa', requestId: 'request-1', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  fake.invalidate();
  release({ code: 'B'.repeat(32) });
  assert.equal(await pending, false);
  assert.equal(snapshot().inviteLink, '', 'a link for the old view is never shown');
  const created = workspace.execute({ action: 'create', teamId: 'team-cccc', name: 'Third' });
  fake.invalidate(); fake.teams.push(team('team-cccc')); release({ teamId: 'team-cccc' });
  assert.equal(await created, false); assert.equal(snapshot().selectedTeamId, 'team-aaaa', 'the new team is not selected on a stale authority');
});

test('join: fresh same-account membership reconciles a success invalidated during dispatch', async context => {
  const target = team('team-joined', { generation: 7 });
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().detail, 'initial detail');
  fake.execute = async () => { fake.invalidate(); return { teamId: target.id, generation: 7, alreadyMember: false }; };
  fake.details.set(target.id, { team: target, members: [{ uid, displayName: 'Alice', role: 'member', generation: 7 }], invites: [] });
  fake.teams.push(target);
  assert.equal(await workspace.execute({ action: 'join', code: 'A'.repeat(32), requestId: 'join-race' }), true);
  assert.equal(snapshot().selectedTeamId, target.id); assert.equal(snapshot().detail?.team.generation, 7);
  assert.equal(snapshot().busy, false); assert.equal(snapshot().error, '');
});

test('join: invalidated success confirms verified self/detail while the remaining team list stays loading', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().detail, 'initial detail');
  fake.execute = async () => { fake.invalidate(); return { teamId: 'team-aaaa', generation: 1, alreadyMember: true }; };
  fake.list = () => new Promise(() => {});
  assert.equal(await workspace.execute({ action: 'join', code: 'A'.repeat(32), requestId: 'join-list-pending' }), true);
  assert.equal(snapshot().detail?.team.id, 'team-aaaa'); assert.equal(snapshot().busy, false);
  assert.equal(snapshot().detail?.members.find(member => member.uid === uid)?.generation, 1);
  assert.equal(snapshot().loading, true, 'the outstanding list has not completed');
  assert.deepEqual(snapshot().teams.map(value => value.id), ['team-aaaa'], 'only the verified Team is reconciled');
});

for (const invalid of ['revoked', 'unavailable', 'account-changed', 'wrong-team', 'missing-self', 'team-generation', 'self-generation'] as const) {
  test(`join: fresh membership proof rejects ${invalid} after an invalidated success`, async context => {
    const target = team('team-joined', { generation: 7 });
    const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
    await until(() => !!snapshot().detail, 'initial detail');
    fake.execute = async () => { fake.invalidate(); return { teamId: target.id, generation: 7 }; };
    const detail = fake.detail;
    fake.detail = async id => {
      if (id !== target.id) return detail(id);
      if (invalid === 'revoked') throw new Error('MEMBERSHIP_REQUIRED');
      if (invalid === 'unavailable') throw new Error('SERVICE_UNAVAILABLE');
      if (invalid === 'account-changed') throw new Error('INVALID_SESSION');
      return { team: { ...target, ...(invalid === 'wrong-team' ? { id: 'team-other' } : {}), ...(invalid === 'team-generation' ? { generation: 8 } : {}) },
        members: [{ uid: invalid === 'missing-self' ? 'other-account' : uid, displayName: 'Alice', role: 'member', generation: invalid === 'self-generation' ? 8 : 7 }], invites: [] };
    };
    assert.equal(await workspace.execute({ action: 'join', code: 'A'.repeat(32), requestId: 'join-rejected' }), false);
    assert.notEqual(snapshot().selectedTeamId, target.id);
  });
}

for (const interruption of ['invalidation', 'close'] as const) {
  test(`join: ${interruption} while current membership proof is pending ${interruption === 'close' ? 'rejects the old response' : 'still confirms the verified join'}`, async context => {
    const target = team('team-joined', { generation: 7 });
    const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
    await until(() => !!snapshot().detail, 'initial detail');
    fake.execute = async () => { fake.invalidate(); return { teamId: target.id, generation: 7 }; };
    let release: ((detail: Awaited<ReturnType<typeof fake.detail>>) => void) | undefined;
    const detail = fake.detail;
    fake.detail = id => id === target.id ? new Promise(resolve => { release = resolve; }) : detail(id);
    const pending = workspace.execute({ action: 'join', code: 'A'.repeat(32), requestId: 'join-interrupted' });
    await until(() => !!release, 'membership proof pending');
    if (interruption === 'close') workspace.close(); else fake.invalidate();
    release!({ team: target, members: [{ uid, displayName: 'Alice', role: 'member', generation: 7 }], invites: [] });
    // The proof is account-scoped HTTP: a cache invalidation cannot make a server-successful join fail; only close does.
    if (interruption === 'close') { assert.equal(await pending, false); assert.notEqual(snapshot().selectedTeamId, target.id); }
    else { assert.equal(await pending, true); assert.equal(snapshot().selectedTeamId, target.id); }
  });
}

for (const generation of [undefined, 0, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
  test(`join: a success without a valid returned membership generation (${generation}) is rejected`, async context => {
    const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
    await until(() => !!snapshot().detail, 'initial detail');
    fake.execute = async () => ({ teamId: 'team-joined', generation });
    assert.equal(await workspace.execute({ action: 'join', code: 'A'.repeat(32), requestId: 'join-no-generation' }), false);
    assert.equal(snapshot().error, 'STALE_MEMBERSHIP'); assert.notEqual(snapshot().selectedTeamId, 'team-joined');
  });
}

test('execute: leaving or deleting drops the selection at once, even after an invalidation, and opens the next team', async context => {
  let release!: (value: object) => void;
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.execute = () => new Promise(resolve => { release = resolve; }); });
  context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  const first = snapshot().messaging!;
  const leaving = workspace.execute({ action: 'leave', teamId: 'team-aaaa', generation: 1 });
  fake.invalidate();
  fake.teams = fake.teams.filter(value => value.id !== 'team-aaaa');
  release({});
  assert.equal(await leaving, false, 'the invalidated view ignores the result');
  assert.equal(snapshot().inviteLink, '');
  await until(() => snapshot().selectedTeamId === 'team-bbbb' && snapshot().detail?.team.id === 'team-bbbb', 'the next team');
  await assert.rejects(first.queue('create', 'late'), /INVALID_INPUT/);
  assert.equal(fake.open('team-aaaa').length, 0);
});

test('an older refresh that finishes last never overwrites a newer one', async context => {
  const gates: ((teams: ReturnType<typeof team>[]) => void)[] = [];
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.list = () => new Promise(resolve => { gates.push(teams => resolve({ teams })); }); });
  context.after(() => workspace.close());
  await until(() => gates.length === 1, 'the first list request');
  void workspace.refresh(); await until(() => gates.length === 2, 'the second list request');
  gates[1]!([team('team-aaaa'), team('team-bbbb')]);
  await until(() => snapshot().teams.length === 2, 'the newer result');
  gates[0]!([team('team-aaaa')]);
  await settle();
  assert.deepEqual(snapshot().teams.map(value => value.id), ['team-aaaa', 'team-bbbb']);
  assert.ok(fake.open('team-aaaa').length > 0);
});

test('an invalidating change hides teams and detail at once, then refetches', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().unreadByTeam['team-bbbb'] === 2, 'load');
  const before = snapshot();
  fake.invalidate();
  const during = snapshot();
  assert.deepEqual(during.teams, []); assert.equal(during.detail, null); assert.equal(during.loading, true); assert.equal(during.inviteLink, '');
  assert.ok(during.authority > before.authority);
  assert.equal(during.selectedTeamId, 'team-aaaa', 'the selection is kept');
  assert.equal(during.team?.id, 'team-aaaa', 'the open conversations stay');
  assert.equal(during.messaging, before.messaging, 'their controller is untouched');
  assert.equal('team-bbbb' in during.unreadByTeam, false, 'other teams\' unread is hidden too');
  assert.equal(fake.open('team-bbbb').length, 0);
  await until(() => snapshot().teams.length === 2 && snapshot().detail !== null && !snapshot().loading, 'the refetch');
  assert.equal(snapshot().messaging, before.messaging, 'refetching the same generation keeps the controller');
  await until(() => snapshot().unreadByTeam['team-bbbb'] === 2, 'the other team\'s count again');
  // A change that is not an invalidation refetches without hiding anything.
  const listed = fake.listCalls;
  fake.invalidate(false);
  assert.equal(snapshot().teams.length, 2);
  await until(() => fake.listCalls > listed, 'a plain refetch');
});

test('a connectivity failure keeps what is shown; a verdict hides it', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging && snapshot().detail !== null, 'load');
  fake.list = async () => { throw new TypeError('Failed to fetch'); };
  await workspace.refresh();
  assert.equal(snapshot().teams.length, 2); assert.ok(snapshot().detail); assert.equal(snapshot().error, 'Failed to fetch');
  fake.list = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await workspace.refresh();
  assert.equal(snapshot().teams.length, 2); assert.equal(snapshot().error, 'SERVICE_UNAVAILABLE');
  fake.list = async () => { throw new Error('SIGN_IN_REQUIRED'); };
  await workspace.refresh();
  assert.deepEqual(snapshot().teams, []); assert.equal(snapshot().detail, null); assert.equal(snapshot().error, 'SIGN_IN_REQUIRED');
  assert.equal(snapshot().loading, false);
});

test('the selection follows the list: a team that disappears hands over to the next one', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging && snapshot().detail !== null, 'load');
  const first = snapshot().messaging!;
  fake.teams = [team('team-bbbb', { generation: 2 })];
  await workspace.refresh();
  await until(() => snapshot().selectedTeamId === 'team-bbbb' && snapshot().detail?.team.id === 'team-bbbb', 'team B');
  assert.notEqual(snapshot().messaging, first);
  fake.teams = [];
  await workspace.refresh();
  assert.equal(snapshot().selectedTeamId, ''); assert.equal(snapshot().messaging, undefined); assert.equal(snapshot().team, undefined);
  assert.equal(fake.openWatches, 0, 'nothing keeps listening');
  fake.teams = [team('team-aaaa')];
  await workspace.refresh();
  await until(() => snapshot().selectedTeamId === 'team-aaaa' && !!snapshot().messaging, 'a team again');
});

test('a new membership generation replaces the controller; the same generation keeps it', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  const first = snapshot().messaging!;
  await workspace.refresh(); assert.equal(snapshot().messaging, first);
  fake.teams = [team('team-aaaa', { generation: 5 }), team('team-bbbb', { generation: 2 })];
  await workspace.refresh();
  assert.notEqual(snapshot().messaging, first); assert.equal(snapshot().messaging!.scope.generation, 5);
  await assert.rejects(first.queue('create', 'late'), /INVALID_INPUT/);
});

test('close releases every listener and the workspace stops reacting', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().unreadByTeam['team-bbbb'] === 2 && !!snapshot().messaging, 'load');
  workspace.selectConversation('chan-a'); await settle();
  assert.ok(fake.openWatches >= 2); assert.ok(fake.changed.size >= 1); assert.ok(fake.signals.size >= 2);
  let notified = 0; workspace.subscribe(() => { notified++; });
  const listed = fake.listCalls;
  workspace.close();
  assert.equal(fake.openWatches, 0); assert.equal(fake.changed.size, 0); assert.equal(fake.signals.size, 0);
  fake.invalidate(); fake.signal({ bots: true }); await settle(); await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(notified, 0); assert.equal(fake.listCalls, listed);
  assert.equal(await workspace.execute({ action: 'create', teamId: 'team-cccc', name: 'x' }), false);
  assert.equal(await workspace.markRead('chan-a'), false);
  workspace.close();
});

test('closing while a command is in flight ignores its result', async context => {
  let release!: (value: object) => void;
  const { workspace, snapshot } = workspaceFixture(value => { value.execute = () => new Promise(resolve => { release = resolve; }); });
  await until(() => !!snapshot().messaging, 'load');
  const pending = workspace.execute({ action: 'create', teamId: 'team-cccc', name: 'x' });
  workspace.close(); release({ teamId: 'team-cccc' });
  assert.equal(await pending, false);
  context.after(() => workspace.close());
});

test('online refetches teams and conversations; focus only the conversations unless there is an error', async context => {
  const environment = new EventTarget();
  const { fake, workspace, snapshot } = workspaceFixture(() => {}, { environment }); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging && snapshot().detail !== null, 'load'); await settle();
  let listed = fake.listCalls;
  environment.dispatchEvent(new Event('focus')); await settle(); assert.equal(fake.listCalls, listed);
  environment.dispatchEvent(new Event('online')); await until(() => fake.listCalls > listed, 'an online refetch');
  fake.list = async () => { throw new Error('SERVICE_UNAVAILABLE'); }; await workspace.refresh();
  fake.list = async () => { fake.listCalls++; return { teams: structuredClone(fake.teams) }; };
  listed = fake.listCalls;
  environment.dispatchEvent(new Event('focus')); await until(() => fake.listCalls > listed, 'a focus retry after an error');
  await until(() => snapshot().error === '', 'the error to clear');
});

test('Bot cards load with the team, reload on a Bot signal, and an unverifiable list is reported', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().bots.length === 1, 'the Bot list');
  const listed = fake.commands.find(value => value.action === 'listSharedBots')!; assert.equal(listed.action, 'listSharedBots'); assert.equal('schemaVersion' in listed && listed.schemaVersion, 1);
  const loaded = () => fake.commands.filter(value => value.action === 'listSharedBots').length, before = loaded();
  fake.signal({ conversationId: 'chan-a' }); await settle(); assert.equal(loaded(), before, 'a conversation signal does not reload Bots');
  fake.bots = [botCard(), botCard({ sharedBotId: 'owner:x:y', name: 'Second' })];
  fake.signal({ bots: true }); await until(() => snapshot().bots.length === 2, 'two Bots');
  fake.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  fake.signal(); await until(() => snapshot().botsUnverified, 'the warning');
  assert.deepEqual(snapshot().bots, []);
  // Switching teams starts from no cards rather than the previous team's.
  workspace.selectTeam('team-bbbb'); assert.deepEqual(snapshot().bots, []); assert.equal(snapshot().botsUnverified, false);
});

test('createConversation puts the viewer first, trims the title and requires usable Bots', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(value => { value.bots = [botCard(), botCard({ sharedBotId: 'owner:x:y', name: 'Off', permitted: false })]; });
  context.after(() => workspace.close());
  await until(() => snapshot().bots.length === 2 && snapshot().offline === false, 'load');
  assert.equal(await workspace.createConversation('group', '  Crew  ', ['bob'], ['owner:x:y']), false);
  assert.equal(snapshot().actionError, 'BOT_USE_REQUIRED'); assert.equal(fake.commands.some(value => value.action === 'create'), false);
  assert.equal(await workspace.createConversation('group', '  Crew  ', ['bob'], [botCard().sharedBotId]), true);
  assert.equal(snapshot().actionError, '');
  const sent = await until(() => fake.commands.some(value => value.action === 'createConversation'), 'the create command').then(() => fake.commands.find(value => value.action === 'createConversation')!);
  assert.equal(sent.action, 'createConversation'); if (sent.action !== 'createConversation') return;
  assert.deepEqual({ ...sent.payload, conversationId: '' }, { conversationId: '', kind: 'group', title: 'Crew', participantUids: ['alice', 'bob'], sharedBotIds: [botCard().sharedBotId] });
  assert.equal(snapshot().messaging!.selected, sent.payload.conversationId, 'the new conversation is selected');
});

test('run keeps one messaging action at a time and records a failure as a code', async context => {
  const { workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  let finish!: () => void;
  const slow = workspace.run(() => new Promise<void>(resolve => { finish = resolve; }));
  assert.equal(snapshot().messageBusy, true);
  assert.equal(await workspace.run(async () => { throw new Error('never runs'); }), false);
  finish(); assert.equal(await slow, true); assert.equal(snapshot().messageBusy, false);
  assert.equal(await workspace.run(async () => { throw new Error('INVALID_INPUT'); }), false); assert.equal(snapshot().actionError, 'INVALID_INPUT');
  assert.equal(await workspace.run(async () => { throw 'plain'; }), false); assert.equal(snapshot().actionError, 'UNKNOWN');
  await workspace.attempt(async () => undefined); assert.equal(snapshot().actionError, '', 'the next action clears the last failure');
});

test('openBotDm opens the viewer\'s existing DM and only creates one when there is none or a fresh one is asked for', async context => {
  const card = botCard();
  const { fake, workspace, snapshot } = workspaceFixture(value => {
    value.bots = [card]; value.conversations.set('team-aaaa', [conversation('bot-chat', { kind: 'bot_dm', participants: ['user:alice', `shared_bot:${card.sharedBotId}`] }), conversation('their-chat', { kind: 'bot_dm', participants: ['user:bob', `shared_bot:${card.sharedBotId}`] })]);
  });
  context.after(() => workspace.close());
  await until(() => snapshot().conversations.length === 2 && snapshot().bots.length === 1, 'load');
  assert.equal(await workspace.openBotDm(card), true);
  assert.equal(snapshot().selectedConversationId, 'bot-chat');
  assert.equal(fake.commands.some(value => value.action === 'createConversation'), false);
  assert.equal(await workspace.openBotDm(card, true), true);
  await until(() => fake.commands.some(value => value.action === 'createConversation'), 'a fresh DM');
  const created = fake.commands.find(value => value.action === 'createConversation')!;
  assert.equal(created.action === 'createConversation' && created.payload.kind, 'bot_dm'); assert.deepEqual(created.action === 'createConversation' && created.payload.participantUids, ['alice']);
  assert.equal(await workspace.openBotDm({ ...card, permitted: false }), false); assert.equal(snapshot().actionError, 'BOT_USE_REQUIRED');
  await assert.rejects(workspace.startBotDm({ ...card, state: 'paused' }), /BOT_USE_REQUIRED/);
});

test('markRead: the open conversation goes through the controller, any other with the same read command up to lastSeq', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().conversations.length === 2, 'load');
  workspace.selectConversation('chan-a'); await until(() => snapshot().messages.length === 5 && !snapshot().offline, 'the thread online');
  const reads = () => fake.commands.filter(value => value.action === 'read');
  assert.equal(await workspace.markRead(), true);
  assert.deepEqual(reads().map(value => value.action === 'read' && [value.conversationId, value.lastReadSeq]), [['chan-a', 5]]);
  assert.equal(await workspace.markRead('dm-a'), true, 'nothing unread: nothing sent');
  assert.equal(reads().length, 1);
  fake.conversations.set('team-aaaa', [conversation('chan-a', { lastSeq: 5, lastReadSeq: 5 }), conversation('dm-a', { kind: 'dm', lastSeq: 8, lastReadSeq: 3 })]);
  fake.deliver(fake.open('team-aaaa', 'chan-a')[0]!);
  await until(() => snapshot().conversations.find(value => value.id === 'dm-a')?.lastSeq === 8, 'the new directory');
  assert.equal(await workspace.markRead('dm-a'), true);
  assert.deepEqual(reads().at(-1), { action: 'read', teamId: 'team-aaaa', generation: 1, conversationId: 'dm-a', lastReadSeq: 8 });
  fake.messages = async () => { throw new Error('STALE_MEMBERSHIP'); };
  fake.conversations.set('team-aaaa', [conversation('chan-a', { lastSeq: 5, lastReadSeq: 5 }), conversation('dm-a', { kind: 'dm', lastSeq: 9, lastReadSeq: 3 })]);
  fake.deliver(fake.open('team-aaaa', 'chan-a')[0]!);
  await until(() => snapshot().conversations.find(value => value.id === 'dm-a')?.lastSeq === 9, 'the next directory');
  assert.equal(await workspace.markRead('dm-a'), false); assert.equal(snapshot().actionError, 'STALE_MEMBERSHIP');
});

test('drafts are kept per team and conversation until they are emptied', async context => {
  const { workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load');
  workspace.setDraft('chan-a', 'half a thought'); workspace.setDraft('dm-a', 'other');
  assert.equal(workspace.draft('chan-a'), 'half a thought'); assert.equal(workspace.draft('dm-a'), 'other'); assert.equal(workspace.draft('none'), '');
  workspace.selectTeam('team-bbbb'); assert.equal(workspace.draft('chan-a'), '', 'another team has its own');
  workspace.selectTeam('team-aaaa'); assert.equal(workspace.draft('chan-a'), 'half a thought');
  workspace.setDraft('chan-a', ''); assert.equal(workspace.draft('chan-a'), '');
});

test('selecting the open team or an unknown team changes nothing', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging && snapshot().detail !== null, 'load');
  const before = snapshot(), calls = fake.listCalls;
  workspace.selectTeam('team-aaaa'); workspace.selectTeam('nope'); await settle();
  assert.equal(snapshot().messaging, before.messaging); assert.equal(fake.listCalls, calls);
});

test('subscribe notifies on change and stops after unsubscribe; snapshots keep their identity between changes', async context => {
  const { workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().messaging, 'load'); await settle();
  assert.equal(workspace.getSnapshot(), workspace.getSnapshot());
  let seen = 0; const stop = workspace.subscribe(() => { seen++; });
  workspace.selectConversation('chan-a'); await until(() => seen > 0, 'a notification');
  stop(); const frozen = seen; workspace.selectConversation('dm-a'); await settle();
  assert.equal(seen, frozen);
});

// ── Direct messages, load errors, Bot list state ──

test('findDm finds the viewer\'s own two-person dm with someone, and nothing else', () => {
  const rows = [
    conversation('group', { kind: 'group', participants: ['user:alice', 'user:bob', 'user:cy'] }),
    conversation('theirs', { kind: 'dm', participants: ['user:bob', 'user:cy'] }),
    conversation('mine', { kind: 'dm', participants: ['user:alice', 'user:bob'] }),
    conversation('bot', { kind: 'bot_dm', participants: ['user:alice', 'user:bob', 'bot:x'] }),
    conversation('crowd', { kind: 'dm', participants: ['user:alice', 'user:bob', 'user:cy'] }),
  ];
  assert.equal(findDm(rows, 'alice', 'bob')?.id, 'mine');
  assert.equal(findDm(rows, 'alice', 'cy'), undefined, 'neither a group nor someone else\'s dm is the viewer\'s dm');
  assert.equal(findDm(rows, 'bob', 'cy')?.id, 'theirs');
  assert.equal(findDm([], 'alice', 'bob'), undefined);
});

test('defaultConversationTitle lists everyone with the creator first, and stops before it passes the limit', () => {
  const people = members('alice', 'bob', 'owner');
  assert.equal(defaultConversationTitle(['bob', 'owner'], people, 'alice'), 'Alice, Bob, Owner');
  assert.equal(defaultConversationTitle(['owner', 'alice', 'bob'], people, 'alice'), 'Alice, Owner, Bob', 'the creator is not listed twice');
  assert.equal(defaultConversationTitle(['bob', 'owner'], people, 'alice', 12), 'Alice, Bob', 'a name that does not fit is left out whole');
  assert.equal(defaultConversationTitle(['bob'], people, 'alice', 3), 'Ali', 'the first name is cut when it alone is too long');
  assert.equal(defaultConversationTitle(['stranger'], people, 'alice'), 'Alice, Member', 'people the directory does not list');
  // conversationLabel recognizes the generated title and shows only the others.
  const dm = conversation('d', { kind: 'group', title: 'Alice, Bob, Owner', participants: ['user:alice', 'user:bob', 'user:owner'] });
  assert.equal(conversationLabel(dm, people, 'alice'), 'Bob, Owner');
  assert.equal(conversationLabel({ ...dm, title: 'Launch crew' }, people, 'alice'), 'Launch crew', 'a name someone chose is kept');
});

test('openDm opens the existing dm, otherwise creates one with exactly the two people; one action at a time', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => snapshot().conversations.length === 2 && !!snapshot().detail, 'team A');
  assert.equal(await workspace.openDm('bob'), true);
  assert.equal(snapshot().selectedConversationId, 'dm-a', 'bob already has a dm with the viewer'); assert.equal(fake.commands.filter(command => command.action === 'create').length, 0);
  const first = workspace.openDm('owner'), second = workspace.openDm('owner');
  assert.equal(await second, false, 'a second action while one runs does nothing'); assert.equal(await first, true);
  await until(() => fake.commands.some(command => command.action === 'create'), 'the creation');
  const created = fake.commands.filter(command => command.action === 'create');
  assert.equal(created.length, 1);
  assert.deepEqual([created[0]!.kind, created[0]!.participantUids, created[0]!.title], ['dm', ['alice', 'owner'], 'Alice, Owner']);
  assert.equal(snapshot().messageBusy, false);
});

test('loadError: a failed refetch is reported and the next good one clears it, without discarding what is shown', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  await until(() => !!snapshot().detail && snapshot().conversations.length === 2, 'loaded');
  assert.equal(snapshot().loadError, ''); assert.equal(snapshot().error, '');
  const detail = fake.detail;
  fake.detail = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await workspace.refresh();
  assert.equal(snapshot().loadError, 'SERVICE_UNAVAILABLE'); assert.equal(snapshot().error, 'SERVICE_UNAVAILABLE');
  assert.deepEqual(snapshot().teams.map(value => value.id), ['team-aaaa', 'team-bbbb'], 'the team list is kept'); assert.equal(snapshot().conversations.length, 2, 'and so are the conversations');
  fake.detail = detail;
  await workspace.refresh();
  assert.equal(snapshot().loadError, ''); assert.equal(snapshot().error, ''); assert.equal(snapshot().detail?.team.id, 'team-aaaa');
  // The team list itself failing is reported the same way.
  fake.list = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await workspace.refresh();
  assert.equal(snapshot().loadError, 'SERVICE_UNAVAILABLE'); assert.equal(snapshot().teams.length, 2);
});

test('botsLoaded: true once the Bot list has been fetched, kept when a later fetch fails, reset for another team', async context => {
  const { fake, workspace, snapshot } = workspaceFixture(); context.after(() => workspace.close());
  assert.equal(snapshot().botsLoaded, false);
  await until(() => snapshot().botsLoaded, 'the first Bot list');
  assert.equal(snapshot().bots.length, 1); assert.equal(snapshot().botsUnverified, false);
  const messages = fake.messages;
  fake.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await workspace.refreshBots();
  assert.equal(snapshot().botsUnverified, true); assert.equal(snapshot().botsLoaded, true, 'the list was known once; availability is what is unverified'); assert.equal(snapshot().bots.length, 0);
  fake.messages = messages;
  await workspace.refreshBots();
  assert.equal(snapshot().botsUnverified, false); assert.equal(snapshot().bots.length, 1);
  fake.messages = () => new Promise(() => {});
  workspace.selectTeam('team-bbbb');
  assert.equal(snapshot().botsLoaded, false, 'another team\'s Bots are not known yet'); assert.equal(snapshot().bots.length, 0);
});
