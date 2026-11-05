import { withDesktopLocale } from './support/language';
import { before, test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Stage } from '../src/renderer/components/Stage';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { translate } from '../src/renderer/i18n/locale';
import type { RunItem } from '../src/renderer/model/runItem';
(globalThis as { React?: typeof React }).React = React;

before(async () => {
  // Non-English catalogs load asynchronously; wait for the requested catalog
  // before asserting the Chinese caption rather than its initial English fallback.
  const deadline = Date.now() + 5_000;
  while (translate('chat_cross_session_origin_fmt', 'zh-Hans', 'alpha') !== '来自其他会话 · alpha') {
    assert.ok(Date.now() < deadline, 'Chinese translation catalog did not load');
    await new Promise<void>((resolve) => setTimeout(resolve, 5));
  }
});

function render(item: RunItem) {
  return withDesktopLocale('zh-Hans', () => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) },
    React.createElement(Stage, { liveItems: [item], sessionKey: 'origin-test' }))));
}
test('peer origin is outside and before a collapsed user bubble', () => {
  const html = render({ id: 'peer', type: 'narration', role: 'user',
    text: `<cross-session-message from="alpha">${'消息正文'.repeat(300)}</cross-session-message>` });
  assert.ok(html.indexOf('message-origin-caption') < html.indexOf('user-message-bubble'));
  assert.match(html, /来自其他会话 · alpha/);
  assert.match(html, /消息正文/);
  assert.doesNotMatch(html, /cross-session-message/);
});
test('device origin is above bubble while time and copy stay below', () => {
  const html = render({ id: 'device', type: 'narration', role: 'user', text: 'From phone',
    origin: { kind: 'device', deviceName: 'My iPhone', turnId: 1 }, sentAt: 1 });
  assert.ok(html.indexOf('来自 My iPhone') < html.indexOf('user-message-bubble'));
  assert.doesNotMatch(html, /remote-message-origin/);
});
test('local user messages have no origin caption', () => {
  assert.doesNotMatch(render({ id: 'local', type: 'narration', role: 'user', text: 'Hello' }), /message-origin-caption/);
});
