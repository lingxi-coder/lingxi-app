import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {File} from 'node:buffer';
import {webcrypto} from 'node:crypto';
import test from 'node:test';
import vm from 'node:vm';
import {fileURLToPath} from 'node:url';

const fixturePath = fileURLToPath(new URL('../../../fixtures/local-apps/v2/agent/index.html', import.meta.url));
const fixtureSource = readFileSync(fixturePath, 'utf8');
const inlineScripts = [...fixtureSource.matchAll(/<script(?:\s+type="module")?>([\s\S]*?)<\/script>/g)].map(match => match[1]);
assert.equal(inlineScripts.length, 2, 'fixture keeps the handoff-capture script before its module bootstrap');

const appId = 'a0038dae947f74bb58924c6b0d582b0fb';
const runId = 'be84ba16-32c0-4c60-bdf8-94d04fbff589';
const threadId = 'thread-resume-71f7';
const storagePrefix = 'lingxi.localApp.agent-device-acceptance.v1';

function savedInput() {
  return {
    protocolVersion: '1.0',
    threadId,
    runId,
    messages: [{id: 'user-existing', role: 'user', content: [{type: 'text', text: 'already submitted input'}]}],
    forwardedProps: {'com.lingxi.localApp': {version: 1, outputSchema: {
      type: 'object', additionalProperties: false, required: ['food', 'description', 'visibleCount'],
      properties: {
        food: {type: 'string', minLength: 1},
        description: {type: 'string', minLength: 1},
        visibleCount: {type: 'integer', minimum: 0},
      },
    }}},
  };
}

function fixtureState({pending = true, lastRun = true} = {}) {
  return {
    version: 1,
    chatId: null,
    pending: pending ? {input: savedInput(), kind: 'image'} : null,
    lastRun: lastRun ? {runId, threadId, kind: 'image'} : null,
  };
}

function storageEntries(state) {
  if (!state) return [];
  const entries = [];
  if (state.lastRun) {
    const id = state.lastRun.runId;
    entries.push([`${storagePrefix}.run.${id}`, JSON.stringify({version: 1, pending: state.pending, lastRun: state.lastRun})]);
    entries.push([`${storagePrefix}.selectedRun`, id]);
  }
  if (state.chatId) entries.push([`${storagePrefix}.selectedChat`, state.chatId]);
  return entries;
}

function successTask() {
  return {runId, threadId, requestId: runId, state: 'succeeded', sequence: 9,
    limits: {maxTurns: 8, timeoutMs: 180000}};
}

function successEvents() {
  return [
    {type: 'RUN_STARTED', protocolVersion: '1.0', threadId, runId},
    {type: 'TEXT_MESSAGE_START', messageId: 'assistant-existing', role: 'assistant'},
    {type: 'TEXT_MESSAGE_CONTENT', messageId: 'assistant-existing', delta: 'Recovered answer'},
    {type: 'TEXT_MESSAGE_END', messageId: 'assistant-existing'},
    {type: 'RUN_FINISHED', threadId, runId, outcome: {type: 'success'},
      result: {food: 'apple', description: 'one apple', visibleCount: 1}},
  ];
}

function eventTarget() {
  const listeners = new Map();
  return {
    addEventListener(type, listener) {
      const handlers = listeners.get(type) || new Set();
      handlers.add(listener);
      listeners.set(type, handlers);
    },
    dispatchEvent(event) {
      for (const listener of listeners.get(event.type) || []) listener.call(this, event);
      return true;
    },
  };
}

class BrowserCustomEvent {
  constructor(type, options = {}) { this.type = type; this.detail = options.detail; }
}

