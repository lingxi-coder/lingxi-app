import assert from 'node:assert/strict';
import { type TestContext } from 'node:test';
import { createElement } from 'react';
import type { MessageCommand, MessageMutation } from '../../../packages/teams/messages';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import { FakeTeams, botCard, conversation, memoryStorage, members, message, team, uid, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';
import { classes, editorNode, hostNode, mount, scenario, textOf, withDom } from './support/workspacePage';

// These tests drive the components' event handlers and effects through react-test-renderer (see support/workspacePage).
const { TeamConversationView } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { TeamDetailsDrawer } = loadTeamsModule<typeof import('../../../packages/teams/TeamDetailsDrawer')>('TeamDetailsDrawer');
const { TeamSidebar } = loadTeamsModule<typeof import('../../../packages/teams/TeamSidebar')>('TeamSidebar');

// ── Fixture ──

const at = (minute: number) => new Date(2026, 10, 2, 11, minute).toISOString();
async function open(context: TestContext, setup: (fake: FakeTeams) => void = () => {}) {
  withDom(context);
  const fake = new FakeTeams();
  const bot = botCard();
  fake.teams = [team('team-aaaa', { name: 'Acme', role: 'owner' }), team('team-bbbb', { name: 'Beta', generation: 2 })];
  fake.bots = [bot];
  fake.conversations.set('team-aaaa', [
    conversation('chan-general', { title: 'general', participants: ['user:alice', 'user:bob', `shared_bot:${bot.sharedBotId}`], lastSeq: 3, lastReadSeq: 1, lastMessage: { id: 'message-3', seq: 3, at: at(3), preview: 'Three', author: { kind: 'user', uid: 'bob' } } }),
    conversation('dm-bob', { kind: 'dm', title: 'Alice, Bob', lastSeq: 0, lastReadSeq: 0 }),
  ]);
  fake.conversations.set('team-bbbb', [conversation('chan-b', { lastSeq: 2, lastReadSeq: 0 })]);
  fake.threads.set('chan-general', [
    message(1, { text: 'One', createdAt: at(1) }),
    message(2, { text: 'Mine', author: { kind: 'user', uid: 'alice' }, createdAt: at(2) }),
    message(3, { text: 'Three', createdAt: at(3) }),
  ]);
  setup(fake);
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  context.after(() => workspace.close());
  const snapshot = (): TeamsSnapshot => workspace.getSnapshot();
  await until(() => snapshot().conversations.length === 2 && snapshot().botsLoaded, 'the team to load');
  return { fake, workspace, snapshot, bot };
}


const conversationView = (workspace: TeamsWorkspace, more: Record<string, unknown> = {}) => (snapshot: TeamsSnapshot) => createElement(TeamConversationView, { workspace, snapshot, locale: 'en', now: new Date(2026, 10, 2, 12).getTime(), ...more } as never);
const mutations = (fake: FakeTeams): MessageMutation[] => fake.commands.filter((command): command is MessageMutation => command.action === 'mutate');
const commandsOf = (fake: FakeTeams, action: MessageCommand['action']) => fake.commands.filter(command => command.action === action);

scenario('a task report is opened by its immutable release and paged within its conversation grant', async context => {
  const calls: Record<string, unknown>[] = [];
  const { workspace, snapshot } = await open(context, fake => {
    fake.threads.set('chan-general', [message(1, {
      author: { kind: 'task', taskId: 'orders-daily', logicalRun: 'today', displayName: 'Daily orders' },
      resourceRef: { kind: 'reportRelease', dataset: 'orders', release: 'morning' },
      resourceGrant: 'grant',
    })]);
    fake.api.datasets = async <T>(request: Record<string, unknown>): Promise<T> => {
      calls.push(request);
      const command = request.command as { after?: string };
      return {
        title: 'Morning orders',
        metric: { kind: 'sum', field: 'amount', scale: 2 },
        groups: command.after ? [{ group: 'south', units: '-5' }] : [{ group: 'north', units: '900719925474099301' }],
        cursor: command.after ? null : 'north',
      } as T;
    };
  });
  workspace.selectConversation('chan-general');
  await until(() => snapshot().messages.length === 1, 'the report message');
  const page = await mount(workspace, conversationView(workspace));
  context.after(() => page.unmount());
  await page.click(page.button('Open fixed data'));
  assert.match(page.text(), /Morning orders.*9007199254740993\.01/);
  await page.click(page.button('Next page'));
  assert.match(page.text(), /south-0\.05/);
  assert.deepEqual(calls.map(({ dataset, via, command }) => [dataset, via, command]), [
    ['orders', 'grant', { action: 'openReport', release: 'morning' }],
    ['orders', 'grant', { action: 'openReport', release: 'morning', after: 'north' }],
  ]);
});

// ── Composer ──

scenario('composer: Enter sends and clears, Shift+Enter and an input method\'s Enter do not, and the unsent text is kept per conversation', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  assert.equal(page.editorText(), '');
  await page.type(box(), 'Hello team');
  assert.equal(page.editorText(), 'Hello team'); assert.equal(workspace.draft('chan-general'), 'Hello team', 'the draft is kept by the workspace');
  assert.equal(await page.key(box(), 'Enter', { shiftKey: true }), false, 'Shift+Enter is a line break'); assert.equal(mutations(fake).length, 0);
  assert.equal(await page.key(box(), 'Enter', { nativeEvent: { isComposing: true }, keyCode: 229 }), false, 'Enter inside an input method is not a send'); assert.equal(mutations(fake).length, 0);
  assert.equal(await page.key(box(), 'Enter'), true);
  await until(() => mutations(fake).length === 1, 'the send');
  const sent = mutations(fake)[0]!;
  assert.deepEqual([sent.operation, sent.conversationId, sent.text, 'targets' in sent, sent.replyTo], ['create', 'chan-general', 'Hello team', false, undefined]);
  assert.equal(page.editorText(), '', 'the box is cleared once the message is queued'); assert.equal(workspace.draft('chan-general'), '');
  assert.equal(await page.key(box(), 'Enter'), true, 'Enter on an empty box is swallowed, not sent'); assert.equal(mutations(fake).length, 1);
  await page.type(box(), 'x'.repeat(8193));
  assert.equal(page.oneByClass('tw-send').props.disabled, true, 'over the byte limit'); assert.ok(page.byClass('tw-count').length === 1);
});

