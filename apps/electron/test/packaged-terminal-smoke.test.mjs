import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { createBrokerEofObserver, runPackagedTerminalSmoke, terminalProbeCommand, verifyPackagedBrokerLaunch, verifyTerminalProbe } from '../scripts/packaged-terminal-smoke.mjs';

const token = '0123456789abcdef01234567';
const begin = `__LX_BEGIN_${token}__`;
const end = `__LX_END_${token}__`;
const absent = '__TERMINAL_ABSENT__';
const names = ['LD_LIBRARY_PATH', 'LD_PRELOAD', 'DYLD_INSERT_LIBRARIES', 'TERMINAL_SMOKE_SECRET', 'DESKTOP_TERMINAL_SHELL_PATH', 'DESKTOP_TERMINAL_SHELL_PATH_PRESENT'];

test('packaged terminal smoke verifies the actual broker executable and loader environment', () => {
  const resources = join(process.cwd(), 'package resources');
  const args = ['--desktop-terminal'];
  const linuxBin = join(resources, 'bin', 'bridge-server');
  const linux = { env: { LD_LIBRARY_PATH: '$ORIGIN/../gstreamer/lib' }, cwd: undefined };
  assert.doesNotThrow(() => verifyPackagedBrokerLaunch(linuxBin, args, linux, resources, 'linux'));
  assert.throws(() => verifyPackagedBrokerLaunch('/external/bridge-server', args, linux, resources, 'linux'), /packaged executable/);
  assert.throws(() => verifyPackagedBrokerLaunch(linuxBin, args, { ...linux, env: { ...linux.env, LD_PRELOAD: 'injected' } }, resources, 'linux'), /loader variable/);
  assert.throws(() => verifyPackagedBrokerLaunch(linuxBin, args, { ...linux, env: { LD_LIBRARY_PATH: '/external' } }, resources, 'linux'), /Linux loader environment/);
  assert.throws(() => verifyPackagedBrokerLaunch(linuxBin, args, { ...linux, cwd: '/external' }, resources, 'linux'), /Linux loader environment/);

  const winBin = join(resources, 'bin', 'bridge-server.exe');
  const windows = { env: { PATH: join(resources, 'gstreamer', 'bin'), DESKTOP_TERMINAL_SHELL_PATH_PRESENT: '1', DESKTOP_TERMINAL_SHELL_PATH: 'C:\\original' }, cwd: join(resources, 'bin') };
  assert.doesNotThrow(() => verifyPackagedBrokerLaunch(winBin, args, windows, resources, 'win32', 'C:\\original'));
  assert.throws(() => verifyPackagedBrokerLaunch(winBin, args, { ...windows, env: { ...windows.env, PATH: 'C:\\external' } }, resources, 'win32', 'C:\\original'), /Windows loader environment/);
  assert.throws(() => verifyPackagedBrokerLaunch(winBin, args, { ...windows, cwd: 'C:\\external' }, resources, 'win32', 'C:\\original'), /Windows loader environment/);
  assert.throws(() => verifyPackagedBrokerLaunch(winBin, args, { ...windows, env: { ...windows.env, TERMINAL_SMOKE_SECRET: 'injected' } }, resources, 'win32', 'C:\\original'), /test secret/);
});

test('terminal probe requires distinct shell output rather than its echoed command', () => {
  const command = terminalProbeCommand('linux', token);
  assert.throws(() => verifyTerminalProbe(command, 'linux', token), /complete probe output/);
  const output = `\r\n${begin}\r\n${names.map(name => `${name}=${absent}`).join('\r\n')}\r\n${end}\r\n`;
  assert.doesNotThrow(() => verifyTerminalProbe(output, 'linux', token));
  assert.throws(() => verifyTerminalProbe(output.replace(`LD_PRELOAD=${absent}`, 'LD_PRELOAD=injected'), 'linux', token), /LD_PRELOAD/);
  assert.throws(() => terminalProbeCommand('linux', 'invalid'), /invalid terminal probe token/);
  assert.throws(() => terminalProbeCommand('darwin', token), /unsupported terminal smoke platform/);
});

