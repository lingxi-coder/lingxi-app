import assert from 'node:assert/strict';
import test from 'node:test';
import { chatRoute, isRemoteRoute, normalizePath, parseChatRoute, parseRemoteBotRoute, parseRemoteRoute, remoteBotRoute, remoteSessionRoute, resolveRoute, securityZone } from '../src/app/router';
import { docPageIds } from '../src/app/docRoutes';
import { docPages } from '../src/data/docs';

test('the lightweight documentation routes cover exactly the handbook pages', () => {
  const ids = docPages.map((page) => page.id);
  assert.equal(new Set(docPageIds).size, docPageIds.length, 'duplicate documentation routes');
  assert.deepEqual([...docPageIds].sort(), ids.sort(), 'update docRoutes.ts when adding or removing handbook pages');
  for (const id of docPageIds) {
    assert.equal(resolveRoute(`/docs/${id}`), `/docs/${id}`);
  }
});

test('normalizes duplicate slashes, query strings, casing, and trailing slashes', () => {
  assert.equal(normalizePath('CONSOLE//API-KEYS/?tab=all'), '/console/api-keys');
  assert.equal(normalizePath('/docs#quick-start'), '/docs');
});

test('resolves known routes and falls back to the public homepage', () => {
  assert.equal(resolveRoute('/console/usage'), '/console/usage');
  assert.equal(resolveRoute('/not-a-route'), '/');
  assert.equal(resolveRoute('/DOCS/LLM-CLIENT/?from=home'), '/docs/llm-client');
  assert.equal(resolveRoute('/docs/mobile-ios'), '/docs/mobile-ios');
  assert.equal(resolveRoute('/docs/no-such-sdk'), '/');
});


test('deep link route preserves case-sensitive resource identity', () => {
  assert.equal(resolveRoute('/open/session/AbC?workspace=project.X#payload=ignored'), '/open/session/AbC');
});

test('remote routes keep the case of host and session ids and reject other shapes', () => {
  assert.equal(resolveRoute('/remote'), '/remote');
  assert.equal(resolveRoute('/Remote/?from=push'), '/remote');
  assert.equal(resolveRoute('/remote/AbC_-xYz0123456789AAAA/sess%3AAb12?x=1#frag'), '/remote/AbC_-xYz0123456789AAAA/sess%3AAb12');
  assert.equal(resolveRoute('/remote/host/session/'), '/remote/host/session');
  assert.equal(resolveRoute('/remote/host'), '/', 'a host without a session is not a page');
  assert.equal(resolveRoute('/remote/host/session/extra'), '/');
  assert.equal(resolveRoute('/remote//session'), '/');
  assert.equal(resolveRoute('/login/remote'), '/login/remote');
});

test('remote session routes round-trip ids that need escaping', () => {
  const route = remoteSessionRoute('AbC_-xYz0123456789AAAA', 'sess:a/b 1');
  assert.equal(route, '/remote/AbC_-xYz0123456789AAAA/sess%3Aa%2Fb%201');
  assert.deepEqual(parseRemoteRoute(route), { hostId: 'AbC_-xYz0123456789AAAA', sessionId: 'sess:a/b 1' });
  assert.equal(resolveRoute(route), route);
  assert.equal(parseRemoteRoute('/remote/%E0%A4%A/x'), null, 'malformed escapes are not a route');
});

test('security zones separate the strict remote documents from the rest of the site', () => {
  assert.ok(isRemoteRoute('/remote') && isRemoteRoute('/remote/h/s'));
  assert.ok(!isRemoteRoute('/remote-control') && !isRemoteRoute('/login/remote') && !isRemoteRoute('/'));
  assert.equal(securityZone('/remote'), 'remote');
  assert.equal(securityZone('/remote/h/s'), 'remote');
  assert.equal(securityZone('/login/remote'), 'sign-in');
  assert.equal(securityZone('/docs'), 'site');
  assert.equal(securityZone('/admin/models'), 'site');
});

test('a Bot route names a computer and one of its Bots, and keeps the case and escapes of both ids', () => {
  const route = remoteBotRoute('AbC_-xYz0123456789AAAA', 'bot:a/b 1');
  assert.equal(route, '/remote/AbC_-xYz0123456789AAAA/bots/bot%3Aa%2Fb%201');
  assert.deepEqual(parseRemoteBotRoute(route), { hostId: 'AbC_-xYz0123456789AAAA', botId: 'bot:a/b 1' });
  assert.equal(parseRemoteRoute(route), null, 'it is not a session route, which has two segments after /remote');
  assert.equal(resolveRoute(route), route);
  assert.equal(resolveRoute(`${route}/?from=push#frag`), route, 'a trailing slash, a query and a fragment are not part of it');
  assert.equal(resolveRoute('/remote/Host_A/bots/BoT-Id'), '/remote/Host_A/bots/BoT-Id');
  assert.deepEqual(parseRemoteBotRoute('/remote/Host_A/bots/BoT-Id?x=1#y'), { hostId: 'Host_A', botId: 'BoT-Id' });
});

