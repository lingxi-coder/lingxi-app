#!/usr/bin/env node

import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { StringDecoder } from 'node:string_decoder';
import { fileURLToPath } from 'node:url';
import { stripVTControlCharacters } from 'node:util';

import { TerminalManager } from '../src/main/terminal.ts';
import { desktopArtifactPaths, formatError, packageRoot, parseDesktopTargetArgs } from './package-support.mjs';

const CHECKED_NAMES = ['LD_LIBRARY_PATH', 'LD_PRELOAD', 'DYLD_INSERT_LIBRARIES', 'TERMINAL_SMOKE_SECRET', 'DESKTOP_TERMINAL_SHELL_PATH', 'DESKTOP_TERMINAL_SHELL_PATH_PRESENT'];
const ABSENT = '__TERMINAL_ABSENT__';

export function terminalProbeCommand(platform, token) {
  if (!/^[a-f0-9]{24}$/.test(token)) throw new Error('invalid terminal probe token');
  if (platform === 'linux') {
    const variables = CHECKED_NAMES.map(name => `printf '${name}=%s\\n' "\${${name}-${ABSENT}}"`);
    return [`printf '\\n__LX_BEGIN_%s__\\n' '${token}'`, ...variables, `printf '__LX_END_%s__\\n' '${token}'`].join('; ') + '\n';
  }
  if (platform === 'win32') return `cmd.exe /d /c "echo __LX_BEGIN_${token}__ & set & echo __LX_END_${token}__"\r`;
  throw new Error(`unsupported terminal smoke platform: ${platform}`);
}

function probeLines(output, token) {
  const lines = stripVTControlCharacters(output).replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
  const begin = lines.indexOf(`__LX_BEGIN_${token}__`);
  const end = begin < 0 ? -1 : lines.indexOf(`__LX_END_${token}__`, begin + 1);
  return begin < 0 || end < 0 ? undefined : lines.slice(begin + 1, end);
}

export function verifyTerminalProbe(output, platform, token, expectedWindowsPath) {
  const lines = probeLines(output, token);
  if (!lines) throw new Error('terminal shell did not produce the complete probe output');
  const environment = new Map();
  for (const line of lines) {
    const separator = line.indexOf('=');
    if (separator > 0) environment.set(line.slice(0, separator).toUpperCase(), line.slice(separator + 1));
  }
  if (platform === 'linux') {
    for (const name of CHECKED_NAMES) if (environment.get(name) !== ABSENT) throw new Error(`terminal shell inherited ${name}`);
  } else if (platform === 'win32') {
    for (const name of CHECKED_NAMES) if (environment.has(name)) throw new Error(`terminal shell inherited ${name}`);
    if (environment.get('PATH') !== expectedWindowsPath) throw new Error('terminal shell PATH differs from its original value');
  } else throw new Error(`unsupported terminal smoke platform: ${platform}`);
}

export function verifyPackagedBrokerLaunch(binary, args, options, resourcesPath, platform, originalPath) {
  const name = platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
  if (resolve(binary) !== resolve(resourcesPath, 'bin', name) || args.length !== 1 || args[0] !== '--desktop-terminal') {
    throw new Error('terminal broker did not use the packaged executable and mode');
  }
  const env = options.env;
  if (!env) throw new Error('terminal broker environment is missing');
  for (const key of Object.keys(env)) {
    if (/^(?:LD_|DYLD_)/i.test(key) && !(platform === 'linux' && key === 'LD_LIBRARY_PATH')) {
      throw new Error(`terminal broker inherited loader variable ${key}`);
    }
    if (key.toUpperCase() === 'TERMINAL_SMOKE_SECRET') throw new Error('terminal broker inherited the test secret');
  }
  if (platform === 'linux') {
    if (env.LD_LIBRARY_PATH !== '$ORIGIN/../gstreamer/lib' || options.cwd !== undefined) {
      throw new Error('terminal broker did not use the packaged Linux loader environment');
    }
  } else if (platform === 'win32') {
    const marker = originalPath === undefined ? '0' : '1';
    if (env.PATH !== join(resourcesPath, 'gstreamer', 'bin')
      || env.DESKTOP_TERMINAL_SHELL_PATH_PRESENT !== marker
      || env.DESKTOP_TERMINAL_SHELL_PATH !== originalPath
      || options.cwd !== join(resourcesPath, 'bin')) {
      throw new Error('terminal broker did not use the packaged Windows loader environment');
    }
  } else throw new Error(`unsupported terminal smoke platform: ${platform}`);
}

export function createBrokerEofObserver() {
  // This frame is a statement by the tested Rust broker: the OS may delay
  // stdout delivery, so JS callback order alone cannot prove when it was
  // written. The Rust EOF tests verify its emission condition. The frame
  // reports completion of cleanup actions, not that all descendants exited.
  const decoder = new StringDecoder('utf8');
  let buffer = ''; let acknowledged = false; let invalid = false; let eofRequested = false;
  return {
    get acknowledged() { return acknowledged; },
    get invalid() { return invalid; },
    expectEof() { eofRequested = true; },
    push(chunk) {
      if (invalid) return;
      buffer += decoder.write(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
      let newline;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        if (newline > 256 * 1024) { invalid = true; return; }
        const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
        if (!line) continue;
        try {
          const frame = JSON.parse(line);
          if (frame && typeof frame === 'object' && !Array.isArray(frame) && frame.kind === 'shutdown' && frame.reason === 'eof') {
            if (!eofRequested) { invalid = true; return; }
            acknowledged = true;
          }
        } catch { invalid = true; return; }
      }
      if (buffer.length > 256 * 1024) invalid = true;
    },
    finish() { buffer += decoder.end(); if (buffer.trim()) invalid = true; },
  };
}