scenario('composer: @ offers the Bots of the conversation, only a chosen Bot is asked, and the text it replaced is removed', async context => {
  const { fake, workspace, snapshot, bot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  assert.equal(page.byClass('tw-mentions').length, 0);
  await page.type(box(), 'hey');
  await page.click(page.oneByLabel('Add to message'));
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node).includes('Mention (@)'))[0]);
  assert.equal(page.editorText(), 'hey @');
  assert.equal(page.byClass('tw-mentions').length, 1);
  await page.type(box(), 'hey @Fix', 8);
  const options = page.find(node => node.props.role === 'option');
  assert.deepEqual(options.map(textOf), ['FBFixture BotStatus unverified'], 'a missing availability projection is not presented as online');
  assert.equal(box().props['aria-activedescendant'], options[0].props.id);
  assert.equal(await page.key(box(), 'Escape'), true, 'Escape closes the list'); assert.equal(page.byClass('tw-mentions').length, 0);
  await page.type(box(), 'hey @Fi', 7); assert.equal(page.byClass('tw-mentions').length, 1, 'typing again reopens it');
  assert.equal(await page.key(box(), 'Enter'), true); assert.equal(mutations(fake).length, 0, 'Enter picks the Bot instead of sending');
  assert.equal(page.editorText(), 'hey ', 'the @ query is replaced by a chip'); assert.deepEqual(page.byClass('tw-target').map(textOf), ['@Fixture Bot']);
  await page.type(box(), 'hey please look');
  await page.key(box(), 'Enter'); await until(() => commandsOf(fake, 'submit').length === 1, 'the send');
  const submitted = commandsOf(fake, 'submit')[0]!; assert.equal(submitted.action, 'submit'); if (submitted.action !== 'submit') return; assert.deepEqual(submitted.payload.targets, [bot.sharedBotId]); assert.equal(submitted.payload.text, 'hey please look');
  assert.equal(page.byClass('tw-target').length, 0, 'targets are per message');
  await page.type(box(), 'just @Fixture Bot typed'); await page.key(box(), 'Enter'); await until(() => mutations(fake).length === 1, 'a second send');
  assert.equal('targets' in mutations(fake)[0]!, false, 'typing a name never asks a Bot');
});

for (const first of ['acknowledgement', 'message stream'] as const) {
  scenario(`outgoing bubble and mention stay mounted when the ${first} arrives first`, async context => {
    const { fake, workspace, snapshot } = await open(context);
    workspace.selectConversation('chan-general');
    await until(() => snapshot().messages.length === 3);
    const original = fake.messages;
    let acknowledge: (() => void) | undefined;
    let sent: MessageMutation | undefined;
    fake.messages = async <T>(command: MessageCommand) => {
      if (command.action === 'mutate') {
        sent = command;
        await new Promise<void>(resolve => { acknowledge = resolve; });
      }
      return original<T>(command);
    };
    context.after(() => acknowledge?.());
    const page = await mount(workspace, conversationView(workspace));
    context.after(() => page.unmount());
    const text = '@Bob hello';
    const mentionEntities = [{ userId: 'bob', offset: 0, length: 4 }];
    await page.during(() => snapshot().messaging!.queue('create', text, 'outgoing-id', undefined, [], [], undefined, undefined, mentionEntities));
    await until(() => !!acknowledge);

    const row = () => {
      const rows = page.find(node => node.props['data-message-id'] === 'outgoing-id');
      assert.equal(rows.length, 1);
      return rows[0];
    };
    const current = row();
    const body = current.findByProps({ className: 'tw-msg-body' });
    const mention = body.findByProps({ className: 'tw-member-mention' });
    const delivery = current.findByProps({ className: 'tw-msg-delivery' });
    assert.equal(textOf(mention), '@Bob');
    assert.equal(delivery.props['data-state'], 'pending');
    assert.equal(current.props['data-message-seq'], undefined);

    const [watch] = fake.open('team-aaaa', 'chan-general');
    assert.ok(watch);
    const [pending] = snapshot().pending;
    const published = message(4, { id: 'outgoing-id', changeId: sent!.changeId, author: { kind: 'user', uid }, text, mentionEntities, createdAt: pending!.enqueuedAt });
    const emit = () => fake.emit(watch, { kind: 'messages', conversationId: 'chan-general', replace: false, messages: [published] });
    if (first === 'message stream') await page.during(emit);
    await page.during(() => { acknowledge!(); });
    await until(() => snapshot().messaging!.state.outbox.length === 0);
    assert.equal(row(), current);
    assert.equal(delivery.props['data-state'], 'sent');
    if (first === 'acknowledgement') await page.during(emit);

    assert.equal(row(), current);
    assert.equal(current.findByProps({ className: 'tw-msg-body' }), body);
    assert.equal(body.findByProps({ className: 'tw-member-mention' }), mention);
    assert.equal(current.findByProps({ className: 'tw-msg-delivery' }), delivery);
    assert.equal(current.props['data-message-seq'], 4);
    assert.equal(snapshot().pending.length, 0);
  });
}

