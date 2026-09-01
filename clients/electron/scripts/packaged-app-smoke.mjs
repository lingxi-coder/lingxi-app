#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, readlinkSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, normalize, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { APP_NAME, BUNDLE_ID, artifactPaths, formatError, packageRoot, repoRoot } from './package-support.mjs';

export const APP_USER_DATA_SUBPATH = join('Library', 'Application Support', APP_NAME);

const SETTINGS_VERSION = 1;
const TRUST_CONFIG_PATHS = [
  '.mcp.json',
  '.claude/settings.json',
  '.claude/settings.local.json',
  '.lingxi/settings.json',
  '.lingxi/settings.local.json',
];
const TRUST_DIRECTORY_PATHS = [
  '.claude/agents',
  '.claude/commands',
  '.claude/plugins',
  '.claude/skills',
  '.lingxi/agents',
  '.lingxi/commands',
  '.lingxi/plugins',
  '.lingxi/skills',
];
const TRUST_MEMORY_PATHS = [
  'CLAUDE.md',
  'CLAUDE.local.md',
  'LINGXI.md',
  'LINGXI.local.md',
];
const MAX_TRUST_FINGERPRINT_ENTRIES = 512;
const MAX_TRUST_FINGERPRINT_BYTES = 512 * 1024;
const MAX_TRUST_FINGERPRINT_DEPTH = 12;

const LAUNCH_ENV_DENYLIST = new Set([
  'ANTHROPIC_API_KEY',
  'ANTHROPIC_AUTH_TOKEN',
  'ELECTRON_RENDERER_URL',
  'LINGXI_API_BASE_URL',
  'LINGXI_BRIDGE_SERVER_BIN',
  'OPENAI_API_KEY',
]);

function log(message) {
  process.stdout.write(`[verify:package:smoke] ${message}\n`);
}