function createBrowser({
  seedState = fixtureState(),
  sharedStorage = null,
  holdGet = false,
  sdkReceipt = null,
  inventoryChats = [],
  inventoryTasks = [],
  readyBeforeBootstrap = true,
  duplicateReady = false,
  errorBeforeBootstrap = null,
  initiallyHidden = false,
  getFailures = 0,
  task = successTask(),
  events = successEvents(),
  connectFailure = null,
  connectEvents = events,
  connectState = null,
  userCancellation = false,
  holdConnect = false,
  result = {result: {food: 'apple', description: 'one apple', visibleCount: 1}, newMessages: [
    {id: 'assistant-existing', role: 'assistant', content: [{type: 'text', text: 'Recovered answer'}]},
  ]},
} = {}) {
  const window = eventTarget();
  window.top = window;
  window.isSecureContext = true;
  const location = {origin: 'http://127.0.0.1:37792', href: 'http://127.0.0.1:37792/index.html'};
  const document = eventTarget();
  document.visibilityState = initiallyHidden ? 'hidden' : 'visible';
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) {
      const handlers = new Map();
      elements.set(id, {
        id, textContent: '', value: '', hidden: ['error', 'recovery-error', 'retry-image', 'host-open'].includes(id),
        disabled: false, handlers,
        addEventListener(type, handler) { handlers.set(type, handler); },
      });
    }
    return elements.get(id);
  };
  document.getElementById = element;
  document.createElement = () => ({style: {}, files: [], addEventListener() {}, click() {}, remove() {}});
  document.body = {append() {}};

  const values = new Map(storageEntries(seedState));
  const localStorage = sharedStorage || {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, String(value)); },
    removeItem(key) { values.delete(key); },
  };

  const counters = {get: 0, getReturned: 0, events: 0, list: 0, chatsList: 0, chatsOpen: 0, connect: 0, cancel: 0, run: 0, start: 0, chatSend: 0};
  const activeTask = task;
  let releaseGet;
  const getGate = new Promise(resolve => { releaseGet = resolve; });
  let remainingGetFailures = getFailures;
  let currentEvents = events;
  let releaseCancellation;
  const cancellationRequested = new Promise(resolve => { releaseCancellation = resolve; });
  let releaseConnect;
  const connectGate = new Promise(resolve => { releaseConnect = resolve; });
  const app = {
    info: async () => ({id: appId, revision: 1}),
    agent: {
      run: async () => { counters.run++; throw new Error('unexpected agent.run'); },
      tasks: {
        async get(id) {
          counters.get++;
          if (holdGet) await getGate;
          assert.equal(id, runId, 'read-only lookup must keep the saved run ID');
          if (remainingGetFailures > 0) {
            remainingGetFailures--;
            const failure = new Error('The Agent task could not complete. Review the host status before retrying.');
            failure.code = 'task_not_found';
            throw failure;
          }
          counters.getReturned++;
          return activeTask;
        },
        async list() { counters.list++; return {tasks: inventoryTasks, nextCursor: undefined}; },
        async events(query) {
          counters.events++;
          assert.equal(query.runId, runId, 'event replay must keep the saved run ID');
          if (query.afterSequence === 0) return {events: currentEvents, sequence: currentEvents.length};
          return {events: [], sequence: currentEvents.length};
        },
        async start() { counters.start++; throw new Error('unexpected tasks.start'); },
        async cancel(id) {
          counters.cancel++;
          assert.equal(id, runId, 'cancel action must target the currently observed run');
          activeTask.state = 'cancelled';
          activeTask.error = {code: 'cancelled', message: 'The run was cancelled.'};
          currentEvents = [{type: 'RUN_STARTED', protocolVersion: '1.0', threadId, runId},
            {type: 'RUN_FINISHED', threadId, runId, outcome: {type: 'cancelled'}}];
          releaseCancellation();
        },
      },
      createClient(config) {
        assert.equal(config.threadId, threadId, 'read-only client must keep the saved thread ID');
        return {
          async connectAgent(parameters, subscriber) {
            counters.connect++;
            assert.equal(parameters.runId, runId, 'reconnect must use the existing run ID');
            assert.deepEqual(Object.keys(parameters), ['runId'], 'reconnect must not create or submit input');
            if (holdConnect) await connectGate;
            if (userCancellation) {
              await cancellationRequested;
              for (const event of currentEvents) subscriber.onEvent?.({event});
              throw Object.assign(new Error('The AG-UI run was cancelled.'), {code: 'run_cancelled'});
            }
            for (const event of connectEvents) subscriber.onEvent?.({event});
            if (connectState) {
              activeTask.state = connectState.state;
              activeTask.error = connectState.error;
              currentEvents = connectState.events;
            }
            if (connectFailure) throw connectFailure;
            return result;
          },
        };
      },
      chats: {
        async list() { counters.chatsList++; return {chats: inventoryChats, nextCursor: undefined}; },
        async open(id) { counters.chatsOpen++; return {id, async send() { counters.chatSend++; throw new Error('unexpected chat.send'); }, async history() { return {messages: []}; }}; },
      },
    },
  };
  window.localApp = app;
  if (sdkReceipt) {
    window.indexedDB = {
      open(name) {
        assert.equal(name, `lingxi-local-app-agui-${appId}`);
        const request = {};
        queueMicrotask(() => {
          const db = {
            objectStoreNames: {contains: value => value === 'pending'},
            transaction(store, mode) {
              assert.equal(store, 'pending');
              assert.equal(mode, 'readonly');
              const tx = {objectStore() { return {get(key) {
                const result = {};
                queueMicrotask(() => {
                  result.result = key === `${appId}:1:${runId}` ? sdkReceipt : undefined;
                  result.onsuccess?.();
                  queueMicrotask(() => tx.oncomplete?.());
                });
                return result;
              }}; }};
              return tx;
            },
            close() {},
          };
          request.result = db;
          request.onsuccess?.();
        });
        return request;
      },
    };
  }
  const context = vm.createContext({
    window, document, location, localStorage, localApp: app,
    console, URL, URLSearchParams, CustomEvent: BrowserCustomEvent,
    TextEncoder, File, AbortController, crypto: webcrypto,
    fetch: async () => ({ok: false, status: 404}),
    setTimeout, clearTimeout, setInterval: () => 1,
  });
  vm.runInContext(inlineScripts[0], context, {filename: `${fixturePath}#capture`});
  if (readyBeforeBootstrap) {
    const record = {type: 'localApp:agent-handoff-ready', detail: {runId}};
    window.dispatchEvent(record);
    if (duplicateReady) window.dispatchEvent(record);
  }
  if (errorBeforeBootstrap) {
    window.dispatchEvent({type: 'localApp:agent-handoff-error', detail: {runId: null, code: errorBeforeBootstrap}});
  }
  vm.runInContext(inlineScripts[1], context, {filename: `${fixturePath}#module`});

  return {
    app, counters, document, elements, localStorage, location, window, context,
    releaseGet() { releaseGet(); },
    setVisible() { document.visibilityState = 'visible'; document.dispatchEvent({type: 'visibilitychange'}); },
    releaseConnect() { releaseConnect(); },
    dispatchReady(id = runId) { window.dispatchEvent({type: 'localApp:agent-handoff-ready', detail: {runId: id}}); },
    dispatchError(code, id = null) { window.dispatchEvent({type: 'localApp:agent-handoff-error', detail: {runId: id, code}}); },
    state() {
      const state = {version: 1, chatId: null, pending: null, lastRun: null};
      const selected = localStorage.getItem(`${storagePrefix}.selectedRun`);
      if (selected !== null) {
        const entry = JSON.parse(localStorage.getItem(`${storagePrefix}.run.${selected}`));
        state.pending = entry.pending;
        state.lastRun = entry.lastRun;
      }
      if (state.pending && localStorage.getItem(`${storagePrefix}.finished.${state.pending.input.runId}`) !== null) state.pending = null;
      state.chatId = localStorage.getItem(`${storagePrefix}.selectedChat`) ?? state.chatId;
      return state;
    },
  };
}

