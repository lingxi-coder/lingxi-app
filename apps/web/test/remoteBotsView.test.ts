import assert from 'node:assert/strict';
import test from 'node:test';
import { Fragment, createElement, type ReactElement } from 'react';
import { RemoteRpcError, type RemotePromptOrigin, type RemoteSeqRecord } from '@lingxi/bridge-client/remote';
import { BotList, BotPage } from '../src/remote/Bots';
import type { RemoteClient, RemoteClientState } from '../src/remote/remoteClient';
import { TranscriptBuilder } from '../src/remote/sessionModel';
import { SessionList } from '../src/remote/SessionList';
import { SessionView } from '../src/remote/SessionView';
import { TranscriptView } from '../src/remote/Transcript';
import type { ConnectionState, ViewerConnection } from '../src/remote/viewerConnection';
import { BotHost, announce, bot, open, quiet, thread } from './support/botHost';
import { Mounted } from './support/render';
import { harness, type Harness } from './support/remoteHarness';
import { drive } from './support/botHost';

// ─────────────────────────────────────────────────────────────────────────────
// A computer with Bots, and the pages that show it
// ─────────────────────────────────────────────────────────────────────────────

interface Computer {
  h: Harness;
  host: BotHost;
  connection: ViewerConnection;
  client: RemoteClient;
  /** Let what is on its way arrive. */
  wait(): Promise<void>;
}

/** A client connected to a scripted computer, past its first answers about sessions and Bots. */
async function computer(setup: (host: BotHost) => void = () => undefined): Promise<Computer> {
  const h = await harness();
  const host = new BotHost(h.host);
  setup(host);
  const { connection, client } = await open(h);
  await drive(h.clock, () => client.getState().sessions !== undefined && client.getState().bots !== undefined, 'the first lists');
  return { h, host, connection, client, wait: () => quiet(h) };
}

/** A client that has been told what its state is and does nothing else. */
const stubClient = (state: RemoteClientState): RemoteClient => ({ getState: () => state, subscribe: () => () => undefined }) as unknown as RemoteClient;

const noop = (): void => undefined;

const BOT_GONE = 'This Bot can’t be reached now. Its Remote Control switch may be off on the computer, or it was deleted.';

function botList(client: RemoteClient, opened: string[] = [], locale: 'en' | 'zh' = 'en'): ReactElement {
  return createElement(BotList, { locale, client, onOpen: (botId: string) => opened.push(botId) });
}

interface PageOptions {
  locale?: 'en' | 'zh';
  connection?: ConnectionState;
  onBack?(): void;
  onOpenSession?(sessionId: string): void;
}

function botPage(client: RemoteClient, connection: ConnectionState, botId: string, options: PageOptions = {}): ReactElement {
  return createElement(BotPage, {
    locale: options.locale ?? 'en',
    client,
    hostName: 'Studio Mac',
    botId,
    connection: options.connection ?? connection,
    onBack: options.onBack ?? noop,
    onOpenSession: options.onOpenSession ?? noop,
  });
}

/** A promise that stays pending until the test lets it go. */
function gate(): { promise: Promise<void>; open(): void } {
  let open!: () => void;
  const promise = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { promise, open };
}

// ─────────────────────────────────────────────────────────────────────────────
// The Bots on the computer's page
// ─────────────────────────────────────────────────────────────────────────────

test('the Bots of a computer are listed with what each is doing, in the order the computer sent them', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a', { status: 'running', waiting_threads: 2, description: 'Plans trips' }), bot('b'), bot('c', { status: 'requires_action', description: '' })];
  });
  const opened: string[] = [];
  const list = await Mounted.create(botList(c.client, opened), c.wait);
  try {
    assert.deepEqual(list.texts('h2'), ['Bots']);
    assert.deepEqual(list.texts('strong'), ['Bot a', 'Bot b', 'Bot c']);
    assert.deepEqual(list.texts('.rc-bot-description'), ['Plans trips', 'b helps'], 'a Bot with no description shows none');
    assert.deepEqual(list.texts('.rc-pending-tag'), ['2 waiting for you'], 'only a Bot with chats waiting says so');
    assert.deepEqual(list.texts('.rc-status'), ['Running', 'Idle', 'Needs you']);
    await list.clickRow('Bot b');
    await list.clickRow('Bot a');
    assert.deepEqual(opened, ['b', 'a']);
  } finally {
    await list.unmount();
  }
});