function escaped(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function canonicalWorkspace(input) {
  if (typeof input !== 'string' || input.length === 0 || input.includes('\0')) {
    throw new Error('invalid workspace path');
  }
  const absolute = isAbsolute(input) ? normalize(input) : resolve(input);
  const canonical = realpathSync.native(absolute);
  if (!lstatSync(canonical).isDirectory()) throw new Error('workspace path is not a directory');
  return canonical;
}

export function workspaceTrustFingerprint(workspace) {
  const root = canonicalWorkspace(workspace);
  const hash = createHash('sha256');
  const budget = {
    remainingBytes: MAX_TRUST_FINGERPRINT_BYTES,
    remainingEntries: MAX_TRUST_FINGERPRINT_ENTRIES,
  };
  hash.update('lingxi-workspace-trust-v1\0');
  for (const relativePath of [...TRUST_CONFIG_PATHS, ...TRUST_MEMORY_PATHS, ...TRUST_DIRECTORY_PATHS]) {
    hashWorkspaceTrustPath(hash, root, relativePath, budget);
  }
  return hash.digest('hex');
}

function hashTrustBuffer(hash, budget, value) {
  if (value.byteLength > budget.remainingBytes) {
    throw new Error('workspace executable configuration exceeds the safe trust fingerprint limit');
  }
  hash.update(value);
  budget.remainingBytes -= value.byteLength;
}

function hashWorkspaceTrustPath(hash, root, relativePath, budget, depth = 0) {
  hash.update(relativePath);
  hash.update('\0');
  const path = join(root, relativePath);
  if (!existsSync(path)) {
    hash.update('absent\0');
    return;
  }
  if (budget.remainingEntries <= 0) {
    throw new Error('workspace executable configuration exceeds the safe trust fingerprint entry limit');
  }
  budget.remainingEntries -= 1;

  const metadata = lstatSync(path);
  if (metadata.isSymbolicLink()) {
    hash.update('symlink\0');
    hash.update(readlinkSync(path));
    hash.update('\0');
    const target = statSync(path);
    if (target.isFile()) {
      hash.update(`target-file:${target.size}\0`);
      hashTrustBuffer(hash, budget, readFileSync(path));
      hash.update('\0');
      return;
    }
    if (!target.isDirectory()) {
      hash.update(`target-non-file:${target.mode}\0`);
      return;
    }
    hash.update('target-dir\0');
    if (depth >= MAX_TRUST_FINGERPRINT_DEPTH) {
      throw new Error('workspace executable configuration exceeds the safe trust fingerprint depth');
    }
    for (const entry of readdirSync(path).sort((left, right) => left.localeCompare(right))) {
      hashWorkspaceTrustPath(hash, root, join(relativePath, entry), budget, depth + 1);
    }
    return;
  }
  if (metadata.isDirectory()) {
    hash.update('dir\0');
    if (depth >= MAX_TRUST_FINGERPRINT_DEPTH) {
      throw new Error('workspace executable configuration exceeds the safe trust fingerprint depth');
    }
    for (const entry of readdirSync(path).sort((left, right) => left.localeCompare(right))) {
      if (budget.remainingEntries <= 0) {
        throw new Error('workspace executable configuration exceeds the safe trust fingerprint entry limit');
      }
      hashWorkspaceTrustPath(hash, root, join(relativePath, entry), budget, depth + 1);
    }
    return;
  }
  if (!metadata.isFile()) {
    hash.update(`non-file:${metadata.mode}\0`);
    return;
  }
  hash.update(`file:${metadata.size}\0`);
  hashTrustBuffer(hash, budget, readFileSync(path));
  hash.update('\0');
}

export function createPackagedSettings({ workspace, theme = 'dark', now = new Date() }) {
  const canonical = canonicalWorkspace(workspace);
  return {
    version: SETTINGS_VERSION,
    theme,
    activeProject: canonical,
    projects: [canonical],
    pinnedSessions: [],
    trustedWorkspaces: {
      [canonical]: {
        fingerprint: workspaceTrustFingerprint(canonical),
        trustedAt: now.toISOString(),
      },
    },
    // Pre-accept Bypass Permissions for the automated profile: the smoke driver
    // issues the raw `set_permission_mode: bypassPermissions` command and has no
    // UI to click the (now-required) acceptance dialog. Persisting the
    // acceptance makes the main-process gate pass without prompting — exactly the
    // "already accepted, don't re-prompt" path (security #34).
    bypassPermissionsModeAccepted: true,
  };
}

export function runtimePathsForHome(homeDir) {
  const userDataDir = join(homeDir, APP_USER_DATA_SUBPATH);
  return {
    homeDir,
    userDataDir,
    settingsPath: join(userDataDir, 'settings.v1.json'),
    diagnosticsPath: join(userDataDir, 'logs', 'desktop.jsonl'),
    bridgeRuntimeDir: join(userDataDir, 'bridge-runtime'),
  };
}

export function sanitizePackagedAppEnvironment(source, overrides = {}) {
  const env = {};
  for (const [name, value] of Object.entries(source)) {
    if (value === undefined) continue;
    if (LAUNCH_ENV_DENYLIST.has(name)) continue;
    env[name] = value;
  }
  for (const [name, value] of Object.entries(overrides)) {
    if (value === undefined) delete env[name];
    else env[name] = value;
  }
  return env;
}

export function isKeylessProviderCredentialSnapshot(providerCredentials) {
  return Array.isArray(providerCredentials)
    && providerCredentials.every((entry) => entry?.configured === false);
}

function writeJson(path, value) {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
}

function writeText(path, value) {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(path, value, { encoding: 'utf8', mode: 0o600 });
}

function delay(ms) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

async function waitFor(predicate, { timeoutMs = 20_000, intervalMs = 100, label = 'condition' } = {}) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  for (;;) {
    try {
      const value = await predicate();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    if (Date.now() >= deadline) {
      throw new Error(`${label} did not become ready within ${timeoutMs}ms${lastError ? `: ${formatError(lastError)}` : ''}`);
    }
    await delay(intervalMs);
  }
}

function reserveLoopbackPort() {
  return new Promise((resolvePromise, rejectPromise) => {
    import('node:net').then(({ createServer }) => {
      const socket = createServer();
      socket.once('error', rejectPromise);
      socket.listen(0, '127.0.0.1', () => {
        const address = socket.address();
        const port = typeof address === 'object' && address ? address.port : 0;
        socket.close((error) => {
          if (error) rejectPromise(error);
          else resolvePromise(port);
        });
      });
    }).catch(rejectPromise);
  });
}

class CdpConnection {
  #nextId = 1;
  #pending = new Map();
  #listeners = new Map();
  #closePromise;
  #closed = false;

  constructor(url) {
    this.ws = new WebSocket(url);
    this.ready = new Promise((resolvePromise, rejectPromise) => {
      this.ws.addEventListener('open', () => resolvePromise());
      this.ws.addEventListener('error', (event) => rejectPromise(event.error ?? new Error('CDP websocket failed to open')));
    });
    this.#closePromise = new Promise((resolvePromise) => {
      this.ws.addEventListener('close', () => {
        this.#closed = true;
        for (const [id, pending] of this.#pending) {
          pending.reject(new Error(`CDP connection closed before response ${id}`));
        }
        this.#pending.clear();
        resolvePromise();
      });
    });
    this.ws.addEventListener('message', (event) => {
      const payload = JSON.parse(String(event.data));
      if (payload.id) {
        const pending = this.#pending.get(payload.id);
        if (!pending) return;
        this.#pending.delete(payload.id);
        if (payload.error) pending.reject(new Error(payload.error.message));
        else pending.resolve(payload.result ?? {});
        return;
      }
      const listeners = this.#listeners.get(payload.method);
      if (!listeners) return;
      for (const listener of listeners) listener(payload.params ?? {});
    });
  }

  async send(method, params = {}) {
    await this.ready;
    if (this.#closed) throw new Error(`CDP connection already closed for ${method}`);
    const id = this.#nextId++;
    const message = JSON.stringify({ id, method, params });
    return await new Promise((resolvePromise, rejectPromise) => {
      this.#pending.set(id, { resolve: resolvePromise, reject: rejectPromise });
      this.ws.send(message);
    });
  }

  on(method, listener) {
    const listeners = this.#listeners.get(method) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(method, listeners);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) this.#listeners.delete(method);
    };
  }

  async close() {
    if (this.#closed) return;
    this.ws.close();
    await this.#closePromise;
  }
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`GET ${url} failed with ${response.status}`);
  return await response.json();
}

