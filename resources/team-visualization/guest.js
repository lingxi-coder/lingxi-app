import { parseHTML } from 'linkedom/worker';
import * as d3 from 'd3';

const { document, Event } = parseHTML('<!doctype html><html><head></head><body></body></html>');
const emit = globalThis.__lingxiEmit;
const schedule = globalThis.__lingxiSchedule;
const cancel = globalThis.__lingxiCancel;
delete globalThis.__lingxiEmit;
delete globalThis.__lingxiSchedule;
delete globalThis.__lingxiCancel;
Object.assign(globalThis, { document, Event, d3, window: globalThis, self: globalThis,
  performance: { now: () => Date.now() },
  getComputedStyle: element => element.style,
  console: Object.freeze({ log() {}, info() {}, warn() {}, error() {} }),
});

const callbacks = new Map(), timers = new Map(), watchers = new Map(), themeListeners = new Set();
const stateListeners = new Set(), expandListeners = new Set(), suspendListeners = new Set();
const elementIds = new WeakMap(), elements = new Map(), canvases = new WeakMap();
let next = 0, renderWidth = 400, theme = { mode: 'light', tokens: {} }, locale = 'en', expanded = false;
let state = { version: 0, modelContent: null, privateContent: null };
let aliases = [];

function request(action, payload) {
  if (callbacks.size >= 16) return Promise.reject(new Error('Too many pending requests'));
  const id = ++next;
  return new Promise((resolve, reject) => {
    callbacks.set(id, { resolve, reject });
    emit(JSON.stringify({ type: 'request', id, action, ...payload }));
  });
}

function timer(callback, delay, repeat, args) {
  if (typeof callback !== 'function' || timers.size >= 32) return 0;
  const id = ++next;
  timers.set(id, { callback, repeat, args });
  schedule(id, Math.max(16, Number(delay) || 16), repeat);
  return id;
}
function stop(id) { timers.delete(id); cancel(id); }
Object.assign(globalThis, {
  setTimeout: (callback, delay, ...args) => timer(callback, delay, false, args),
  setInterval: (callback, delay, ...args) => timer(callback, delay, true, args),
  clearTimeout: stop,
  clearInterval: stop,
  requestAnimationFrame: callback => timer(() => callback(Date.now()), 16, false, []),
  cancelAnimationFrame: stop,
});

function subscribe(set, callback) {
  if (typeof callback !== 'function' || set.size >= 32) throw new Error('Invalid subscriber');
  set.add(callback);
  return () => set.delete(callback);
}
function binding(alias) {
  if (!aliases.includes(alias)) throw new Error('Unknown data binding');
}
const data = Object.freeze({
  read(alias, options = {}) {
    binding(alias);
    return request('read', { alias, after: options.after, block: options.block });
  },
  watch(alias, callback) {
    binding(alias);
    if (typeof callback !== 'function') throw new Error('Invalid subscriber');
    const set = watchers.get(alias) ?? new Set();
    watchers.set(alias, set);
    const unsubscribe = subscribe(set, callback);
    void request('watch', { alias }).then(result => { if (set.has(callback)) callback(result); });
    return unsubscribe;
  },
  patch(alias, changes) {
    binding(alias);
    return request('patch', { alias, changes });
  },
});
Object.defineProperty(globalThis, 'lingxi', { value: Object.freeze({
  data,
  get theme() { return theme; },
  get locale() { return locale; },
  get expanded() { return expanded; },
  get state() { return state; },
  async saveState(update) {
    const saved = await request('state', { expected: state.version, state: {
      modelContent: update.modelContent === undefined ? state.modelContent : update.modelContent,
      privateContent: update.privateContent === undefined ? state.privateContent : update.privateContent,
    } });
    state = saved;
    for (const callback of stateListeners) callback(state);
    return saved;
  },
  requestExpand: () => request('expand', {}),
  draftFollowup: text => request('draft', { text }),
  onTheme: callback => subscribe(themeListeners, callback),
  onExpanded: callback => subscribe(expandListeners, callback),
  onState: callback => subscribe(stateListeners, callback),
  onSuspend: callback => subscribe(suspendListeners, callback),
}) });

// The guest has a DOM without browser I/O. Only the bounded draw operations
// listed here are sent to the renderer's real CanvasRenderingContext2D.
const drawMethods = new Set(['clearRect', 'fillRect', 'strokeRect', 'fillText', 'strokeText', 'beginPath', 'closePath',
  'moveTo', 'lineTo', 'arc', 'arcTo', 'ellipse', 'rect', 'roundRect', 'bezierCurveTo',
  'quadraticCurveTo', 'fill', 'stroke', 'clip', 'save', 'restore', 'translate', 'rotate',
  'scale', 'setTransform', 'resetTransform', 'setLineDash']);
const drawProperties = new Set(['fillStyle', 'strokeStyle', 'lineWidth', 'font', 'textAlign', 'textBaseline',
  'globalAlpha', 'lineCap', 'lineJoin', 'globalCompositeOperation']);

