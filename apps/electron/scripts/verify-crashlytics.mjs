// Explicit, isolated acceptance test. Never target an existing user's process.
import { spawn } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const [appPath, confirmation] = process.argv.slice(2);
if (process.platform !== 'darwin' || !appPath || confirmation !== '--confirm-isolated-test-crash') {
  throw new Error('Usage: node scripts/verify-crashlytics.mjs <signed-app-path> --confirm-isolated-test-crash');
}
const executable = join(resolve(appPath), 'Contents', 'MacOS', 'LingXi Code');
if (!existsSync(executable)) throw new Error('Signed Desktop app not found');
const root = mkdtempSync(join(tmpdir(), 'lingxi-crashlytics-acceptance-'));
const home = join(root, 'home'), temporary = join(root, 'tmp'), userData = join(root, 'userData');
for (const directory of [home, temporary, userData]) mkdirSync(directory, { recursive: true, mode: 0o700 });
const logPath = join(userData, 'logs', 'crashes.jsonl');
const records = () => existsSync(logPath) ? readFileSync(logPath, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line)) : [];
const waitFor = async (check, milliseconds) => {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) { if (check()) return; await delay(200); }
  throw new Error(`Bounded acceptance timed out; evidence: ${root}`);
};
const spawnIsolated = (phase) => {
  const env = { ...process.env, HOME: home, CFFIXED_USER_HOME: home, TMPDIR: temporary,
    LINGXI_CRASHLYTICS_TEST_ROOT: root, LINGXI_CRASHLYTICS_TEST_TMP_BASE: tmpdir() };
  for (const key of ['ELECTRON_RUN_AS_NODE', 'ELECTRON_RENDERER_URL', 'NODE_OPTIONS']) delete env[key];
  const args = [`--user-data-dir=${userData}`, '-FIRDebugEnabled'];
  if (phase === 'capture') args.push('--verify-crashlytics-isolated');
  const child = spawn(executable, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  child.stdout.on('data', data => { output += data; });
  child.stderr.on('data', data => { output += data; });
  child.once('exit', () => writeFileSync(join(root, `${phase}.log`), output, { mode: 0o600 }));
  return { child, output: () => output };
};
let capture, restart;
try {
  capture = spawnIsolated('capture');
  await waitFor(() => records().some(event => event.event === 'crashlytics_started'), 30_000);
  await waitFor(() => capture.child.exitCode !== null || capture.child.signalCode !== null, 30_000);
  cpSync(join(home, 'Library', 'Caches', 'com.crashlytics.data'), join(root, 'captured-reports'), { recursive: true });
  restart = spawnIsolated('restart');
  await waitFor(() => records().some(event => event.previousExecutionCrashed === true), 30_000);
  await delay(45_000);
  writeFileSync(join(root, 'result.json'), JSON.stringify({ result: 'captured-and-recovered', appPath: resolve(appPath), root, records: records(), cloudReception: 'verify separately via Firebase report API' }, null, 2), { mode: 0o600 });
  console.log(JSON.stringify({ result: 'captured-and-recovered', root }));
} finally {
  for (const instance of [capture, restart]) {
    if (instance && instance.child.exitCode === null && instance.child.signalCode === null) {
      instance.child.kill('SIGTERM');
      await delay(2_000);
      if (instance.child.exitCode === null && instance.child.signalCode === null) instance.child.kill('SIGKILL');
    }
  }
}
