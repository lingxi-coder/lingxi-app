import { browserAgentSource } from '@lingxi/local-app-agui/browser/source';
import { LOCAL_APP_CREDENTIALS_HEADER } from '../main/local-apps/serviceCookies.js';
import { LOCAL_APP_AGENT_MAX_FILE_BYTES, LOCAL_APP_AGENT_MAX_FILES, LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES } from '../shared/localAppAgent.js';

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
  const terminalStates = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);
  const isRecord = value => !!value && typeof value === 'object' && !Array.isArray(value);
  const hostEventError = (runId, error) => window.dispatchEvent(new CustomEvent('localApp:agent-events-error', {
    detail: {runId, code: error && error.code || 'host_unavailable', message: error && error.message || 'Could not read AG-UI events.'}
  }));
  const chatHistoryError = (chatId, error) => window.dispatchEvent(new CustomEvent('localApp:chat-history-error', {
    detail: {chatId, code: error && error.code || 'host_unavailable', message: error && error.message || 'Could not read chat history.'}
  }));

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

  function chatHandle(summary) {
    const chatId = summary && summary.chatId;
    if (typeof chatId !== 'string' || !chatId) throw new TypeError('The host returned an invalid Chat ID.');
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
  const chats = Object.freeze({
    async create(options = {}) { return chatHandle(await bridge.agent.chats.create(options)); },
    async open(chatId) { return chatHandle(await bridge.agent.chats.open(chatId)); },
    list: options => bridge.agent.chats.list(options || {}),
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

// Browser-only bridge: all privileged operations stay on the Desktop host. Native
// UI and host filesystem APIs are deliberately unavailable in a remote page.
const remoteBridge = String.raw`(() => {
  if (window.top !== window) return;
  // Keep the host session cookie while preserving the app's logical omit mode.
  const hostFetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    let request;
    try { request = new Request(input, init); } catch { return hostFetch(input, init); }
    const target = new URL(request.url);
    if (target.origin !== location.origin || request.credentials !== 'omit') return hostFetch(request);
    const headers = new Headers(request.headers); headers.set('${LOCAL_APP_CREDENTIALS_HEADER}', 'omit');
    return hostFetch(new Request(request, {credentials:'same-origin', headers}));
  };
  const invoke = async (method, args = {}) => {
    const response = await fetch('/__lingxi_remote_invoke', {method:'POST', headers:{'content-type':'application/json'}, body:JSON.stringify({method,args})});
    if (!response.ok) throw new Error('Application operation is unavailable');
    return (await response.json()).result;
  };
  const unsupported = async () => { throw new Error('This native feature is unavailable in a remote browser'); };
  const task = (method, args) => invoke('agent.tasks.' + method, args);
  const chat = (method, args) => invoke('agent.chats.' + method, args);
  window.__lingxiLocalAppBridge = Object.freeze({
    info: () => invoke('info'),
    clipboard: Object.freeze({read: async () => { await invoke('capability.clipboard'); return navigator.clipboard.readText(); }, write: async text => { await invoke('capability.clipboard'); return navigator.clipboard.writeText(text); }}),
    notify: unsupported, process: Object.freeze({exec: unsupported}),
    files: Object.freeze({
      pick: options => new Promise((resolve, reject) => {
        const input = document.createElement('input'); input.type='file'; input.multiple=!!options.multiple;
        if (options.accept) input.accept=Array.isArray(options.accept) ? options.accept.join(',') : options.accept;
        const cleanup = () => { input.onchange = null; input.oncancel = null; };
        input.onchange = async () => {
          try {
            const selected = Array.from(input.files || []);
            if (selected.length > ${LOCAL_APP_AGENT_MAX_FILES} || (!options.multiple && selected.length > 1)) throw new Error('Too many files selected');
            if (selected.some(file => !Number.isSafeInteger(file.size) || file.size < 1 || file.size > ${LOCAL_APP_AGENT_MAX_FILE_BYTES})
                || selected.reduce((total, file) => total + file.size, 0) > ${LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES}) throw new Error('Selected files exceed the size limit');
            const files = await Promise.all(selected.map(async file => ({name:file.name,mediaType:file.type,size:file.size,lastModified:file.lastModified,bytes:await file.arrayBuffer()})));
            resolve({files});
          } catch (error) { reject(error); } finally { cleanup(); }
        };
        input.oncancel = () => { cleanup(); resolve({files:[]}); };
        try { input.click(); } catch (error) { cleanup(); reject(error); }
      }), pickDirectory: unsupported, read: unsupported, write: unsupported, list: unsupported,
    }),
    serviceSocketUrl: async (name, path = '/') => {
      if (!/^[\w-]+$/.test(name) || typeof path !== 'string' || !path.startsWith('/') || path.startsWith('//') || /[\\#]/.test(path)) throw new TypeError('Invalid service URL');
      return (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/__services/' + name + path;
    },
    agent: Object.freeze({tasks:Object.freeze({start:input=>task('start',{input}),get:runId=>task('get',{runId}),list:args=>task('list',args||{}),events:args=>task('events',args),cancel:runId=>task('cancel',{runId})}),
      chats:Object.freeze({create:args=>chat('create',args||{}),open:chatId=>chat('get',{chatId}),get:chatId=>chat('get',{chatId}),list:args=>chat('list',args||{}),archive:chatId=>chat('archive',{chatId}),delete:chatId=>chat('delete',{chatId}),history:(chatId,args)=>chat('history',{chatId,...args}),send:(chatId,input)=>chat('send',{chatId,input}),cancel:chatId=>chat('cancel',{chatId})})}),
  });
})();`;
export const REMOTE_LOCAL_APP_SDK = `${remoteBridge}\n${LOCAL_APP_SDK}`;
