import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, statfsSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const scratch = mkdtempSync(join(tmpdir(), 'lingxi-teams-emulators-'));
const space = statfsSync(root);
if (space.bavail * space.bsize < 2 * 1024 ** 3) throw new Error('Team checks require at least 2 GiB free; no caches were deleted');
const config = JSON.parse(readFileSync(join(root, 'firebase.json'), 'utf8'));
const port = name => {
  const value = Number(process.env[name]);
  if (Number.isInteger(value) && value > 1024 && value <= 65535) return value;
  if (process.env[name]) throw new Error(`Invalid ${name}`);
};
const defaultDatabase = config.firestore.find(db => db.database === '(default)');
// firebase-tools' emulator does not load rules from a multi-database array.
// The suite checks production mapping and compiles each deny-all file separately.
writeFileSync(join(scratch, 'firebase.json'), JSON.stringify({
  firestore: { rules: resolve(root, defaultDatabase.rules), indexes: resolve(root, defaultDatabase.indexes) },
  emulators: {
    auth: { host: '127.0.0.1', port: port('TEAM_AUTH_PORT') ?? 9098 },
    firestore: { host: '127.0.0.1', port: port('TEAM_FIRESTORE_PORT') ?? 8085 },
    hub: { host: '127.0.0.1', port: 4408 }, logging: { host: '127.0.0.1', port: 4508 },
    ui: { enabled: false }, singleProjectMode: true,
  },
}, null, 2));
const javaHome = process.env.JAVA_HOME ?? '/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home';
const environment = { ...process.env, CI: '1', FIREBASE_CLI_DISABLE_UPDATE_CHECK: '1',
  JAVA_TOOL_OPTIONS: process.env.JAVA_TOOL_OPTIONS ?? '-Xms64m -Xmx384m',
  ...(existsSync(javaHome) ? { JAVA_HOME: javaHome, PATH: `${join(javaHome, 'bin')}:${process.env.PATH}` } : {}) };
const quote = value => `'${value.replaceAll("'", "'\"'\"'")}'`;
const testCommand = [
  `${quote(process.execPath)} --test --test-concurrency=1 ${quote(join(root, 'apps/firebase/test/teams.rules.emulator.test.mjs'))} ${quote(join(root, 'apps/firebase/test/teams.members.emulator.test.mjs'))} ${quote(join(root, 'apps/firebase/test/teams.messages.emulator.test.mjs'))}`,
  `${quote(process.execPath)} --import ${quote(join(root, 'packages/bridge-client/node_modules/tsx/dist/loader.mjs'))} --test ${quote(join(root, 'apps/electron/test/teams-firestore.emulator.test.ts'))} ${quote(join(root, 'apps/web/test/teams-firestore.emulator.test.ts'))} ${quote(join(root, 'apps/web/test/teams-members.emulator.test.ts'))}`,
].join(' && ');
console.log(`Isolated Team emulator project: demo-lingxi-teams-t0; logs: ${scratch}`);
const child = spawn('firebase', ['emulators:exec', '--only', 'auth,firestore', '--project', 'demo-lingxi-teams-t0',
  '--config', join(scratch, 'firebase.json'), '--non-interactive', process.env.TEAM_SPIKE_COMMAND ?? testCommand],
{ cwd: scratch, env: environment, stdio: 'inherit' });
process.on('SIGINT', () => child.kill('SIGINT'));
process.on('SIGTERM', () => child.kill('SIGTERM'));
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', (code, signal) => { process.exitCode = signal ? 1 : code ?? 1; });