test('the Bots are in the language of the page', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a', { status: 'running', waiting_threads: 1 })];
  });
  const list = await Mounted.create(botList(c.client, [], 'zh'), c.wait);
  try {
    assert.deepEqual(list.texts('h2'), ['Bot']);
    assert.deepEqual(list.texts('.rc-pending-tag'), ['1 个对话等你回复']);
    assert.deepEqual(list.texts('.rc-status'), ['运行中']);
  } finally {
    await list.unmount();
  }
});

test('a Bot list changes as the computer says its Bots changed', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a')];
  });
  const list = await Mounted.create(botList(c.client), c.wait);
  try {
    assert.deepEqual(list.texts('strong'), ['Bot a']);
    c.host.bots = [bot('b', { status: 'running' }), bot('a', { name: 'Renamed' })];
    await list.during(() => announce(c.h, c.client, c.host));
    assert.deepEqual(list.texts('strong'), ['Bot b', 'Renamed']);
    c.host.bots = [];
    await list.during(() => announce(c.h, c.client, c.host));
    assert.ok(list.empty, 'the last Bot going away takes the section with it');
  } finally {
    await list.unmount();
  }
});

// What is shown of the sessions on a computer page, as it was before Bots: from the markup of the
// `SessionList` that preceded them (checked once against that source, state by state).
const SESSIONS = [
  { session_id: 's1', title: 'Fix bug', project_label: 'proj', model: 'm1', status: 'running', viewers: 0, pending_action: 'permission' },
  { session_id: 's2', title: '', project_label: 'other', status: 'idle', viewers: 0 },
] as const;
const BEFORE = {
  loading: '<section className="rc-panel"><h2>Sessions</h2><p className="rc-note" role="status">Loading…</p><ul className="rc-list"></ul></section>',
  none:
    '<section className="rc-panel"><h2>Sessions</h2><div className="rc-empty"><h3>No shared sessions</h3><p>Open a session in LingXi on the computer and allow remote access for it; it will show up here.</p></div><ul className="rc-list"></ul></section>',
  listed:
    '<section className="rc-panel"><h2>Sessions</h2><ul className="rc-list">' +
    '<li><button type="button" className="rc-row rc-row-button"><span className="rc-row-main"><strong>Fix bug</strong><span className="rc-row-sub">proj · m1</span><span className="rc-pending-tag">Waiting for permission</span></span><span className="rc-status rc-status-running">Running</span></button></li>' +
    '<li><button type="button" className="rc-row rc-row-button"><span className="rc-row-main"><strong>Untitled session</strong><span className="rc-row-sub">other</span></span><span className="rc-status rc-status-idle">Idle</span></button></li>' +
    '</ul></section>',
} as const;

test('a computer page with no Bots to show is what it was before Bots: the sessions, and nothing above them', async () => {
  const sessionStates = { loading: undefined, none: [], listed: SESSIONS } as const;
  for (const [name, sessions] of Object.entries(sessionStates)) {
    for (const bots of [undefined, 'unsupported', []] as const) {
      const client = stubClient({ sessions: sessions as RemoteClientState['sessions'], bots });
      const page = await Mounted.create(createElement(Fragment, null, botList(client), createElement(SessionList, { locale: 'en', client, onOpen: noop })));
      try {
        assert.equal(page.markup, BEFORE[name as keyof typeof BEFORE], `sessions ${name}, Bots ${JSON.stringify(bots)}`);
      } finally {
        await page.unmount();
      }
    }
  }
});