test('POSIX probe command verifies the real shell environment', {
  skip: process.platform === 'win32' ? 'requires a POSIX shell' : false,
}, () => {
  const command = terminalProbeCommand('linux', token);
  const clean = spawnSync('/bin/sh', [], { input: command, env: { PATH: '/usr/bin:/bin' }, encoding: 'utf8' });
  assert.equal(clean.status, 0, clean.stderr);
  assert.doesNotThrow(() => verifyTerminalProbe(clean.stdout, 'linux', token));
  const contaminated = spawnSync('/bin/sh', [], { input: command, env: { PATH: '/usr/bin:/bin', LD_LIBRARY_PATH: 'injected' }, encoding: 'utf8' });
  assert.equal(contaminated.status, 0, contaminated.stderr);
  assert.throws(() => verifyTerminalProbe(contaminated.stdout, 'linux', token), /LD_LIBRARY_PATH/);
});

test('Windows probe detects loader variables and requires the original PATH', () => {
  const command = terminalProbeCommand('win32', token);
  assert.throws(() => verifyTerminalProbe(command, 'win32', token, 'C:\\User\\bin'), /complete probe output/);
  const output = `\u001b[0m\r\n${begin}\r\nPath=C:\\User\\bin\r\n${end}\r\n`;
  assert.doesNotThrow(() => verifyTerminalProbe(output, 'win32', token, 'C:\\User\\bin'));
  assert.throws(() => verifyTerminalProbe(output, 'win32', token, 'C:\\Different'), /PATH differs/);
  assert.throws(() => verifyTerminalProbe(output.replace(end, `Ld_Preload=injected\r\n${end}`), 'win32', token, 'C:\\User\\bin'), /LD_PRELOAD/);
  assert.throws(() => verifyTerminalProbe(output.replace(end, `DESKTOP_TERMINAL_SHELL_PATH_PRESENT=1\r\n${end}`), 'win32', token, 'C:\\User\\bin'), /DESKTOP_TERMINAL_SHELL_PATH_PRESENT/);
  assert.doesNotThrow(() => verifyTerminalProbe(`\r\n${begin}\r\n${end}\r\n`, 'win32', token, undefined));
});

test('EOF acknowledgement must be a complete top-level broker frame', () => {
  const observer = createBrokerEofObserver();
  observer.push(Buffer.from(JSON.stringify({ kind: 'output', data: '{"kind":"shutdown","reason":"eof"}' }) + '\n'));
  assert.equal(observer.acknowledged, false);
  observer.expectEof();
  observer.push(Buffer.from('{"kind":"shut'));
  assert.equal(observer.acknowledged, false);
  observer.push(Buffer.from('down","reason":"eof"}\n'));
  observer.finish();
  assert.equal(observer.acknowledged, true);
  assert.equal(observer.invalid, false);
  const premature = createBrokerEofObserver(); premature.push(Buffer.from('{"kind":"shutdown","reason":"eof"}\n'));
  premature.expectEof(); assert.equal(premature.invalid, true); assert.equal(premature.acknowledged, false);
  const incomplete = createBrokerEofObserver(); incomplete.push(Buffer.from('{"kind":"shutdown"')); incomplete.finish();
  assert.equal(incomplete.invalid, true);
});

