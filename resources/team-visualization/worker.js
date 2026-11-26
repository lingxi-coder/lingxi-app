import { newQuickJSWASMModuleFromVariant } from 'quickjs-emscripten-core';
import variant from '@jitl/quickjs-singlefile-browser-release-sync';

const ready = newQuickJSWASMModuleFromVariant(variant);
const timers = new Map();
const numberFormats = new Map();
let vm;
let deadline = 0;
let frame = null;
let stopped = false;

function evaluate(code, milliseconds = 120) {
  deadline = performance.now() + milliseconds;
  const result = vm.evalCode(code);
  if (result.error) { result.error.dispose(); throw new Error('VISUALIZATION_SCRIPT_FAILED'); }
  return result.value;
}
function invoke(method, argument) {
  evaluate(`__lingxiSurface.${method}(${JSON.stringify(argument ?? null)})`).dispose();
  const jobs = vm.runtime.executePendingJobs(512);
  if (jobs.error) { jobs.error.dispose(); throw new Error('VISUALIZATION_SCRIPT_FAILED'); }
}
function emitFrame() {
  if (stopped) return;
  frame = setTimeout(() => {
    frame = null;
    try {
      const rendered = evaluate('JSON.stringify(__lingxiSurface.snapshot())');
      const json = vm.getString(rendered);
      rendered.dispose();
      if (json.length > 1024 * 1024) throw new Error('VISUALIZATION_TOO_LARGE');
      postMessage({ type: 'frame', nodes: JSON.parse(json) });
    } catch (error) { fail(error); }
  }, 16);
}
function fail(error) {
  stopped = true;
  postMessage({ type: 'error', code: error?.message === 'VISUALIZATION_TOO_LARGE' ? error.message : 'VISUALIZATION_SCRIPT_FAILED' });
  close();
}
function removeTimer(id) {
  clearTimeout(timers.get(id));
  clearInterval(timers.get(id));
  timers.delete(id);
}
function expose(name, fn) {
  const handle = vm.newFunction(name, fn);
  vm.setProp(vm.global, name, handle);
  handle.dispose();
}

async function initialize(input) {
  if (vm || stopped) return;
  const module = await ready;
  vm = module.newContext();
  const { html, css, scripts, ...settings } = input;
  const characters = html.length + css.length + Object.values(scripts).reduce((sum, text) => sum + text.length, 0);
  const mib = 1024 * 1024;
  vm.runtime.setMemoryLimit(Math.min(224 * mib, 48 * mib + 4 * characters));
  vm.runtime.setMaxStackSize(512 * 1024);
  vm.runtime.setInterruptHandler(() => performance.now() > deadline);
  expose('atob', value => vm.newString(atob(vm.getString(value))));
  expose('btoa', value => vm.newString(btoa(vm.getString(value))));
  expose('__lingxiNumberFormat', (number, locale, options) => {
    const language = vm.getString(locale), settings = vm.getString(options);
    if (language.length > 64 || settings.length > 512) throw new Error('Invalid number format');
    const key = JSON.stringify([language, settings]);
    let formatter = numberFormats.get(key);
    if (!formatter) {
      formatter = new Intl.NumberFormat(language, JSON.parse(settings));
      if (numberFormats.size >= 16) numberFormats.clear();
      numberFormats.set(key, formatter);
    }
    return vm.newString(formatter.format(vm.getNumber(number)));
  });
  evaluate(`(() => {
    const format = globalThis.__lingxiNumberFormat;
    delete globalThis.__lingxiNumberFormat;
    const NumberFormat = function(locale = 'en', options = {}) {
      const settings = JSON.stringify(options);
      return Object.freeze({ format: value => format(Number(value), String(locale), settings) });
    };
    globalThis.Intl = Object.freeze({ NumberFormat });
  })()`).dispose();
  expose('__lingxiEmit', payload => {
    const json = vm.getString(payload);
    if (json.length <= 128 * 1024) postMessage(JSON.parse(json));
  });
  expose('__lingxiSchedule', (rawId, rawDelay, rawRepeat) => {
    const id = vm.getNumber(rawId), delay = Math.min(60_000, Math.max(16, vm.getNumber(rawDelay)));
    const repeat = vm.dump(rawRepeat) === true;
    if (timers.size >= 32) return;
    const run = () => {
      if (!repeat) timers.delete(id);
      try { invoke('timer', id); if (frame === null) emitFrame(); } catch (error) { fail(error); }
    };
    timers.set(id, repeat ? setInterval(run, delay) : setTimeout(run, delay));
  });
  expose('__lingxiCancel', handle => removeTimer(vm.getNumber(handle)));
  evaluate(LINGXI_GUEST, 500).dispose();
  // Pass the large strings as VM values instead of compiling them as JS source.
  const fields = vm.newObject();
  const scriptSources = vm.newObject();
  const setString = (target, key, text) => {
    const handle = vm.newString(text);
    try { vm.setProp(target, key, handle); } finally { handle.dispose(); }
  };
  try {
    setString(fields, 'html', html);
    setString(fields, 'css', css);
    for (const [name, text] of Object.entries(scripts)) setString(scriptSources, name, text);
    vm.setProp(fields, 'scripts', scriptSources);
    vm.setProp(vm.global, '__lingxiInitialFields', fields);
    evaluate(`(() => {
      const fields = globalThis.__lingxiInitialFields;
      delete globalThis.__lingxiInitialFields;
      __lingxiSurface.initialize(Object.assign(${JSON.stringify(settings)}, fields));
    })()`, Math.min(40_000, 120 + 800 * characters / mib)).dispose();
  } finally {
    fields.dispose();
    scriptSources.dispose();
  }
  invoke('start');
  if (frame === null) emitFrame();
}

onmessage = async ({ data }) => {
  if (stopped) return;
  try {
    if (data.type === 'initialize') { await initialize(data.value); return; }
    if (!vm || !['event', 'result', 'data', 'layout', 'suspend'].includes(data.type)) return;
    invoke(data.type, data.value);
    if (frame === null) emitFrame();
  } catch (error) { fail(error); }
};