async function waitFor(predicate, message) {
  for (let attempt = 0; attempt < 500; attempt++) {
    if (predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  assert.fail(message);
}

function assertNoSubmission(browser) {
  assert.equal(browser.counters.run, 0);
  assert.equal(browser.counters.start, 0);
  assert.equal(browser.counters.chatSend, 0);
}

test('accepted existing run reconnects read-only and preserves input and IDs', async () => {
  const browser = createBrowser();
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'existing run did not recover');
  const state = browser.state();
  assert.equal(state.pending, null);
  assert.deepEqual(state.lastRun, {runId, threadId, kind: 'image'});
  assert.equal(browser.counters.connect, 1);
  assertNoSubmission(browser);
  assert.match(browser.elements.get('result').textContent, /Recovered answer/);
  assert.match(browser.elements.get('result').textContent, /RunAgentResult/);
  assert.match(browser.elements.get('recovery').textContent, /saved input and IDs were unchanged/);
});

test('matching ready event can recover from lastRun metadata after pending input was already cleared', async () => {
  const browser = createBrowser({seedState: fixtureState({pending: false, lastRun: true})});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'saved lastRun did not reconnect');
  assert.equal(browser.state().pending, null);
  assert.deepEqual(browser.state().lastRun, {runId, threadId, kind: 'image'});
  assert.equal(browser.counters.connect, 1);
  assertNoSubmission(browser);
});

test('plain refresh finds an already accepted pending run and reconnects without an SDK ready replay', async () => {
  const browser = createBrowser({readyBeforeBootstrap: false});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'plain refresh did not resume the accepted run');
  assert.equal(browser.counters.connect, 1);
  assert.equal(browser.state().pending, null);
  assert.deepEqual(browser.state().lastRun, {runId, threadId, kind: 'image'});
  assertNoSubmission(browser);
});