async function connectToDebugger(port) {
  const version = await waitFor(
    () => fetchJson(`http://127.0.0.1:${port}/json/version`),
    { timeoutMs: 15_000, label: 'CDP version endpoint' },
  );
  const targets = await waitFor(
    async () => {
      const value = await fetchJson(`http://127.0.0.1:${port}/json/list`);
      return Array.isArray(value) && value.find((entry) => entry.type === 'page' && typeof entry.webSocketDebuggerUrl === 'string');
    },
    { timeoutMs: 15_000, label: 'CDP page target' },
  );
  return {
    browser: new CdpConnection(version.webSocketDebuggerUrl),
    page: new CdpConnection(targets.webSocketDebuggerUrl),
  };
}

async function evaluate(page, expression) {
  const result = await page.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(`renderer evaluation failed: ${result.exceptionDetails.text ?? 'unknown error'}${result.exceptionDetails.exception?.description ? `: ${result.exceptionDetails.exception.description}` : ''}`);
  }
  return result.result?.value;
}

function collectLeakMatches(text, patterns) {
  return patterns.filter((pattern) => pattern.test(text));
}

function trackedCommands(tempRoot) {
  try {
    return execFileSync('/bin/ps', ['-ax', '-o', 'pid=,command='], { encoding: 'utf8' })
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .filter((line) => line.includes(tempRoot));
  } catch {
    return [];
  }
}

async function reapTrackedCommands(tempRoot) {
  const evidence = trackedCommands(tempRoot);
  for (const line of evidence) {
    const pid = Number.parseInt(line, 10);
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === process.pid) continue;
    try { process.kill(pid, 'SIGTERM'); } catch {}
  }
  if (evidence.length > 0) await delay(500);
  for (const line of trackedCommands(tempRoot)) {
    const pid = Number.parseInt(line, 10);
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === process.pid) continue;
    try { process.kill(pid, 'SIGKILL'); } catch {}
  }
  if (evidence.length > 0) await delay(100);
  return trackedCommands(tempRoot);
}

