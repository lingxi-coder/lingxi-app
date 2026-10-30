import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import test from 'node:test';
import { createElement } from 'react';
import { JsxEmit, ModuleKind, ScriptTarget, transpileModule } from 'typescript';
import type { TeamsApi } from '../../../packages/teams';
import type { TeamBotCard } from '../../../packages/teams/bots';
import * as botsModule from '../../../packages/teams/bots';
import * as messagesModule from '../../../packages/teams/messages';
import type { MessageCommand, MessageProjection, MessageStorage, TeamMessage } from '../../../packages/teams/messages';
import { textOf } from './support/render';

const require = createRequire(import.meta.url), { create, act } = require('react-test-renderer');
const botKey = `bot:owner:${'h'.repeat(22)}:${'b'.repeat(22)}`;
const team = { id: 'test-team', name: 'Fixture team', generation: 7, role: 'member' as const, memberCount: 3 };
const members = ['alice', 'owner', 'reader'].map(uid => ({ uid, displayName: uid, generation: 7, role: 'member' as const }));
const question: TeamMessage = { id: 'question-message', seq: 1, author: { kind: 'bot', uid: 'owner', botKey }, kind: 'question', text: 'Choose a value',
  triggerId: 'trigger01', origin: { uid: 'alice', generation: 7 }, question: { id: 'question01', questions: ['Which value?'] }, createdAt: null, changeSeq: 1, changeId: 'change001' };
function loadPanel(storage: MessageStorage): typeof import('../../../packages/teams/MessagesPanel.js').MessagesPanel {
  // Shared packages have no standalone React install. Resolve rendering dependencies
  // through this Web package, as Vite does, without changing application resolution.
  const source = readFileSync(new URL('../../../packages/teams/MessagesPanel.tsx', import.meta.url), 'utf8');
  const compiled = transpileModule(source, { compilerOptions: { module: ModuleKind.CommonJS, target: ScriptTarget.ES2020, jsx: JsxEmit.ReactJSX } });
  const module = { exports: {} as Pick<typeof import('../../../packages/teams/MessagesPanel'), 'MessagesPanel'> };
  function resolve(id: string): unknown {
    switch (id) {
      case './messages.js':
        return messagesModule;
      case './bots.js':
        return botsModule;
      case './message-storage.js':
        return { indexedDbMessageStorage: () => storage };
      default:
        return require(id);
    }
  }
  new Function('require', 'module', 'exports', compiled.outputText)(resolve, module, module.exports);
  return module.exports.MessagesPanel;
}
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(resolve => setTimeout(resolve, 0)); };

