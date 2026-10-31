import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement, useState } from 'react';
import * as router from '../src/app/router';
import { loadRemotePage } from './support/loadRemotePage';
import { Mounted } from './support/render';

/**
 * The routing of the remote page, as it is, with its views replaced by ones
 * that show a marker and keep the props the page gave them. What a Bot page
 * shows is `remoteBotsView.test.ts`'s; this is which page the route names, with
 * what, and where the page's own buttons go.
 */

const HOST = 'host-1';
const COMPUTER = { hostId: HOST, name: 'Studio Mac', platform: 'darwin', online: true };

type Props = Record<string, any>;

function fixture() {
  const navigations: string[] = [];
  /** The Bot each chat belongs to, as the client has learned it. */
  const botOfThread = new Map<string, string>();
  const client = { botOfThread: (sessionId: string) => botOfThread.get(sessionId) };
  const seen = { botPage: [] as Props[], botList: [] as Props[], sessionView: [] as Props[], sessionList: [] as Props[], hostList: [] as Props[] };
  const marker = (view: string) => createElement('div', { 'data-view': view });
  const keepState = (view: string, props: Props) => {
    // A view with a draft of its own, as a real one has.
    const [draft, setDraft] = useState('');
    return createElement('input', { 'data-view': view, 'data-id': props['botId'] ?? props['sessionId'], value: draft, onChange: (event: { target: { value: string } }) => setDraft(event.target.value) });
  };
  const modules = {
    '../app/router': { ...router, navigateTo: (route: string) => void navigations.push(route) },
    '../remote/useRemote': {
      getDeviceKeys: () => ({}),
      signOutUser: async () => undefined,
      supportsRemoteCrypto: async () => true,
      useAuthState: () => ({ status: 'signedIn', uid: 'u1', email: 'a@example.test' }),
      useConnectionState: () => ({ phase: 'ready', everReady: true }),
      useHosts: () => ({ status: 'ready', hosts: [COMPUTER] }),
      usePersistentPairing: () => true,
      usePins: () => [],
      useRemoteHost: (_uid: string, _rtdb: unknown, hostId: string | null) => (hostId === null ? null : { connection: {}, client }),
      useRtdb: () => ({}),
    },
    '../remote/Bots': {
      BotList: (props: Props) => (seen.botList.push(props), marker('bot-list')),
      BotPage: (props: Props) => (seen.botPage.push(props), keepState('bot-page', props)),
    },
    '../remote/SessionView': { SessionView: (props: Props) => (seen.sessionView.push(props), keepState('session-view', props)) },
    '../remote/SessionList': { SessionList: (props: Props) => (seen.sessionList.push(props), marker('session-list')) },
    '../remote/HostList': { HostList: (props: Props) => (seen.hostList.push(props), marker('host-list')) },
    '../remote/ConnectionScreen': { ConnectionScreen: () => marker('connection-screen') },
  };
  const RemotePage = loadRemotePage(modules);
  const pageProps = (route: string, extra: Props = {}): Props => ({ locale: 'en', theme: 'light', route, onLocaleChange() {}, onThemeToggle() {}, ...extra });
  return { RemotePage, pageProps, seen, navigations, botOfThread };
}

/** The page is at `window.location`, which the routing component reads. */
async function onPage<T>(body: () => Promise<T>): Promise<T> {
  const original = globalThis.window;
  globalThis.window = { location: { pathname: '/remote', search: '', hash: '', replace() {} } } as unknown as Window & typeof globalThis;
  try {
    return await body();
  } finally {
    globalThis.window = original;
  }
}

const views = (page: Mounted): string[] => page.markup.match(/data-view="[a-z-]+"/gu)?.map((attribute) => attribute.slice(11, -1)) ?? [];

test('the route of a Bot is the page of that Bot on that computer, with what the page needs', async () => {
  const { RemotePage, pageProps, seen } = fixture();
  await onPage(async () => {
    const page = await Mounted.create(createElement(RemotePage, pageProps(router.remoteBotRoute(HOST, 'bot:a/b 1'))));
    try {
      assert.deepEqual(views(page), ['bot-page']);
      const props = seen.botPage.at(-1)!;
      assert.equal(props['botId'], 'bot:a/b 1', 'the id as the Bot has it, not as the address spells it');
      assert.equal(props['hostName'], 'Studio Mac');
      assert.equal(props['locale'], 'en');
      assert.equal(props['connection'].phase, 'ready');
    } finally {
      await page.unmount();
    }
  });
});