export async function runPackagedTerminalSmoke(resourcesPath, platform, arch, { spawnProcess = spawn, windowsShellFileExists } = {}) {
  if (!['linux', 'win32'].includes(platform) || process.platform !== platform || process.arch !== arch) {
    throw new Error(`native Linux or Windows ${platform}-${arch} host required for packaged terminal smoke`);
  }
  const root = mkdtempSync(join(tmpdir(), 'lingxi-packaged-terminal-'));
  const workspace = join(root, 'workspace');
  for (const name of ['workspace', 'home', 'appdata', 'localappdata', 'tmp']) mkdirSync(join(root, name));
  const token = randomBytes(12).toString('hex');
  const injected = `__TERMINAL_INJECTED_${token}__`;
  const overrides = {
    HOME: join(root, 'home'), USERPROFILE: join(root, 'home'), APPDATA: join(root, 'appdata'),
    LOCALAPPDATA: join(root, 'localappdata'), TMPDIR: join(root, 'tmp'), TMP: join(root, 'tmp'), TEMP: join(root, 'tmp'),
    LD_LIBRARY_PATH: injected, LD_PRELOAD: injected, DYLD_INSERT_LIBRARIES: injected, TERMINAL_SMOKE_SECRET: injected,
  };
  const saved = new Map(Object.keys(overrides).map(name => [name, process.env[name]]));
  const expectedWindowsPath = process.env.PATH;
  Object.assign(process.env, overrides);
  let child; let stderr = ''; let closed = false; let closeStatus;
  let shutdownRequested = false; let exitedBeforeShutdown = false; let killRequested = false;
  const eofObserver = createBrokerEofObserver();
  let resolveClose;
  const closeObserved = new Promise(resolvePromise => { resolveClose = resolvePromise; });
  const waitForClose = async () => {
    let timeout;
    try {
      return await Promise.race([closeObserved, new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error('terminal broker close was not observed')), 2_000);
      })]);
    } finally { clearTimeout(timeout); }
  };
  const manager = new TerminalManager({ isPackaged: true, resourcesPath, windowsShellFileExists, spawnProcess: (binary, args, options) => {
    verifyPackagedBrokerLaunch(binary, args, options, resourcesPath, platform, expectedWindowsPath);
    child = spawnProcess(binary, args, options);
    child.stderr.on('data', chunk => { stderr = (stderr + String(chunk)).slice(-16_384); });
    // Observe complete top-level protocol frames directly. The manager's
    // async reader can still be suspended when the child's close event fires.
    child.stdout.on('data', chunk => eofObserver.push(chunk));
    child.stdout.once('end', () => eofObserver.finish());
    const kill = child.kill;
    child.kill = function (...args) { killRequested = true; return kill.apply(this, args); };
    child.once('exit', () => { if (!shutdownRequested) exitedBeforeShutdown = true; });
    child.once('close', (code, signal) => { closed = true; closeStatus = { code, signal }; resolveClose(closeStatus); });
    return child;
  } });
  let failure;
  try {
    const terminal = await manager.create({ projectPath: workspace, sessionId: 'packaged-terminal-smoke' });
    await manager.input(terminal.id, terminalProbeCommand(platform, token));
    const deadline = Date.now() + 20_000;
    let output = '';
    while (Date.now() < deadline) {
      output = manager.get(terminal.id)?.output ?? '';
      if (probeLines(output, token)) break;
      await new Promise(resolvePromise => setTimeout(resolvePromise, 20));
    }
    verifyTerminalProbe(output, platform, token, expectedWindowsPath);
    await manager.close(terminal.id);
  } catch (error) { failure = error; }
  if (!failure && (!child || exitedBeforeShutdown || child.exitCode !== null || child.signalCode !== null || closed)) {
    failure = new Error('terminal broker exited before shutdown');
  }
  eofObserver.expectEof();
  shutdownRequested = true;
  try {
    await manager.dispose();
    if (child) {
      const status = await waitForClose();
      if (killRequested) throw new Error('terminal broker required forced termination');
      if (status.code !== 0 || status.signal !== null) throw new Error('terminal broker did not exit cleanly');
      if (!eofObserver.acknowledged || eofObserver.invalid) throw new Error('terminal broker EOF cleanup acknowledgement missing');
    }
  } catch (error) { failure ??= error; }
  finally {
    if (child && !closed) {
      try { child.kill('SIGKILL'); } catch { /* Preserve the original failure. */ }
      try { await waitForClose(); } catch { /* Report that shutdown was not observed. */ }
    }
    for (const [name, value] of saved) {
      if (value === undefined) delete process.env[name]; else process.env[name] = value;
    }
    rmSync(root, { recursive: true, force: true });
  }
  if (failure) throw new Error(`${failure instanceof Error ? failure.message : String(failure)}; broker exit=${closeStatus?.code ?? child?.exitCode ?? 'none'} signal=${closeStatus?.signal ?? child?.signalCode ?? 'none'} stderr=${JSON.stringify(stderr)}`, { cause: failure });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const target = parseDesktopTargetArgs(process.argv.slice(2));
    const paths = desktopArtifactPaths(packageRoot, target.platform, target.arch);
    await runPackagedTerminalSmoke(join(paths.payloadRoot, 'resources'), target.platform, target.arch);
    process.stdout.write(`[verify:terminal-smoke] OK ${target.id}\n`);
  } catch (error) {
    process.stderr.write(`[verify:terminal-smoke] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  }
}
