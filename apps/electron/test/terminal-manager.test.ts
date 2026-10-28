import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join, win32 } from 'node:path';
import { TerminalManager, terminalBrokerEnvironment, terminalBrokerLaunchOptions, terminalEnvironment, windowsShellExecutable, windowsShellSearchPaths, type TerminalManagerOptions } from '../src/main/terminal';
import type { TerminalSnapshot } from '../src/shared/terminal';
function fixture(requestTimeoutMs?: number, options: TerminalManagerOptions = {}) {
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
  const manager = new TerminalManager({ serverBin: '/fake', requestTimeoutMs, ...options, spawnProcess: ((_bin: string, ...rest: any[]) => { args = rest; return child; }) as any });
  return { manager, child, commands, send, stopReply: () => { reply = false; }, args: () => args };
}
const scope = { projectPath: process.cwd(), sessionId: 'draft-a' };
const tick = () => new Promise(resolve => setImmediate(resolve));
test('credential-free environment and dedicated startup', async () => {
  assert.deepEqual(terminalEnvironment({ HOME: '/home/me', PATH: '/bin', OPENAI_API_KEY: 'secret', NODE_OPTIONS: 'injection' }), { HOME: '/home/me', PATH: '/bin', TERM: 'xterm-256color', COLORTERM: 'truecolor' });
  assert.equal(terminalEnvironment({ pAtH: 'C:\\Mixed' }, 'win32').PATH, 'C:\\Mixed');
  const f = fixture(); await f.manager.create(scope); assert.deepEqual(f.args()[0], ['--desktop-terminal']); assert.equal(f.commands[0].cwd, scope.projectPath); await f.manager.dispose();
});
test('packaged terminal broker loads only app-owned native libraries and preserves the shell PATH', () => {
  const source = {
    HOME: '/home/me', PATH: 'C:\\user\\bin;;C:\\untrusted', LD_LIBRARY_PATH: '/injected::/empty-entry', LD_PRELOAD: '/injected.so',
    DYLD_INSERT_LIBRARIES: '/injected.dylib', DESKTOP_TERMINAL_SHELL_PATH: '1attacker',
    DESKTOP_TERMINAL_SHELL_PATH_PRESENT: '1', OPENAI_API_KEY: 'secret',
  };
  const resourcesPath = join(process.cwd(), 'application resources');
  const options = { isPackaged: true, resourcesPath };
  const linux = terminalBrokerEnvironment(options, source, 'linux');
  assert.equal(linux.LD_LIBRARY_PATH, '$ORIGIN/../gstreamer/lib');
  assert.equal(linux.PATH, source.PATH);
  assert.equal(linux.LD_PRELOAD, undefined);
  assert.equal(linux.DYLD_INSERT_LIBRARIES, undefined);
  assert.equal(linux.DESKTOP_TERMINAL_SHELL_PATH, undefined);
  assert.equal(linux.DESKTOP_TERMINAL_SHELL_PATH_PRESENT, undefined);
  assert.equal(linux.OPENAI_API_KEY, undefined);

  const windows = terminalBrokerEnvironment(options, source, 'win32');
  assert.equal(windows.PATH, join(resourcesPath, 'gstreamer', 'bin'));
  assert.equal(windows.DESKTOP_TERMINAL_SHELL_PATH, source.PATH);
  assert.equal(windows.DESKTOP_TERMINAL_SHELL_PATH_PRESENT, '1');
  assert.equal(windows.LD_LIBRARY_PATH, undefined);
  assert.equal(windows.LD_PRELOAD, undefined);
  assert.equal(windows.DYLD_INSERT_LIBRARIES, undefined);
  assert.equal(windows.OPENAI_API_KEY, undefined);
  assert.equal(terminalBrokerEnvironment(options, {}, 'win32').DESKTOP_TERMINAL_SHELL_PATH_PRESENT, '0');
  assert.equal(terminalBrokerEnvironment(options, { PATH: '' }, 'win32').DESKTOP_TERMINAL_SHELL_PATH_PRESENT, '1');
  assert.equal(terminalBrokerEnvironment(options, { PATH: '' }, 'win32').DESKTOP_TERMINAL_SHELL_PATH, '');
  assert.equal(terminalBrokerLaunchOptions(options, source, 'win32').cwd, join(resourcesPath, 'bin'));
  assert.equal(terminalBrokerLaunchOptions(options, source, 'linux').cwd, undefined);
  for (const unsafe of ['relative/path', join(resourcesPath, 'bad;path')]) {
    assert.throws(() => terminalBrokerEnvironment({ isPackaged: true, resourcesPath: unsafe }, source, 'win32'), /resources path/);
  }
  for (const odd of [join(resourcesPath, 'with:colon'), join(resourcesPath, 'with;semicolon'), join(resourcesPath, '$ORIGIN')]) {
    assert.equal(terminalBrokerEnvironment({ isPackaged: true, resourcesPath: odd }, source, 'linux').LD_LIBRARY_PATH, '$ORIGIN/../gstreamer/lib');
  }
  assert.equal(terminalBrokerEnvironment({ isPackaged: false }, source, 'linux').LD_LIBRARY_PATH, undefined);
});
test('Windows shell search resolves quoted, empty and drive-relative PATH entries before the broker changes cwd', async () => {
  const result = windowsShellSearchPaths('C:\\absolute;"C:\\with;separator";.;tools;;C:child;D:other', 'C:\\original');
  assert.deepEqual(result, ['C:\\absolute', 'C:\\with;separator', 'C:\\original', 'C:\\original\\tools', 'C:\\original', 'C:\\original\\child', 'D:\\other']);
  assert.equal(await windowsShellExecutable('tools;"C:\\with;separator"', 'C:\\original', candidate => candidate === 'C:\\with;separator\\pwsh.exe'), 'C:\\with;separator\\pwsh.exe');
  assert.equal(await windowsShellExecutable(undefined, 'C:\\original', () => true), 'C:\\original\\pwsh.exe');
  assert.equal(await windowsShellExecutable('', 'C:\\original', () => true), 'C:\\original\\pwsh.exe');
  assert.equal(await windowsShellExecutable(undefined, 'C:\\original', () => false), undefined);
});
test('Windows shell search ignores the Electron process hidden per-drive cwd', () => {
  const originalEnvironment = process.env;
  process.env = { ...originalEnvironment, '=D:': 'D:\\untrusted' };
  try {
    assert.equal(win32.resolve('C:\\original', 'D:tools'), 'D:\\untrusted\\tools');
    assert.deepEqual(windowsShellSearchPaths('D:tools;C:tools', 'C:\\original'), ['D:\\tools', 'C:\\original\\tools']);
    assert.deepEqual(windowsShellSearchPaths('C:tools;D:tools', '\\\\?\\C:\\original'), ['\\\\?\\C:\\original\\tools', 'D:\\tools']);
    assert.deepEqual(windowsShellSearchPaths('D:tools', '\\\\server\\share\\original'), ['D:\\tools']);
  } finally { process.env = originalEnvironment; }
});
test('long Windows PATH is preserved without expanded paths in the broker environment', async () => {
  const path = `${'.;'.repeat(1000)}tools`;
  const environment = terminalBrokerEnvironment({ isPackaged: true, resourcesPath: process.cwd() }, { PATH: path }, 'win32');
  assert.equal(environment.DESKTOP_TERMINAL_SHELL_PATH, path);
  assert.equal(environment.DESKTOP_TERMINAL_SHELL_PATH_PRESENT, '1');
  assert.equal(Object.keys(environment).some(name => name === 'DESKTOP_TERMINAL_SHELL_SEARCH'), false);
  assert.equal(await windowsShellExecutable(path, 'C:\\original', candidate => candidate === 'C:\\original\\tools\\pwsh.exe'), 'C:\\original\\tools\\pwsh.exe');
  const longest = 'a'.repeat(32_766);
  assert.equal(terminalBrokerEnvironment({ isPackaged: true, resourcesPath: process.cwd() }, { PATH: longest }, 'win32').DESKTOP_TERMINAL_SHELL_PATH?.length, longest.length);
});
test('Windows shell choice follows files available at each terminal creation', async () => {
  const available = new Set(['C:\\original\\tools\\pwsh.exe']);
  const find = () => windowsShellExecutable('tools', 'C:\\original', candidate => available.has(candidate));
  assert.equal(await find(), 'C:\\original\\tools\\pwsh.exe');
  available.clear(); available.add('C:\\original\\tools\\powershell.exe');
  assert.equal(await find(), 'C:\\original\\tools\\powershell.exe');
  available.clear();
  assert.equal(await find(), undefined);
});
if (process.platform === 'win32') test('Windows shell discovery finds a real file in a cwd-relative PATH', async (t) => {
  const directory = mkdtempSync(join(process.cwd(), 'terminal-native-search-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const program = join(directory, 'pwsh.exe');
  writeFileSync(program, 'fixture');
  assert.equal(await windowsShellExecutable(basename(directory), process.cwd()), program);
});
test('packaged Windows fallback sends only the original launch cwd as private context', async (t) => {
  const resourcesPath = mkdtempSync(join(tmpdir(), 'terminal-fallback-'));
  t.after(() => rmSync(resourcesPath, { recursive: true, force: true }));
  mkdirSync(join(resourcesPath, 'bin'));
  writeFileSync(join(resourcesPath, 'bin', process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server'), 'fixture');
  const descriptor = Object.getOwnPropertyDescriptor(process, 'platform')!;
  Object.defineProperty(process, 'platform', { value: 'win32' });
  try {
    for (const found of [false, true]) {
      const f = fixture(undefined, { isPackaged: true, resourcesPath, windowsShellFileExists: () => found });
      try {
        await f.manager.create(scope);
        const command = f.commands[0];
        assert.equal(command.shellSelection, true);
        assert.equal(command.shellLaunchCwd, found ? undefined : process.cwd());
        assert.equal(typeof command.shellProgram, found ? 'string' : 'undefined');
      } finally { await f.manager.dispose(); }
    }
  } finally { Object.defineProperty(process, 'platform', descriptor); }
});
test('slow Windows shell probes time out without blocking the event loop or launching unbounded filesystem work', async () => {
  const release: Array<(value: boolean) => void> = [];
  let timerRan = false;
  setTimeout(() => { timerRan = true; }, 5);
  const neverFinishes = () => new Promise<boolean>(resolve => release.push(resolve));
  const operations = Array.from({ length: 3 }, () => windowsShellExecutable('slow;later', 'C:\\original', neverFinishes, 30));
  const results = await Promise.allSettled(operations);
  assert.equal(timerRan, true, 'the main event loop remains responsive');
  assert.equal(release.length, 2, 'unresolved filesystem probes are globally bounded');
  assert.ok(results.every(result => result.status === 'rejected' && /timed out/.test(String(result.reason))));
  for (const resolve of release) resolve(false);
  await tick();
});
test('cancelled searches do not accumulate reactions on stuck Windows probes', async () => {
  const releases: Array<(value: boolean) => void> = [];
  const neverFinishes = () => new Promise<boolean>(resolve => releases.push(resolve));
  const originalThen = Promise.prototype.then;
  const reactions = new Map<Promise<unknown>, number>();
  Promise.prototype.then = function (this: Promise<unknown>, ...callbacks: Parameters<typeof originalThen>) {
    reactions.set(this, (reactions.get(this) ?? 0) + 1);
    return originalThen.apply(this, callbacks);
  } as typeof Promise.prototype.then;
  const active = [windowsShellExecutable('slow', 'C:\\original', neverFinishes, 10_000), windowsShellExecutable('slow', 'C:\\original', neverFinishes, 10_000)];
  const done = Promise.allSettled(active);
  try {
    await tick();
    assert.equal(releases.length, 2);
    for (let i = 0; i < 100; i++) {
      const controller = new AbortController();
      const waiting = windowsShellExecutable('slow', 'C:\\original', neverFinishes, 10_000, controller.signal);
      controller.abort();
      await assert.rejects(waiting, /cancelled|abort/i);
    }
    assert.ok(Math.max(...reactions.values()) < 20, 'retries must not subscribe repeatedly to any stuck probe');
  } finally {
    Promise.prototype.then = originalThen;
    for (const release of releases) release(true);
    await done;
  }
});
test('terminal broker requests reject frames exceeding the native protocol bound', async () => {
  const f = fixture();
  try {
    await assert.rejects((f.manager as any).request({ kind: 'create', terminalId: 'x', cwd: 'x'.repeat(512 * 1024) }), /request exceeds limit/);
    const terminal = await f.manager.create(scope);
    assert.equal(terminal.status, 'running');
  } finally { await f.manager.dispose(); }
});
test('the manager uses the packaged broker environment when spawning', async (t) => {
  const resourcesPath = mkdtempSync(join(tmpdir(), 'lingxi-terminal-resources-'));
  t.after(() => rmSync(resourcesPath, { recursive: true, force: true }));
  mkdirSync(join(resourcesPath, 'bin'));
  writeFileSync(join(resourcesPath, 'bin', process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server'), 'fixture');
  const options = { isPackaged: true, resourcesPath };
  const f = fixture(undefined, options);
  try {
    await f.manager.create(scope);
    assert.deepEqual(f.args()[1].env, terminalBrokerEnvironment(options));
    assert.equal(f.args()[1].cwd, terminalBrokerLaunchOptions(options).cwd);
  } finally { await f.manager.dispose(); }
});
test('closing or disposing during Windows shell discovery cannot start an unowned broker terminal', async (t) => {
  const resourcesPath = mkdtempSync(join(tmpdir(), 'terminal-discovery-'));
  t.after(() => rmSync(resourcesPath, { recursive: true, force: true }));
  mkdirSync(join(resourcesPath, 'bin'));
  writeFileSync(join(resourcesPath, 'bin', process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server'), 'fixture');
  const descriptor = Object.getOwnPropertyDescriptor(process, 'platform')!;
  const originalPath = process.env.PATH;
  Object.defineProperty(process, 'platform', { value: 'win32' });
  process.env.PATH = 'C:\\tools';
  try {
    for (const action of ['scope', 'project', 'dispose', 'timeout', 'broker'] as const) {
      let started!: () => void; let release!: (value: boolean) => void;
      const probeStarted = new Promise<void>(resolve => { started = resolve; });
      const gate = new Promise<boolean>(resolve => { release = resolve; });
      const f = fixture(undefined, {
        isPackaged: true, resourcesPath,
        windowsShellFileExists: () => { started(); return gate; },
        windowsShellSearchTimeoutMs: action === 'timeout' ? 20 : 5_000,
      });
      const creating = f.manager.create(scope);
      const rejected = assert.rejects(creating, action === 'timeout' ? /timed out/ : action === 'broker' ? /could not start/ : /cancelled/);
      try {
        await probeStarted;
        if (action === 'scope') await f.manager.closeScope(scope);
        if (action === 'project') await f.manager.closeProject(scope.projectPath);
        if (action === 'dispose') await f.manager.dispose();
        if (action === 'broker') f.child.emit('error', new Error('fixture startup failed'));
        await rejected;
        assert.deepEqual(f.manager.list(scope), []);
        assert.deepEqual(f.commands, [], `${action} must not send close or create for an unstarted terminal`);
        release(true); await tick();
        assert.deepEqual(f.commands, [], `${action} must ignore a late successful filesystem probe`);
      } finally { release(false); await f.manager.dispose(); }
    }
    const failing = fixture(undefined, { isPackaged: true, resourcesPath: join(resourcesPath, 'missing') });
    try {
      await assert.rejects(failing.manager.create(scope), /packaged bridge-server is missing/);
      assert.deepEqual(failing.manager.list(scope), []);
      assert.deepEqual(failing.commands, []);
    } finally { await failing.manager.dispose(); }
  } finally {
    if (originalPath === undefined) delete process.env.PATH; else process.env.PATH = originalPath;
    Object.defineProperty(process, 'platform', descriptor);
  }
});
test('closing while the broker is creating a terminal keeps protocol order and rejects the stale create', async () => {
  const f = fixture(); f.stopReply();
  const creating = f.manager.create(scope);
  const rejected = assert.rejects(creating, /cancelled/);
  try {
    await tick();
    const create = f.commands[0];
    assert.equal(create.kind, 'create');
    const closing = f.manager.close(create.terminalId);
    await tick();
    const close = f.commands[1];
    assert.equal(close.kind, 'close');
    assert.equal(close.terminalId, create.terminalId);
    f.send({ kind: 'response', requestId: create.requestId });
    await rejected;
    f.send({ kind: 'response', requestId: close.requestId });
    await closing;
    assert.equal(f.manager.get(create.terminalId), undefined);
  } finally { await f.manager.dispose(); }
});
test('terminal close remains bounded and succeeds when ordinary requests saturate the queue', async () => {
  for (const phase of ['creating', 'running'] as const) {
    const f = fixture();
    let creating: Promise<TerminalSnapshot> | undefined;
    let rejected: Promise<unknown> | undefined;
    let id: string;
    if (phase === 'creating') {
      f.stopReply();
      creating = f.manager.create(scope);
      rejected = assert.rejects(creating, /cancelled/);
      await tick(); id = f.commands[0].terminalId;
    } else {
      id = (await f.manager.create(scope)).id;
      f.stopReply();
    }
    const inputs = Array.from({ length: phase === 'creating' ? 127 : 128 }, () => f.manager.input(id, 'x'));
    const settled = Promise.allSettled(inputs);
    try {
      const closing = f.manager.close(id);
      assert.equal(f.manager.close(id), closing, 'concurrent closes share one bounded operation');
      await tick();
      const closeCommands = f.commands.filter(command => command.kind === 'close');
      assert.equal(closeCommands.length, 1, `${phase}: close must be sent despite saturation`);
      if (phase === 'creating') {
        f.send({ kind: 'response', requestId: f.commands[0].requestId });
        await rejected;
      }
      f.send({ kind: 'response', requestId: closeCommands[0].requestId });
      await closing;
      assert.equal(f.manager.get(id), undefined);
    } finally {
      await f.manager.dispose();
      await settled;
    }
  }
});
test('failed creations with close in flight remain counted until broker cleanup replies', async () => {
  const f = fixture(); f.stopReply();
  const closes: Array<Promise<void>> = [];
  try {
    for (let i = 0; i < 32; i++) {
      const creating = f.manager.create({ ...scope, sessionId: `failed-${i}` });
      const rejected = assert.rejects(creating, /fixture failure/);
      await tick();
      const create = f.commands.at(-1);
      closes.push(f.manager.close(create.terminalId));
      await tick();
      f.send({ kind: 'response', requestId: create.requestId, error: 'fixture failure' });
      await rejected;
    }
    await assert.rejects(f.manager.create(scope), /Terminal limit reached/);
    for (const command of f.commands.filter(command => command.kind === 'close')) f.send({ kind: 'response', requestId: command.requestId });
    await Promise.all(closes);
    assert.equal(f.manager.list(scope).length, 0);
  } finally { await f.manager.dispose(); }
});
test('unknown terminal close is safe, but other close errors stop the broker before rejection', async () => {
  const f = fixture();
  const a = await f.manager.create(scope); const b = await f.manager.create({ ...scope, sessionId: 'other' });
  f.stopReply();
  try {
    const unknown = f.manager.close(a.id);
    await tick();
    f.send({ kind: 'response', requestId: f.commands.at(-1).requestId, error: 'unknown terminal' });
    await unknown;
    assert.equal(f.manager.get(a.id), undefined);
    assert.equal(f.manager.get(b.id)?.status, 'running');
    assert.equal(f.child.stdin.writableEnded, false);
    const failed = f.manager.close(b.id);
    const rejected = assert.rejects(failed, /fixture close failure/);
    await tick();
    f.send({ kind: 'response', requestId: f.commands.at(-1).requestId, error: 'fixture close failure' });
    await rejected;
    assert.equal(f.child.stdin.writableEnded, true);
    assert.equal(f.manager.get(b.id)?.status, 'exited');
  } finally { await f.manager.dispose(); }
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
test('directory relocation and aliased draft adoption preserve terminals and other owners', async (t) => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-terminal-move-')));
  const source = join(directory, 'source'), target = join(directory, 'target');
  mkdirSync(source); mkdirSync(target);
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const [fromId, toId] of [['same-session', 'same-session'], ['__draft__', 'restored-session']]) {
    const f = fixture();
    try {
      const from = { projectPath: source, sessionId: fromId }, to = { projectPath: target, sessionId: toId };
      const owned = await f.manager.create(from);
      const other = await f.manager.create({ ...from, sessionId: 'other-source-session' });
      const targetOther = await f.manager.create({ ...to, sessionId: 'other-target-session' });
      f.send({ kind: 'output', terminalId: owned.id, data: 'existing shell output' }); await tick();
      const commandsBefore = f.commands.length;
      const events: any[] = []; f.manager.onEvent(event => events.push(event));
      f.manager.migrateScope(from, to);
      assert.deepEqual(f.manager.list(from), []);
      assert.equal(f.manager.list(to)[0]?.id, owned.id);
      assert.equal(f.manager.get(owned.id)?.output, 'existing shell output');
      assert.equal(f.manager.get(owned.id)?.status, 'running');
      assert.deepEqual(f.manager.get(other.id)?.scope, { ...from, sessionId: 'other-source-session' });
      assert.deepEqual(f.manager.get(targetOther.id)?.scope, { ...to, sessionId: 'other-target-session' });
      assert.equal(f.commands.length, commandsBefore, 'ownership migration must not restart or send commands to a running shell');
      assert.deepEqual(events, [{ kind: 'scope', terminalId: owned.id, scope: to }]);
      await f.manager.closeProject(source);
      assert.equal(f.manager.get(other.id), undefined);
      assert.equal(f.manager.get(owned.id)?.status, 'running');
      assert.equal(f.manager.get(targetOther.id)?.status, 'running');
    } finally { await f.manager.dispose(); }
  }
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
test('terminal output remains visible after the shell exit event', async () => {
  const f = fixture(); const terminal = await f.manager.create(scope);
  const events: any[] = []; f.manager.onEvent(event => events.push(event));
  f.send({ kind: 'exit', terminalId: terminal.id, exitCode: 7 });
  f.send({ kind: 'output', terminalId: terminal.id, data: 'late output 中文' });
  await tick();
  assert.equal(f.manager.get(terminal.id)?.status, 'exited');
  assert.equal(f.manager.get(terminal.id)?.exitCode, 7);
  assert.equal(f.manager.get(terminal.id)?.output, 'late output 中文');
  assert.deepEqual(events.map(event => event.kind), ['exit', 'output']);
  await assert.rejects(f.manager.input(terminal.id, 'x'), /not running/);
  await f.manager.dispose();
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
test('broker EOF acknowledgement remains a host-only frame', async () => {
  const f = fixture(); const events: unknown[] = [];
  f.manager.onEvent(event => events.push(event));
  const terminal = await f.manager.create(scope);
  f.send({ kind: 'shutdown', reason: 'eof' }); await tick();
  assert.deepEqual(events, []);
  assert.equal(f.manager.get(terminal.id)?.status, 'running');
  await f.manager.input(terminal.id, 'x');
  await f.manager.dispose();
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
