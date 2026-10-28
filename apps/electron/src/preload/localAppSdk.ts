import { browserAgentSource } from '@lingxi/local-app-agui/browser/source';
import { LOCAL_APP_CREDENTIALS_HEADER } from '../main/local-apps/serviceCookies.js';

/**
 * Injected into the app's page world. The public agent surface comes from the
 * pinned official AG-UI browser bundle; this adapter only binds its standard
 * RunAgentInput/events to the authenticated, app-scoped IPC resources.
 */
const credentialsBootstrap = String.raw`(() => {
  const nativeFetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    const request = input instanceof Request ? input : undefined;
    let target;
    try { target = new URL(request ? request.url : input, window.location.href); }
    catch { return nativeFetch(input, init); }
    if (target.protocol !== window.location.protocol || target.host !== window.location.host || target.username || target.password) return nativeFetch(input, init);
    const credentials = init && init.credentials !== undefined ? init.credentials : request ? request.credentials : 'same-origin';
    const headers = new Headers(init && init.headers !== undefined ? init.headers : request ? request.headers : undefined);
    if (credentials === 'omit') headers.set('${LOCAL_APP_CREDENTIALS_HEADER}', 'omit');
    else headers.delete('${LOCAL_APP_CREDENTIALS_HEADER}');
    return nativeFetch(input, { ...(init || {}), headers });
  };
})();`;

