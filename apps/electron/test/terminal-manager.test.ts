import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { TerminalManager, terminalEnvironment } from '../src/main/terminal';
function fixture(requestTimeoutMs?: number) {
  const child = new EventEmitter() as any;
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  const commands: any[] = [];
  let reply = true;
  const send = (value: unknown) => child.stdout.write(JSON.stringify(value) + '\n');
  child.stdin = new Writable({ write(chunk, _encoding, callback) {
    const command = JSON.parse(chunk.toString()); commands.push(command);
    if (reply) send({ kind: 'response', requestId: command.requestId }); callback();
  } });
  child.stdin.on('finish', () => { child.stdout.end(); child.emit('close', 0); });
  child.kill = () => { child.stdout.end(); child.emit('close', 0); return true; };
  let args: any;
  const manager = new TerminalManager({ serverBin: '/fake', requestTimeoutMs, spawnProcess: ((_bin: string, ...rest: any[]) => { args = rest; return child; }) as any });
  return { manager, child, commands, send, stopReply: () => { reply = false; }, args: () => args };
}
const scope = { projectPath: process.cwd(), sessionId: 'draft-a' };
const tick = () => new Promise(resolve => setImmediate(resolve));
test('credential-free environment and dedicated startup', async () => {
  assert.deepEqual(terminalEnvironment({ HOME: '/home/me', PATH: '/bin', OPENAI_API_KEY: 'secret', NODE_OPTIONS: 'injection' }), { HOME: '/home/me', PATH: '/bin', TERM: 'xterm-256color', COLORTERM: 'truecolor' });
  const f = fixture(); await f.manager.create(scope); assert.deepEqual(f.args()[0], ['--desktop-terminal']); assert.equal(f.commands[0].cwd, scope.projectPath); await f.manager.dispose();
});
test('scope isolation, migration and close preserve other sessions', async () => {
  const f = fixture(); const events: any[] = []; f.manager.onEvent(event => events.push(event));
  const a = await f.manager.create(scope); const b = await f.manager.create({ ...scope, sessionId: 'other' });
  const actual = { ...scope, sessionId: 'actual' }; f.manager.migrateScope(scope, actual);
  assert.equal(f.manager.list(scope).length, 0); assert.equal(f.manager.list(actual)[0].id, a.id);
  const snapshot = f.manager.get(a.id)!; snapshot.scope.sessionId = 'mutated'; assert.equal(f.manager.get(a.id)!.scope.sessionId, 'actual');
  await f.manager.closeScope(actual); assert.equal(f.manager.get(a.id), undefined); assert.ok(f.manager.get(b.id));
  assert.ok(events.some(e => e.kind === 'scope')); assert.ok(events.some(e => e.kind === 'closed')); await f.manager.dispose();
});
test('UTF-8 split frames, ordered bounded history and exit state', async () => {
  const f = fixture(); const a = await f.manager.create(scope); const events: any[] = []; f.manager.onEvent(event => events.push(event));
  const frame = Buffer.from(JSON.stringify({ kind: 'output', terminalId: a.id, data: '中文😀' }) + '\n');
  for (const byte of frame) f.child.stdout.write(Buffer.from([byte]));
  await tick(); assert.equal(f.manager.get(a.id)!.output, '中文😀');
  for (let i = 0; i < 24; i++) f.send({ kind: 'output', terminalId: a.id, data: 'a'.repeat(64 * 1024) });
  f.send({ kind: 'exit', terminalId: a.id, exitCode: 7 }); await tick();
  const result = f.manager.get(a.id)!; assert.equal(result.sequence, 25); assert.ok(result.output.length <= 1024 * 1024); assert.equal(result.exitCode, 7);
  assert.deepEqual(events.filter(e => e.kind === 'output').map(e => e.sequence), Array.from({ length: 25 }, (_, i) => i + 1));
  await assert.rejects(f.manager.input(a.id, 'x'), /not running/); await f.manager.dispose();
});
test('input and dimensions bounded, control characters preserved', async () => {
  const f = fixture(); const a = await f.manager.create(scope); await f.manager.input(a.id, '\u0003中文\t'); await f.manager.resize(a.id, 120, 32);
  assert.equal(f.commands[1].data, '\u0003中文\t'); assert.equal(f.commands[2].cols, 120);
  await assert.rejects(f.manager.input(a.id, 'a'.repeat(65537)), /too large/); await assert.rejects(f.manager.resize(a.id, 0, 10), /dimensions/); await assert.rejects(f.manager.resize(a.id, 80, 2.5), /dimensions/); await f.manager.dispose();
});
test('malformed oversized output closes broker and marks sessions exited', async () => {
  const f = fixture(); const a = await f.manager.create(scope); f.child.stdout.write('x'.repeat(256 * 1024 + 1)); await tick(); assert.equal(f.manager.get(a.id)!.status, 'exited'); await f.manager.dispose();
});
test('broker error stops sessions', async () => {
  const f = fixture(); const a = await f.manager.create(scope); f.child.emit('error', new Error('crash')); await tick(); assert.equal(f.manager.get(a.id)!.status, 'exited'); await f.manager.dispose();
});