test('a computer from before Bots, asked, has a page that is what it was; one with Bots has them above the sessions', async () => {
  const old = await computer((host) => {
    host.supported = false;
    host.sessions = [...SESSIONS];
  });
  assert.equal(old.client.getState().bots, 'unsupported');
  const before = await Mounted.create(createElement(Fragment, null, botList(old.client), createElement(SessionList, { locale: 'en', client: old.client, onOpen: noop })), old.wait);
  try {
    assert.equal(before.markup, BEFORE.listed);
  } finally {
    await before.unmount();
  }

  const current = await computer((host) => {
    host.bots = [bot('a')];
    host.sessions = [...SESSIONS];
  });
  const after = await Mounted.create(createElement(Fragment, null, botList(current.client), createElement(SessionList, { locale: 'en', client: current.client, onOpen: noop })), current.wait);
  try {
    assert.ok(after.markup.endsWith(BEFORE.listed), 'the sessions are as they were');
    assert.deepEqual(after.texts('h2'), ['Bots', 'Sessions'], 'Bots first');
  } finally {
    await after.unmount();
  }
});

// ─────────────────────────────────────────────────────────────────────────────
// One Bot
// ─────────────────────────────────────────────────────────────────────────────

const withChats = (host: BotHost): void => {
  host.bots = [bot('a', { description: 'Plans trips', status: 'running', waiting_threads: 1 })];
  host.threads.set('a', [thread('t2', 'a', { title: 'Newer', status: 'running' }), thread('t1', 'a', { title: 'Older', model: 'p/m' })]);
};

test('a Bot page shows what the Bot is and its chats, newest first, asking for them once', async () => {
  const c = await computer(withChats);
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a'), c.wait);
  try {
    assert.deepEqual(page.texts('.rc-crumb button'), ['Back to Studio Mac']);
    assert.deepEqual(page.texts('h2'), ['Bot a']);
    assert.deepEqual(page.texts('.rc-bot-head p'), ['Plans trips']);
    assert.deepEqual(page.texts('.rc-bot-head .rc-pending-tag'), ['1 waiting for you']);
    assert.deepEqual(page.texts('.rc-bot-head .rc-status'), ['Running']);
    assert.deepEqual(page.texts('strong'), ['Newer', 'Older'], 'in the order the computer listed them');
    assert.deepEqual(page.texts('.rc-row-sub'), ['Bot a', 'Bot a · p/m']);
    assert.ok(!page.disabled('New chat'));
    assert.equal(page.texts('.rc-note').length, 0, 'nothing is still loading');
    assert.deepEqual(c.host.requests('bot.threads.list').map((request) => request.params), [{ bot_id: 'a' }]);
  } finally {
    await page.unmount();
  }
});

test('a chat of the Bot opens as a session', async () => {
  const c = await computer(withChats);
  const opened: string[] = [];
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { onOpenSession: (id) => opened.push(id) }), c.wait);
  try {
    await page.clickRow('Older');
    assert.deepEqual(opened, ['t1']);
    assert.equal(c.client.botOfThread('t1'), 'a', 'the session that opens knows which Bot it is a chat of');
  } finally {
    await page.unmount();
  }
});

test('the chats are said to be loading until they are listed, and a Bot with none says so', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a')];
  });
  const listing = gate();
  c.host.holdThreads = listing.promise;
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a'), c.wait);
  try {
    assert.deepEqual(page.texts('.rc-note'), ['Loading…']);
    assert.deepEqual(page.texts('h3'), [], 'not "no chats" before it is known');
    listing.open();
    await page.settle();
    assert.deepEqual(page.texts('.rc-note'), []);
    assert.deepEqual(page.texts('h3'), ['No chats yet']);
    assert.deepEqual(page.texts('li'), []);
  } finally {
    await page.unmount();
  }
});

test('the page is in the language of the page', async () => {
  const c = await computer(withChats);
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { locale: 'zh' }), c.wait);
  try {
    assert.deepEqual(page.texts('.rc-crumb button'), ['返回 Studio Mac']);
    assert.deepEqual(page.texts('.rc-bot-head .rc-pending-tag'), ['1 个对话等你回复']);
    assert.ok(page.has('新对话'));
  } finally {
    await page.unmount();
  }
});