function runningAppCommands() {
  try {
    return execFileSync('/bin/ps', ['-ax', '-o', 'pid=,command='], { encoding: 'utf8' })
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .filter((line) => line.includes(`/${APP_NAME}.app/Contents/MacOS/${APP_NAME}`) && !line.includes('Frameworks/Electron Helper'));
  } catch {
    return [];
  }
}

function bridgeRuntimeEntries(bridgeRuntimeDir) {
  try {
    return readdirSync(bridgeRuntimeDir).filter(Boolean);
  } catch {
    return [];
  }
}

function createAdversarialWorkspace(tempRoot) {
  const workspace = join(tempRoot, 'workspace');
  mkdirSync(join(workspace, '.claude'), { recursive: true, mode: 0o700 });
  mkdirSync(join(workspace, '.lingxi'), { recursive: true, mode: 0o700 });
  writeText(join(workspace, '.claude', 'settings.json'), JSON.stringify({
    hooks: {
      preToolUse: [
        {
          command: 'echo',
          note: 'LINGXI_SECRET_CANARY_DO_NOT_PACKAGE',
          pathHint: '/Users/example/Projects/private/source.ts',
        },
      ],
    },
  }));
  writeText(join(workspace, '.lingxi', 'settings.local.json'), JSON.stringify({
    mirrors: ['LINGXI_SECRET_CANARY_DO_NOT_PACKAGE'],
  }));
  writeText(join(workspace, 'src', 'smoke-context.ts'), 'export const smokeContext = true;\n');
  writeText(join(workspace, 'node_modules', 'fixture', 'smoke-context-secret.ts'), 'must not be indexed\n');
  return workspace;
}

function copyPackagedApp(appPath, tempRoot) {
  mkdirSync(tempRoot, { recursive: true, mode: 0o700 });
  const destination = join(tempRoot, `${APP_NAME}.app`);
  cpSync(appPath, destination, {
    dereference: false,
    errorOnExist: true,
    force: false,
    preserveTimestamps: true,
    recursive: true,
    verbatimSymlinks: true,
  });
  return destination;
}