test('pending commands are capped and service failure rejects all waiters', async () => {
  const f = fixture(); const a = await f.manager.create(scope); f.stopReply();
  const operations = Array.from({ length: 128 }, () => f.manager.input(a.id, 'x'));
  const settled = Promise.allSettled(operations);
  await assert.rejects(f.manager.input(a.id, 'overflow'), /Too many pending/);
  f.child.emit('error', new Error('failed'));
  assert.ok((await settled).every(result => result.status === 'rejected'));
  await f.manager.dispose();
});
test('per-session terminal count is bounded', async () => {
  const f = fixture();
  for (let i = 0; i < 8; i++) await f.manager.create(scope);
  await assert.rejects(f.manager.create(scope), /limit/);
  await f.manager.dispose();
});

test('request timeout closes stdin gracefully and rejects pending work', async () => {
  const f = fixture(20); const a = await f.manager.create(scope); f.stopReply();
  const keeper = setTimeout(() => {}, 100);
  await assert.rejects(f.manager.input(a.id, 'x'), /timed out/);
  assert.equal(f.child.stdin.writableEnded, true);
  assert.equal(f.manager.get(a.id)!.status, 'exited');
  clearTimeout(keeper); await f.manager.dispose();
});
test('dispose rejects queued requests and prevents subsequent creation', async () => {
  const f = fixture(); const a = await f.manager.create(scope); f.stopReply();
  const pending = f.manager.input(a.id, 'x'); const assertion = assert.rejects(pending, /disposed/);
  await f.manager.dispose(); await assertion;
  await assert.rejects(f.manager.create(scope), /disposed/);
});
test('closing an exited terminal releases the retained broker handle', async () => {
  const f = fixture(); const a = await f.manager.create(scope);
  f.send({ kind: 'exit', terminalId: a.id, exitCode: 0 }); await tick();
  await f.manager.close(a.id);
  assert.equal(f.commands.at(-1).kind, 'close'); assert.equal(f.commands.at(-1).terminalId, a.id);
  await f.manager.dispose();
});
test('renderer backpressure pauses ordered reading and suspends request timeout until all owners resume', async () => {
  const f = fixture(20); const a = await f.manager.create(scope);
  f.manager.setOutputPaused('window-a', true); f.manager.setOutputPaused('window-b', true);
  let settled = false; const resize = f.manager.resize(a.id, 100, 30).finally(() => { settled = true; });
  f.send({ kind: 'output', terminalId: a.id, data: 'preserve \u001b[31mred\u001b[0m' });
  await new Promise(resolve => setTimeout(resolve, 45));
  assert.equal(settled, false); assert.equal(f.manager.get(a.id)?.status, 'running'); assert.equal(f.manager.get(a.id)?.output, '');
  f.manager.setOutputPaused('window-a', false); await tick(); assert.equal(settled, false);
  f.manager.setOutputPaused('window-b', false); await resize; await tick();
  assert.equal(f.manager.get(a.id)?.output, 'preserve \u001b[31mred\u001b[0m');
  await f.manager.dispose();
});
test('dispose releases paused output without waiting for renderer acknowledgement', async () => {
  const f = fixture(); const a = await f.manager.create(scope); f.manager.setOutputPaused('window-a', true);
  f.send({ kind: 'output', terminalId: a.id, data: 'buffered' }); await tick();
  await f.manager.dispose(); assert.equal(f.manager.get(a.id), undefined);
});