test('plain refresh with no durable task keeps pending input and sends nothing', async () => {
  const browser = createBrowser({readyBeforeBootstrap: false, getFailures: 8});
  await waitFor(() => browser.elements.get('task-state')?.textContent.includes('waiting for host acceptance'), 'missing task did not remain pending');
  assert.equal(browser.counters.get, 8);
  assert.equal(browser.counters.connect, 0);
  assert.deepEqual(browser.state().pending.input, savedInput());
  assertNoSubmission(browser);
});

test('short task_not_found window retries only read-only task lookup', async () => {
  const browser = createBrowser({getFailures: 2});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'task did not recover after bounded lookup retry');
  assert.equal(browser.counters.get, 5, 'startup lookup, reconnect lookup, and durable evidence lookup are all read-only');
  assert.equal(browser.counters.connect, 1);
  assertNoSubmission(browser);
});

test('duplicate early SDK ready events start only one reconnect; hidden pages wait until visible', async () => {
  const browser = createBrowser({initiallyHidden: true, duplicateReady: true, holdConnect: true});
  await new Promise(resolve => setTimeout(resolve, 40));
  assert.equal(browser.counters.connect, 0, 'a hidden page must not start recovery');
  browser.setVisible();
  await waitFor(() => browser.counters.connect === 1, 'visible page did not consume the queued ready event');
  browser.dispatchReady();
  browser.dispatchReady();
  browser.setVisible();
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(browser.counters.connect, 1, 'duplicate ready/pageshow notifications must be idempotent');
  browser.releaseConnect();
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'held reconnect did not finish');
  assertNoSubmission(browser);
});

test('ready arriving after a pending refresh task_not_found resumes the saved run once', async () => {
  const browser = createBrowser({readyBeforeBootstrap: false, getFailures: 8});
  await waitFor(() => browser.elements.get('task-state')?.textContent.includes('waiting for host acceptance'), 'bootstrap did not preserve pending input after task_not_found');
  assert.equal(browser.counters.connect, 0, 'bootstrap alone must not start or reconnect a task');
  browser.dispatchReady();
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'late ready event did not resume the saved run');
  assert.equal(browser.counters.connect, 1);
  assertNoSubmission(browser);
});

test('RUN_ERROR displays the terminal code and clears only the terminal pending request', async () => {
  const failedEvents = [{type: 'RUN_STARTED', protocolVersion: '1.0', threadId, runId},
    {type: 'RUN_ERROR', code: 'model_provider_http_401', message: 'provider authentication rejected at https://private.invalid/path?token=synthetic-secret'}];
  const browser = createBrowser({task: {...successTask(), state: 'running'}, connectEvents: failedEvents,
    connectFailure: Object.assign(new Error('Agent run failed'), {code: 'model_provider_http_401'}),
    connectState: {state: 'failed', error: {code: 'model_provider_http_401', message: failedEvents.at(-1).message}, events: failedEvents}});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('model_provider_http_401'), 'RUN_ERROR code was not surfaced');
  assert.match(browser.elements.get('recovery-error').textContent, /\[URL redacted\]/);
  assert.doesNotMatch(browser.elements.get('recovery-error').textContent, /synthetic-secret/);
  assert.equal(browser.state().pending, null);
  assert.deepEqual(browser.state().lastRun, {runId, threadId, kind: 'image'});
  assert.match(browser.elements.get('recovery').textContent, /terminal \(failed\)/);
  assertNoSubmission(browser);
});

test('cancelled terminal is shown as cancellation and clears pending without replacing lastRun', async () => {
  const cancelledEvents = [{type: 'RUN_STARTED', protocolVersion: '1.0', threadId, runId},
    {type: 'RUN_FINISHED', threadId, runId, outcome: {type: 'cancelled'}}];
  const browser = createBrowser({task: {...successTask(), state: 'running'}, connectEvents: cancelledEvents,
    connectFailure: Object.assign(new Error('The AG-UI run was cancelled.'), {code: 'run_cancelled'}),
    connectState: {state: 'cancelled', error: {code: 'cancelled', message: 'The run was cancelled.'}, events: cancelledEvents}});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('cancelled'), 'cancel terminal was not surfaced');
  assert.equal(browser.state().pending, null);
  assert.deepEqual(browser.state().lastRun, {runId, threadId, kind: 'image'});
  assertNoSubmission(browser);
});

test('user cancel targets the observed run while the single read-only reconnect remains in flight', async () => {
  const browser = createBrowser({task: {...successTask(), state: 'running'}, connectEvents: [], userCancellation: true});
  await waitFor(() => browser.counters.connect === 1 && browser.elements.get('cancel-run')?.disabled === false,
    'recovery did not expose cancellation for its active task');
  await browser.elements.get('cancel-run').handlers.get('click')();
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('cancelled'), 'user cancellation terminal was not surfaced');
  assert.equal(browser.counters.connect, 1, 'cancel must not start a competing reconnect');
  assert.equal(browser.counters.cancel, 1);
  assert.equal(browser.state().pending, null);
  assert.deepEqual(browser.state().lastRun, {runId, threadId, kind: 'image'});
  assertNoSubmission(browser);
});

