import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { buildDeepLink, parseDeepLink } from '../../../packages/deep-links/index';
import { parseChatRoute } from '../src/app/router';
import { agentShareLink, SHARED_PROMPT_LIMIT } from '../src/chat/agent/share';
import { OpenInAppBanner } from '../src/chat/OpenInAppBanner';
import { withViaLink } from '../src/pages/OpenLinkPage';
import { appLinkForChat, openLinkTarget } from '../src/pages/openLinkTarget';

const web = (link: string) => openLinkTarget(parseDeepLink(link));

test('a shared conversation link continues at the same conversation in the Chat workspace', () => {
  assert.equal(web('https://lingxios.net/open/team/chat?team=design-team&conversation=c_1'), '/chat/team/design-team/c_1');
  assert.equal(web('https://lingxios.net/open/team/chat?team=f_abcdef&conversation=friend_chat'), '/chat/team/f_abcdef/friend_chat');
  assert.equal(web('https://lingxios.net/open/team/bot?team=design-team&bot=Helper_01'), '/chat/bot/design-team/Helper_01', 'ids keep their case');
  assert.equal(web('https://lingxios.net/open/friend/chat?uid=UidB'), '/chat/friend/UidB');
  for (const path of ['/chat/team/design-team/c_1', '/chat/bot/design-team/Helper_01', '/chat/friend/UidB']) assert.ok(parseChatRoute(path), path);
});

test('a shared prompt becomes a new chat with the prompt in the composer; other links keep the Open LingXi page', () => {
  const link = buildDeepLink({ route: 'new', params: {}, payload: { prompt: '解释 #1 & 50% 的含义?' } });
  assert.equal(web(link), `/chat#prompt=${encodeURIComponent('解释 #1 & 50% 的含义?')}`);
  assert.equal(web('https://lingxios.net/open/new'), '/chat');
  assert.equal(web(`https://lingxios.net/open/team/join?code=${'A'.repeat(32)}`), null);
  assert.equal(web('https://lingxios.net/open/team/inbox'), null);
  assert.equal(web(`https://lingxios.net/open/friend/add?code=${'B'.repeat(32)}`), null);
  assert.equal(withViaLink('/chat/team/t/c'), '/chat/team/t/c?via=link');
  assert.equal(withViaLink('/chat#prompt=hi%23'), '/chat?via=link#prompt=hi%23', 'the marker goes before the fragment');
});

test('the workspace offers the app at the same place, and nothing for places the app does not have', () => {
  assert.equal(appLinkForChat({ kind: 'team', teamId: 'design-team', conversationId: 'c_1' }), 'lingxi://team/chat?conversation=c_1&team=design-team');
  assert.equal(appLinkForChat({ kind: 'bot', teamId: 'design-team', botId: 'helper' }), 'lingxi://team/bot?bot=helper&team=design-team');
  assert.equal(appLinkForChat({ kind: 'friend', uid: 'UidB' }), 'lingxi://friend/chat?uid=UidB');
  assert.equal(appLinkForChat({ kind: 'home' }), 'lingxi://new');
  assert.equal(appLinkForChat({ kind: 'agent', id: 'x' }), null);
  assert.equal(appLinkForChat({ kind: 'team', teamId: 'bad:id', conversationId: 'c' }), null, 'an id a link cannot carry is no link');
  const banner = renderToStaticMarkup(createElement(OpenInAppBanner, { target: { kind: 'friend', uid: 'UidB' }, locale: 'zh', onClose() {} }));
  assert.match(banner, /href="lingxi:\/\/friend\/chat\?uid=UidB"/u);
  assert.match(banner, /在灵犀 App 中打开/u);
  assert.equal(renderToStaticMarkup(createElement(OpenInAppBanner, { target: { kind: 'agent', id: 'x' }, locale: 'en', onClose() {} })), '');
});

test('an Agent chat is shared as its first question only, cut to a link-sized prompt', () => {
  const at = 1;
  assert.equal(agentShareLink({ messages: [] }), null);
  const link = agentShareLink({ messages: [
    { id: 'a', role: 'user', content: '  What is a monad?  ', at },
    { id: 'b', role: 'assistant', content: 'secret answer', at },
    { id: 'c', role: 'user', content: 'second', at },
  ] as never })!;
  const target = parseDeepLink(link);
  assert.equal(target.route, 'new');
  assert.deepEqual(target.payload, { prompt: 'What is a monad?' });
  assert.doesNotMatch(link, /secret|second/u);
  const long = parseDeepLink(agentShareLink({ messages: [{ id: 'a', role: 'user', content: '字'.repeat(5000), at }] as never })!);
  assert.equal([...(long.payload!.prompt as string)].length, SHARED_PROMPT_LIMIT);
});