scenario('composer: a Bot whose Desktop is off still takes a message in a channel and the hint says it answers when it is back; a Bot DM says no at once', async context => {
  const { fake, workspace, snapshot, bot } = await open(context, fake => {
    fake.bots = [botCard({ availability: 'offline', ready: false })];
    fake.conversations.set('team-aaaa', [
      conversation('chan-general', { title: 'general', participants: ['user:alice', 'user:bob', 'shared_bot:assistant-fixture'], lastSeq: 3, lastReadSeq: 1 }),
      conversation('bot-dm', { kind: 'bot_dm', title: 'Fixture Bot', participants: ['user:alice', 'shared_bot:assistant-fixture'] }),
    ]);
  });
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message'), hint = () => page.byClass('tw-hint').map(textOf).join(' ');
  await page.type(box(), 'hey @Fix', 8); await page.key(box(), 'Enter'); assert.deepEqual(page.byClass('tw-target').map(textOf), ['@Fixture Bot']);
  await page.type(box(), 'hey please look');
  assert.equal(page.oneByClass('tw-send').props.disabled, false, 'a Desktop that is off does not stop a message to a Bot in a channel');
  assert.match(hint(), /Fixture Bot is offline and will reply when it’s back online/); assert.doesNotMatch(hint(), /cannot accept a new request/);
  await page.key(box(), 'Enter'); await until(() => commandsOf(fake, 'submit').length === 1, 'the send');
  const submitted = commandsOf(fake, 'submit')[0]!; if (submitted.action !== 'submit') return; assert.deepEqual(submitted.payload.targets, [bot.sharedBotId]);
  // An edit asks no Bot (rule 8): it drops the chosen Bots, so under the box there is the edit hint and no line about waiting for a Desktop.
  await page.click(page.oneByLabel('Edit'));
  assert.match(hint(), /Editing doesn’t notify Bots/); assert.doesNotMatch(hint(), /back online/);
  await page.key(box(), 'Escape'); assert.doesNotMatch(hint(), /notify Bots/);
  // A Bot DM has the one Bot, and the service still refuses it while its Desktop is off, so the composer does too.
  workspace.selectConversation('bot-dm'); await until(() => snapshot().selectedConversationId === 'bot-dm', 'the Bot DM');
  await page.type(box(), 'hello');
  assert.equal(page.oneByClass('tw-send').props.disabled, true); assert.match(hint(), /This Bot cannot accept a new request now/); assert.doesNotMatch(hint(), /will reply when it’s back online/);
  // A Bot that is paused or in maintenance is refused in a channel too: only a Desktop that is off is waited for.
  fake.bots = [botCard({ availability: 'maintenance', ready: false })]; await workspace.refreshBots(); await until(() => snapshot().bots[0]?.availability === 'maintenance', 'the card');
  workspace.selectConversation('chan-general'); await until(() => snapshot().selectedConversationId === 'chan-general', 'the channel');
  await page.type(box(), 'hey @Fix', 8); await page.key(box(), 'Enter'); await page.type(box(), 'again please');
  assert.equal(page.oneByClass('tw-send').props.disabled, true); assert.match(hint(), /This Bot cannot accept a new request now/);
});

scenario('composer: a full display name selects one member ID and survives draft and cursor changes', async context => {
  const { fake, workspace, snapshot, bot } = await open(context, fake => {
    const chat = fake.conversations.get('team-aaaa')![0]!;
    chat.participants.push('user:carol');
    fake.details.set('team-aaaa', { team: fake.teams[0]!, invites: [], members: members('alice', 'bob', 'carol', 'owner').map(member => ({
      ...member, displayName: member.uid === 'alice' ? 'Alice' : 'Mary Jane',
    })) });
  });
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3 && snapshot().directory.length === 4);
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  await page.type(box(), '👋 @mary ja');
  assert.deepEqual(page.byClass('tw-mention').map(textOf), ['MJMary Janebob', 'MJMary Janecarol']);
  await page.key(box(), 'ArrowDown'); await page.key(box(), 'Enter');
  assert.equal(page.editorText(), '👋 @Mary Jane ');
  const selected = { userId: 'carol', offset: 3, length: 10 };
  assert.deepEqual(workspace.draftMentions('chan-general'), [selected]);

  workspace.selectConversation('dm-bob');
  await until(() => page.editorText() === '');
  workspace.selectConversation('chan-general');
  await until(() => page.editorText() === '👋 @Mary Jane ');
  await page.type(box(), 'Hi 👋 @Mary Jane please');
  const shifted = { ...selected, offset: 6 };
  assert.deepEqual(workspace.draftMentions('chan-general'), [], 'an ambiguous multi-range edit drops the association');
  await page.type(box(), 'Hi 👋 @mary ja');
  await page.key(box(), 'ArrowDown'); await page.key(box(), 'Enter');
  await page.type(box(), 'Hi 👋 @Mary Jane please @Fixture B');
  await page.key(box(), 'Enter'); await page.key(box(), 'Enter');
  await until(() => commandsOf(fake, 'submit').length === 1);
  const [sent] = commandsOf(fake, 'submit');
  if (sent?.action !== 'submit') throw new Error('Submit expected');
  assert.deepEqual(sent.payload.mentionEntities, [shifted]);
  assert.deepEqual(sent.payload.targets, [bot.sharedBotId]);

  await page.type(box(), '@Mary Jane');
  await page.key(box(), 'Escape'); await page.key(box(), 'Enter');
  await until(() => mutations(fake).length === 1);
  assert.equal(mutations(fake)[0]?.mentionEntities, undefined);
});

scenario('composer replaces the rest of a spaced name and preserves the mention after a repeated paste', async context => {
  const { fake, workspace, snapshot } = await open(context, fake => {
    fake.details.set('team-aaaa', { team: fake.teams[0]!, invites: [], members: members('alice', 'bob').map(member =>
      member.uid === 'bob' ? { ...member, displayName: 'Mary Jane' } : member) });
    fake.bots = [{ ...fake.bots[0]!, name: 'Build  Tools' }];
  });
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3 && snapshot().directory.length === 2);
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  await page.type(box(), '👋 @Mary Jane, please review', 8);
  await page.key(box(), 'Enter');
  assert.equal(page.editorText(), '👋 @Mary Jane, please review');
  assert.deepEqual(workspace.draftMentions('chan-general'), [{ userId: 'bob', offset: 3, length: 10 }]);
  await page.type(box(), ''); await page.type(box(), '@Mary');
  await page.key(box(), 'Enter');
  await page.type(box(), '@Mary Jane @Mary Jane ');
  await page.key(box(), 'Escape');
  await page.key(box(), 'Enter');
  await until(() => mutations(fake).length === 1);
  assert.deepEqual(mutations(fake)[0]?.mentionEntities, [{ userId: 'bob', offset: 0, length: 10 }]);
  await page.type(box(), '@Build  Tools');
  assert.match(page.byClass('tw-mention').map(textOf).join(), /Build  Tools/);
});