test('pending_request_missing stays visible when Host Agent becomes ready and sends nothing', async () => {
  const browser = createBrowser({seedState: fixtureState({pending: false, lastRun: true}), readyBeforeBootstrap: false,
    errorBeforeBootstrap: 'pending_request_missing'});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('pending_request_missing'), 'missing pending request was not visible');
  assert.match(browser.elements.get('recovery-error').textContent, /No request was retried/);
  assert.doesNotMatch(browser.elements.get('status').textContent, /^Host Agent ready/);
  assert.equal(browser.counters.connect, 0);
  assert.equal(browser.counters.get, 1, 'known lastRun status may be read without clearing the missing-pending warning');
  assertNoSubmission(browser);
});

test('ready for an unknown run never auto-connects or submits a request', async () => {
  const browser = createBrowser({seedState: null, readyBeforeBootstrap: true});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('pending_request_missing'), 'unknown ready run was not rejected clearly');
  assert.equal(browser.counters.connect, 0);
  assertNoSubmission(browser);
});

test('terminal success with an invalid structured result is not reported as a recovered result', async () => {
  const browser = createBrowser({result: {result: {food: 'apple'}, newMessages: [
    {id: 'assistant-existing', role: 'assistant', content: [{type: 'text', text: 'not enough fields'}]},
  ]}});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('output_schema_invalid'), 'invalid structured result was accepted');
  assert.equal(browser.state().pending, null, 'terminal request is not left available for duplicate execution');
  assert.doesNotMatch(browser.elements.get('status').textContent, /Read-only reconnect succeeded/);
  assertNoSubmission(browser);
});

test('identical TEXT_MESSAGE_CONTENT callbacks are preserved as distinct protocol events', async () => {
  const events = successEvents();
  events.splice(3, 0, {...events[2]});
  const browser = createBrowser({events, connectEvents: events});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'run with identical text chunks did not recover');
  const displayedEvents = JSON.parse(browser.elements.get('events').textContent);
  const contentEvents = displayedEvents.filter(event => event.type === 'TEXT_MESSAGE_CONTENT');
  const result = JSON.parse(browser.elements.get('result').textContent);
  assert.equal(contentEvents.length, 2);
  assert.equal(result.callbackEventTypes.filter(type => type === 'TEXT_MESSAGE_CONTENT').length, 2);
  assertNoSubmission(browser);
});

test('duplicate terminal callbacks are rejected rather than accepted as success', async () => {
  const events = successEvents();
  const callbackEvents = [...events, {...events.at(-1)}];
  const browser = createBrowser({events, connectEvents: callbackEvents});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('duplicate_terminal_event'), 'duplicate terminal was not rejected');
  assert.equal(browser.state().pending, null);
  assert.deepEqual(browser.state().lastRun, {runId, threadId, kind: 'image'});
  assert.doesNotMatch(browser.elements.get('status').textContent, /Read-only reconnect succeeded/);
  assertNoSubmission(browser);
});

test('pending_request_missing is cleared only after a matching saved run is fully recovered', async () => {
  const browser = createBrowser({seedState: fixtureState({pending: false, lastRun: true}),
    readyBeforeBootstrap: true, errorBeforeBootstrap: 'pending_request_missing'});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'), 'known lastRun did not recover after SDK pending warning');
  assert.equal(browser.elements.get('recovery-error')?.hidden, true);
  assertNoSubmission(browser);
});


test('terminal recovery refreshes safe diagnostics without a page reload', async () => {
  const cancelledEvents = [{type: 'RUN_STARTED', protocolVersion: '1.0', threadId, runId},
    {type: 'RUN_FINISHED', threadId, runId, outcome: {type: 'cancelled'}}];
  const browser = createBrowser({task: {...successTask(), state: 'running'}, connectEvents: cancelledEvents,
    connectFailure: Object.assign(new Error('The AG-UI run was cancelled.'), {code: 'run_cancelled'}),
    connectState: {state: 'cancelled', error: {code: 'cancelled', message: 'The run was cancelled.'}, events: cancelledEvents}});
  await waitFor(() => browser.state().pending === null, 'terminal recovery did not clear its pending request');
  await waitFor(() => {
    const raw = browser.elements.get('diagnostics')?.textContent;
    if (!raw) return false;
    const observed = JSON.parse(raw);
    return observed.localPending === false && observed.pendingRunId === null && observed.lastRunId === runId;
  }, 'terminal recovery kept stale pending diagnostics until a page reload');
  assertNoSubmission(browser);
});


