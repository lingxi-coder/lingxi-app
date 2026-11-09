import assert from 'node:assert/strict';
import { createElement } from 'react';
import test from 'node:test';
import { TranscriptView } from '../src/remote/Transcript';
import { TranscriptBuilder } from '../src/remote/sessionModel';
import { messageUIData } from './fixtures/message-ui-data';
import { Mounted } from './support/render';
const element = (mode = 'running', count = 3) => createElement(TranscriptView, { locale: 'en', transcript: messageUIData(mode, count), running: false, deviceName: 'Test', fetchImage: async () => '' });
test('adjacent agents collapse into one stable summary with at most three avatars', async () => {
  const view = await Mounted.create(element());
  try {
    assert.equal(view.texts('.rc-agent-group button').length, 1);
    assert.match(view.texts('.rc-agent-group button')[0]!, /Claude 319 oracle.*Autocompact core.*started working/);
    await view.click(view.texts('.rc-agent-group button')[0]!);
    assert.equal(view.texts('.rc-agent-member summary').length, 3);
    await view.update(element('done', 6));
    assert.equal(view.texts('.rc-agent-member summary').length, 6, 'updates preserve expansion');
    assert.match(view.texts('.rc-agent-group button')[0]!, /\+3.*finished/);
    assert.equal((view.markup.match(/className="rc-agent-avatar"/g) ?? []).length, 9);
  } finally { await view.unmount(); }
});
test('task coordination stays collapsed while running and preserves individual failures', async () => {
  const view = await Mounted.create(element('mixed'));
  try {
    const summary = view.texts('button').find(text => text.includes('Task coordination'))!;
    assert.match(summary, /4 calls.*1 running.*1 unsuccessful/);
    assert.equal(view.texts('.rc-tool').length, 2);
    await view.click(summary);
    assert.equal(view.texts('.rc-tool').length, 6);
    assert.match(view.markup, /rc-danger/);
    assert.match(view.texts('.rc-agent-group button')[0]!, /Report withheld/);
    assert.doesNotMatch(view.texts('.rc-agent-group button')[0]!, /finished/);
  } finally { await view.unmount(); }
});
test('roster refreshes preserve first-seen order and fallback anchors', () => {
  const builder = new TranscriptBuilder(); const agents = messageUIData().agents!;
  builder.apply({ type: 'event', event: { type: 'session_agent_list', session_id: 's', agents: [...agents] } });
  const first = builder.snapshot();
  builder.apply({ type: 'event', event: { type: 'session_agent_list', session_id: 's', agents: [...agents].reverse() } });
  assert.deepEqual(builder.snapshot().agents?.map(agent => agent.agent_id), first.agents?.map(agent => agent.agent_id));
  assert.deepEqual([...builder.snapshot().agentAnchors!], [...first.agentAnchors!]);
});

test('LocalApp gets an application icon and an adjacent, readable summary', async () => {
  const view = await Mounted.create(element());
  try {
    assert.match(view.markup, /data-tool-icon="local-app"/);
    assert.ok(view.texts('.rc-tool-title').some(text => text.startsWith('Local app 全家人的终身学习空间')));
  } finally { await view.unmount(); }
});