scenario('composer replaces a partial surname and removes the recipient on identical literal replacement', async context => {
  const { fake, workspace, snapshot } = await open(context, fake => {
    fake.details.set('team-aaaa', { team: fake.teams[0]!, invites: [], members: members('alice', 'bob').map(member =>
      ({ ...member, displayName: member.uid === 'bob' ? 'Mary Jane' : 'Alice' })) });
  });
  workspace.selectConversation('chan-general');
  await until(() => snapshot().directory.length === 2 && snapshot().messages.length === 3);
  const input = editorNode();
  const page = await mount(workspace, conversationView(workspace), element => element.props.contentEditable ? input : hostNode());
  context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  await page.type(box(), '@Jane, thanks', 3);
  await page.click(page.oneByClass('tw-mention'));
  assert.equal(page.editorText(), '@Mary Jane, thanks');
  assert.equal(workspace.draftMentions('chan-general')[0]?.userId, 'bob');

  await page.during(() => {
    const value = input.textContent;
    input.selectText(value, 0, 10);
    input.dispatchEvent(new Event('beforeinput'));
    input.selectText(value, 10);
    box().props.onInput({ nativeEvent: { inputType: 'insertFromPaste' } });
  });
  assert.equal(page.editorText(), '@Mary Jane, thanks');
  assert.deepEqual(workspace.draftMentions('chan-general'), []);
  await page.key(box(), 'Enter');
  await until(() => mutations(fake).length === 1);
  assert.equal(mutations(fake)[0]?.mentionEntities, undefined);
});

scenario('composer windows a large roster and keeps the wrapped active row available', async context => {
  const { workspace, snapshot } = await open(context, fake => {
    const people = Array.from({ length: 50 }, (_, index) => 'member-' + index);
    const chat = fake.conversations.get('team-aaaa')![0]!;
    chat.participants.push(...people.map(id => 'user:' + id));
    fake.details.set('team-aaaa', { team: fake.teams[0]!, invites: [], members: members('alice', 'bob', ...people) });
  });
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3 && snapshot().directory.length === 52);
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  await page.type(box(), '@member');
  assert.ok(page.byClass('tw-mention').length <= 10);
  await page.key(box(), 'ArrowUp');
  const active = page.byClass('tw-mention').find(node => node.props.id === box().props['aria-activedescendant']);
  assert.equal(active?.props['aria-posinset'], 50);
  assert.equal(active?.props['aria-setsize'], 50);
  await page.key(box(), 'Enter');
  assert.equal(workspace.draftMentions('chan-general')[0]?.userId, 'member-49');
});

scenario('composer preserves a new mention selected while an identical text draft is sending', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3 && snapshot().directory.length > 0);
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  const run = workspace.run;
  let finish!: () => void;
  const sending = new Promise<void>(resolve => { finish = resolve; });
  context.after(() => finish());
  workspace.run = async action => run(async () => { await action(); await sending; });
  await page.type(box(), '@Bob ', 4);
  await page.submit();
  await until(() => mutations(fake).length === 1);
  await page.key(box(), 'Enter');
  assert.equal(page.editorText(), '@Bob ');
  assert.equal(workspace.draftMentions('chan-general')[0]?.userId, 'bob');
  finish();
  await until(() => !snapshot().messageBusy);
  assert.equal(page.editorText(), '@Bob ');
  assert.equal(workspace.draftMentions('chan-general')[0]?.userId, 'bob');
});

scenario('a failed Bot request can be retried from its message after the host returns', async context => {
  const { fake, workspace, snapshot, bot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3);
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const original = fake.messages; let ready = false; const submits: MessageCommand[] = [];
  fake.messages = async <T>(command: MessageCommand) => {
    if (command.action === 'submit') { submits.push(structuredClone(command)); if (!ready) throw new Error('HOST_UNAVAILABLE'); return {} as T; }
    return original<T>(command);
  };
  await snapshot().messaging!.queue('create', 'Please help', undefined, undefined, [bot.sharedBotId]);
  await until(() => snapshot().pending[0]?.error === 'HOST_UNAVAILABLE');
  await until(() => page.has('Retry'));
  ready = true; await page.click(page.button('Retry'));
  await until(() => submits.length === 2 && snapshot().pending[0]?.acknowledged === true);
  assert.deepEqual(submits[1], submits[0]);
  const retried = submits[1];
  assert.ok(retried?.action === 'submit');
  const [watch] = fake.open('team-aaaa', 'chan-general');
  assert.ok(watch);
  await page.during(() => {
    fake.emit(watch, { kind: 'messages', conversationId: 'chan-general', replace: false,
      messages: [message(4, { id: retried.payload.messageId, changeId: retried.payload.changeId,
        author: { kind: 'user', uid }, text: 'Please help' })] });
  });
  assert.equal(snapshot().pending.length, 0);
});

scenario('replying, editing and recalling go through the same queue as sending', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  const box = () => page.oneByLabel('Message');
  await page.type(box(), 'a draft');
  // Reply to message 3.
  const reply = page.find(node => node.props['aria-label'] === 'Reply'); assert.equal(reply.length, 3);
  await page.click(reply[2]);
  assert.match(page.byClass('tw-context').map(textOf).join(), /Replying to.*Bob.*Three/);
  await page.key(box(), 'Enter'); await until(() => mutations(fake).length === 1, 'the reply');
  assert.equal(mutations(fake)[0]!.replyTo, 'message-3'); assert.equal(mutations(fake)[0]!.text, 'a draft'); assert.equal(page.byClass('tw-context').length, 0, 'the reply bar goes with the send');
  // Edit message 2 (the only one that is the viewer\'s). The draft is set aside and comes back.
  await page.type(box(), 'second draft');
  await page.click(page.oneByLabel('Edit'));
  assert.equal(page.editorText(), 'Mine'); assert.match(page.byClass('tw-context').map(textOf).join(), /Editing message/);
  // An edit never asks a Bot (rule 8), so the box says so while a message is edited in a conversation that has Bots.
  assert.match(page.byClass('tw-hint').map(textOf).join(' '), /Editing doesn’t notify Bots\. To ask one, send a new message\./);
  assert.equal(page.byLabel('Mention (@)').length, 0);
  await page.type(box(), 'Mine @Bob'); assert.equal(page.byClass('tw-mentions').length, 1, 'an edit can still mention members by typing @');
  await page.type(box(), 'Mine, corrected');
  await page.key(box(), 'Enter'); await until(() => mutations(fake).length === 2, 'the edit');
  assert.deepEqual([mutations(fake)[1]!.operation, mutations(fake)[1]!.messageId, mutations(fake)[1]!.text], ['edit', 'message-2', 'Mine, corrected']);
  assert.equal(page.editorText(), 'second draft', 'the draft returns after the edit');
  assert.doesNotMatch(page.byClass('tw-hint').map(textOf).join(' '), /notify Bots/, 'the hint goes with the edit');
  // Escape abandons an edit.
  await page.click(page.oneByLabel('Edit')); assert.equal(page.editorText(), 'Mine');
  await page.key(box(), 'Escape'); assert.equal(page.editorText(), 'second draft'); assert.equal(page.byClass('tw-context').length, 0);
  // Up arrow in an empty box edits the newest own message.
  await page.type(box(), ''); await page.key(box(), 'ArrowUp'); assert.equal(page.editorText(), 'Mine');
  await page.key(box(), 'Escape');
  // Recall asks first.
  await page.click(page.oneByLabel('Recall'));
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.equal(dialog.props.role, 'alertdialog');
  assert.match(textOf(dialog), /Recall this message for everyone\?/); assert.equal(mutations(fake).length, 2);
  await page.click(page.button('Cancel')); assert.equal(page.find(node => node.type === 'dialog').length, 0); assert.equal(mutations(fake).length, 2);
  await page.click(page.oneByLabel('Recall')); await page.click(page.button('Recall'));
  await until(() => mutations(fake).length === 3, 'the recall');
  assert.deepEqual([mutations(fake)[2]!.operation, mutations(fake)[2]!.messageId], ['recall', 'message-2']); assert.equal(page.find(node => node.type === 'dialog').length, 0);
});

