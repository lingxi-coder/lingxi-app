import { EventTarget, parseHTML } from 'linkedom/worker';
import * as d3 from 'd3';

const { document, Event } = parseHTML('<!doctype html><html><head></head><body></body></html>');
const emit = globalThis.__lingxiEmit;
const schedule = globalThis.__lingxiSchedule;
const cancel = globalThis.__lingxiCancel;
delete globalThis.__lingxiEmit;
delete globalThis.__lingxiSchedule;
delete globalThis.__lingxiCancel;
const windowEvents = new EventTarget();
let readyState = 'loading';
Object.defineProperty(document, 'readyState', { get: () => readyState });
Object.assign(globalThis, { document, Event, d3, window: globalThis, self: globalThis,
  addEventListener: windowEvents.addEventListener.bind(windowEvents),
  removeEventListener: windowEvents.removeEventListener.bind(windowEvents),
  dispatchEvent: windowEvents.dispatchEvent.bind(windowEvents),
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
let initialScripts;

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
function deliver(alias, subscription, value) {
  const entry = watchers.get(alias);
  if (!entry || subscription !== entry.subscription) return;
  entry.value = value;
  entry.ready = true;
  for (const callback of entry.callbacks) callback(value);
}

const data = Object.freeze({
  read(alias, options = {}) {
    binding(alias);
    return request('read', { alias, after: options.after, block: options.block });
  },
  watch(alias, callback) {
    binding(alias);
    if (typeof callback !== 'function') throw new Error('Invalid subscriber');
    let entry = watchers.get(alias);
    if (!entry) {
      const subscription = ++next;
      entry = { subscription, callbacks: new Set(), ready: false };
      watchers.set(alias, entry);
      const watched = entry;
      void request('watch', { alias, subscription }).then(value => {
        if (watchers.get(alias) === watched && !watched.ready) deliver(alias, subscription, value);
      }, () => { if (watchers.get(alias) === watched) watchers.delete(alias); });
    }
    const listener = value => callback(value);
    const unsubscribe = subscribe(entry.callbacks, listener);
    if (entry.ready) listener(entry.value);
    return () => {
      unsubscribe();
      if (watchers.get(alias) === entry && entry.callbacks.size === 0) {
        watchers.delete(alias);
        emit(JSON.stringify({ type: 'request', id: ++next, action: 'unwatch', alias, subscription: entry.subscription }));
      }
    };
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

const pixelMethods = new Set(['clearRect', 'fillRect', 'strokeRect', 'fillText', 'strokeText', 'fill', 'stroke']);
const pathMethods = new Set(['beginPath', 'closePath', 'moveTo', 'lineTo', 'arc', 'arcTo', 'ellipse',
  'rect', 'roundRect', 'bezierCurveTo', 'quadraticCurveTo']);
const complexState = new Set(['save', 'restore', 'clip', 'translate', 'rotate', 'scale', 'setTransform', 'resetTransform']);

function compactCanvas(commands) {
  const lastPath = commands.findLastIndex(([command]) => command === 'beginPath');
  const state = [], scopes = [];
  for (const [index, operation] of commands.entries()) {
    const [command] = operation;
    if (pixelMethods.has(command) || index < lastPath && pathMethods.has(command)) continue;
    if (command === 'save') scopes.push({ start: state.length, path: false });
    if (command === 'restore' && scopes.length) {
      const scope = scopes.pop();
      if (!scope.path) {
        state.length = scope.start;
        continue;
      }
      if (scopes.length) scopes.at(-1).path = true;
    }
    if (pathMethods.has(command) && scopes.length) scopes.at(-1).path = true;
    state.push(operation);
  }
  if (state.some(([command]) => complexState.has(command))) return state;
  const setters = new Map();
  let path = [];
  for (const operation of state) {
    const [command, property] = operation;
    if (command === 'beginPath') path = [operation];
    else if (pathMethods.has(command)) path.push(operation);
    else setters.set(command === 'set' ? property : command, operation);
  }
  return [...setters.values(), ...path];
}

function canvasContext(canvas) {
  if (canvases.has(canvas)) return canvases.get(canvas).context;
  const state = { commands: [], revision: 0 };
  const stack = [];
  let current = { identity: true, clipped: false, properties: {} };
  function push(command) {
    if (state.commands.length < 6000) {
      state.commands.push(command);
      state.revision++;
    }
  }
  state.context = new Proxy({}, {
    get(_, key) {
      if (key === 'canvas') return canvas;
      if (key === 'measureText') return text => ({ width: String(text).length * 7 });
      if (drawMethods.has(key)) return (...args) => {
        if (key === 'clearRect' && current.identity && !current.clipped && +args[0] === 0 && +args[1] === 0
            && args[2] >= (Number(canvas.getAttribute('width')) || 300)
            && args[3] >= (Number(canvas.getAttribute('height')) || 150)) {
          state.commands = compactCanvas(state.commands);
          state.revision++;
          return; // Replay starts from an empty backing buffer.
        }
        if (state.commands.length >= 6000) return;
        if (key === 'save') stack.push({ ...current, properties: { ...current.properties } });
        if (key === 'restore' && stack.length) current = stack.pop();
        if (key === 'clip') current.clipped = true;
        if (key === 'resetTransform') current.identity = true;
        if (key === 'setTransform') current.identity = args.length === 0 ||
          args.length === 6 && args.every((value, index) => Number(value) === [1, 0, 0, 1, 0, 0][index]);
        if (key === 'translate' && (Number(args[0]) !== 0 || Number(args[1]) !== 0)
            || key === 'rotate' && Number(args[0]) !== 0
            || key === 'scale' && (Number(args[0]) !== 1 || Number(args[1]) !== 1)) current.identity = false;
        push([key, ...args]);
      };
      return current.properties[key];
    },
    set(_, key, value) {
      if (drawProperties.has(key) && state.commands.length < 6000) {
        current.properties[key] = value;
        push(['set', key, value]);
      }
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

const select = Object.getPrototypeOf(document.createElement('select'));
const option = Object.getPrototypeOf(document.createElement('option'));
const emptySelections = new WeakSet();
function selectedOptions(node) {
  const options = Array.from(node.options);
  const explicit = options.flatMap((item, index) => item.hasAttribute('selected') ? [index] : []);
  if (node.hasAttribute('multiple')) return explicit;
  if (explicit.length) return explicit.slice(-1);
  if (emptySelections.has(node) || Number(node.getAttribute('size')) > 1) return [];
  const first = options.findIndex(item => !item.hasAttribute('disabled') &&
    !(item.parentElement?.localName === 'optgroup' && item.parentElement.hasAttribute('disabled')));
  return first >= 0 ? [first] : [];
}
function selectOptions(node, indices) {
  const selected = new Set(indices);
  emptySelections.add(node);
  Array.from(node.options).forEach((item, index) => {
    if (selected.has(index)) item.setAttribute('selected', '');
    else item.removeAttribute('selected');
  });
}
Object.defineProperties(select, {
  value: {
    get() { return this.options[this.selectedIndex]?.value ?? ''; },
    set(value) { selectOptions(this, [Array.from(this.options).findIndex(item => item.value === String(value))]); },
  },
  selectedIndex: {
    get() { return selectedOptions(this)[0] ?? -1; },
    set(index) { selectOptions(this, [Math.trunc(Number(index))]); },
  },
  selectedOptions: { get() { return selectedOptions(this).map(index => this.options[index]); } },
  multiple: {
    get() { return this.hasAttribute('multiple'); },
    set(value) { this.toggleAttribute('multiple', Boolean(value)); },
  },
});
Object.defineProperties(option, {
  value: {
    get() { return this.getAttribute('value') ?? this.textContent.trim().replace(/\s+/g, ' '); },
    set(value) { this.setAttribute('value', value); },
  },
  selected: {
    get() {
      const parent = this.closest('select');
      return parent ? parent.selectedOptions.includes(this) : this.hasAttribute('selected');
    },
    set(value) {
      const parent = this.closest('select');
      if (parent && !parent.multiple && value) {
        parent.selectedIndex = Array.from(parent.options).indexOf(this);
      } else {
        this.toggleAttribute('selected', Boolean(value));
      }
    },
  },
});

const inputControl = Object.getPrototypeOf(document.createElement('input'));
const checkedState = new WeakMap();
const inputType = node => node.getAttribute('type')?.toLowerCase();
const hasCheckedState = node => ['checkbox', 'radio'].includes(inputType(node));
function formOwner(node) {
  const id = node.getAttribute('form');
  return id ? document.getElementById(id) : node.closest('form');
}
function radioGroup(node) {
  const name = node.getAttribute('name');
  if (inputType(node) !== 'radio' || !name) return [];
  const form = formOwner(node);
  return [...document.querySelectorAll('input')].filter(item => inputType(item) === 'radio'
    && item.getAttribute('name') === name && formOwner(item) === form);
}
function inputChecked(node) {
  if (checkedState.has(node)) return checkedState.get(node);
  if (!node.defaultChecked) return false;
  const group = radioGroup(node);
  return !group.length || !group.includes(node) || group.filter(item => checkedState.get(item) ?? item.defaultChecked).at(-1) === node;
}

Object.defineProperties(inputControl, {
  defaultChecked: {
    get() { return this.hasAttribute('checked'); },
    set(value) { this.toggleAttribute('checked', Boolean(value)); },
  },
  checked: {
    get() { return inputChecked(this); },
    set(value) {
      if (value) for (const other of radioGroup(this)) if (other !== this) checkedState.set(other, false);
      checkedState.set(this, Boolean(value));
    },
  },
});

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
    const value = ['input', 'textarea'].includes(node.localName) ? String(node.value ?? '') : undefined;
    const checked = node.localName === 'input' && hasCheckedState(node) ? node.checked : undefined;
    const canvas = canvases.get(node);
    const selected = node.localName === 'select' ? selectedOptions(node) : undefined;
    return { id, tag: node.localName, attributes, children, value, checked, selected,
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
  if (input.css) {
    const style = document.createElement('style');
    style.textContent = input.css;
    document.body.prepend(style);
  }
  initialScripts = input.scripts;
}

function start() {
  const packages = initialScripts;
  initialScripts = undefined;
  const scripts = Array.from(document.querySelectorAll('script'));
  for (const script of scripts) {
    const source = script.getAttribute('src');
    const text = source ? packages[source] : script.textContent;
    script.remove();
    if (text && (!script.type || ['text/javascript', 'application/javascript'].includes(script.type))) (0, eval)(text);
  }
  readyState = 'interactive';
  document.dispatchEvent(new Event('readystatechange'));
  document.dispatchEvent(new Event('DOMContentLoaded', { bubbles: true }));
  windowEvents.dispatchEvent(new Event('DOMContentLoaded'));
  readyState = 'complete';
  document.dispatchEvent(new Event('readystatechange'));
  const loaded = new Event('load');
  windowEvents.dispatchEvent(loaded);
  if (typeof globalThis.onload === 'function') globalThis.onload(loaded);
}

Object.defineProperty(globalThis, '__lingxiSurface', { value: Object.freeze({
  initialize,
  start,
  snapshot,
  timer(id) {
    const timer = timers.get(id);
    if (!timer) return;
    if (!timer.repeat) timers.delete(id);
    timer.callback(...timer.args);
  },
  event({ id, kind, value, checked, selected, ...fields }) {
    const node = elements.get(id);
    if (!node) return;
    if (node.localName === 'select') {
      if (Array.isArray(selected)) selectOptions(node, selected);
    } else if (value !== undefined) node.value = value;
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
  data({ alias, subscription, value }) { deliver(alias, subscription, value); },
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
