import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Stage } from '../src/renderer/components/Stage';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { RunItem, ToolRunItem } from '../src/renderer/model/runItem';

(globalThis as { React?: typeof React }).React = React;
function render(props: React.ComponentProps<typeof Stage>): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(Stage, { sessionKey: 'session', ...props })));
}
function anchorTag(html: string, uuid: string): string {
  const tags = html.match(new RegExp(`<[^>]+data-message-uuid="${uuid}"[^>]*>`, 'g')) ?? [];
  assert.equal(tags.length, 1, `expected one DOM anchor for ${uuid}`);
  return tags[0]!;
}
function anchorContents(html: string, uuid: string): string {
  const tag = anchorTag(html, uuid);
  return html.slice(html.indexOf(tag) + tag.length);
}
const tool: ToolRunItem = { type: 'tool', id: 'read', tool: 'Read', status: 'done', view: { verb: 'read', label: 'Read', title: 'Read file' } };

test('DOM transcript rows retain UUIDs and highlights on spacing wrappers without altering content', () => {
  const items: RunItem[] = [
    { type: 'narration', id: 'user', role: 'user', text: 'Build an app' },
    { type: 'narration', id: 'assistant', role: 'assistant', text: 'Building it' },
    { ...tool, id: 'publish', tool: 'LocalApp', nativeOutput: { action: 'publish', id: 'a12345678901234567890', name: 'Garden', revision: 1 } },
    { type: 'mod-log', id: 'log', plugin: 'test', text: 'Ready' },
    { type: 'command', id: 'command', name: '/status', output: 'Ready', isError: false },
    { type: 'visualization', id: 'visual', status: 'pending' },
    { type: 'compaction', id: 'compact', status: 'complete' },
  ].map(item => ({ ...item, transcriptUuid: item.id })) as RunItem[];
  const html = render({ liveItems: items, messageTarget: 'assistant' });
  for (const item of items) {
    const tag = anchorTag(html, item.id);
    assert.match(tag, /class="transcript-row"/);
    assert.match(tag, new RegExp(`data-row-type="${item.type === 'tool' ? 'tool-group' : item.type}"`));
    assert.match(anchorContents(html, item.id), /^<div class="transcript-run-item(?: [^"]*)?"/);
  }
  const highlighted = anchorTag(html, 'assistant');
  assert.match(highlighted, /outline:2px solid/);
  assert.match(highlighted, /outline-offset:4px/);
  assert.match(anchorContents(html, 'assistant'), /^<div[^>]*style="[^"]*display:flex[^"]*width:100%/);
  assert.match(html, /class="desktop-stage-feed"[^>]*><div class="transcript-row" data-row-type="narration"[^>]*><div class="transcript-run-item transcript-user-message"/);
  assert.equal((html.match(/class="local-app-chat-card"/g) ?? []).length, 1);
});

test('grouped tools keep every durable alias on the direct spacing wrapper', () => {
  const html = render({ liveItems: [
    { ...tool, transcriptUuid: 'request', transcriptUuids: ['result'] },
    { ...tool, id: 'second', transcriptUuid: 'second-request', transcriptUuids: ['second-result'] },
  ], messageTarget: 'second-result' });
  const tag = anchorTag(html, 'second-result');
  assert.match(tag, /class="transcript-row"/);
  assert.match(tag, /data-row-type="tool-group"/);
  assert.match(tag, /data-message-uuids="request result second-request second-result"/);
  assert.match(tag, /outline:2px solid/);
  assert.match(html, /class="desktop-stage-feed"[^>]*><div class="transcript-row"/);
});

test('custom plan and file-summary anchors preserve their own component styles', () => {
  const plan: RunItem = { type: 'narration', id: 'plan', role: 'assistant', text: 'Plan', transcriptUuid: 'plan-message' };
  const planHtml = render({ liveItems: [plan], submittedPlans: [{ id: 'plan', content: 'Build the garden', status: 'approved' }], messageTarget: 'plan-message' });
  const planTag = anchorTag(planHtml, 'plan-message');
  assert.match(planTag, /class="transcript-row"/);
  assert.match(planTag, /outline:2px solid/);
  assert.match(anchorContents(planHtml, 'plan-message'), /^<div style="[^"]*position:relative[^"]*border-radius:14px[^"]*padding:16px 18px/);
  const filesHtml = render({ liveItems: [{ type: 'meta', id: 'meta', dur: '1s', tokens: '2', transcriptUuid: 'files-message', files: [{ path: 'app.ts', additions: 1, removals: 0, diffs: [] }] }], messageTarget: 'files-message' });
  const filesTag = anchorTag(filesHtml, 'files-message');
  assert.match(filesTag, /class="transcript-row"/);
  assert.match(filesTag, /data-row-type="meta"/);
  assert.match(filesTag, /outline:2px solid/);
  assert.match(anchorContents(filesHtml, 'files-message'), /^<section class="turn-file-summary"[^>]*style="--files-border:/);
});

test('retry and agent components participate in the shared feed spacing contract', () => {
  const retryHtml = render({ liveItems: [{ type: 'thinking', id: 'thinking', text: 'Working', done: false, streamed: true, transcriptUuid: 'retry-message' }, tool], running: true, apiRetry: { message: 'Backoff', attempt: 1, max_retries: 3, delay_ms: 2000 } });
  assert.match(anchorTag(retryHtml, 'retry-message'), /class="transcript-row" data-row-type="thinking"/);
  assert.match(anchorContents(retryHtml, 'retry-message'), /^<div class="transcript-run-item transcript-api-retry"/);
  const agentsHtml = render({ agents: [{ agent_id: 'worker', name: 'Worker', status: 'running', agent_type: 'explorer' }] });
  assert.match(agentsHtml, /class="desktop-stage-feed"[^>]*><div class="transcript-row" data-row-type="agents"><div class="transcript-agents"/);
});

test('hidden rows do not leave empty anchor wrappers between visible siblings', () => {
  const html = render({ liveItems: [
    { ...tool, transcriptUuid: 'tool-message' },
    { type: 'meta', id: 'hidden', dur: '', tokens: '', transcriptUuid: 'hidden-message' },
  ] });
  assert.doesNotMatch(html, /data-message-uuid="hidden-message"/);
  assert.doesNotMatch(html, /<div><\/div>/);
  assert.match(anchorTag(html, 'tool-message'), /class="transcript-row"/);
});

test('a deep link materializes completed reasoning with its anchor on the visible row', () => {
  const html = render({ liveItems: [{ type: 'thinking', id: 'reasoning', text: 'Inspect the app', done: true, transcriptUuid: 'reasoning-message' }], messageTarget: 'reasoning-message' });
  const tag = anchorTag(html, 'reasoning-message');
  assert.match(tag, /class="transcript-row"/);
  assert.match(tag, /data-row-type="narration"/);
  assert.match(tag, /outline:2px solid/);
  assert.match(html, /Inspect the app/);
});