scenario('marking read goes to the server and clears the unread divider and the button', async context => {
  const { fake, workspace, snapshot } = await open(context);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  const page = await mount(workspace, conversationView(workspace)); context.after(() => page.unmount());
  assert.equal(page.find(node => node.props['data-unread-divider'] !== undefined).length, 1);
  await page.click(page.button('Mark as read'));
  await until(() => snapshot().conversation?.lastReadSeq === 3, 'the read mark');
  assert.deepEqual(commandsOf(fake, 'read').map(command => (command as { lastReadSeq: number }).lastReadSeq), [3]);
  assert.equal(page.has('Mark as read'), false); assert.equal(page.find(node => node.props['data-unread-divider'] !== undefined).length, 0);
});

// ── Sidebar ──

scenario('sidebar: choosing a row opens it, the team menu switches teams, the row menu marks read', async context => {
  const { fake, workspace, snapshot } = await open(context);
  const chosen: string[] = [];
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, locale: 'en', now: new Date(2026, 10, 2, 12).getTime(), onConversationSelected: (id: string) => chosen.push(id) } as never)); context.after(() => page.unmount());
  const row = (id: string) => page.find(node => node.props['data-row'] === id)[0];
  await page.click(row('dm-bob'));
  assert.equal(snapshot().selectedConversationId, 'dm-bob'); assert.deepEqual(chosen, ['dm-bob']);
  assert.equal(row('dm-bob').props['aria-current'], 'page'); assert.equal(row('dm-bob').props.tabIndex, 0);
  // The row menu.
  await page.click(page.oneByLabel('Conversation actions'));
  const menu = page.find(node => node.props.role === 'menu'); assert.equal(menu.length, 1);
  await page.click(page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Mark as read')[0]);
  await until(() => commandsOf(fake, 'read').length === 1, 'the read command');
  assert.equal((commandsOf(fake, 'read')[0] as { conversationId: string }).conversationId, 'chan-general', 'the menu\'s own row, not the open one');
  assert.equal(page.find(node => node.props.role === 'menu').length, 0, 'choosing closes the menu');
  // The team menu.
  const switcher = page.oneByClass('tw-switcher');
  await page.click(switcher); assert.equal(switcher.props['aria-expanded'], true);
  const teamItems = page.find(node => node.props.role === 'menuitemradio'); assert.deepEqual(teamItems.map(node => node.props['aria-checked']), [true, false]);
  await page.click(teamItems[1]);
  await until(() => snapshot().selectedTeamId === 'team-bbbb', 'team B');
  assert.equal(page.find(node => node.props.role === 'menu').length, 0);
  await until(() => snapshot().conversations.some(value => value.id === 'chan-b'), 'its conversations');
  assert.ok(page.find(node => node.props['data-row'] === 'chan-b').length === 1);
});

scenario('sidebar: the team menu is the way into the details panel, and the sidebar has no button of its own for it', async context => {
  const { workspace } = await open(context);
  const opened: Array<string | undefined> = [];
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, locale: 'en', onOpenSettings: (tab?: string) => { opened.push(tab); } } as never)); context.after(() => page.unmount());
  assert.equal(page.byLabel('Team members & settings').length, 0, 'the conversation header has the panel\'s button; the sidebar does not repeat it');
  await page.click(page.oneByClass('tw-switcher'));
  const entry = page.find(node => node.props.role === 'menuitem' && textOf(node) === 'Team members & settings');
  assert.equal(entry.length, 1, 'the team menu offers the panel, so it can be reached where no conversation (and no header) is open');
  await page.click(entry[0]);
  assert.deepEqual(opened, [undefined], 'it opens on the tab it was last on');
  assert.equal(page.find(node => node.props.role === 'menu').length, 0, 'choosing closes the menu');
});