test('a new chat is asked for once, and the page goes to it when the computer has made it', async () => {
  const c = await computer(withChats);
  const opened: string[] = [];
  const creating = gate();
  c.host.holdCreate = creating.promise;
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { onOpenSession: (id) => opened.push(id) }), c.wait);
  try {
    await page.click('New chat');
    assert.ok(page.disabled('New chat'), 'a second tap while the first is on its way cannot make a second chat');
    await assert.rejects(() => page.click('New chat'), /disabled/u);
    assert.deepEqual(opened, [], 'not before the computer has answered');

    creating.open();
    await page.settle();
    assert.deepEqual(opened, ['chat-1']);
    const requests = c.host.requests('bot.thread.create');
    assert.equal(requests.length, 1);
    assert.equal(requests[0]!.params.bot_id, 'a');
    assert.match(requests[0]!.params.client_request_id, /^\S{8,}$/u);
    assert.equal(c.client.botOfThread('chat-1'), 'a');
  } finally {
    await page.unmount();
  }
});

test('a new chat the computer refuses says why, in place, and can be tried again', async () => {
  const c = await computer(withChats);
  const opened: string[] = [];
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { onOpenSession: (id) => opened.push(id) }), c.wait);
  try {
    c.host.failWith.set('bot.thread.create', new RemoteRpcError('capacity', 'full'));
    await page.click('New chat');
    assert.deepEqual(page.texts('.rc-danger'), ['The computer has reached its limit of remotely opened sessions.']);
    assert.ok(!page.disabled('New chat'), 'the button is back');
    assert.deepEqual(page.texts('strong'), ['Newer', 'Older'], 'the chats are still there');
    assert.deepEqual(opened, []);

    c.host.failWith.set('bot.thread.create', new RemoteRpcError('not_remote_enabled', 'off'));
    await page.click('New chat');
    assert.deepEqual(page.texts('.rc-danger'), [BOT_GONE], 'a Bot that was switched off is told as such');

    c.host.failWith.delete('bot.thread.create');
    await page.click('New chat');
    assert.deepEqual(opened, ['chat-1']);
    assert.deepEqual(page.texts('.rc-danger'), [], 'the earlier failure is gone');
  } finally {
    await page.unmount();
  }
});

test('a Bot that goes away is told as such and the way back still works; one that returns has its chats again', async () => {
  const c = await computer(withChats);
  let back = 0;
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { onBack: () => void (back += 1) }), c.wait);
  try {
    assert.deepEqual(page.texts('strong'), ['Newer', 'Older']);

    c.host.bots = [];
    await page.during(() => announce(c.h, c.client, c.host));
    assert.deepEqual(page.texts('p'), [BOT_GONE]);
    assert.deepEqual(page.texts('h2'), []);
    assert.ok(!page.has('New chat'), 'nothing to start a chat of');
    await page.click('Back to Studio Mac');
    assert.equal(back, 1);

    const listed = c.host.requests('bot.threads.list').length;
    c.host.bots = [bot('a', { description: 'Plans trips' })];
    c.host.threads.set('a', [thread('t3', 'a', { title: 'Newest' }), thread('t2', 'a', { title: 'Newer' })]);
    await page.during(() => announce(c.h, c.client, c.host));
    assert.deepEqual(page.texts('h2'), ['Bot a']);
    assert.equal(c.host.requests('bot.threads.list').length, listed + 1, 'its chats are asked for again');
    assert.deepEqual(page.texts('strong'), ['Newest', 'Newer']);
  } finally {
    await page.unmount();
  }
});

test('a Bot the computer cannot list the chats of is told as gone, whichever way the computer says so', async () => {
  for (const code of ['unknown_session', 'not_remote_enabled'] as const) {
    const c = await computer(withChats);
    c.host.failWith.set('bot.threads.list', new RemoteRpcError(code, 'no'));
    const page = await Mounted.create(botPage(c.client, c.connection.state, 'a'), c.wait);
    try {
      assert.deepEqual(page.texts('p'), [BOT_GONE], code);
      assert.ok(!page.has('New chat'), code);
    } finally {
      await page.unmount();
    }
  }
});