test('packaged smoke requires an observed clean broker exit and cleans up on failure', { timeout: 30_000 }, async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'terminal-smoke-fixture-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const resources = join(root, 'resources');
  mkdirSync(join(resources, 'bin'), { recursive: true });
  const platform = process.platform === 'win32' ? 'win32' : 'linux';
  writeFileSync(join(resources, 'bin', platform === 'win32' ? 'bridge-server.exe' : 'bridge-server'), '');
  const fixture = join(root, 'broker.cjs');
  writeFileSync(fixture, String.raw`
const readline = require('node:readline');
const fs = require('node:fs');
const path = require('node:path');
const [mode, platform, markers] = process.argv.slice(2);
if (mode === 'ignore-eof' || mode === 'handle-sigterm') setInterval(() => {}, 1000);
if (mode === 'handle-sigterm') process.on('SIGTERM', () => process.exit(0));
const input = readline.createInterface({ input: process.stdin });
input.on('line', line => {
  const message = JSON.parse(line);
  const response = { kind: 'response', requestId: message.requestId };
  if (mode === 'create-error' && message.kind === 'create') response.error = 'fixture create failed';
  if ((mode === 'early-close' || mode === 'premature-ack') && message.kind === 'close') {
    fs.writeFileSync(path.join(markers, mode + '.exit'), '1');
    process.stdout.write(JSON.stringify(response) + '\n', () => process.exit(0));
    return;
  }
  process.stdout.write(JSON.stringify(response) + '\n');
  if (mode === 'premature-ack' && message.kind === 'create') process.stdout.write(JSON.stringify({ kind: 'shutdown', reason: 'eof' }) + '\n');
  if (message.kind === 'input') {
    const token = message.data.match(/[a-f0-9]{24}/)[0];
    const environment = platform === 'linux'
      ? ${JSON.stringify(names)}.map(name => name + '=${absent}').join('\n')
      : (process.env.DESKTOP_TERMINAL_SHELL_PATH_PRESENT === '1' ? 'PATH=' + process.env.DESKTOP_TERMINAL_SHELL_PATH : '');
    const data = '__LX_BEGIN_' + token + '__\n' + environment + '\n__LX_END_' + token + '__\n' + (mode === 'spoofed-ack' ? '{"kind":"shutdown","reason":"eof"}\n' : '');
    process.stdout.write(JSON.stringify({ kind: 'output', terminalId: message.terminalId, data }) + '\n');
  }
});
input.on('close', () => {
  fs.writeFileSync(path.join(markers, mode + '.eof'), '1');
  if (mode === 'ignore-eof' || mode === 'handle-sigterm') return;
  if (mode === 'spoofed-ack') return process.exit(0);
  const ack = JSON.stringify({ kind: 'shutdown', reason: 'eof' }) + '\n';
  process.stdout.write(ack.slice(0, 9));
  setImmediate(() => process.stdout.write(ack.slice(9), () => process.exit(mode === 'nonzero' ? 37 : 0)));
});
`);
  const descriptor = Object.getOwnPropertyDescriptor(process, 'platform');
  const originalHome = process.env.HOME;
  Object.defineProperty(process, 'platform', { value: platform });
  try {
    const modes = ['clean', 'nonzero', 'ignore-eof', 'create-error', 'observed-early-exit', 'early-close', 'premature-ack', 'spoofed-ack'];
    if (platform !== 'win32') modes.push('handle-sigterm');
    for (const mode of modes) {
      let broker; let isolatedHome; const signals = [];
      const run = runPackagedTerminalSmoke(resources, platform, process.arch, {
        windowsShellFileExists: () => false,
        spawnProcess: (_binary, _args, options) => {
          isolatedHome = options.env.HOME;
          broker = spawn(process.execPath, [fixture, mode, platform, root], options);
          if (mode === 'observed-early-exit') broker.stdout.on('data', chunk => {
            if (String(chunk).includes('"kind":"output"')) broker.emit('exit', 0, null);
          });
          const kill = broker.kill.bind(broker);
          broker.kill = signal => { signals.push(signal ?? 'SIGTERM'); return kill(signal); };
          return broker;
        },
      });
      if (mode === 'clean') await run;
      else await assert.rejects(run, mode === 'create-error' ? /fixture create failed/ : mode === 'nonzero' ? /did not exit cleanly/ : mode === 'observed-early-exit' ? /exited before shutdown/ : mode === 'early-close' || mode === 'premature-ack' ? /acknowledgement missing|stopped|before shutdown/ : mode === 'spoofed-ack' ? /acknowledgement missing/ : /required forced termination/);
      assert.equal(existsSync(isolatedHome), false, `${mode}: isolated smoke files are removed`);
      assert.equal(process.env.HOME, originalHome, `${mode}: environment is restored`);
      assert.ok(broker.exitCode !== null || broker.signalCode !== null, `${mode}: broker has stopped`);
      if (mode === 'nonzero') assert.equal(broker.exitCode, 37);
      if (mode === 'early-close' || mode === 'premature-ack') {
        assert.equal(existsSync(join(root, mode + '.exit')), true);
        assert.equal(existsSync(join(root, mode + '.eof')), false);
      }
      if (mode === 'ignore-eof' || mode === 'handle-sigterm') assert.ok(signals.includes('SIGTERM'), 'the EOF-resistant broker required termination');
      if (mode === 'handle-sigterm') { assert.equal(broker.exitCode, 0); assert.equal(broker.signalCode, null); }
    }
  } finally { Object.defineProperty(process, 'platform', descriptor); }
});