scenario('sidebar: arrow keys move between rows; the + buttons open dialogs that create things', async context => {
  const { fake, workspace, snapshot } = await open(context);
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, locale: 'en' } as never)); context.after(() => page.unmount());
  // Keyboard movement uses the DOM; give the container the rows to move between.
  const focused: string[] = [];
  const rows = ['chan-general', 'dm-bob'].map(id => ({ id, focus: () => focused.push(id), matches: () => true, props: {} }));
  const container = { querySelectorAll: () => rows };
  const sections = page.oneByClass('tw-sections');
  let prevented = false;
  await page.during(() => sections.props.onKeyDown({ key: 'ArrowDown', target: rows[0], currentTarget: container, preventDefault() { prevented = true; } }));
  assert.deepEqual(focused, ['dm-bob']); assert.equal(prevented, true);
  // A channel.
  await page.click(page.oneByLabel('New channel'));
  const dialog = () => page.find(node => node.type === 'dialog')[0];
  assert.match(textOf(dialog()), /New channel/); assert.equal(page.button('Create channel').props.disabled, true, 'a channel needs a name and someone in it');
  const name = page.find(node => node.type === 'input' && node.props.maxLength === 120)[0];
  await page.type(name, 'planning');
  assert.equal(page.button('Create channel').props.disabled, true, 'a name alone is not enough');
  await page.check(page.find(node => node.type === 'input' && node.props.type === 'checkbox')[0]);
  assert.equal(page.button('Create channel').props.disabled, false);
  await page.submit();
  await until(() => commandsOf(fake, 'create').length === 1, 'the create command');
  const created = commandsOf(fake, 'create')[0] as { kind: string; title: string; participantUids: string[] };
  assert.deepEqual([created.kind, created.title, created.participantUids], ['channel', 'planning', ['alice', 'bob']]);
  await until(() => page.find(node => node.type === 'dialog').length === 0, 'the dialog to close');
  // A person you already talk to: the existing dm opens, no second one is made.
  const radios = () => page.find(node => node.type === 'input' && node.props.type === 'radio' && node.props.name === 'tw-people');
  await page.click(page.oneByLabel('New message'));
  assert.match(textOf(dialog()), /New message/); assert.equal(page.button('Open message').props.disabled, true);
  await page.check(radios()[0]);
  await page.submit();
  await until(() => snapshot().selectedConversationId === 'dm-bob', 'the existing dm');
  assert.equal(commandsOf(fake, 'create').length, 1); await until(() => page.find(node => node.type === 'dialog').length === 0, 'the dialog to close');
  // A person you do not: a new dm with exactly the two of you.
  await page.click(page.oneByLabel('New message'));
  await page.check(radios()[1]);
  await page.submit();
  await until(() => commandsOf(fake, 'create').length === 2, 'the new dm');
  const dm = commandsOf(fake, 'create')[1] as { kind: string; participantUids: string[] };
  assert.deepEqual([dm.kind, dm.participantUids], ['dm', ['alice', 'owner']]);
});

scenario('sidebar: an invitation link opens the join dialog without joining; a bad code cannot be confirmed', async context => {
  const { fake, workspace, snapshot } = await open(context);
  let handled = 0;
  const code = 'B'.repeat(32);
  const page = await mount(workspace, snapshot => createElement(TeamSidebar, { workspace, snapshot, locale: 'en', initialInvite: code, onInviteHandled: () => { handled++; } } as never)); context.after(() => page.unmount());
  assert.equal(handled, 1); assert.equal(fake.teamCommands.length, 0, 'opening a link never joins');
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.match(textOf(dialog), /Join a team/);
  const field = page.find(node => node.type === 'input')[0]; assert.equal(field.props.value, code);
  await page.type(field, 'not an invite'); assert.equal(page.button('Confirm and join').props.disabled, true);
  await page.type(field, `https://lingxios.net/open/team/join?code=${code}`); assert.equal(page.button('Confirm and join').props.disabled, false);
  fake.execute = async command => { fake.teamCommands.push(command); fake.invalidate(); return { teamId: 'team-aaaa', generation: 1, alreadyMember: true }; };
  fake.list = () => new Promise(() => {});
  await page.submit();
  await until(() => fake.teamCommands.length === 1, 'the join');
  assert.equal(fake.teamCommands[0]!.action, 'join'); assert.equal((fake.teamCommands[0] as { code: string }).code, code);
  await until(() => page.find(node => node.type === 'dialog').length === 0, 'the dialog to close');
  assert.equal(snapshot().detail?.members.find(member => member.uid === uid)?.generation, 1);
  assert.equal(snapshot().detail?.team.id, 'team-aaaa');
  assert.equal(snapshot().loading, true);
  assert.match(page.text(), /Loading teams/);
});

// ── Drawer ──

scenario('drawer: tabs follow the arrow keys, Escape closes, the handle resizes, and leaving or deleting asks first', async context => {
  const { fake, workspace, snapshot } = await open(context);
  await until(() => !!snapshot().detail, 'detail');
  const events: string[] = [];
  const widths: number[] = [];
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, locale: 'en', onClose: () => events.push('close'), onTabChange: (tab: string) => events.push(`tab:${tab}`), onWidthChange: (width: number) => widths.push(width) } as never)); context.after(() => page.unmount());
  const aside = () => page.find(node => node.type === 'aside')[0];
  const tablist = page.oneByClass('tw-tabs');
  assert.deepEqual(page.find(node => node.props.role === 'tab').map(node => node.props['aria-selected']), [true, false, false]);
  await page.key(tablist, 'ArrowRight'); assert.deepEqual(events, ['tab:invites']);
  assert.deepEqual(page.find(node => node.props.role === 'tab').map(node => node.props['aria-selected']), [false, true, false]); assert.ok(page.has('Create invite link'));
  await page.key(tablist, 'End'); assert.deepEqual(events.slice(-1), ['tab:bots']); await page.key(tablist, 'ArrowRight'); assert.deepEqual(events.slice(-1), ['tab:members'], 'tabs wrap');
  // Resize.
  const handle = page.find(node => node.props.role === 'separator')[0];
  assert.equal(aside().props.style.width, 360);
  await page.key(handle, 'ArrowLeft'); assert.deepEqual(widths, [376]); assert.equal(aside().props.style.width, 376);
  await page.key(handle, 'End'); await page.key(handle, 'ArrowLeft'); assert.deepEqual(widths, [376, 480], 'the limit is not passed, and a change to nothing is not reported');
  await page.key(handle, 'Home'); assert.equal(aside().props.style.width, 280);
  const pointer = (clientX: number) => ({ clientX, pointerId: 1, currentTarget: { setPointerCapture() {}, releasePointerCapture() {} } });
  await page.during(() => { handle.props.onPointerDown(pointer(800)); handle.props.onPointerMove(pointer(700)); handle.props.onPointerUp(pointer(700)); handle.props.onPointerMove(pointer(0)); });
  assert.equal(aside().props.style.width, 380, 'dragging left by 100 widens by 100; after release the pointer does nothing');
  // Escape closes, but not from under a confirmation.
  await page.key(aside(), 'Escape'); assert.deepEqual(events.slice(-1), ['close']);
  events.length = 0;
  await page.click(page.button('Delete team'));
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.equal(dialog.props.role, 'alertdialog'); assert.match(textOf(dialog), /Delete “Acme”\?/); assert.match(textOf(dialog), /Everyone loses access to its conversations and shared Bots\. This can’t be undone\./);
  assert.ok(dialog.props['aria-describedby'], 'the consequence is what the dialog is described by');
  await page.key(aside(), 'Escape'); assert.deepEqual(events, [], 'Escape belongs to the confirmation');
  assert.equal(fake.teamCommands.length, 0, 'nothing happens until it is confirmed');
  await page.click(page.button('Cancel')); assert.equal(page.find(node => node.type === 'dialog').length, 0);
  await page.click(page.button('Delete team'));
  await page.click(page.find(node => node.type === 'dialog')[0].findAll((node: any) => node.type === 'button' && textOf(node) === 'Delete')[0]);
  await until(() => fake.teamCommands.length === 1, 'the delete');
  assert.deepEqual(fake.teamCommands[0], { action: 'delete', teamId: 'team-aaaa' }); assert.deepEqual(events, ['close'], 'the drawer was about the team that is gone');
});