test('a listing that fails is shown as such, in place, with a way to try again', async () => {
  const c = await computer(withChats);
  c.host.failWith.set('bot.threads.list', new RemoteRpcError('internal', 'boom'));
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a'), c.wait);
  try {
    assert.deepEqual(page.texts('.rc-danger'), ['Something went wrong. Try again. Try again']);
    assert.deepEqual(page.texts('strong'), []);
    assert.ok(!page.disabled('New chat'), 'a chat can still be started');
    assert.deepEqual(page.texts('h3'), [], 'not "no chats", which is not known');

    c.host.failWith.delete('bot.threads.list');
    await page.click('Try again');
    assert.deepEqual(page.texts('.rc-danger'), []);
    assert.deepEqual(page.texts('strong'), ['Newer', 'Older']);
  } finally {
    await page.unmount();
  }
});

test('on a computer that does not know Bots, a Bot page says the Bot cannot be reached and asks nothing more about Bots', async () => {
  const c = await computer((host) => {
    host.supported = false;
  });
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a'), c.wait);
  try {
    assert.deepEqual(page.texts('p'), [BOT_GONE]);
    assert.ok(!page.has('New chat'));
    assert.deepEqual(c.host.botRequests(), ['bots.watch']);
  } finally {
    await page.unmount();
  }
});

test('before the computer has said which Bots it has, the Bot page is loading and asks for nothing', async () => {
  const client = stubClient({ sessions: undefined, bots: undefined });
  const page = await Mounted.create(botPage(client, { phase: 'ready' } as ConnectionState, 'a'));
  try {
    assert.deepEqual(page.texts('.rc-note'), ['Loading…']);
    assert.deepEqual(page.texts('.rc-crumb button'), ['Back to Studio Mac'], 'the way back is there');
    assert.ok(!page.has('New chat'));
  } finally {
    await page.unmount();
  }
});

test('while the connection is down the page says so and starts no chat', async () => {
  const c = await computer(withChats);
  const page = await Mounted.create(botPage(c.client, { ...c.connection.state, phase: 'reconnecting' }, 'a'), c.wait);
  try {
    assert.deepEqual(page.texts('.rc-banner'), ['Connection lost. Reconnecting…']);
    assert.ok(page.disabled('New chat'));
    assert.deepEqual(page.texts('strong'), ['Newer', 'Older'], 'what is known stays');

    await page.update(botPage(c.client, c.connection.state, 'a'));
    assert.deepEqual(page.texts('.rc-banner'), []);
    assert.ok(!page.disabled('New chat'));
  } finally {
    await page.unmount();
  }
});

test('a page that was left before the computer made the chat does not take the reader to it', async () => {
  const c = await computer(withChats);
  const opened: string[] = [];
  const creating = gate();
  c.host.holdCreate = creating.promise;
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { onOpenSession: (id) => opened.push(id) }), c.wait);
  await page.click('New chat');
  await page.unmount();
  creating.open();
  await page.settle();
  assert.equal(c.host.requests('bot.thread.create').length, 1, 'the chat was made on the computer');
  assert.deepEqual(opened, []);
});

test('a Bot page lists the chats again as the Bot is active, and stops when it is left', async () => {
  const c = await computer(withChats);
  const listings = (): number => c.host.requests('bot.threads.list').length;
  const page = await Mounted.create(botPage(c.client, c.connection.state, 'a'), c.wait);
  try {
    assert.equal(listings(), 1);
    c.host.threads.set('a', [thread('t3', 'a', { title: 'Newest' }), ...c.host.threads.get('a')!]);
    c.host.bots = [bot('a', { last_activity_at: 10 })];
    await page.during(() => announce(c.h, c.client, c.host));
    assert.equal(listings(), 2);
    assert.deepEqual(page.texts('strong'), ['Newest', 'Newer', 'Older']);

    c.host.bots = [bot('a', { last_activity_at: 10, name: 'Renamed' })];
    await page.during(() => announce(c.h, c.client, c.host));
    assert.equal(listings(), 2, 'a change that is not in the Bot’s activity does not list the chats again');
    assert.deepEqual(page.texts('h2'), ['Renamed']);
  } finally {
    await page.unmount();
  }
  c.host.bots = [bot('a', { last_activity_at: 20 })];
  await page.during(() => announce(c.h, c.client, c.host));
  assert.equal(listings(), 2, 'nothing asks for the chats of a page nobody sees');
});

