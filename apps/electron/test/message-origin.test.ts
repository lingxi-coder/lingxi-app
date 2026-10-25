import { tr } from '../src/renderer/i18n/locale';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { crossSessionMessage } from '../src/renderer/components/messageOrigin.ts';

test('peer envelopes show a sender and preserve multiline Unicode body', () => {
  assert.deepEqual(crossSessionMessage('<cross-session-message from="session-a" from-name="设计 &amp; 开发">\n第一行🙂\n**正文**\n</cross-session-message>'),
    { text: '第一行🙂\n**正文**', label: tr('chat_cross_session_origin_fmt', '设计 & 开发') });
});
test('falls back to address or generic origin', () => {
  assert.equal(crossSessionMessage('<cross-session-message from="alpha">hi</cross-session-message>')?.label, tr('chat_cross_session_origin_fmt', 'alpha'));
  assert.equal(crossSessionMessage('<cross-session-message>hi</cross-session-message>')?.label, tr('chat_cross_session_origin'));
});
test('ordinary text, incomplete envelopes and lookalike tags remain intact', () => {
  for (const text of ['hello', 'Example: <cross-session-message>hi</cross-session-message>', '<cross-session-message>hi', '<cross-session-messages>hi</cross-session-message>']) {
    assert.equal(crossSessionMessage(text), null);
  }
});
