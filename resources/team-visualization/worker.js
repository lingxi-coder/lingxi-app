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
  vm.runtime.setMemoryLimit(48 * 1024 * 1024);
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
  invoke('initialize', input);
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
