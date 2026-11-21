import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { prepareMcpServer } from '../src/main/mcpPrepare';
import { mcpPrepareRequest } from '../src/shared/mcpPrepare';
import { publicExtensionItems } from '../src/shared/publicExtensionDirectory';

// A stand-in MCP server. MODE: `answer` answers initialize; `stubborn` answers but ignores both stdin closing and
// SIGTERM; `silent` never answers; `exit` exits at once.
const SERVER = `
const fs = require('node:fs');
fs.appendFileSync(process.env.PIDS, process.pid + '\\n');
const mode = process.env.MODE;
if (mode === 'exit') process.exit(3);
if (mode === 'stubborn' || mode === 'silent') setInterval(() => {}, 1000);
if (mode === 'stubborn') process.on('SIGTERM', () => {});
let buffer = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  buffer += chunk;
  for (let newline = buffer.indexOf('\\n'); newline >= 0; newline = buffer.indexOf('\\n')) {
    const message = JSON.parse(buffer.slice(0, newline));
    buffer = buffer.slice(newline + 1);
    if (mode === 'silent') continue;
    process.stdout.write('starting\\n');
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: 0, result: {} }) + '\\n');
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2025-06-18', capabilities: {} } }) + '\\n');
  }
});
`;

// Runs the server under itself, as npx and uvx do.
const RUNNER = `
const { spawn } = require('node:child_process');
const fs = require('node:fs');
fs.appendFileSync(process.env.PIDS, process.pid + '\\n');
spawn(process.execPath, [process.env.SERVER], { stdio: 'inherit' }).on('exit', (code) => process.exit(code ?? 1));
`;

async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const directory = await mkdtemp(join(tmpdir(), 'mcp-prepare-'));
  const server = join(directory, 'server.js');
  const runner = join(directory, 'runner.js');
  await writeFile(server, SERVER);
  await writeFile(runner, RUNNER);
  const pids = join(directory, 'pids');
  const run = (script: string, mode: string, deadlineMs = 20_000) => prepareMcpServer({
    command: process.execPath, args: [script], cwd: directory, deadlineMs, stopGraceMs: 300,
    env: { ...process.env, MODE: mode, PIDS: pids, SERVER: server },
  });
  const started = async () => (await readFile(pids, 'utf8')).trim().split('\n').map(Number);
  // A server left running would hold the test's pipe open and hang the run instead of failing it.
  t.after(async () => {
    for (const pid of await started().catch(() => [])) {
      try {
        process.kill(pid, 'SIGKILL');
      } catch {
        // Already stopped.
      }
    }
    await rm(directory, { recursive: true, force: true });
  });
  return { server, runner, run, started };
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

// A killed grandchild stays a zombie until init reaps it, so give that a moment.
async function assertGone(pids: number[]) {
  for (let attempt = 0; attempt < 50 && pids.some(alive); attempt += 1) await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(pids.filter(alive), []);
}

test('a server that answers initialize is ready and is stopped', async (t) => {
  const { server, run, started } = await fixture(t);
  assert.equal(await run(server, 'answer'), 'ready');
  await assertGone(await started());
});

test('a server under a package runner that ignores stdin and SIGTERM is killed with its runner', async (t) => {
  const { runner, run, started } = await fixture(t);
  assert.equal(await run(runner, 'stubborn'), 'ready');
  const pids = await started();
  assert.equal(pids.length, 2);
  await assertGone(pids);
});

test('a server that never answers times out and is stopped', async (t) => {
  const { runner, run, started } = await fixture(t);
  assert.equal(await run(runner, 'silent', 500), 'timeout');
  await assertGone(await started());
});

test('a server that exits before answering, or a missing command, ends as exited', async (t) => {
  const { server, run } = await fixture(t);
  assert.equal(await run(server, 'exit'), 'exited');
  assert.equal(await prepareMcpServer({ command: 'lingxi-no-such-mcp-runner', args: [], cwd: tmpdir(), env: process.env, deadlineMs: 5_000 }), 'exited');
});

test('only npx and uvx stdio configurations are prepared, with their exact arguments and environment', () => {
  assert.deepEqual(mcpPrepareRequest({ command: 'npx', args: ['-y', 'pkg@1.0.0', '--readOnly'], env: { TOKEN: 'x' } }),
    { command: 'npx', args: ['-y', 'pkg@1.0.0', '--readOnly'], env: { TOKEN: 'x' } });
  assert.deepEqual(mcpPrepareRequest({ type: 'stdio', command: 'uvx', args: ['--from', 'pkg==1.0.0', 'pkg'] }),
    { command: 'uvx', args: ['--from', 'pkg==1.0.0', 'pkg'], env: {} });
  for (const config of [
    null, [], 'npx', { command: 'node', args: ['server.js'] }, { command: '/usr/local/bin/npx', args: [] },
    { type: 'http', url: 'https://example.com/mcp' }, { type: 'http', command: 'npx' }, { command: 'npx', args: 'pkg' },
    { command: 'npx', args: [1] }, { command: 'npx', args: ['a\0b'] }, { command: 'npx', env: { TOKEN: 1 } },
    { command: 'npx', env: { 'A=B': 'x' } }, { command: 'npx', env: { '': 'x' } }, { command: 'npx', env: [] },
    { command: 'npx', args: Array.from({ length: 257 }, () => 'a') },
  ]) assert.equal(mcpPrepareRequest(config), null, JSON.stringify(config));
});

test('every recommended npx and uvx server is prepared before it is saved', () => {
  const recipes = publicExtensionItems.flatMap((item) => item.mcp?.config.command ? [item.mcp.config] : []);
  assert(recipes.length > 0);
  for (const recipe of recipes) assert.notEqual(mcpPrepareRequest(recipe), null, JSON.stringify(recipe));
});