const localAppBootstrap = String.raw`(() => {
  if (window.top !== window || window.localApp) return;
  const bridge = window.__lingxiLocalAppBridge;
  const agui = window.LingxiLocalAppAGUI;
  if (!bridge || !agui || typeof agui.installBrowserAgent !== 'function') return;
  try { delete window.__lingxiLocalAppBridge; } catch { /* Keep the closed bridge only in this closure. */ }

  const chatIds = new Set();
  const chatCreateFlights = new Map();
  let chatCreationRevision;
  let chatCreationIdentityFlight;
  const chatCreationStore = 'creations';
  const maxChatCreations = 64;
  const textEncoder = new TextEncoder();
  const makeError = (message, code = 'agent_error', cause) => Object.assign(new Error(message, cause ? {cause} : undefined), {code});
  const terminalStates = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);
  const isRecord = value => !!value && typeof value === 'object' && !Array.isArray(value);
  const hostEventError = (runId, error) => window.dispatchEvent(new CustomEvent('localApp:agent-events-error', {
    detail: {runId, code: error && error.code || 'host_unavailable', message: error && error.message || 'Could not read AG-UI events.'}
  }));
  const chatHistoryError = (chatId, error) => window.dispatchEvent(new CustomEvent('localApp:chat-history-error', {
    detail: {chatId, code: error && error.code || 'host_unavailable', message: error && error.message || 'Could not read chat history.'}
  }));

  // Chat creation is a LingXi host operation, separate from AG-UI run
  // submission. Keep its unresolved identity independent of run/revision TTLs.
  async function chatCreationDatabase() {
    if (chatCreationRevision === undefined) {
      chatCreationIdentityFlight ||= bridge.info().then(info => {
        if (!info || info.id !== window.location.host || !Number.isSafeInteger(info.revision) || info.revision < 1) {
          throw makeError('The application identity is unavailable.', 'host_unavailable');
        }
        chatCreationRevision = info.revision;
      }).catch(error => { chatCreationIdentityFlight = undefined; throw makeError('The application identity is unavailable.', 'host_unavailable', error); });
      await chatCreationIdentityFlight;
    }
    const chatCreationDbName = 'lingxi-local-app-chat-creates-' + window.location.host + '-revision-' + chatCreationRevision;
    if (!('indexedDB' in window)) return Promise.reject(makeError('Persistent browser storage is unavailable.', 'storage_unavailable'));
    return new Promise((resolve, reject) => {
      const opening = indexedDB.open(chatCreationDbName, 1);
      opening.onupgradeneeded = () => {
        if (!opening.result.objectStoreNames.contains(chatCreationStore)) {
          opening.result.createObjectStore(chatCreationStore, {keyPath: 'key'});
        }
      };
      opening.onsuccess = () => resolve(opening.result);
      opening.onerror = () => reject(makeError('Could not open Chat recovery storage.', 'storage_unavailable', opening.error));
      opening.onblocked = () => reject(makeError('Chat recovery storage is busy in another tab.', 'storage_unavailable'));
    });
  }

  function chatCreationOptions(options) {
    if (!isRecord(options) || Object.keys(options).some(key => !['requestId', 'title'].includes(key))
        || typeof options.requestId !== 'string'
        || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(options.requestId)
        || (Object.hasOwn(options, 'title') && (typeof options.title !== 'string'
            || options.title.length > 512
            || textEncoder.encode(options.title).byteLength > 512
            || /[\u0000-\u001f\u007f-\u009f]/u.test(options.title)
            || [...options.title].some(char => {
              const code = char.codePointAt(0);
              return code >= 0xd800 && code <= 0xdfff;
            })
            || !options.title.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '')
            || [...options.title.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '')].length > 80))) {
      throw makeError('Chat creation requires a UUIDv4 requestId and an optional bounded title.', 'invalid_chat');
    }
    return {requestId: options.requestId, ...(Object.hasOwn(options, 'title') ? {title: options.title} : {})};
  }

  function sameChatCreation(record, options) {
    return record?.appId === window.location.host && record.revision === chatCreationRevision && record.key === options.requestId
      && record.requestId === options.requestId && record.options?.requestId === options.requestId
      && Object.hasOwn(record.options || {}, 'title') === Object.hasOwn(options, 'title')
      && record.options?.title === options.title;
  }

  async function reserveChatCreation(options) {
    const db = await chatCreationDatabase();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(chatCreationStore, 'readwrite');
      const store = tx.objectStore(chatCreationStore);
      let failure = null;
      let count = 0;
      const cursorRequest = store.openCursor();
      cursorRequest.onsuccess = () => {
        const cursor = cursorRequest.result;
        if (cursor) { count++; cursor.continue(); return; }
        const request = store.get(options.requestId);
        request.onsuccess = () => {
          const existing = request.result;
          if (existing) {
            if (!['pending', 'accepted', 'deleted', 'rejected'].includes(existing.state)) {
              failure = makeError('Chat recovery data is invalid.', 'storage_unavailable');
            } else if (!sameChatCreation(existing, options)) {
              failure = makeError('This Chat creation requestId is already bound to different input.', 'chat_create_conflict');
            } else if (existing.state !== 'pending') {
              // A new explicit submission is unresolved until this attempt returns.
              store.put({...existing, state: 'pending'});
            }
            return;
          }
          if (count >= maxChatCreations) {
            failure = makeError('Chat recovery storage is full. Forget a resolved creation before continuing.', 'storage_full');
            return;
          }
          store.add({key: options.requestId, appId: window.location.host, revision: chatCreationRevision, requestId: options.requestId,
            options, state: 'pending', createdAt: Date.now()});
        };
      };
      tx.oncomplete = () => { db.close(); failure ? reject(failure) : resolve(); };
      tx.onerror = () => { db.close(); reject(makeError('Could not save Chat creation for recovery.', 'storage_unavailable', tx.error)); };
      tx.onabort = () => { db.close(); reject(makeError('Could not save Chat creation for recovery.', 'storage_unavailable', tx.error)); };
    });
  }

  async function markChatCreationState(options, state) {
    const db = await chatCreationDatabase();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(chatCreationStore, 'readwrite');
      const store = tx.objectStore(chatCreationStore);
      const request = store.get(options.requestId);
      request.onsuccess = () => {
        if (sameChatCreation(request.result, options)) store.put({...request.result, state});
      };
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = () => { db.close(); reject(makeError('Could not update Chat recovery storage.', 'storage_unavailable', tx.error)); };
      tx.onabort = () => { db.close(); reject(makeError('Could not update Chat recovery storage.', 'storage_unavailable', tx.error)); };
    });
  }

  async function pendingChatCreations() {
    const db = await chatCreationDatabase();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(chatCreationStore, 'readonly');
      const records = [];
      let failure = null;
      const request = tx.objectStore(chatCreationStore).openCursor();
      request.onsuccess = () => {
        const cursor = request.result;
        if (!cursor) return;
        const record = cursor.value;
        let valid = false;
        try {
          const stored = record?.options;
          if (!stored || typeof stored !== 'object' || Array.isArray(stored)
              || Object.keys(stored).some(key => !['requestId', 'title'].includes(key))) throw new Error('invalid receipt');
          const options = chatCreationOptions({requestId: stored.requestId,
            ...(Object.hasOwn(stored, 'title') ? {title: stored.title} : {})});
          valid = sameChatCreation(record, options)
            && ['pending', 'accepted', 'deleted', 'rejected'].includes(record.state);
        } catch { /* report invalid recovery data below */ }
        if (!valid) {
          failure = makeError('Chat recovery data is invalid.', 'storage_unavailable');
        } else {
          records.push({...record.options, state: record.state});
        }
        cursor.continue();
      };
      tx.oncomplete = () => { db.close(); failure ? reject(failure) : resolve(records); };
      tx.onerror = () => { db.close(); reject(makeError('Could not read Chat recovery storage.', 'storage_unavailable', tx.error)); };
      tx.onabort = () => { db.close(); reject(makeError('Could not read Chat recovery storage.', 'storage_unavailable', tx.error)); };
    });
  }

  async function forgetChatCreation(requestId) {
    const options = chatCreationOptions({requestId});
    const db = await chatCreationDatabase();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(chatCreationStore, 'readwrite');
      const store = tx.objectStore(chatCreationStore);
      let failure = null;
      const request = store.get(options.requestId);
      request.onsuccess = () => {
        const record = request.result;
        if (record && record.appId === window.location.host && record.revision === chatCreationRevision && record.requestId === requestId
            && ['accepted', 'deleted', 'rejected'].includes(record.state)) {
          store.delete(requestId);
        } else if (record) {
          failure = makeError('Only resolved Chat creations can be forgotten.', 'chat_creation_unresolved');
        }
      };
      tx.oncomplete = () => { db.close(); failure ? reject(failure) : resolve(); };
      tx.onerror = () => { db.close(); reject(makeError('Could not remove Chat recovery data.', 'storage_unavailable', tx.error)); };
      tx.onabort = () => { db.close(); reject(makeError('Could not remove Chat recovery data.', 'storage_unavailable', tx.error)); };
    });
  }

  async function taskEvents(input) {
    if (!isRecord(input) || typeof input.runId !== 'string' || !input.runId
        || (input.afterSequence !== undefined && (!Number.isSafeInteger(input.afterSequence) || input.afterSequence < 0))) {
      throw new TypeError('tasks.events requires a runId and a nonnegative afterSequence.');
    }
    const page = await bridge.agent.tasks.events(input);
    if (!isRecord(page) || !Array.isArray(page.events) || !Number.isSafeInteger(page.sequence) || page.sequence < 0) {
      throw new TypeError('The host returned an invalid AG-UI event page.');
    }
    const events = page.events.map(event => agui.validateEvent(event));
    for (const event of events) {
      if ((event.type === 'RUN_STARTED' || event.type === 'RUN_FINISHED')
          && (event.runId !== input.runId || typeof event.threadId !== 'string')) {
        throw new TypeError('The host returned an event for a different AG-UI run.');
      }
    }
    return {events, sequence: page.sequence};
  }

  function subscribeTaskEvents(runId, listener) {
    if (typeof runId !== 'string' || !runId || typeof listener !== 'function') {
      throw new TypeError('tasks.subscribe requires a runId and listener.');
    }
    let active = true;
    let timer;
    let afterSequence;
    const stop = () => { active = false; clearTimeout(timer); };
    const poll = async () => {
      try {
        const page = await taskEvents(afterSequence === undefined ? {runId} : {runId, afterSequence});
        if (!active) return;
        for (const event of page.events) {
          listener(event);
          if (event.type === 'RUN_FINISHED' || event.type === 'RUN_ERROR') { stop(); return; }
        }
        afterSequence = page.sequence;
        const task = await bridge.agent.tasks.get(runId);
        if (terminalStates.has(task && task.state)) {
          hostEventError(runId, Object.assign(new Error('The host task ended without a terminal AG-UI event.'), {code: 'missing_terminal_event'}));
          stop();
          return;
        }
      } catch (error) {
        if (active) hostEventError(runId, error);
      }
      if (active) timer = setTimeout(poll, 500);
    };
    void poll();
    return stop;
  }

  function chatHandle(summary, expectedId) {
    const chatId = summary && summary.chatId;
    if (typeof chatId !== 'string' || !chatId || (expectedId !== undefined && chatId !== expectedId)) throw makeError('The host returned an invalid Chat ID.', 'invalid_host_response');
    chatIds.add(chatId);
    const handle = {
      id: chatId,
      send(input) {
        if (!input || input.threadId !== chatId) throw new TypeError('A chat RunAgentInput.threadId must equal the chat ID.');
        return agent.run(input);
      },
      async history(options = {}) {
        if (!isRecord(options) || Object.keys(options).some(key => key !== 'cursor' && key !== 'limit')) {
          throw new TypeError('Chat history options may contain only cursor and limit.');
        }
        const page = await bridge.agent.chats.history(chatId, options);
        if (!isRecord(page) || !Array.isArray(page.messages)
            || (page.nextCursor !== undefined && typeof page.nextCursor !== 'string')) {
          throw new TypeError('The host returned invalid chat history.');
        }
        const messages = page.messages.map(message => agui.validateMessage(message));
        return {messages, ...(page.nextCursor === undefined ? {} : {nextCursor: page.nextCursor})};
      },
      subscribe(listener) {
        if (typeof listener !== 'function') throw new TypeError('chat.subscribe requires a listener.');
        let active = true;
        let timer;
        // The host cursor is opaque. Keep the cursor that began the last page
        // we read, so the next poll revisits only that bounded tail page.
        let cursor;
        let pending = false;
        const seen = new Set();
        const seenOrder = [];
        const deliver = message => {
          if (seen.has(message.id)) return;
          seen.add(message.id);
          seenOrder.push(message.id);
          if (seenOrder.length > 200) seen.delete(seenOrder.shift());
          listener(message);
        };
        const stop = () => { active = false; clearTimeout(timer); };
        const poll = async () => {
          if (!active || pending) return;
          pending = true;
          try {
            let pageCursor = cursor;
            let page = await handle.history({limit: 50, ...(pageCursor === undefined ? {} : {cursor: pageCursor})});
            while (active) {
              for (const message of page.messages) deliver(message);
              if (page.nextCursor === undefined) {
                cursor = pageCursor;
                break;
              }
              pageCursor = page.nextCursor;
              cursor = pageCursor;
              page = await handle.history({cursor: pageCursor, limit: 50});
            }
          } catch (error) {
            if (active) chatHistoryError(chatId, error);
          } finally { pending = false; }
          if (active) timer = setTimeout(poll, 1000);
        };
        void poll();
        return stop;
      },
      cancel: () => bridge.agent.chats.cancel(chatId),
    };
    return Object.freeze(handle);
  }

  const tasks = Object.freeze({
    start(input) {
      const validated = agui.validateLocalAppRunInput(input);
      if (chatIds.has(validated.threadId)) throw new TypeError('This thread belongs to a persistent chat; use its chat handle.');
      return bridge.agent.tasks.start(validated);
    },
    get: runId => bridge.agent.tasks.get(runId),
    list: options => bridge.agent.tasks.list(options || {}),
    events: taskEvents,
    subscribe: subscribeTaskEvents,
    cancel: runId => bridge.agent.tasks.cancel(runId),
  });

  let agent;
  async function invokeChatResource(method, options) {
    let result;
    try { result = await bridge.agent.chats[method](options); }
    catch (cause) { throw makeError('The Agent host is unavailable.', 'host_unavailable', cause); }
    if (!isRecord(result) || typeof result.ok !== 'boolean') throw makeError('The Agent host returned an invalid response.', 'invalid_host_response');
    if (!result.ok) {
      const code = typeof result.error?.code === 'string' ? result.error.code : 'host_unavailable';
      const error = makeError(result.error?.message || 'The Agent host is unavailable.', code);
      error.definitive = ['invalid_chat', 'chat_create_conflict', 'chat_create_deleted'].includes(code);
      throw error;
    }
    if (result.value === undefined) throw makeError('The Agent host returned an invalid response.', 'invalid_host_response');
    return result.value;
  }

  async function createChat(rawOptions) {
    const options = chatCreationOptions(rawOptions);
    await reserveChatCreation(options);
    const existing = chatCreateFlights.get(options.requestId);
    if (existing) return existing;
    const flight = (async () => {
      const timer = setTimeout(() => window.dispatchEvent(new CustomEvent('localApp:chat-creation-pending', {
        detail: {requestId: options.requestId}
      })), 5000);
      try {
        const summary = await invokeChatResource('create', options);
        const handle = chatHandle(summary, options.requestId);
        // If the acknowledgement cannot be saved, the original pending record
        // remains recoverable. The caller still receives the confirmed Chat ID.
        try { await markChatCreationState(options, 'accepted'); } catch { /* retain pending */ }
        return handle;
      } catch (error) {
        if (error?.definitive && ['chat_create_deleted', 'chat_create_conflict', 'invalid_chat'].includes(error.code)) {
          try { await markChatCreationState(options, error.code === 'chat_create_deleted' ? 'deleted' : 'rejected'); }
          catch { /* retain pending */ }
        }
        if (['host_unavailable', 'chat_storage_uncertain', 'invalid_host_response'].includes(error?.code)) {
          error.definitive = false;
        }
        if (error && typeof error === 'object') error.requestId = options.requestId;
        throw error;
      } finally { clearTimeout(timer); }
    })();
    chatCreateFlights.set(options.requestId, flight);
    try { return await flight; }
    finally { if (chatCreateFlights.get(options.requestId) === flight) chatCreateFlights.delete(options.requestId); }
  }

  async function chatCreationStatus(rawOptions) {
    const options = chatCreationOptions(rawOptions);
    let summary;
    try { summary = await invokeChatResource('creationStatus', options); }
    catch (error) {
      if (error?.definitive && ['chat_create_deleted', 'chat_create_conflict', 'invalid_chat'].includes(error.code)) {
        try { await markChatCreationState(options, error.code === 'chat_create_deleted' ? 'deleted' : 'rejected'); }
        catch { /* retain pending */ }
      }
      throw error;
    }
    if (summary === null) return null;
    const handle = chatHandle(summary, options.requestId);
    try { await markChatCreationState(options, 'accepted'); } catch { /* status remains read-only on the host */ }
    return handle;
  }

  const chats = Object.freeze({
    create: createChat,
    creationStatus: chatCreationStatus,
    pendingCreations: pendingChatCreations,
    forgetCreation: forgetChatCreation,
    async open(chatId) { return chatHandle(await bridge.agent.chats.open(chatId)); },
    list: options => invokeChatResource('list', options || {}),
    archive: chatId => bridge.agent.chats.archive(chatId),
    async delete(chatId) {
      await bridge.agent.chats.delete(chatId);
      chatIds.delete(chatId);
    },
  });
  const transport = {
    start(input, options = {}) {
      if (options.persistentChat) {
        if (!chatIds.has(input.threadId)) throw new TypeError('Open the persistent chat before sending a message.');
        return bridge.agent.chats.send(input.threadId, input);
      }
      if (chatIds.has(input.threadId)) throw new TypeError('This thread belongs to a persistent chat; use its chat handle.');
      return bridge.agent.tasks.start(input);
    },
    events: taskEvents,
    status: ({runId}) => bridge.agent.tasks.get(runId),
    cancel: ({runId}) => bridge.agent.tasks.cancel(runId),
  };
  agent = agui.installBrowserAgent({transport, tasks, chats, isPersistentThread: threadId => chatIds.has(threadId)});

  function serviceUrl(name, path = '/') {
    if (typeof name !== 'string' || !name || typeof path !== 'string' || !path.startsWith('/')
        || path.startsWith('//') || path.includes('#') || path.includes('\\')) throw new TypeError('Invalid service URL.');
    let decoded;
    try { decoded = decodeURIComponent(path.split('?')[0]); } catch { throw new TypeError('Invalid service URL.'); }
    if (/%2f|%5c/iu.test(path.split('?')[0]) || decoded.includes('\\') || decoded.includes('\0')
        || decoded.split('/').some(part => part === '.' || part === '..')) throw new TypeError('Invalid service URL.');
    return '/__services/' + encodeURIComponent(name) + path;
  }

  const files = Object.freeze({
    async pick(options) {
      const result = await bridge.files.pick(options || {});
      if (!result || !Array.isArray(result.files)) throw new TypeError('The host returned an invalid File picker result.');
      return result.files.map(file => {
        if (!file || typeof file.name !== 'string' || !(file.bytes instanceof ArrayBuffer)) {
          throw new TypeError('The host returned invalid File data.');
        }
        return new File([file.bytes], file.name, {
          type: file.mediaType || 'application/octet-stream',
          lastModified: Number.isFinite(file.lastModified) ? file.lastModified : Date.now(),
        });
      });
    },
    pickDirectory: () => bridge.files.pickDirectory(),
    read: (root, path) => bridge.files.read(root, path),
    write: (root, path, text) => bridge.files.write(root, path, text),
    list: (root, path) => bridge.files.list(root, path === undefined ? '.' : path),
  });
  Object.defineProperty(window, 'localApp', {configurable: false, enumerable: true, writable: false,
    value: Object.freeze({
      info: () => bridge.info(),
      agent,
      clipboard: Object.freeze({read: () => bridge.clipboard.read(), write: text => bridge.clipboard.write(text)}),
      notify: body => bridge.notify(body),
      process: Object.freeze({exec: command => bridge.process.exec(command)}),
      files,
      serviceUrl,
      serviceSocketUrl: (name, path) => bridge.serviceSocketUrl(name, path || '/'),
    })
  });
})();`;

export const LOCAL_APP_SDK = `${credentialsBootstrap}\n${browserAgentSource}\n${localAppBootstrap}`;
