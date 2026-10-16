import assert from 'node:assert/strict';
import test from 'node:test';
import { acknowledgeSidePrompt, newSideChat, reduceSideChat, sideCommandInput, sideCreationInput } from '../src/renderer/bridge/sideChatState';

test('/side parsing matches only a full invocation and preserves multiline initial input', () => {
  assert.equal(sideCreationInput('/side'), undefined);
  assert.equal(sideCreationInput('/side first\nsecond'), 'first\nsecond');
  assert.equal(sideCreationInput('/sideways hello'), null);
  assert.equal(sideCreationInput('explain /side'), null);
});
test('side state starts empty and parent restored context never populates its timeline', () => {
  const side = newSideChat('child', 'parent');
  const next = reduceSideChat(side, { type: 'session_resumed', messages: [{ role: 'user', content: 'private parent' }] } as any);
  assert.equal(next, side);
  assert.deepEqual(next.conversation.items, []);
});
test('ack clears only matching draft and appends the child prompt once at admission', () => {
  const side = { ...newSideChat('child', 'parent'), draft: 'question' };
  const next = acknowledgeSidePrompt(side, 'question');
  assert.equal(next.draft, '');
  assert.equal(next.conversation.items.length, 1);
  assert.equal(acknowledgeSidePrompt({ ...side, draft: 'new typing' }, 'question').draft, 'new typing');
  assert.deepEqual(side.conversation.items, []);
});
test('side events route only to the targeted RAM conversation', () => {
  const parent = newSideChat('parent', 'host');
  const child = reduceSideChat(newSideChat('child', 'host'), { type: 'text_delta', text: 'side answer' } as any);
  assert.equal(parent.conversation.items.length, 0);
  assert.equal(child.conversation.items.length, 1);
  assert.equal(child.conversation.sessionKey, 'side:child');
});
test('child slash input is sent to runtime admission rather than local parent commands', () => {
  assert.deepEqual(sideCommandInput('/side should fail'), { type: 'run_slash_command', raw: '/side should fail', images: [] });
  assert.deepEqual(sideCommandInput('/model ready-model'), { type: 'run_slash_command', raw: '/model ready-model', images: [] });
  assert.deepEqual(sideCommandInput('normal question'), { type: 'send_prompt', text: 'normal question', images: [] });
  const images = [{ media_type: 'image/png', base64: 'AQID' }];
  assert.deepEqual(sideCommandInput('/review image', images), { type: 'run_slash_command', raw: '/review image', images });
  assert.deepEqual(sideCommandInput('describe this', images), { type: 'send_prompt', text: 'describe this', images });
  const side = { ...newSideChat('child', 'parent'), draft: '/side should fail' };
  assert.equal(side.draft, '/side should fail');
});
test('RAM snapshot replays only new side user messages with stable image presentation', () => {
  const side = newSideChat('child', 'parent');
  const replayed = reduceSideChat(side, { type: 'message_complete', message: { role: 'user', blocks: [{ type: 'text', text: 'side only' }], images: [{ media_type: 'image/png', url: 'data:image/png;base64,aGVsbG8=' }] } });
  assert.equal(replayed.conversation.items.length, 1);
  assert.equal(replayed.conversation.items[0].type, 'narration');
  assert.equal((replayed.conversation.items[0] as any).role, 'user');
  assert.equal((replayed.conversation.items[0] as any).text, 'side only');
});
test('RAM replay restores pending side permissions and resolution removes only that request', () => {
  const side = newSideChat('child', 'parent');
  const request = { request_id: 7, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{"command":"pwd"}', default_allow: false }, suppress_always_allow_rule: true } as const;
  const pending = reduceSideChat(side, { type: 'side_chat_permission', side_id: 'child', request });
  assert.equal(pending.permissions[0]?.request_id, 7);
  assert.equal(reduceSideChat(side, { type: 'side_chat_permission', side_id: 'other', request }), side);
  assert.equal(reduceSideChat(pending, { type: 'permission_request_resolved', request_id: 7, resolution: 'approved' } as any).permissions.length, 0);
});

test('/side creation retains selected rich-file mentions without matching /sideways', () => {
  assert.equal(sideCreationInput('/side explain ', ['src/app.ts', 'docs/my notes.md']), '@src/app.ts @"docs/my notes.md"\n\nexplain');
  assert.equal(sideCreationInput('/side', ['src/app.ts']), '@src/app.ts');
  assert.equal(sideCreationInput('/sideways explain', ['src/app.ts']), null);
});