// ─────────────────────────────────────────────────────────────────────────────
// A session of a Bot
// ─────────────────────────────────────────────────────────────────────────────

/** `SessionView` listens to the page's visibility. */
async function withDocument<T>(body: () => Promise<T>): Promise<T> {
  const original = globalThis.document;
  globalThis.document = { visibilityState: 'visible', addEventListener() {}, removeEventListener() {} } as unknown as Document;
  try {
    return await body();
  } finally {
    globalThis.document = original;
  }
}

function sessionView(c: Computer, sessionId: string, locale: 'en' | 'zh' = 'en'): ReactElement {
  return createElement(SessionView, { locale, client: c.client, sessionId, connection: c.connection.state, deviceName: 'Test Browser', onBack: noop });
}

test('a chat opened from its Bot is titled as the Bot lists it, belongs to the Bot, and goes back to it', async () => {
  const c = await computer(withChats);
  await withDocument(async () => {
    const opened: string[] = [];
    const page = await Mounted.create(botPage(c.client, c.connection.state, 'a', { onOpenSession: (id) => opened.push(id) }), c.wait);
    await page.clickRow('Older');
    await page.unmount();

    const view = await Mounted.create(sessionView(c, opened[0]!), c.wait);
    try {
      assert.deepEqual(view.texts('h1'), ['Older']);
      assert.deepEqual(view.texts('.rc-session-title p'), ['Bot a']);
      assert.equal(view.label('.rc-back'), 'Back to Bot a');
    } finally {
      await view.unmount();
    }
    const zh = await Mounted.create(sessionView(c, opened[0]!, 'zh'), c.wait);
    try {
      assert.equal(zh.label('.rc-back'), '返回 Bot a');
    } finally {
      await zh.unmount();
    }
  });
});

test('a chat just started has no title yet: it is untitled, under the Bot’s name, and gets its title once it has one', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a')];
  });
  await withDocument(async () => {
    const sessionId = await c.client.createBotThread('a');
    const view = await Mounted.create(sessionView(c, sessionId), c.wait);
    try {
      assert.deepEqual(view.texts('h1'), ['Untitled session']);
      assert.deepEqual(view.texts('.rc-session-title p'), ['Bot a']);
      assert.equal(view.label('.rc-back'), 'Back to Bot a');
      assert.equal(c.host.requests('bot.threads.list').length, 1);

      // The first prompt is sent: the chat has a title, and the Bot has been active.
      c.host.threads.set('a', [thread(sessionId, 'a', { title: 'Plan the trip' })]);
      c.host.bots = [bot('a', { last_activity_at: 100 })];
      await view.during(() => announce(c.h, c.client, c.host));
      assert.deepEqual(view.texts('h1'), ['Plan the trip']);
      assert.equal(c.host.requests('bot.threads.list').length, 2);

      await view.unmount();
      c.host.bots = [bot('a', { last_activity_at: 200 })];
      await view.during(() => announce(c.h, c.client, c.host));
      assert.equal(c.host.requests('bot.threads.list').length, 2, 'once the chat is closed nothing asks for the Bot’s chats');
    } finally {
      await view.unmount();
    }
  });
});

test('a session that is not a Bot’s is shown as it was, on a computer that has Bots too', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a')];
    host.sessions = [
      { session_id: 's1', title: 'Fix bug', project_label: 'proj', status: 'idle', viewers: 0 },
      { session_id: 's2', title: 'No project', project_label: '', status: 'idle', viewers: 0 },
    ];
  });
  await withDocument(async () => {
    const withProject = await Mounted.create(sessionView(c, 's1'), c.wait);
    try {
      assert.deepEqual(withProject.texts('h1'), ['Fix bug']);
      assert.deepEqual(withProject.texts('.rc-session-title p'), ['proj']);
      assert.equal(withProject.label('.rc-back'), 'Back');
    } finally {
      await withProject.unmount();
    }
    const without = await Mounted.create(sessionView(c, 's2'), c.wait);
    try {
      assert.deepEqual(without.texts('h1'), ['No project']);
      assert.deepEqual(without.texts('.rc-session-title p'), [], 'no Bot’s name where there is no Bot');
      assert.equal(without.label('.rc-back'), 'Back');
    } finally {
      await without.unmount();
    }
    assert.equal(c.host.requests('bot.threads.list').length, 0, 'no Bot’s chats are asked for on behalf of a session');
  });
});