test('the page of a Bot goes back to its computer, and to a chat of the Bot', async () => {
  const { RemotePage, pageProps, seen, navigations } = fixture();
  await onPage(async () => {
    const route = router.remoteBotRoute(HOST, 'a');
    const page = await Mounted.create(createElement(RemotePage, pageProps(route)));
    try {
      await page.during(async () => seen.botPage.at(-1)!['onOpenSession']('chat-9'));
      assert.deepEqual(navigations, [router.remoteSessionRoute(HOST, 'chat-9')]);

      await page.during(async () => seen.botPage.at(-1)!['onBack']());
      assert.deepEqual(navigations.slice(1), ['/remote']);
      // The address is the computer's now; the page has not forgotten which computer it is on.
      await page.update(createElement(RemotePage, pageProps('/remote')));
      assert.deepEqual(views(page), ['bot-list', 'session-list']);
    } finally {
      await page.unmount();
    }
  });
});

test('the computer page has the Bots above the sessions, and opens a Bot at its route', async () => {
  const { RemotePage, pageProps, seen, navigations } = fixture();
  await onPage(async () => {
    const page = await Mounted.create(createElement(RemotePage, pageProps('/remote')));
    try {
      assert.deepEqual(views(page), ['host-list']);
      await page.during(async () => seen.hostList.at(-1)!['onConnect'](HOST));
      assert.deepEqual(views(page), ['bot-list', 'session-list']);

      await page.during(async () => seen.botList.at(-1)!['onOpen']('bot:a/b 1'));
      await page.during(async () => seen.sessionList.at(-1)!['onOpen']('s1'));
      assert.deepEqual(navigations, ['/remote/host-1/bots/bot%3Aa%2Fb%201', '/remote/host-1/s1']);
    } finally {
      await page.unmount();
    }
  });
});

test('a session goes back to its Bot when this browser knows which Bot that is, and to the computer when it does not', async () => {
  const { RemotePage, pageProps, seen, navigations, botOfThread } = fixture();
  botOfThread.set('chat-9', 'bot:a/b 1');
  await onPage(async () => {
    const chat = await Mounted.create(createElement(RemotePage, pageProps(router.remoteSessionRoute(HOST, 'chat-9'))));
    try {
      assert.deepEqual(views(chat), ['session-view']);
      await chat.during(async () => seen.sessionView.at(-1)!['onBack']());
      assert.deepEqual(navigations, [router.remoteBotRoute(HOST, 'bot:a/b 1')]);
    } finally {
      await chat.unmount();
    }

    const plain = await Mounted.create(createElement(RemotePage, pageProps(router.remoteSessionRoute(HOST, 'plain'))));
    try {
      await plain.during(async () => seen.sessionView.at(-1)!['onBack']());
      assert.deepEqual(navigations.slice(1), ['/remote']);
      await plain.update(createElement(RemotePage, pageProps('/remote')));
      assert.deepEqual(views(plain), ['bot-list', 'session-list'], 'the computer, not the list of computers');
    } finally {
      await plain.unmount();
    }
  });
});

test('another Bot, or another computer, is a new page; the same one drawn again keeps what it had', async () => {
  const { RemotePage, pageProps } = fixture();
  await onPage(async () => {
    const page = await Mounted.create(createElement(RemotePage, pageProps(router.remoteBotRoute(HOST, 'a'))));
    const draft = (): string => page.markup.match(/value="([^"]*)"/u)?.[1] ?? '';
    try {
      await page.type('a draft for Bot a');
      await page.update(createElement(RemotePage, pageProps(router.remoteBotRoute(HOST, 'a'), { theme: 'dark' })));
      assert.equal(draft(), 'a draft for Bot a', 'a new theme does not make a new page');

      await page.update(createElement(RemotePage, pageProps(router.remoteBotRoute(HOST, 'b'))));
      assert.equal(draft(), '', 'Bot b does not start with what was written for Bot a');
      assert.match(page.markup, /data-id="b"/u);

      await page.type('a draft for Bot b');
      await page.update(createElement(RemotePage, pageProps(router.remoteBotRoute('host-2', 'b'))));
      assert.equal(draft(), '', 'the Bot of that id on another computer is another Bot');

      await page.update(createElement(RemotePage, pageProps(router.remoteSessionRoute(HOST, 'b'))));
      assert.deepEqual(views(page), ['session-view'], 'a session that has the id of a Bot is a session');
    } finally {
      await page.unmount();
    }
  });
});
