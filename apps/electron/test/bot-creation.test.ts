import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  BOT_CREATION_REQUEST_MAX, botCreationPrompt, draftFromProposal, parseBotProposal, prepareBotCreationChat,
  type BotCreationProposal,
} from '../src/renderer/bridge/botCreation';
import { botCreateRequest, BOT_PERSONA_MAX_BYTES } from '../src/renderer/bridge/botForm';
import { BotCreate } from '../src/renderer/components/Bots';
import { BotProposalAction } from '../src/renderer/components/BotProposalCard';
import { MarkdownContent } from '../src/renderer/components/MarkdownContent';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { botCreationCatalog } from '../src/renderer/i18n/botCreationCatalog';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const proposal: BotCreationProposal = {
  schemaVersion: 1, name: '写作伙伴', description: '修改文章，保留作者的语气。',
  persona: '你是写作伙伴。\n先了解读者和写作目的，保留作者的语气，说明主要修改。',
};
const source = JSON.stringify(proposal);
const fence = `\`\`\`lingxi-bot\n${source}\n\`\`\``;
const target = { projectPath: '/fixture/global-chat', sessionId: 'new-setup-chat' };

test('a Bot proposal preserves its persona and creates only text fields with native access defaults', () => {
  const parsed = parseBotProposal(JSON.stringify({ ...proposal, name: ' 写作伙伴 ', description: ' 简介 ' }));
  assert.deepEqual(parsed, { ...proposal, name: '写作伙伴', description: '简介' });
  const draft = draftFromProposal(parsed!);
  assert.equal(draft.persona, proposal.persona);
  assert.equal(draft.permissionMode, 'default');
  assert.equal(draft.model, '');
  assert.deepEqual(draft.workspaces, []);
  assert.deepEqual(draft.acceptFrom, []);
  assert.equal(draft.remote, false);
  assert.deepEqual(botCreateRequest(draft), {
    name: '写作伙伴', description: '简介', persona: proposal.persona, permissionMode: 'default',
  });
});

test('malformed, unsupported and access-bearing proposals stay ordinary text', () => {
  const invalid = [
    '', source.slice(0, -1), 'null', '[]', '{}', source.repeat(2),
    ...[
      { schemaVersion: 2 }, { name: '' }, { name: 'a'.repeat(81) }, { name: 'bad\nname' },
      { description: 'a'.repeat(501) }, { description: null }, { persona: ' ' }, { persona: 'bad\0text' },
      { persona: '中'.repeat(Math.floor(BOT_PERSONA_MAX_BYTES / 3) + 1) },
      { permissionMode: 'bypassPermissions' }, { model: 'provider/model' }, { workspaces: [{ path: '/private', mode: 'write' }] },
      { remote: true }, { acceptFrom: ['other-bot'] }, { unexpected: {} },
    ].map(change => JSON.stringify({ ...proposal, ...change })),
  ];
  for (const value of invalid) assert.equal(parseBotProposal(value), undefined, value.slice(0, 140));
  assert.ok(parseBotProposal(JSON.stringify({ ...proposal, persona: 'a'.repeat(BOT_PERSONA_MAX_BYTES) })));
});

test('creating a setup chat targets a new conversation and offers a draft without sending it', async () => {
  const drafts: unknown[] = [];
  let sessions = 0;
  assert.deepEqual(await prepareBotCreationChat('  帮我创建写作伙伴  ', {
    newSession: async () => { sessions++; return target; },
    offerLinkDraft: draft => drafts.push(draft),
  }, () => true), target);
  assert.equal(sessions, 1);
  assert.deepEqual(drafts, [{ sessionId: target.sessionId, text: '/create-bot 帮我创建写作伙伴', requestId: `create-bot:${target.sessionId}` }]);
  assert.equal(botCreationPrompt('Keep !`literal text`'), '/create-bot Keep !`literal text`');
});

test('invalid requests and canceled or failed navigation never offer a draft', async () => {
  let calls = 0;
  const navigation = { newSession: async () => { calls++; return target; }, offerLinkDraft: () => assert.fail('unexpected draft') };
  for (const request of ['', ' \n ', 'a'.repeat(BOT_CREATION_REQUEST_MAX + 1)]) {
    await assert.rejects(prepareBotCreationChat(request, navigation, () => true));
  }
  assert.equal(calls, 0);
  assert.equal(await prepareBotCreationChat('writer', navigation, () => false), undefined);
  assert.equal(calls, 0);
  let current = true;
  assert.equal(await prepareBotCreationChat('writer', {
    ...navigation, newSession: async () => { current = false; return target; },
  }, () => current), undefined);
  assert.equal(await prepareBotCreationChat('writer', { ...navigation, newSession: async () => undefined }, () => true), undefined);
  await assert.rejects(prepareBotCreationChat('writer', { ...navigation, newSession: async () => { throw new Error('offline'); } }, () => true), /offline/);
});

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

function markdown(text: string, enabled: boolean, review = true): string {
  return render(React.createElement(BotProposalAction.Provider, { value: review ? () => {} : undefined },
    React.createElement(MarkdownContent, { text, botProposals: enabled })));
}

test('only opted-in complete valid assistant fences render the Bot preview action', () => {
  const html = markdown(fence, true);
  assert.match(html, /class="bot-proposal"/);
  assert.match(html, /写作伙伴/);
  assert.match(html, /Review &amp; create/);
  assert.doesNotMatch(html, /class="code-card/);
  for (const text of [fence.slice(0, -3), fence.replace('lingxi-bot', 'json'), fence.replace('"schemaVersion":1', '"schemaVersion":2')]) {
    assert.doesNotMatch(markdown(text, true), /class="bot-proposal"/);
  }
  assert.doesNotMatch(markdown(fence, false), /class="bot-proposal"/);
  assert.doesNotMatch(markdown(fence, true, false), /Review &amp; create/);
  assert.match(markdown(fence, true, false), /Set up manually/);
  const escaped = markdown(fence.replace('写作伙伴', '<img src=x onerror=alert(1)>'), true);
  assert.match(escaped, /&lt;img/);
  assert.doesNotMatch(escaped, /<img/);
});

test('new Bot setup offers AI assistance and a manual path; a proposal opens an editable native form', () => {
  const props = {
    api: { create: async () => { throw new Error('rendering must not create'); } }, models: { references: [], details: [] },
    pickFolder: async () => null, onCancel: () => {}, onCreated: () => {}, onAssist: async () => {},
  };
  const intro = render(React.createElement(BotCreate, props));
  assert.match(intro, /Continue in chat/);
  assert.match(intro, /Set up manually/);
  assert.match(intro, /type="submit"[^>]*disabled=""/);
  assert.doesNotMatch(intro, /bots-persona/);
  const form = render(React.createElement(BotCreate, { ...props, initialProposal: proposal }));
  assert.match(form, /Review your Bot/);
  assert.match(form, /value="写作伙伴"/);
  assert.match(form, /你是写作伙伴。/);
  assert.match(form, /value="default" selected=""/);
  assert.match(form, />Create Bot</);
  assert.doesNotMatch(form, /bot-create-mode/);
});

test('every Bot setup message is translated for each supported interface language', () => {
  for (const [key, messages] of Object.entries(botCreationCatalog)) {
    for (const locale of ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko']) assert.ok(messages[locale]?.trim(), `${key}: ${locale}`);
  }
});