for (const uid of ['alice', 'reader']) test(`T3 rendered question grants answer controls only to its original human (${uid})`, async context => {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  let renderer: any;
  context.after(async () => {
    if (renderer) await act(async () => renderer.unmount());
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow); else Reflect.deleteProperty(globalThis, 'window');
  });
  const records = new Map<string, MessageProjection>();
  const storage: MessageStorage = { async update(key, reduce) { const value = reduce(structuredClone(records.get(key))); records.set(key, structuredClone(value)); return structuredClone(value); } };
  const calls: MessageCommand[] = [];
  const conversation = { id: 'test-chat', kind: 'group', title: 'Shared thread', participants: ['user:alice', 'user:reader', botKey], lastSeq: 1, lastChangeSeq: 1, lastReadSeq: 0 };
  const api: TeamsApi = {
    async list() { return { teams: [team] }; }, async detail() { return { team, members, invites: [] }; }, async execute() { return {}; },
    onChanged: () => () => {}, onMessagesChanged: () => () => {},
    async messages<T>(command: MessageCommand): Promise<T> {
      calls.push(command);
      if (command.action === 'bots') return { bots: [{ key: botKey, ownerUid: 'owner', hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture Bot', description: 'Shared helper', revision: 1, use: 'listed', uids: ['alice'], permitted: uid === 'alice', online: false }] } as T;
      if (command.action === 'list') return { conversations: [conversation], next: null } as T;
      if (command.action === 'sync') return { conversation, changes: command.after ? [] : [{ id: question.changeId, seq: 1, messageId: question.id, operation: 'create' }], messages: command.after ? [] : [question], hasMore: false } as T;
      if (command.action === 'botState') return { state: 'running', ownerUid: 'owner', botKey, origin: question.origin, questionId: question.question!.id, leaseUntil: Date.now() + 100000, answered: false } as T;
      return {} as T;
    },
  };
  const Panel = loadPanel(storage);
  await act(async () => { renderer = create(createElement(Panel, { api, uid, team, members, zh: false, onClose() {} })); await settle(); });
  const card = renderer.root.findByProps({ className: 'team-bot-grid' });
  assert.match(textOf(card), /Offline/); assert.match(textOf(card), /owner/);
  const cardButton = card.findByType('button'); assert.equal(Boolean(cardButton.props.disabled), uid !== 'alice');
  assert.match(textOf(renderer.root), /Open Teams in LingXi Desktop/);
  await act(async () => { renderer.root.findByProps({ className: 'team-thread' }).props.onClick(); await settle(); });
  const prompt = renderer.root.findByProps({ className: 'team-bot-question' });
  if (uid === 'alice') {
    const answer = prompt.findByType('textarea'); assert.equal(Boolean(answer.props.disabled), false);
    await act(async () => { answer.props.onChange({ target: { value: '42' } }); });
    await act(async () => { prompt.findByType('form').props.onSubmit({ preventDefault() {} }); await settle(); });
    assert.equal(calls.filter(command => command.action === 'botAnswer').length, 1);
    assert.match(textOf(prompt), /Answer received/);
  } else {
    assert.equal(prompt.findAllByType('textarea').length, 0); assert.equal(prompt.findAllByType('form').length, 0);
    assert.match(textOf(prompt), /Only the original requester/);
  }
  assert.equal(calls.some(command => ['claimBot', 'startBot', 'finishBot', 'botPoll'].includes(command.action)), false);
});

test('T3 owner unbind retains the tombstone revision for rebind and disables Bot conversation affordances', async context => {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  let renderer: any;
  context.after(async () => {
    if (renderer) await act(async () => renderer.unmount());
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow); else Reflect.deleteProperty(globalThis, 'window');
  });
  const records = new Map<string, MessageProjection>();
  const storage: MessageStorage = { async update(key, reduce) { const value = reduce(structuredClone(records.get(key))); records.set(key, structuredClone(value)); return structuredClone(value); } };
  let card: TeamBotCard = { key: botKey, ownerUid: 'owner', hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture Bot', description: '', revision: 7, use: 'all', uids: [], permitted: true, online: true, enabled: true };
  const calls: MessageCommand[] = [], binds: { expectedRevision: number }[] = [];
  const conversation = { id: 'test-chat', kind: 'bot_dm', title: 'Existing Bot thread', participants: ['user:owner', botKey], lastSeq: 0, lastChangeSeq: 0, lastReadSeq: 0 };
  const api: TeamsApi = {
    async list() { return { teams: [team] }; }, async detail() { return { team, members, invites: [] }; }, async execute() { return {}; },
    onChanged: () => () => {}, onMessagesChanged: () => () => {},
    async localBots() { return [{ botId: card.botId, name: card.name, description: '' }]; },
    async bindLocalBot(input) { binds.push(input); card = { ...card, revision: 9, enabled: true, permitted: true }; return { botKey, revision: 9 }; },
    async messages<T>(command: MessageCommand): Promise<T> {
      calls.push(command);
      if (command.action === 'bots') return { bots: [{ ...card }] } as T;
      if (command.action === 'list') return { conversations: [conversation], next: null } as T;
      if (command.action === 'sync') return { conversation, changes: [], messages: [], hasMore: false } as T;
      if (command.action === 'unbindBot') { assert.equal(command.expectedRevision, 7); card = { ...card, revision: 8, enabled: false, permitted: false }; }
      return {} as T;
    },
  };
  const Panel = loadPanel(storage);
  await act(async () => { renderer = create(createElement(Panel, { api, uid: 'owner', team, members, zh: false, onClose() {} })); await settle(); });
  const button = (label: string) => renderer.root.findAllByType('button').find((node: any) => textOf(node) === label);
  await act(async () => { renderer.root.findByProps({ className: 'team-thread' }).props.onClick(); await settle(); });
  await act(async () => { button('Unbind').props.onClick(); });
  await act(async () => { renderer.root.findByProps({ className: 'teams-modal' }).findAllByType('button').find((node: any) => textOf(node) === 'Unbind').props.onClick(); await settle(); });
  const grid = renderer.root.findByProps({ className: 'team-bot-grid' });
  assert.match(textOf(grid), /Disabled/); assert.doesNotMatch(textOf(grid), /Online|All team members may use/); assert.equal(button('Unbind'), undefined);
  const disabled = grid.findByType('button'); assert.equal(disabled.props.disabled, true);
  assert.equal(renderer.root.findByProps({ className: 'team-composer' }).findByType('textarea').props.disabled, true);
  await act(async () => { disabled.props.onClick(); await settle(); });
  assert.equal(calls.some(command => command.action === 'create' || command.action === 'mutate'), false);
  await act(async () => { renderer.root.findByType('aside').findByType('select').props.onChange({ target: { value: 'group' } }); });
  const botParties = renderer.root.findByType('aside').findAllByType('fieldset').find((node: any) => textOf(node.findByType('legend')) === 'Participating Bots');
  assert.equal(botParties.findAllByType('input').length, 0);
  await act(async () => { button('Load local Bots').props.onClick(); await settle(); });
  const directory = renderer.root.findByProps({ className: 'team-bots' });
  await act(async () => { directory.findAllByType('select')[0].props.onChange({ target: { value: card.botId } }); });
  assert.ok(button('Rebind Bot'));
  await act(async () => { directory.findByType('form').props.onSubmit({ preventDefault() {} }); await settle(); });
  assert.equal(binds.length, 1); assert.equal(binds[0]!.expectedRevision, 8);
  assert.equal(Boolean(button('Message Bot').props.disabled), false);
});

test('T3 two owner windows keep the audience draft revision after refreshed cards and reject stale regrant', async context => {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  const renderers: any[] = [];
  context.after(async () => {
    for (const renderer of renderers) await act(async () => renderer.unmount());
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow); else Reflect.deleteProperty(globalThis, 'window');
  });
  let card: TeamBotCard = { key: botKey, ownerUid: 'owner', hostId: 'h'.repeat(22), botId: 'b'.repeat(22), name: 'Fixture Bot', description: '', revision: 7, use: 'listed', uids: ['alice'], permitted: true, online: true, enabled: true };
  const revisions: number[] = [], records = new Map<string, MessageProjection>();
  const storage: MessageStorage = { async update(key, reduce) { const value = reduce(structuredClone(records.get(key))); records.set(key, structuredClone(value)); return structuredClone(value); } };
  const api: TeamsApi = {
    async list() { return { teams: [team] }; }, async detail() { return { team, members, invites: [] }; }, async execute() { return {}; },
    onChanged: () => () => {}, onMessagesChanged: () => () => {},
    async localBots() { return [{ botId: card.botId, name: card.name, description: '' }]; },
    async bindLocalBot(input) {
      revisions.push(input.expectedRevision);
      if (input.expectedRevision !== card.revision) throw new Error('BOT_REVISION_CONFLICT');
      card = { ...card, revision: card.revision + 1, use: input.use, uids: [...input.uids] }; return { botKey, revision: card.revision };
    },
    async messages<T>(command: MessageCommand): Promise<T> { return (command.action === 'bots' ? { bots: [structuredClone(card)] } : { conversations: [], next: null }) as T; },
  };
  const Panel = loadPanel(storage), props = { uid: 'owner', team, members, zh: false, onClose() {} };
  await act(async () => { renderers.push(create(createElement(Panel, { ...props, api })), create(createElement(Panel, { ...props, api }))); await settle(); });
  const directory = (renderer: any) => renderer.root.findByProps({ className: 'team-bots' });
  for (const renderer of renderers) {
    await act(async () => { directory(renderer).findAllByType('button').find((node: any) => textOf(node) === 'Load local Bots').props.onClick(); await settle(); });
    await act(async () => { directory(renderer).findAllByType('select')[0].props.onChange({ target: { value: card.botId } }); });
  }
  const aliceInput = (renderer: any) => directory(renderer).findAllByType('label').find((node: any) => textOf(node) === 'alice').findByType('input');
  assert.equal(aliceInput(renderers[0]).props.checked, true);
  await act(async () => { aliceInput(renderers[1]).props.onChange({ target: { checked: false } }); });
  await act(async () => { directory(renderers[1]).findByType('form').props.onSubmit({ preventDefault() {} }); await settle(); });
  assert.equal(card.revision, 8); assert.deepEqual(card.uids, []);
  // A's card refresh sees revision 8, while its loaded permission draft still contains Alice.
  await act(async () => { renderers[0].update(createElement(Panel, { ...props, api: { ...api } })); await settle(); });
  assert.equal(aliceInput(renderers[0]).props.checked, true);
  await act(async () => { directory(renderers[0]).findByType('form').props.onSubmit({ preventDefault() {} }); await settle(); });
  assert.deepEqual(revisions, [7, 7]); assert.equal(card.revision, 8); assert.deepEqual(card.uids, []);
  assert.match(textOf(renderers[0].root.findByProps({ role: 'alert' })), /Bot settings changed/);
  await act(async () => { directory(renderers[0]).findAllByType('button').find((node: any) => textOf(node) === 'Reload current permissions').props.onClick(); });
  assert.equal(aliceInput(renderers[0]).props.checked, false);
  await act(async () => { directory(renderers[0]).findByType('form').props.onSubmit({ preventDefault() {} }); await settle(); });
  assert.deepEqual(revisions, [7, 7, 8]); assert.equal(card.revision, 9); assert.deepEqual(card.uids, []);
});