function spawnPackagedApp(appPath, env, cdpPort, userDataDir) {
  const executable = join(appPath, 'Contents', 'MacOS', APP_NAME);
  const child = spawn(executable, [`--user-data-dir=${userDataDir}`, `--remote-debugging-port=${cdpPort}`], {
    cwd: dirname(appPath),
    env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (chunk) => { stdout += chunk.toString(); });
  child.stderr.on('data', (chunk) => { stderr += chunk.toString(); });
  return { child, executable, output: { get stdout() { return stdout; }, get stderr() { return stderr; } } };
}

async function closePackagedApp(browser, child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  try {
    execFileSync('/usr/bin/osascript', ['-e', `tell application id "${BUNDLE_ID}" to quit`], { stdio: 'pipe' });
  } catch {
    try {
      await browser.send('Browser.close');
    } catch {
      // Best effort fallback when Apple Events are unavailable.
    }
  }
  await Promise.race([
    new Promise((resolvePromise) => child.once('exit', resolvePromise)),
    delay(10_000).then(() => { throw new Error('packaged app did not exit after Browser.close'); }),
  ]);
}

async function assertRendererContract(page, leakPatterns) {
  const details = await waitFor(
    () => evaluate(page, `(() => ({
      hasLingxi: typeof window.lingxi === 'object' && window.lingxi !== null,
      isElectron: window.lingxi?.isElectron === true,
      hasWorkspaceFileSearch: typeof window.lingxi?.searchWorkspaceFiles === 'function',
      protocol: window.location.protocol,
      hasNodeRequire: typeof window.require !== 'undefined',
      hasNodeProcess: typeof window.process !== 'undefined',
      bodyText: document.body.innerText,
    }))()`).then((value) => value?.hasLingxi && /Set up LingXi Code Beta/.test(value.bodyText) ? value : undefined),
    { timeoutMs: 15_000, label: 'renderer bootstrap' },
  );
  assert.equal(details.hasLingxi, true, 'window.lingxi must be exposed');
  assert.equal(details.isElectron, true, 'window.lingxi must identify Electron');
  assert.equal(details.hasWorkspaceFileSearch, true, 'preload must expose bounded workspace file search');
  assert.equal(details.protocol, 'file:', 'packaged app must load the bundled file renderer');
  assert.equal(details.hasNodeRequire, false, 'nodeIntegration must remain disabled');
  assert.equal(details.hasNodeProcess, false, 'process must not be exposed to the renderer');
  assert.match(details.bodyText, /Set up LingXi Code Beta/);
  assert.match(details.bodyText, /Choose a workspace/);
  assert.match(details.bodyText, /Trust executable workspace settings/);
  assert.match(details.bodyText, /Connect a provider/);
  assert.match(details.bodyText, /OpenAI/);
  assert.match(details.bodyText, /DeepSeek/);
  assert.match(details.bodyText, /Start the local engine/);
  assert.doesNotMatch(details.bodyText, /MLPlatform|Dispatching Task 1|Placeholder for the desktop mock/);
  const leaks = collectLeakMatches(details.bodyText, leakPatterns);
  assert.equal(leaks.length, 0, `renderer leaked forbidden content: ${leaks.map((pattern) => pattern.source).join(', ')}`);
}

async function assertSecurityBehavior(page) {
  const notification = await evaluate(page, `(async () => await Notification.requestPermission())()`);
  assert.equal(notification, 'denied', 'renderer permissions must be denied by the host session');

  const navigation = await evaluate(page, `(async () => {
    const before = window.location.href;
    window.location.href = 'https://example.com/';
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 300));
    return { before, after: window.location.href };
  })()`);
  assert.equal(navigation.after, navigation.before, 'cross-origin navigation must be blocked');
}

async function setupSmokeCollectors(page) {
  await evaluate(page, `(() => {
    window.__lingxiSmoke?.unsubs?.forEach((unsubscribe) => unsubscribe());
    const state = { events: [], states: [], permissions: [], unsubs: [] };
    state.unsubs.push(window.lingxi.onEvent((event) => { state.events.push(event); }));
    state.unsubs.push(window.lingxi.onConnectionStateChanged((connection) => { state.states.push(connection); }));
    state.unsubs.push(window.lingxi.onPermission((permission) => { state.permissions.push(permission); }));
    window.__lingxiSmoke = state;
    return true;
  })()`);
}

async function assertKeylessBundledSidecar(page, appPath, tempRoot) {
  const bootstrap = await waitFor(
    async () => {
      const value = await evaluate(page, `window.lingxi.bootstrap()`);
      return value?.connection?.status === 'connected' ? value : undefined;
    },
    { timeoutMs: 20_000, label: 'keyless bundled bridge connection' },
  );
  assert.equal(
    isKeylessProviderCredentialSnapshot(bootstrap.providerCredentials),
    true,
    'packaged smoke profile must remain keyless',
  );
  assert.equal(bootstrap.workspace.trusted, true, 'workspace should be pretrusted for automated smoke verification');

  const fileSearch = await evaluate(page, `window.lingxi.searchWorkspaceFiles('smoke-context')`);
  assert.deepEqual(fileSearch.files, ['src/smoke-context.ts'], 'workspace file search must return relative source paths and skip dependencies');
  assert.equal(fileSearch.truncated, false, 'small workspace file search should not be truncated');

  await setupSmokeCollectors(page);
  await evaluate(page, `window.lingxi.command({ type: 'list_models' })`);
  await evaluate(page, `window.lingxi.command({ type: 'list_sessions', limit: 5 })`);
  await evaluate(page, `window.lingxi.command({ type: 'new_session' })`);
  await evaluate(page, `window.lingxi.command({ type: 'set_permission_mode', mode: 'acceptEdits' })`);
  // bypassPermissions now requires explicit, persisted acceptance (security #34).
  // This profile pre-accepts it (createPackagedSettings sets
  // bypassPermissionsModeAccepted), so the raw command passes the gate without a
  // blocking dialog — the "already accepted, don't re-prompt" path. A fresh
  // profile with no acceptance would have this command rejected.
  await evaluate(page, `window.lingxi.command({ type: 'set_permission_mode', mode: 'bypassPermissions' })`);

  const smokeState = await waitFor(
    async () => {
      const value = await evaluate(page, `window.__lingxiSmoke && ({
        events: window.__lingxiSmoke.events,
        states: window.__lingxiSmoke.states,
        permissions: window.__lingxiSmoke.permissions,
      })`);
      const types = new Set((value?.events ?? []).map((event) => event.type));
      const modes = (value?.events ?? [])
        .filter((event) => event.type === 'permission_mode_changed')
        .map((event) => event.mode);
      if (
        types.has('model_list')
        && types.has('session_list')
        && types.has('session_started')
        && modes.includes('acceptEdits')
        && modes.includes('bypassPermissions')
      ) return value;
      return undefined;
    },
    { timeoutMs: 20_000, label: 'bundled sidecar keyless event flow' },
  );
  assert.equal(smokeState.permissions.length, 0, 'packaged keyless smoke must not trigger permission prompts during listing/session setup');
  assert.deepEqual(
    smokeState.events
      .filter((event) => event.type === 'permission_mode_changed')
      .map((event) => event.mode),
    ['acceptEdits', 'bypassPermissions'],
    'trusted desktop sessions apply live permission-mode changes; bypassPermissions applies here only because the profile pre-accepted it (a fresh profile requires the acceptance dialog first)',
  );

  const diagnostics = await evaluate(page, `window.lingxi.diagnostics()`);
  const diagnosticText = JSON.stringify(diagnostics);
  assert.doesNotMatch(diagnosticText, /LINGXI_SECRET_CANARY_DO_NOT_PACKAGE|\/Users\/example\/Projects\/private/);

  const commands = trackedCommands(tempRoot);
  assert.ok(commands.some((line) => line.includes(join(`${APP_NAME}.app`, 'Contents', 'Resources', 'bin', 'bridge-server'))), 'bundled sidecar must run from the copied app resources');
  assert.equal(commands.some((line) => line.includes(repoRoot)), false, 'tracked packaged processes must not reference the repository path');
  assert.equal(commands.some((line) => line.includes(appPath)), true, 'tracked packaged processes must reference the copied external app');
}

async function assertRuntimeCleanup(tempRoot, bridgeRuntimeDir) {
  await waitFor(
    () => {
      const survivors = trackedCommands(tempRoot);
      return survivors.length === 0 ? true : undefined;
    },
    { timeoutMs: 10_000, intervalMs: 100, label: 'packaged process cleanup' },
  );
  const leftoverBridgeEntries = await waitFor(
    () => {
      const entries = bridgeRuntimeEntries(bridgeRuntimeDir)
        .filter((entry) => entry.endsWith('.lock') || entry.startsWith('launch-'));
      return entries.length === 0 ? entries : undefined;
    },
    { timeoutMs: 10_000, intervalMs: 100, label: 'bridge runtime cleanup' },
  );
  assert.deepEqual(leftoverBridgeEntries, [], 'bridge runtime launch artifacts must be cleaned up');
}

export async function runPackagedAppSmoke(root = packageRoot) {
  if (process.platform !== 'darwin') {
    throw new Error(`packaged smoke verification requires macOS; received ${process.platform}`);
  }

  const metadata = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const { appPath } = artifactPaths(root, metadata);
  if (!existsSync(appPath)) {
    throw new Error(`packaged app is missing: ${appPath}`);
  }
  const preexistingAppCommands = runningAppCommands();
  if (preexistingAppCommands.length > 0) {
    throw new Error(`refusing packaged smoke launch while another ${APP_NAME} instance is running: ${preexistingAppCommands.join(' | ')}`);
  }

  const tempRoot = mkdtempSync(join(tmpdir(), 'lingxi-packaged-smoke-'));
  const tempHome = join(tempRoot, 'home');
  const tempTmp = join(tempRoot, 'tmp');
  mkdirSync(tempHome, { recursive: true, mode: 0o700 });
  mkdirSync(tempTmp, { recursive: true, mode: 0o700 });
  const workspace = createAdversarialWorkspace(tempRoot);
  const runtimePaths = runtimePathsForHome(tempHome);
  writeJson(runtimePaths.settingsPath, createPackagedSettings({ workspace }));
  const copiedAppPath = copyPackagedApp(appPath, join(tempRoot, 'app-copy'));
  const cdpPort = await reserveLoopbackPort();
  const env = sanitizePackagedAppEnvironment(process.env, {
    HOME: tempHome,
    TMPDIR: tempTmp,
  });

  const leakPatterns = [
    /LINGXI_SECRET_CANARY_DO_NOT_PACKAGE/,
    /\/Users\/example\/Projects\/private/,
    new RegExp(escaped(repoRoot)),
  ];

  let browser;
  let page;
  let child;
  let output = { stdout: '', stderr: '' };
  try {
    const launch = spawnPackagedApp(copiedAppPath, env, cdpPort, runtimePaths.userDataDir);
    child = launch.child;
    output = launch.output;
    const debuggerConnection = await connectToDebugger(cdpPort);
    browser = debuggerConnection.browser;
    page = debuggerConnection.page;

    const consoleErrors = [];
    const runtimeExceptions = [];
    const logErrors = [];
    await page.send('Page.enable');
    await page.send('Runtime.enable');
    await page.send('Log.enable');
    page.on('Runtime.consoleAPICalled', (params) => {
      if (params.type === 'error') consoleErrors.push(params);
    });
    page.on('Runtime.exceptionThrown', (params) => runtimeExceptions.push(params));
    page.on('Log.entryAdded', (params) => {
      if (params.entry?.level === 'error') logErrors.push(params.entry);
    });

    await assertRendererContract(page, leakPatterns);
    await assertSecurityBehavior(page);
    await assertKeylessBundledSidecar(page, copiedAppPath, tempRoot);

    assert.equal(consoleErrors.length, 0, `renderer console errors detected: ${JSON.stringify(consoleErrors)}`);
    assert.equal(runtimeExceptions.length, 0, `renderer exceptions detected: ${JSON.stringify(runtimeExceptions)}`);
    assert.equal(logErrors.length, 0, `renderer log errors detected: ${JSON.stringify(logErrors)}`);

    await closePackagedApp(browser, child);
    child = null;
    await browser.close();
    await page.close();
    browser = null;
    page = null;

    await assertRuntimeCleanup(tempRoot, runtimePaths.bridgeRuntimeDir);

    return {
      appPath: copiedAppPath,
      cdpPort,
      runtimePaths,
      notes: [
        'Automated: bundled file:// renderer, preload contract, bounded @file workspace search, keyless bundled sidecar session/listing flow, live permission-mode switching, renderer security checks, and exact temp cleanup.',
        'Manual-only: Gatekeeper transfer prompts, ad-hoc signature approval on a different Mac, full Developer ID notarization/stapling, and native workspace-trust dialog text on physical user interaction.',
      ],
    };
  } finally {
    if (browser) {
      try { await browser.close(); } catch {}
    }
    if (page) {
      try { await page.close(); } catch {}
    }
    if (child) {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill('SIGKILL');
        await new Promise((resolvePromise) => child.once('exit', resolvePromise));
      }
    }
    const survivors = await reapTrackedCommands(tempRoot);
    rmSync(tempRoot, { recursive: true, force: true });
    if (output.stdout.trim()) log(`app stdout: ${output.stdout.trim()}`);
    if (output.stderr.trim()) log(`app stderr: ${output.stderr.trim()}`);
    if (survivors.length > 0) {
      throw new Error(`tracked packaged processes survived cleanup: ${survivors.join(' | ')}`);
    }
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    const result = await runPackagedAppSmoke();
    log(`OK ${result.appPath}`);
    for (const note of result.notes) log(note);
  } catch (error) {
    process.stderr.write(`[verify:package:smoke] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  }
}