scenario('drawer: the owner gives the team to a member after reading what does and does not change (M3-3)', async context => {
  const { fake, workspace, snapshot } = await open(context);
  await until(() => !!snapshot().detail, 'detail');
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, locale: 'en', onClose() {} } as never)); context.after(() => page.unmount());
  const offers = page.find(node => node.type === 'button' && textOf(node) === 'Make owner');
  assert.equal(offers.length, snapshot().detail!.members.length - 1, 'every other member can be given the team, and the owner cannot give it to themselves');
  await page.click(offers[0]);
  const dialog = page.find(node => node.type === 'dialog')[0]; assert.equal(dialog.props.role, 'alertdialog');
  assert.match(textOf(dialog), /Make (Bob|Owner) the owner of Acme\?/); assert.match(textOf(dialog), /You stay in the team as an admin/); assert.match(textOf(dialog), /Each Bot stays with the member who maintains it/);
  assert.equal(fake.teamCommands.length, 0, 'nothing happens until it is confirmed');
  await page.click(dialog.findAll((node: any) => node.type === 'button' && textOf(node) === 'Make owner')[0]);
  await until(() => fake.teamCommands.length === 1, 'the handover');
  const target = snapshot().detail!.members.find(member => member.uid === (fake.teamCommands[0] as { uid: string }).uid)!;
  assert.deepEqual(fake.teamCommands[0], { action: 'handOver', teamId: 'team-aaaa', uid: target.uid, generation: target.generation });
});

scenario('drawer: members can be promoted and removed after a confirmation, and invitations are created and revoked', async context => {
  const { fake, workspace, snapshot } = await open(context);
  await until(() => !!snapshot().detail, 'detail');
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, locale: 'en', onClose() {} } as never)); context.after(() => page.unmount());
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Make admin')[0]);
  const inDialog = (name: string) => page.find(node => node.type === 'dialog')[0].findAll((node: any) => node.type === 'button' && textOf(node) === name)[0];
  assert.match(textOf(page.find(node => node.type === 'dialog')[0]), /Make (Bob|Owner) an admin\?/); assert.match(textOf(page.find(node => node.type === 'dialog')[0]), /Admins can invite people and remove members\./);
  assert.match(inDialog('Make admin').props.className, /tw-primary/, 'a change of role is not drawn as destruction');
  await page.click(inDialog('Make admin'));
  await until(() => fake.teamCommands.length === 1, 'the role command');
  assert.equal(fake.teamCommands[0]!.action, 'role'); assert.equal((fake.teamCommands[0] as { role: string }).role, 'admin');
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Remove' && classes(node).includes('tw-danger-btn'))[0]);
  assert.match(textOf(page.find(node => node.type === 'dialog')[0]), /Remove (Bob|Owner) from the team\?/); assert.match(textOf(page.find(node => node.type === 'dialog')[0]), /They lose access to its conversations and shared Bots\. They can rejoin with a new invite\./);
  assert.match(inDialog('Remove').props.className, /tw-danger-btn/, 'removing is destructive');
  await page.click(inDialog('Remove'));
  await until(() => fake.teamCommands.length === 2, 'the remove command'); assert.equal(fake.teamCommands[1]!.action, 'remove');
  fake.execute = async command => { fake.teamCommands.push(command); return command.action === 'invite' ? { code: 'C'.repeat(32) } : {}; };
  await page.click(page.find(node => node.props.role === 'tab' && textOf(node) === 'Invites')[0]);
  const uses = page.find(node => node.type === 'input' && node.props.type === 'number')[0];
  await page.during(() => uses.props.onChange({ target: { value: '5' } }));
  await page.during(() => page.find(node => node.type === 'form')[0].props.onSubmit({ preventDefault() {} }));
  await until(() => fake.teamCommands.length === 3, 'the invite command');
  const invite = fake.teamCommands[2] as { role: string; maxUses: number; ttlSeconds: number };
  assert.deepEqual([invite.role, invite.maxUses, invite.ttlSeconds], ['member', 5, 86400]);
  await until(() => !!snapshot().inviteLink, 'the link');
  assert.equal(page.find(node => node.type === 'input' && node.props.readOnly === true)[0].props.value, `https://lingxios.net/open/team/join?code=${'C'.repeat(32)}`);
});

