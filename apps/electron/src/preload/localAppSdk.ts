import { LOCAL_APP_CREDENTIALS_HEADER } from '../main/local-apps/serviceCookies.js';

/** Injected before application code; it is kept in the main bundle so packaged
 * Local Apps do not depend on a writable SDK file or an untrusted CDN. */
export const LOCAL_APP_SDK = String.raw`(() => {
  // Electron's protocol Request omits Fetch's credentials mode. Carry only the
  // restrictive omit choice to the host adapter; it strips this marker before
  // forwarding any service request. Install this in same-origin frames too.
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
  if (window.top !== window) return;
  const bridge = window.__lingxiLocalAppBridge;
  if (!bridge || window.localApp) return;
  try { delete window.__lingxiLocalAppBridge; } catch { /* Electron may expose a non-configurable bridge property. */ }
  const MAX_FILES = 8, MAX_FILE = 25 * 1024 * 1024, MAX_TOTAL = 50 * 1024 * 1024, POLL_MS = 350;
  const terminal = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);
  const requestId = () => {
    if (crypto.randomUUID) return crypto.randomUUID();
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
    const hex = Array.from(bytes, value => value.toString(16).padStart(2, '0')).join('');
    return hex.slice(0, 8) + '-' + hex.slice(8, 12) + '-' + hex.slice(12, 16) + '-' + hex.slice(16, 20) + '-' + hex.slice(20);
  };
  const hostError = value => {
    const error = new Error(value && value.message ? value.message : String(value || 'The application host is unavailable'));
    if (value && value.code) error.code = value.code;
    return error;
  };
  const inputForHost = async input => {
    if (!input || typeof input !== 'object' || typeof input.prompt !== 'string') throw new TypeError('Expected an Agent prompt');
    const files = input.attachments === undefined ? [] : input.attachments;
    if (!Array.isArray(files) || files.length > MAX_FILES) throw new RangeError('Choose at most eight files');
    let total = 0;
    const attachments = [];
    for (const file of files) {
      if (!(file instanceof File)) throw new TypeError('Agent attachments must be File objects');
      if (file.size < 1 || file.size > MAX_FILE) throw new RangeError('Each Agent attachment must be 25 MiB or smaller');
      total += file.size;
      if (total > MAX_TOTAL) throw new RangeError('Agent attachments must total 50 MiB or less');
      // This conversion runs in Chromium's page world. Only a bounded
      // ArrayBuffer and display metadata cross the contextBridge boundary.
      attachments.push({ name: file.name, mediaType: file.type || 'application/octet-stream', size: file.size, bytes: await file.arrayBuffer() });
    }
    const result = { requestId: input.requestId === undefined ? requestId() : input.requestId, prompt: input.prompt };
    if (attachments.length) result.attachments = attachments;
    for (const key of ['outputSchema', 'tools', 'skills', 'maxTurns', 'timeoutMs']) if (input[key] !== undefined) result[key] = input[key];
    return result;
  };
  const taskFailure = task => hostError(task && task.error ? task.error : { code: task && task.state, message: 'Agent task ' + (task && task.state || 'failed') });
  const usageFromTask = value => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw hostError({ code: 'invalid_result', message: 'The host returned a successful task without usage data.' });
    const usage = {};
    for (const key of ['inputTokens', 'outputTokens', 'costUsd', 'turns']) {
      if (value[key] === undefined) continue;
      if (typeof value[key] !== 'number' || !Number.isFinite(value[key]) || value[key] < 0) throw hostError({ code: 'invalid_result', message: 'The host returned invalid task usage data.' });
      usage[key] = value[key];
    }
    if (value.costEstimated !== undefined) {
      if (typeof value.costEstimated !== 'boolean') throw hostError({ code: 'invalid_result', message: 'The host returned invalid task usage data.' });
      usage.costEstimated = value.costEstimated;
    }
    return usage;
  };
  const replyFromTask = task => {
    if (!task || task.state !== 'succeeded' || !task.result) throw taskFailure(task);
    if (typeof task.runId !== 'string' || typeof task.result.text !== 'string' || !Array.isArray(task.result.attachments) || task.result.attachments.length !== 0) {
      throw hostError({ code: 'invalid_result', message: 'The host returned an invalid successful task result.' });
    }
    return { runId: task.runId, text: task.result.text, ...(task.result.output === undefined ? {} : { output: task.result.output }), attachments: [], usage: usageFromTask(task.usage) };
  };
  const waitForTask = async runId => {
    for (;;) {
      const task = await bridge.agent.tasks.get(runId);
      if (terminal.has(task.state)) return task;
      await new Promise(resolve => setTimeout(resolve, POLL_MS));
    }
  };
  const taskApi = Object.freeze({
    start: async input => bridge.agent.tasks.start(await inputForHost(input)),
    get: runId => bridge.agent.tasks.get(runId),
    list: options => bridge.agent.tasks.list(options || {}),
    cancel: runId => bridge.agent.tasks.cancel(runId),
    subscribe: (runId, listener) => {
      if (typeof runId !== 'string' || typeof listener !== 'function') throw new TypeError('Expected a run ID and listener');
      const subscriptionId = requestId();
      let active = true, subscribed = false, lastSequence = -1;
      const stop = () => {
        if (!active) return;
        active = false; unlisten();
        if (subscribed) void bridge.agent.tasks.unsubscribe(subscriptionId).catch(() => undefined);
      };
      const unlisten = bridge.agent.tasks.onEvent(payload => {
        if (!active || !payload || payload.subscriptionId !== subscriptionId || !payload.event) return;
        const event = payload.event;
        if (event.runId !== runId) return;
        if (event.type === 'connection') {
          try { listener(event); } catch (error) { setTimeout(() => { throw error; }, 0); }
          stop();
          return;
        }
        if (event.type !== 'state' || !Number.isSafeInteger(event.sequence) || event.sequence <= lastSequence) return;
        lastSequence = event.sequence;
        try { listener(event); } catch (error) { setTimeout(() => { throw error; }, 0); }
        if (event.type === 'state' && terminal.has(event.task && event.task.state)) stop();
      });
      void bridge.agent.tasks.subscribe(runId, subscriptionId).then(() => {
        subscribed = true;
        if (!active) void bridge.agent.tasks.unsubscribe(subscriptionId).catch(() => undefined);
      }, error => {
        if (!active) return;
        try { listener({ type: 'connection', runId, error: { code: error && error.code || 'connection_failed', message: error && error.message || 'The Agent task connection failed' } }); }
        catch (listenerError) { setTimeout(() => { throw listenerError; }, 0); }
        finally { stop(); }
      });
      return stop;
    },
  });
  const chatHandle = chatId => Object.freeze({
    id: chatId,
    send: async input => {
      const admitted = await bridge.agent.chats.send(chatId, await inputForHost(input));
      const task = await waitForTask(admitted.runId), reply = replyFromTask(task);
      const result = task.result;
      if (!result.messageId || result.chatId !== chatId) throw hostError({ code: 'invalid_chat_result', message: 'The host returned an incomplete chat result' });
      return { ...reply, chatId, messageId: result.messageId };
    },
    history: options => bridge.agent.chats.history(chatId, options || {}),
    subscribe: listener => {
      if (typeof listener !== 'function') throw new TypeError('Expected a chat listener');
      let active = true, pending = false, lastState = '', lastHistory = '', lastError = '';
      const notify = event => { try { listener(event); } catch (error) { setTimeout(() => { throw error; }, 0); } };
      const stop = () => { if (!active) return; active = false; clearInterval(timer); };
      const poll = async () => {
        if (!active || pending) return;
        pending = true;
        try {
          const values = await Promise.all([bridge.agent.chats.get(chatId), bridge.agent.chats.history(chatId, { limit: 100 })]);
          if (!active) return;
          lastError = '';
          const stateKey = JSON.stringify(values[0]);
          if (stateKey !== lastState) { lastState = stateKey; notify({ type: 'state', chatId, chat: values[0] }); }
          const historyKey = JSON.stringify(values[1]);
          if (historyKey !== lastHistory) { lastHistory = historyKey; notify({ type: 'messages', chatId, messages: values[1].messages, ...(values[1].nextCursor ? { nextCursor: values[1].nextCursor } : {}) }); }
        } catch (error) {
          const message = error && error.message || 'The Agent chat connection failed';
          if (message !== lastError) {
            lastError = message;
            notify({ type: 'connection', chatId, error: { code: error && error.code || 'connection_failed', message } });
          }
        } finally { pending = false; }
      };
      const timer = setInterval(() => void poll(), 900);
      void poll();
      return stop;
    },
    cancel: () => bridge.agent.chats.cancel(chatId),
  });
  const chats = Object.freeze({
    create: async input => chatHandle((await bridge.agent.chats.create(input || {})).chatId),
    open: async chatId => { await bridge.agent.chats.open(chatId); return chatHandle(chatId); },
    list: options => bridge.agent.chats.list(options || {}),
    archive: chatId => bridge.agent.chats.archive(chatId),
    delete: chatId => bridge.agent.chats.delete(chatId),
  });
  const agent = Object.freeze({
    run: async input => { const started = await taskApi.start(input); return replyFromTask(await waitForTask(started.runId)); },
    tasks: taskApi,
    chats,
  });
  const files = Object.freeze({
    pick: async options => {
      const result = await bridge.files.pick(options || {});
      return (result && result.files || []).map(item => new File([item.bytes], item.name, { type: item.mediaType, lastModified: item.lastModified }));
    },
    pickDirectory: () => bridge.files.pickDirectory(),
    read: (root, path) => bridge.files.read(root, path),
    write: (root, path, text) => bridge.files.write(root, path, text),
    list: (root, path) => bridge.files.list(root, path === undefined ? '.' : path),
  });
  Object.defineProperty(window, 'localApp', { configurable: false, enumerable: true, writable: false, value: Object.freeze({
    info: () => bridge.info(), agent,
    clipboard: Object.freeze({ read: () => bridge.clipboard.read(), write: text => bridge.clipboard.write(text) }),
    notify: body => bridge.notify(body),
    process: Object.freeze({ exec: command => bridge.process.exec(command) }),
    files,
    serviceUrl: (name, path) => '/__services/' + encodeURIComponent(name) + '/' + (path || '/').replace(/^\/+/, ''),
    serviceSocketUrl: (name, path) => bridge.serviceSocketUrl(name, path || '/'),
  }) });
})();`;
