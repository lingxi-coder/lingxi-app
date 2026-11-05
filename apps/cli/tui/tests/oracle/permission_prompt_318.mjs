// Replays only hash-pinned permission timing predicates from the official
// executable. No Claude startup, terminal session, provider, or credentials.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import vm from 'node:vm';

const binary = readFileSync(process.argv[2]);
const sha256 = createHash('sha256').update(binary).digest('hex');
if (sha256 !== '677a2ea7bf576cedd20e191d973914ed8a41796f53a63b1923381ddc4d5b5e7d') {
  throw Error('Unexpected Claude Code baseline');
}
const ranges = {
  windows: [279075097, 279075126],
  within: [279075126, binary.indexOf('function o3n(', 279075126)],
  answerGuard: [296713746, 296714577],
  drawnAt: [279075357, binary.indexOf('function s(){return()=>{}}', 279075357)],
};
const within = binary.subarray(...ranges.within).toString();
const answerGuard = binary.subarray(...ranges.answerGuard).toString();
const drawnAt = binary.subarray(...ranges.drawnAt).toString();
if (!within.startsWith('function la(') || !answerGuard.startsWith('function No(')) {
  throw Error('Permission guard range drifted');
}
const constants = binary.subarray(279075097, 279075126).toString();
if (constants !== 'var OO=150,Co=250;var aJ=600;') {
  throw Error('Permission input grace constants drifted');
}
const inputs = [
  { name: 'before first question grace', shown_at: 0, now: 149 },
  { name: 'first question boundary', shown_at: 0, now: 150 },
  { name: 'after submit', shown_at: 10, submitted_at: 0, now: 599 },
  { name: 'submit boundary', shown_at: 10, submitted_at: 0, now: 600 },
  { name: 'escape before short boundary', shown_at: 0, submitted_at: 0, now: 149, escape: true },
  { name: 'escape short boundary', shown_at: 0, submitted_at: 0, now: 150, escape: true },
  { name: 'slow prompt still needs a new grace', shown_at: 30000, now: 30149 },
  { name: 'slow prompt boundary', shown_at: 30000, now: 30150 },
  { name: 'slow release frame keeps submit grace', submitted_at: 0, submission_drawn_at: 1000, shown_at: 1000, now: 1150 },
  { name: 'slow release frame before boundary', submitted_at: 0, submission_drawn_at: 1000, shown_at: 1000, now: 1599 },
  { name: 'slow release frame boundary', submitted_at: 0, submission_drawn_at: 1000, shown_at: 1000, now: 1600 },
  { name: 'slow release escape boundary', submitted_at: 0, submission_drawn_at: 1000, shown_at: 1000, now: 1150, escape: true },
  { name: 'earlier release frame is not restarted by question', submitted_at: 0, submission_drawn_at: 10, shown_at: 1000, now: 1150 },
];

// Replay Vk's actual subscription callback, as an already-mounted dialog host.
// A delayed terminal frame must advance the stamp; identity stubs miss this.
function releaseDrawnAt(test) {
  if (test.submitted_at === undefined) return (stamp) => stamp;
  let clock = 10000 + test.submitted_at;
  const listeners = [];
  const ref = { current: undefined };
  const hooks = vm.createContext({
    Date: { now: () => clock }, OO: 150, h1r: {},
    y: () => [0],
    R: (value) => { ref.current ??= value; return ref; },
    Ee: () => (callback) => { listeners.push(callback); return () => {}; },
    Ore: (effect) => effect(), Zt: (effect) => effect(), ue: (callback) => callback,
  });
  const lookup = vm.runInContext(`${drawnAt};Vk(${clock})`, hooks);
  clock = 10000 + (test.submission_drawn_at ?? test.submitted_at);
  for (const callback of listeners) callback();
  return lookup;
}
const cases = inputs.map((test) => {
  const now = 10000 + test.now;
  const payload = {};
  class Key { constructor(name) { this.name = name; } }
  const active = { id: 'permission', payload, shownAt: 10000 + test.shown_at };
  const context = vm.createContext({
    Date: { now: () => now },
    qe: () => now,
    je: { getState: () => ({ open: [active] }) },
    O: active,
    Oe: (stamp) => stamp,
    Hte: { currentEvent: new Key(test.escape ? 'escape' : 'return') },
    iun: class WrappedEvent {},
    nme: Key,
    bt: { current: undefined },
    Ke: { current: undefined },
    Ue: undefined,
    Ze: { current: null },
    bWt: () => undefined,
    Re: {
      displacedAt: 0,
      displaceDrawnAt: (stamp) => stamp,
      releasedAt: test.submitted_at === undefined ? 0 : 10000 + test.submitted_at,
      releaseDrawnAt: releaseDrawnAt(test),
      releaseGraceMs: test.submitted_at === undefined ? 150 : 600,
    },
  });
  const allowed = vm.runInContext(`${constants}${within}${answerGuard};No()`, context, { timeout: 1000 });
  return { ...test, allowed };
});
const fixture = {
  version: '2.1.318', sha256, ranges,
  scope: 'Visible permission answer callback with an active unchanged request, no scroll clipping or user-invoked bypass; first-paint/composer-release timing, including Native Vk subscription-frame replay. Selector refusal extension and terminal presentation are separately covered by Rust tests.',
  cases,
};
const target = new URL('../fixtures/permission_prompt_2_1_318.json', import.meta.url);
if (process.argv.includes('--write')) {
  writeFileSync(target, JSON.stringify(fixture, null, 2) + '\n');
} else if (JSON.stringify(JSON.parse(readFileSync(target))) !== JSON.stringify(fixture)) {
  throw Error('Permission input timing oracle differs from the committed fixture');
}
console.log(`Claude Code 2.1.318 permission input guard: ${cases.length} cases match`);