scenario('drawer: revoking an invite asks first, and a confirmation can be cancelled while its change is still in flight', async context => {
  const invite = { id: 'invite-1', role: 'member' as const, expiresAt: new Date(2026, 11, 2).toISOString(), uses: 0, maxUses: 5, revoked: false };
  const { fake, workspace, snapshot } = await open(context, setup => { const base = setup.detail; setup.detail = async id => { const value = await base(id); return id === 'team-aaaa' ? { ...value, invites: [invite] } : value; }; });
  await until(() => snapshot().detail?.invites.length === 1, 'the invite');
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { workspace, snapshot, locale: 'en', onClose() {} } as never)); context.after(() => page.unmount());
  const dialog = () => page.find(node => node.type === 'dialog')[0], inDialog = (name: string) => dialog().findAll((node: any) => node.type === 'button' && textOf(node) === name)[0];
  await page.click(page.find(node => node.props.role === 'tab' && textOf(node) === 'Invites')[0]);
  await page.click(page.find(node => node.type === 'button' && textOf(node) === 'Revoke')[0]);
  assert.equal(dialog().props.role, 'alertdialog'); assert.match(textOf(dialog()), /Revoke this invite\?/); assert.match(textOf(dialog()), /The link stops working right away\. People who already joined stay in the team\./);
  assert.equal(fake.teamCommands.length, 0, 'asking is not doing');
  assert.equal(inDialog('Cancel').props['data-autofocus'], true, 'the safe choice has the focus');
  // The change takes its time: the confirm waits, Cancel does not.
  const release: (() => void)[] = [];
  fake.execute = command => new Promise(resolve => { fake.teamCommands.push(command); release.push(() => resolve({})); });
  await page.click(inDialog('Revoke'));
  await until(() => fake.teamCommands.length === 1, 'the revoke'); assert.equal(fake.teamCommands[0]!.action, 'revokeInvite');
  assert.equal(inDialog('Revoke').props.disabled, true, 'it is not sent twice'); assert.notEqual(inDialog('Cancel').props.disabled, true, 'but the person is not held in the dialog');
  await page.click(inDialog('Cancel')); assert.equal(page.find(node => node.type === 'dialog').length, 0);
  await page.during(() => { release.forEach(done => done()); });
});

scenario('the conversation view toggles the details drawer through its header button', async context => {
  const { workspace, snapshot } = await open(context, fake => {
    fake.api.datasets = async () => { throw new Error('unexpected dataset request'); };
  });
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 3, 'the thread');
  let toggled = 0;
  const page = await mount(workspace, conversationView(workspace, { detailsOpen: true, onToggleDetails: () => { toggled++; } })); context.after(() => page.unmount());
  const button = page.find(node => node.type === 'button' && node.props['aria-pressed'] === true && node.props['aria-label'] === 'Channel details')[0];
  assert.ok(button);
  assert.doesNotMatch(page.text(), /Add data/);
  await page.click(button); assert.equal(toggled, 1);
});

scenario('channel settings change Bot behavior for the active channel and stay separate from team management', async context => {
  const { fake, workspace, snapshot } = await open(context, fake => {
    fake.bots = [botCard({ autoReply: { daily: 7 }, scope: { localApps: [{ id: 'calendar', name: 'Calendar' }], datasets: [] } })];
    fake.api.datasets = async () => { throw new Error('unexpected dataset request'); };
  });
  workspace.selectConversation('chan-general');
  await until(() => snapshot().conversation?.id === 'chan-general' && !snapshot().offline, 'the channel');
  const page = await mount(workspace, snapshot => createElement(TeamDetailsDrawer, { target: 'conversation', workspace, snapshot, locale: 'en', onClose() {} }));
  assert.match(page.text(), /general.*Channel · 2 members/);
  assert.doesNotMatch(page.text(), /Acme|Invites|Delete team|Visualization size limit/);
  assert.match(page.text(), /Bob.*Fixture Bot.*Alice/);
  assert.doesNotMatch(page.text(), /Owner/, 'the remaining teammate is outside this channel');
  await page.unmount();
  const settings = await mount(workspace, conversationView(workspace));
  context.after(() => settings.unmount());
  await settings.click(settings.oneByLabel('Channel settings'));
  await settings.click(settings.button('Bots'));
  await settings.click(settings.button('What Bots can use in this chat'));
  assert.match(settings.text(), /Calendar/);
  await settings.click(settings.button('Close'));
  await settings.click(settings.button('Bots that answer on their own'));
  const enabled = settings.find(node => node.type === 'input' && node.props.type === 'checkbox' && textOf(node.parent) === 'Fixture Bot')[0];
  await settings.check(enabled);
  await settings.click(settings.button('Save'));
  assert.deepEqual(fake.commands.find(command => command.action === 'setConversationAutoReply')?.payload, {
    conversationId: 'chan-general', expectedRevision: 0, targets: ['assistant-fixture'],
  });
  assert.equal(settings.find(node => node.type === 'dialog').length, 0);
});

scenario('channel gear preserves the chat draft and saves independent post and reply settings', async context => {
  const { workspace, snapshot } = await open(context, fake => {
    const old = fake.messages;
    fake.messages = async <T>(command: MessageCommand) => {
      if (command.action === 'channelSettings' && command.request.op === 'posting') {
        fake.commands.push(command);
        const { conversationId, posting } = command.request;
        const rows = fake.conversations.get('team-aaaa')!;
        fake.conversations.set('team-aaaa', rows.map(row => row.id === conversationId ? { ...row, posting } : row));
        for (const watch of fake.open('team-aaaa')) fake.deliver(watch);
        return { ok: true } as T;
      }
      return old<T>(command);
    };
  });
  workspace.selectConversation('chan-general');
  await until(() => snapshot().messages.length === 3 && !snapshot().offline, 'the channel');
  const page = await mount(workspace, conversationView(workspace));
  context.after(() => page.unmount());
  const input = () => page.messageEditor();
  await page.type(input(), 'Keep this draft');
  await page.click(page.oneByLabel('Channel settings'));
  assert.match(page.text(), /General.*Name.*Description/);
  await page.click(page.button('Posting'));
  const fields = () => page.find(node => node.type === 'select');
  await page.type(fields()[0], 'posts');
  await page.type(fields()[1], 'selected');
  await page.check(page.find(node => node.type === 'input' && textOf(node.parent).includes('Fixture Bot'))[0]);
  await page.type(fields()[2], 'none');
  await page.submit(page.find(node => node.type === 'form' && node.props.className === 'tw-form')[0]);
  assert.deepEqual(snapshot().conversation?.posting, {
    mode: 'posts', senders: 'selected', selected: ['shared_bot:assistant-fixture'], replies: 'none',
  });
  await page.click(page.oneByLabel('Back to channel'));
  assert.match(page.text(), /You do not have permission to post here/);
  assert.equal(workspace.draft('chan-general'), 'Keep this draft');
});