test('two tabs: an older read-only task lookup must not overwrite a newer pending run', async () => {
  const values = new Map(storageEntries(fixtureState({pending: false, lastRun: true})));
  const sharedStorage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, String(value)); },
    removeItem(key) { values.delete(key); },
  };
  const oldTab = createBrowser({sharedStorage, holdGet: true, readyBeforeBootstrap: false});
  await waitFor(() => oldTab.counters.get === 1, 'old tab did not begin its read-only lookup');
  const newTab = createBrowser({sharedStorage, holdGet: true, readyBeforeBootstrap: false});
  await waitFor(() => newTab.counters.get === 1, 'new tab did not read the shared old state');
  const newInput = {...savedInput(), runId: 'new-run-123', threadId: 'new-thread-456'};
  newTab.context.newInput = newInput;
  vm.runInContext("void submitInput(newInput, 'image')", newTab.context);
  await waitFor(() => newTab.counters.run === 1, 'new tab did not submit');
  assert.equal(newTab.state().pending.input.runId, 'new-run-123');
  oldTab.releaseGet();
  await waitFor(() => oldTab.counters.getReturned === 1, 'old read-only lookup did not finish');
  assert.equal(newTab.state().pending?.input?.runId, 'new-run-123', 'old tab overwrote new pending request');
  assert.equal(newTab.state().lastRun?.runId, 'new-run-123', 'old tab overwrote new lastRun');
  newTab.releaseGet();
});


function acceptedReceipt(overrides = {}) {
  return {key: `${appId}:1:${runId}`, appId, revision: 1, runId, threadId,
    method: 'agent.chats.send', fingerprint: 'a'.repeat(64), state: 'accepted',
    acceptedAt: Date.now(), ...overrides};
}

function chatResult() {
  return {result: {answer: 'KIWI-482'}, newMessages: [
    {id: 'assistant-chat', role: 'assistant', content: [{type: 'text', text: 'KIWI-482'}]},
  ]};
}

test('accepted Chat receipt recovers the exact host run and thread when fixture pending is gone', async () => {
  const events = successEvents().map(event => event.type === 'RUN_FINISHED'
    ? {...event, result: {answer: 'KIWI-482'}} : event);
  const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt(),
    events, connectEvents: events, result: chatResult()});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'),
    'matching SDK receipt did not reconnect');
  assert.equal(browser.counters.connect, 1);
  assert.match(browser.elements.get('result').textContent, /KIWI-482/);
  assert.doesNotMatch(browser.elements.get('recovery-error').textContent, /pending_request_missing/);
  assertNoSubmission(browser);
});

test('accepted Chat receipt with a different host thread never connects or resubmits', async () => {
  const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt(),
    task: {...successTask(), threadId: 'not-the-receipt-thread'}});
  await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('task_identity_mismatch'),
    'host identity mismatch was not reported');
  assert.equal(browser.counters.connect, 0);
  assertNoSubmission(browser);
});

test('unaccepted, stale, or mismatched SDK receipts cannot establish a recovery identity', async () => {
  const cases = [
    {state: 'pending'}, {state: 'sending'}, {state: 'uncertain'}, {state: 'rejected'},
    {appId: 'another-app'}, {revision: 2}, {key: 'wrong-key'}, {runId: 'another-run'},
    {fingerprint: 'invalid'}, {acceptedAt: Date.now() - 8 * 24 * 60 * 60 * 1000},
    {method: 'agent.chats.delete'},
  ];
  for (const item of cases) {
    const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt(item)});
    await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('pending_request_missing'),
      `unexpectedly accepted receipt ${JSON.stringify(item)}`);
    assert.equal(browser.counters.connect, 0);
    assertNoSubmission(browser);
  }
});