function canvasContext(canvas) {
  if (canvases.has(canvas)) return canvases.get(canvas).context;
  const state = { commands: [], revision: 0 };
  function push(command) {
    if (state.commands.length < 6000) state.commands.push(command);
    state.revision++;
  }
  state.context = new Proxy({}, {
    get(target, key) {
      if (key === 'canvas') return canvas;
      if (key === 'measureText') return text => ({ width: String(text).length * 7 });
      if (drawMethods.has(key)) return (...args) => {
        if (key === 'clearRect' && +args[0] === 0 && +args[1] === 0
            && args[2] >= (Number(canvas.width) || 300) && args[3] >= (Number(canvas.height) || 150)) state.commands = [];
        push([key, ...args]);
      };
      return target[key];
    },
    set(target, key, value) {
      if (drawProperties.has(key)) { target[key] = value; push(['set', key, value]); }
      return true;
    },
  });
  canvases.set(canvas, state);
  return state.context;
}
const element = Object.getPrototypeOf(document.createElement('div'));
element.getBoundingClientRect = function () {
  const width = Number.parseFloat(this.getAttribute('width') ?? this.style?.width) || renderWidth;
  const height = Number.parseFloat(this.getAttribute('height') ?? this.style?.height) || 150;
  return { x: 0, y: 0, top: 0, left: 0, width, height, right: width, bottom: height };
};
const canvas = Object.getPrototypeOf(document.createElement('canvas'));
canvas.getContext = function (context) { return context === '2d' ? canvasContext(this) : null; };

function nodeId(node) {
  if (!elementIds.has(node)) elementIds.set(node, ++next);
  const id = elementIds.get(node);
  elements.set(id, node);
  return id;
}

function snapshot() {
  elements.clear();
  let count = 0;
  function serialize(node, depth) {
    if (++count > 2500 || depth > 48) return null;
    const id = nodeId(node);
    if (node.nodeType === 3) return { id, text: node.textContent.slice(0, 32768) };
    if (node.nodeType !== 1 || node.localName === 'script') return null;
    const attributes = Object.fromEntries(Array.from(node.attributes).slice(0, 64)
      .map(item => [item.name, item.value.slice(0, 32768)]));
    const children = Array.from(node.childNodes, child => serialize(child, depth + 1)).filter(Boolean);
    const value = ['input', 'select', 'textarea'].includes(node.localName) ? String(node.value ?? '') : undefined;
    const checked = node.localName === 'input' ? Boolean(node.checked) : undefined;
    const canvas = canvases.get(node);
    return { id, tag: node.localName, attributes, children, value, checked,
      canvas: canvas && { revision: canvas.revision, commands: canvas.commands } };
  }
  return Array.from(document.body.childNodes, node => serialize(node, 0)).filter(Boolean);
}

function initialize(input) {
  aliases = input.aliases;
  locale = input.locale;
  theme = input.theme;
  if (Number.isFinite(input.width)) renderWidth = input.width;
  expanded = input.expanded ?? false;
  state = input.state ?? state;
  document.documentElement.setAttribute('data-theme', theme.mode);
  if (/^\s*(?:(?:<!--[\s\S]*?-->)\s*)*<(?:!doctype\s+html|html|head|body)\b/iu.test(input.html)) {
    const parsed = parseHTML(input.html).document;
    const head = [...parsed.head.querySelectorAll('style,script')].map(node => node.outerHTML).join('');
    document.body.innerHTML = head + parsed.body.innerHTML;
  } else {
    document.body.innerHTML = input.html;
  }
  const scripts = Array.from(document.querySelectorAll('script'));
  for (const script of scripts) {
    const source = script.getAttribute('src');
    const text = source ? input.scripts[source] : script.textContent;
    script.remove();
    if (text && (!script.type || ['text/javascript', 'application/javascript'].includes(script.type))) (0, eval)(text);
  }
  document.dispatchEvent(new Event('DOMContentLoaded', { bubbles: true }));
}

Object.defineProperty(globalThis, '__lingxiSurface', { value: Object.freeze({
  initialize,
  snapshot,
  timer(id) {
    const timer = timers.get(id);
    if (!timer) return;
    if (!timer.repeat) timers.delete(id);
    timer.callback(...timer.args);
  },
  event({ id, kind, value, checked, ...fields }) {
    const node = elements.get(id);
    if (!node) return;
    if (value !== undefined) node.value = value;
    if (checked !== undefined) node.checked = checked;
    const event = new Event(kind, { bubbles: true, cancelable: true });
    Object.assign(event, fields);
    if (!node.dispatchEvent(event)) return;
    const submitter = kind === 'click' ? node.closest('button[type="submit"],button:not([type]),input[type="submit"]') : null;
    const enter = kind === 'keydown' && fields.key === 'Enter' && node.localName === 'input';
    const form = (submitter ?? (enter ? node : null))?.closest('form');
    if (form) {
      const submit = new Event('submit', { bubbles: true, cancelable: true });
      submit.submitter = submitter;
      form.dispatchEvent(submit);
    }
  },
  result({ id, value, error }) {
    const call = callbacks.get(id);
    if (!call) return;
    callbacks.delete(id);
    if (error) call.reject(new Error(error)); else call.resolve(value);
  },
  data({ alias, value }) { for (const callback of watchers.get(alias) ?? []) callback(value); },
  layout(input) {
    if (Number.isFinite(input.width)) renderWidth = input.width;
    if (input.theme?.mode && input.theme.mode !== theme.mode) {
      theme = input.theme;
      document.documentElement.setAttribute('data-theme', theme.mode);
      for (const callback of themeListeners) callback(theme);
    }
    if (input.expanded !== undefined && input.expanded !== expanded) {
      expanded = input.expanded;
      for (const callback of expandListeners) callback(expanded);
    }
  },
  suspend() { for (const callback of suspendListeners) callback(); },
}) });