test('a session opened by its address, which is not known to be a Bot’s, is untitled and goes back to the computer', async () => {
  const c = await computer((host) => {
    host.bots = [bot('a')];
    host.threads.set('a', [thread('t1', 'a', { title: 'Older' })]);
  });
  await withDocument(async () => {
    const view = await Mounted.create(sessionView(c, 't1'), c.wait);
    try {
      assert.deepEqual(view.texts('h1'), ['Untitled session']);
      assert.deepEqual(view.texts('.rc-session-title p'), []);
      assert.equal(view.label('.rc-back'), 'Back');
      assert.equal(c.host.requests('bot.threads.list').length, 0);
    } finally {
      await view.unmount();
    }
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// A prompt that a Bot handed over
// ─────────────────────────────────────────────────────────────────────────────

const prompt = (origin: RemotePromptOrigin | 'from-the-future', rest: Record<string, unknown> = {}): RemoteSeqRecord =>
  ({ seq: 1, record: { type: 'prompt', turn_id: 1, text: 'hi', origin, ...rest } }) as unknown as RemoteSeqRecord;

/** The labels under the user's messages of a transcript. */
async function originLabels(records: RemoteSeqRecord[], locale: 'en' | 'zh' = 'en'): Promise<string[]> {
  const transcript = TranscriptBuilder.from([], records).snapshot();
  const view = await Mounted.create(createElement(TranscriptView, { locale, transcript, running: false, deviceName: 'Test Browser', fetchImage: async () => '' }));
  try {
    return view.texts('.rc-origin');
  } finally {
    await view.unmount();
  }
}

test('a prompt from a Bot is labelled with the Bot’s name', async () => {
  assert.deepEqual(await originLabels([prompt('bot', { from_bot_name: 'Alpha' })]), ['From Bot Alpha']);
  assert.deepEqual(await originLabels([prompt('bot', { from_bot_name: 'Alpha' })], 'zh'), ['来自 Bot「Alpha」']);
});

test('a prompt from a Bot that has no usable name is labelled as from another Bot', async () => {
  for (const record of [{}, { from_bot_name: '' }, { from_bot_name: '‮​ \n' }, { from_bot_name: 7 }]) {
    assert.deepEqual(await originLabels([prompt('bot', record)]), ['From another Bot'], JSON.stringify(record));
    assert.deepEqual(await originLabels([prompt('bot', record)], 'zh'), ['来自另一个 Bot'], JSON.stringify(record));
  }
});

test('the name of a Bot is cleaned and cut like any name from the computer', async () => {
  assert.deepEqual(await originLabels([prompt('bot', { from_bot_name: 'Al‮pha\nBeta' })]), ['From Bot Alpha Beta']);
  assert.deepEqual(await originLabels([prompt('bot', { from_bot_name: 'N'.repeat(100) })]), [`From Bot ${'N'.repeat(59)}…`]);
});

test('only a prompt from a Bot is labelled with a Bot; the other labels are as they were', async () => {
  assert.deepEqual(
    await originLabels([
      prompt('desktop', { from_bot_name: 'Alpha' }),
      prompt('scheduled', { from_bot_name: 'Alpha' }),
      prompt('device', { device_name: 'Test Browser', from_bot_name: 'Alpha' }),
      prompt('device', { device_name: 'Pixel 8', from_bot_name: 'Alpha' }),
      prompt('device', { from_bot_name: 'Alpha' }),
      prompt('from-the-future', { from_bot_name: 'Alpha' }),
    ]),
    ['From the desktop', 'Scheduled task', 'From this browser', 'From Pixel 8', 'From ?', 'From ?'],
  );
  const meta = TranscriptBuilder.from([], [prompt('device', { device_name: 'Pixel 8', from_bot_name: 'Alpha' })]).snapshot().promptMeta;
  assert.deepEqual([...meta.values()], [{ origin: 'device', deviceName: 'Pixel 8', turnId: 1 }], 'a name that is not from a Bot is not kept');
});