test('old read-only completion cannot clear the newer tab pending run', async () => {
  const values = new Map(storageEntries(fixtureState()));
  const sharedStorage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, String(value)); },
    removeItem(key) { values.delete(key); },
  };
  const oldTab = createBrowser({sharedStorage, holdConnect: true});
  await waitFor(() => oldTab.counters.connect === 1, 'old run did not start its read-only reconnect');
  const newTab = createBrowser({sharedStorage, holdGet: true, readyBeforeBootstrap: false});
  await waitFor(() => newTab.counters.get === 1, 'new tab did not load its previous state');
  newTab.context.newInput = {...savedInput(), runId: 'newer-run-123', threadId: 'newer-thread-456'};
  vm.runInContext("void submitInput(newInput, 'image')", newTab.context);
  await waitFor(() => newTab.counters.run === 1, 'new tab did not submit');
  oldTab.releaseConnect();
  await waitFor(() => sharedStorage.getItem(`${storagePrefix}.finished.${runId}`) === '1', 'old run did not record its terminal state');
  assert.equal(newTab.state().pending?.input?.runId, 'newer-run-123');
  assert.equal(newTab.state().lastRun?.runId, 'newer-run-123');
  assertNoSubmission(oldTab);
  newTab.releaseGet();
});


test('accepted task without fixture input reconnects by exact ID and marks its original schema unverified', async () => {
  const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt({method: 'agent.tasks.start'})});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('original output schema unavailable'),
    'matching task receipt did not reconnect with an explicit schema limitation');
  assert.equal(browser.counters.connect, 1);
  assert.match(browser.elements.get('result').textContent, /"outputSchemaVerified": false/);
  assertNoSubmission(browser);
});

test('saved Chat run with a conflicting chat ID is rejected without looking up another Chat', async () => {
  const invalid = fixtureState();
  invalid.pending.kind = 'chat';
  invalid.pending.chatId = 'another-chat';
  invalid.lastRun = {runId, threadId, kind: 'chat', chatId: 'another-chat'};
  const browser = createBrowser({seedState: invalid, readyBeforeBootstrap: true, sdkReceipt: acceptedReceipt()});
  await waitFor(() => browser.elements.get('error')?.textContent.includes('identity is inconsistent')
    || browser.elements.get('recovery-error')?.textContent.includes('task_identity_mismatch'),
    'conflicting saved chat identity was not rejected');
  assert.equal(browser.counters.connect, 0);
  assert.equal(browser.counters.chatsOpen, 0);
  assertNoSubmission(browser);
});

test('startup does not infer an associated run or Chat from the latest host inventory', async () => {
  const browser = createBrowser({seedState: null, readyBeforeBootstrap: false,
    inventoryTasks: [{runId: 'unrelated-run', threadId: 'unrelated-chat', updatedAt: 999}],
    inventoryChats: [{chatId: 'chat-a', title: 'same title', updatedAt: 998},
      {chatId: 'chat-b', title: 'same title', updatedAt: 999}]});
  await waitFor(() => browser.elements.get('recovery')?.textContent.includes('No saved task or Chat'),
    'empty state was not shown');
  assert.equal(browser.counters.list, 0);
  assert.equal(browser.counters.chatsList, 0);
  assert.equal(browser.counters.chatsOpen, 0);
  assertNoSubmission(browser);
});


test('event with a different run or thread is rejected despite a matching task and accepted receipt', async () => {
  for (const wrong of [{threadId: 'other-thread'}, {runId: 'other-run'}]) {
    const events = successEvents();
    events[events.length - 1] = {...events.at(-1), ...wrong};
    const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt(), events, connectEvents: events, result: chatResult()});
    await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes('task_identity_mismatch'),
      `mismatching event was accepted: ${JSON.stringify(wrong)}`);
    assert.doesNotMatch(browser.elements.get('status').textContent, /Read-only reconnect succeeded/);
    assertNoSubmission(browser);
  }
});

test('late ready for an older run does not replace a newer explicitly selected run', async () => {
  const values = new Map(storageEntries(fixtureState({pending: false})));
  const sharedStorage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, String(value)); },
    removeItem(key) { values.delete(key); },
  };
  const browser = createBrowser({sharedStorage, holdGet: true, readyBeforeBootstrap: false});
  await waitFor(() => browser.counters.get === 1, 'old task lookup did not start');
  browser.context.newInput = {...savedInput(), runId: 'fresh-run-2', threadId: 'fresh-thread-2'};
  vm.runInContext("void submitInput(newInput, 'image')", browser.context);
  await waitFor(() => browser.counters.run === 1, 'new run was not submitted');
  browser.releaseGet();
  await waitFor(() => browser.counters.getReturned === 1, 'old task lookup did not finish');
  browser.dispatchReady(runId);
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.equal(browser.counters.connect, 0);
  assert.equal(browser.state().pending?.input.runId, 'fresh-run-2');
  assert.equal(browser.state().lastRun?.runId, 'fresh-run-2');
});