test('a session whose id is "bots" is still a session, and a computer whose id is "bots" still has Bots', () => {
  const session = remoteSessionRoute('h', 'bots');
  assert.deepEqual(parseRemoteRoute(session), { hostId: 'h', sessionId: 'bots' });
  assert.equal(parseRemoteBotRoute(session), null);
  assert.equal(resolveRoute('/remote/h/bots'), '/remote/h/bots');
  assert.equal(resolveRoute('/remote/h/bots/'), '/remote/h/bots', 'with a slash it is that session still, as every session route is');
  assert.deepEqual(parseRemoteBotRoute('/remote/bots/bots/x'), { hostId: 'bots', botId: 'x' });
  assert.equal(parseRemoteRoute('/remote/bots/bots/x'), null);
});

test('only a computer, "bots" and a Bot make a Bot route; anything else under a computer is not a page', () => {
  for (const path of [
    '/remote/h/bots/b/extra',
    '/remote//bots/b',
    '/remote/h/bots//b',
    '/remote/h/Bots/b',
    '/remote/h/BOTS/b',
    '/remote/h/bot/b',
    '/remote/h/bots/%E0%A4%A',
    '/remote/%E0%A4%A/bots/b',
  ]) {
    assert.equal(parseRemoteBotRoute(path), null, path);
    assert.equal(resolveRoute(path), '/', path);
  }
});

test('the page of a Bot is a remote page: it gets the strict document policy', () => {
  const route = remoteBotRoute('h', 'b');
  assert.ok(isRemoteRoute(route));
  assert.equal(securityZone(route), 'remote');
  assert.equal(securityZone(resolveRoute('/remote/h/bots/b')), 'remote');
});

test('chat routes keep id case, round-trip, and reject other shapes', () => {
  assert.deepEqual(parseChatRoute('/chat'), { kind: 'home' });
  assert.deepEqual(parseChatRoute('/chat/?via=link'), { kind: 'home' });
  assert.deepEqual(parseChatRoute('/chat/settings/providers'), { kind: 'providers' });
  assert.deepEqual(parseChatRoute('/chat/team/T_Ab1/Conv_X9'), { kind: 'team', teamId: 'T_Ab1', conversationId: 'Conv_X9' });
  assert.deepEqual(parseChatRoute('/chat/friend/UidAbC'), { kind: 'friend', uid: 'UidAbC' });
  assert.deepEqual(parseChatRoute('/chat/bot/f_abc/Bot1'), { kind: 'bot', teamId: 'f_abc', botId: 'Bot1' });
  assert.deepEqual(parseChatRoute('/chat/agent/0F2a-11'), { kind: 'agent', id: '0F2a-11' });
  for (const bad of ['/chat/team/onlyone', '/chat/friend/a/b', '/chat/bot/a', '/chat/agent', '/chat/unknown/x', '/chat/team/a%2Fb/c', '/chat/team/%E0%A4%A/c', '/chats']) {
    assert.equal(parseChatRoute(bad), null, bad);
  }
  for (const target of [{ kind: 'home' }, { kind: 'providers' }, { kind: 'team', teamId: 'T', conversationId: 'c:1' }, { kind: 'friend', uid: 'U' }, { kind: 'bot', teamId: 'T', botId: 'B' }, { kind: 'agent', id: 'A' }] as const) {
    assert.deepEqual(parseChatRoute(chatRoute(target)), target);
    assert.equal(resolveRoute(chatRoute(target)), chatRoute(target));
  }
  assert.equal(resolveRoute('/CHAT'), '/chat');
  assert.equal(resolveRoute('/chat/team/a'), '/chat', 'a malformed chat path opens the workspace home');
  for (const other of ['/open/friend/add?code=AbC', '/open/friend/requests', '/open/friend/chat', '/docs/agent/x', '/inbox/team/a/b']) {
    assert.equal(parseChatRoute(other), null, `${other} is not a chat path`);
  }
  assert.equal(resolveRoute('/open/friend/add?code=AbC'), '/open/friend/add', 'a friend invite stays an open-link page');
  assert.equal(resolveRoute('/open/friend/requests'), '/open/friend/requests');
});

test('the chat workspace shares one security zone with Inbox, Teams, Friends and Services', () => {
  for (const route of ['/chat', '/chat/team/T/C', '/inbox', '/teams', '/friends/add', '/services/s_abcdef']) assert.equal(securityZone(route), 'workspace', route);
  assert.equal(securityZone('/chatter'), 'site');
});