test('an exact accepted receipt can recover when fixture pending is lost and the saved selection names an older run', async () => {
  const oldRun = 'older-saved-run', oldThread = 'older-saved-thread';
  const values = new Map([
    [`${storagePrefix}.selectedRun`, oldRun],
    [`${storagePrefix}.run.${oldRun}`, JSON.stringify({version: 1, pending: null,
      lastRun: {runId: oldRun, threadId: oldThread, kind: 'image'}})],
  ]);
  const sharedStorage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, String(value)); },
    removeItem(key) { values.delete(key); },
  };
  const events = successEvents().map(event => event.type === 'RUN_FINISHED'
    ? {...event, result: {answer: 'KIWI-482'}} : event);
  const browser = createBrowser({sharedStorage, seedState: null, sdkReceipt: acceptedReceipt(),
    events, connectEvents: events, result: chatResult()});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'),
    'receipt was not recovered alongside the older selection');
  assert.equal(browser.counters.connect, 1);
  assert.equal(sharedStorage.getItem(`${storagePrefix}.selectedRun`), oldRun);
  assert.match(browser.elements.get('result').textContent, /KIWI-482/);
  assertNoSubmission(browser);
});

test('a ready event uses the exact other-tab record and refreshes this tab after terminal completion', async () => {
  const oldInput = {...savedInput(), runId: 'old-other-run', threadId: 'old-other-thread'};
  const initial = {version: 1, chatId: null, pending: {input: oldInput, kind: 'image'},
    lastRun: {runId: oldInput.runId, threadId: oldInput.threadId, kind: 'image'}};
  const values = new Map(storageEntries(initial));
  const sharedStorage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, String(value)); },
    removeItem(key) { values.delete(key); },
  };
  const events = successEvents().map(event => event.type === 'RUN_FINISHED'
    ? {...event, result: {answer: 'KIWI-482'}} : event);
  const browser = createBrowser({sharedStorage, seedState: null, readyBeforeBootstrap: false,
    events, connectEvents: events, result: chatResult()});
  await waitFor(() => browser.counters.get === 1, 'old local state was not checked');
  const input = {...savedInput(), threadId, runId};
  sharedStorage.setItem(`${storagePrefix}.run.${runId}`, JSON.stringify({version: 1,
    pending: {input, kind: 'chat', chatId: threadId}, lastRun: {runId, threadId, kind: 'chat', chatId: threadId}}));
  sharedStorage.setItem(`${storagePrefix}.selectedRun`, runId);
  browser.dispatchReady(runId);
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'),
    'the other-tab record was not recovered');
  await waitFor(() => vm.runInContext('state.pending === null && state.lastRun?.runId', browser.context) === runId,
    'the old tab did not refresh its own in-memory selection');
  assert.equal(browser.elements.get('retry-pending').hidden, true);
  assertNoSubmission(browser);
});

test('duplicate accepted SDK notices reconnect only once without a fixture input', async () => {
  const events = successEvents().map(event => event.type === 'RUN_FINISHED'
    ? {...event, result: {answer: 'KIWI-482'}} : event);
  const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt(), duplicateReady: true,
    events, connectEvents: events, result: chatResult()});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Read-only reconnect succeeded'),
    'accepted receipt was not recovered');
  browser.dispatchReady();
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.equal(browser.counters.connect, 1);
  assertNoSubmission(browser);
});

test('accepted receipt for failed, cancelled, or interrupted host task never starts or reconnects a run', async () => {
  for (const status of ['failed', 'cancelled', 'interrupted']) {
    const browser = createBrowser({seedState: null, sdkReceipt: acceptedReceipt(),
      task: {...successTask(), state: status, error: {code: status, message: 'synthetic terminal state'}},
      events: []});
    await waitFor(() => browser.elements.get('recovery-error')?.textContent.includes(status),
      `terminal ${status} state was not reported`);
    assert.equal(browser.counters.connect, 0);
    assertNoSubmission(browser);
  }
});

test('failed persistence before selection does not submit the model request', async () => {
  const values = new Map();
  const sharedStorage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) {
      if (key === `${storagePrefix}.selectedRun`) throw new Error('synthetic quota failure');
      values.set(key, String(value));
    },
    removeItem(key) { values.delete(key); },
  };
  const browser = createBrowser({sharedStorage, seedState: null, readyBeforeBootstrap: false});
  await waitFor(() => browser.elements.get('status')?.textContent.includes('Host Agent ready'), 'bootstrap did not finish');
  browser.context.newInput = {...savedInput(), runId: 'quota-run', threadId: 'quota-thread'};
  await vm.runInContext("submitInput(newInput, 'image').catch(() => {})", browser.context);
  assert.equal(sharedStorage.getItem(`${storagePrefix}.selectedRun`), null);
  assertNoSubmission(browser);
});
