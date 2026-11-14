#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { constants, cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, readlinkSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, normalize, relative, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import {
  APP_NAME,
  PACKAGE_OUTPUT_ROOT_ENV,
  artifactPaths,
  formatError,
  packageRoot,
  repoRoot,
} from './package-support.mjs';

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
  '.lingxi/agents',
  '.lingxi/commands',
  '.lingxi/plugins',
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

export function createPackagedConversationSpaces({ workspace }) {
  const canonical = canonicalWorkspace(workspace);
  return {
    version: 2,
    activeMode: 'code',
    projects: { [canonical]: ['code'] },
    trustedWorkspaces: [canonical],
    navigation: { chat: { projectId: null }, code: { projectId: canonical } },
    sessions: [],
  };
}

function writePackagedProfile(paths, workspace) {
  writeJson(paths.settingsPath, createPackagedSettings({ workspace }));
  writeJson(join(paths.userDataDir, 'conversation-spaces.v2.json'), createPackagedConversationSpaces({ workspace }));
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

const SAFE_BOOTSTRAP_PROMPT_PLACEHOLDERS = new Set([
  'Connect a provider in Settings to start coding…',
  'Waiting for the local engine…',
]);

export function isSafeProviderAccountSnapshot(snapshot) {
  const only = (row, keys) => row && typeof row === 'object' && !Array.isArray(row)
    && Object.keys(row).every(key => keys.includes(key));
  if (!only(snapshot, ['storeScope', 'configRevision', 'accounts', 'health', 'modelEntitlements', 'modelAvailability', 'providerPolicies', 'scopedHealth'])
    || typeof snapshot.storeScope !== 'string' || !Number.isSafeInteger(snapshot.configRevision)
    || !Array.isArray(snapshot.accounts) || !Array.isArray(snapshot.health)) return false;
  return snapshot.accounts.every(row => only(row, ['accountId', 'providerId', 'displayName', 'authKind', 'executionBindingId', 'credentialRef', 'enabled', 'userPriority', 'configRevision', 'identityGeneration'])
    && typeof row.accountId === 'string' && ['api_key', 'oauth'].includes(row.authKind)
    && only(row.credentialRef, ['storeScope', 'secretRef']) && typeof row.credentialRef.secretRef === 'string')
    && (snapshot.scopedHealth ?? []).every(row => only(row, ['accountId', 'identityGeneration', 'scopeKind', 'scopeId', 'unavailableUntilMs', 'reason']))
    && snapshot.health.every(row => only(row, ['accountId', 'identityGeneration', 'status', 'reason', 'cooldownUntilMs', 'healthRevision']))
    && (snapshot.modelEntitlements ?? []).every(row => only(row, ['accountId', 'identityGeneration', 'modelId', 'status', 'observedAtMs', 'retryAtMs', 'reason']))
    && (snapshot.modelAvailability ?? []).every(row => only(row, ['providerId', 'executionBindingId', 'modelId', 'eligibleAccountIds', 'confirmedAccountIds', 'unverifiedAccountIds', 'forbiddenAccountIds']))
    && Object.values(snapshot.providerPolicies ?? {}).every(row => only(row, ['allowSubscriptionToMetered']) && typeof row.allowSubscriptionToMetered === 'boolean');
}

export function isExpectedBootstrapPromptPlaceholder(placeholder, bootstrap) {
  // Hydration may complete before the first renderer snapshot. Accept the idle
  // composer only when the actual bridge and trusted workspace are ready.
  if (placeholder === 'Do anything') {
    return bootstrap?.connection?.status === 'connected' && bootstrap?.workspace?.trusted === true;
  }
  return SAFE_BOOTSTRAP_PROMPT_PLACEHOLDERS.has(placeholder);
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

  async send(method, params = {}, timeoutMs = 10_000) {
    await this.ready;
    if (this.#closed) throw new Error(`CDP connection already closed for ${method}`);
    const id = this.#nextId++;
    const message = JSON.stringify({ id, method, params });
    return await new Promise((resolvePromise, rejectPromise) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        rejectPromise(new Error(`CDP ${method} timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      this.#pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolvePromise(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          rejectPromise(error);
        },
      });
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

  async close(timeoutMs = 2_000) {
    if (this.#closed) return;
    this.ws.close();
    await Promise.race([this.#closePromise, delay(timeoutMs)]);
  }
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`GET ${url} failed with ${response.status}`);
  return await response.json();
}

export async function waitForRendererCommit(page, rendererUrl, options = {}) {
  await page.send('Page.enable');
  // A target can advertise its pending URL while its initial empty document
  // is still current. Runtime.enable there can run Electron's sandbox preload
  // before startup data exists. Read the committed frame without creating a
  // script context, and keep all renderer errors observable after attaching.
  return await waitFor(async () => {
    const { frameTree } = await page.send('Page.getFrameTree');
    const frame = frameTree?.frame;
    return frame?.url === rendererUrl && typeof frame.loaderId === 'string' && frame.loaderId.length > 0
      ? frame : undefined;
  }, { timeoutMs: 15_000, label: 'committed packaged renderer document', ...options });
}

async function connectToDebugger(port, rendererUrl) {
  const version = await waitFor(
    () => fetchJson(`http://127.0.0.1:${port}/json/version`),
    { timeoutMs: 15_000, label: 'CDP version endpoint' },
  );
  const targets = await waitFor(
    async () => {
      const value = await fetchJson(`http://127.0.0.1:${port}/json/list`);
      return Array.isArray(value) && value.find((entry) => entry.type === 'page'
        && entry.url === rendererUrl && typeof entry.webSocketDebuggerUrl === 'string');
    },
    { timeoutMs: 15_000, label: 'CDP page target' },
  );
  const page = new CdpConnection(targets.webSocketDebuggerUrl);
  try {
    await waitForRendererCommit(page, rendererUrl);
    return { browser: new CdpConnection(version.webSocketDebuggerUrl), page };
  } catch (error) {
    await page.close();
    throw error;
  }
}

async function evaluate(page, expression) {
  // Bootstrap can include health, account listing and an OAuth metadata read.
  // Each native helper has a bounded 90s cold-start deadline. Keep CDP alive
  // for that chain; individual UI checks retain their own readiness deadlines.
  const result = await page.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  }, 280_000);
  if (result.exceptionDetails) {
    throw new Error(`renderer evaluation failed: ${result.exceptionDetails.text ?? 'unknown error'}${result.exceptionDetails.exception?.description ? `: ${result.exceptionDetails.exception.description}` : ''}`);
  }
  return result.result?.value;
}

function collectLeakMatches(text, patterns) {
  return patterns.filter((pattern) => pattern.test(text));
}

function processIdFromCommand(line) {
  const match = line.match(/^\s*(\d+)\s+/);
  return match ? Number.parseInt(match[1], 10) : undefined;
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

async function reapTrackedCommands(tempRoot, protectedProcessIds = new Set()) {
  const evidence = trackedCommands(tempRoot)
    .filter((line) => !protectedProcessIds.has(processIdFromCommand(line)));
  for (const line of evidence) {
    const pid = processIdFromCommand(line);
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === process.pid) continue;
    try { process.kill(pid, 'SIGTERM'); } catch {}
  }
  if (evidence.length > 0) await delay(500);
  for (const line of trackedCommands(tempRoot)
    .filter((entry) => !protectedProcessIds.has(processIdFromCommand(entry)))) {
    const pid = processIdFromCommand(line);
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === process.pid) continue;
    try { process.kill(pid, 'SIGKILL'); } catch {}
  }
  if (evidence.length > 0) await delay(100);
  return trackedCommands(tempRoot);
}

export function runningAppCommands(execute = execFileSync) {
  try {
    return execute('/bin/ps', ['-ax', '-o', 'pid=,command='], { encoding: 'utf8' })
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .filter((line) => line.includes(`/${APP_NAME}.app/Contents/MacOS/${APP_NAME}`) && !line.includes('Frameworks/Electron Helper'));
  } catch (error) {
    throw new Error(
      `cannot inspect running ${APP_NAME} processes; refusing packaged smoke: ${error?.message ?? error}`,
      { cause: error },
    );
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
    // APFS can isolate the test copy without allocating another full app bundle.
    mode: constants.COPYFILE_FICLONE,
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

function packagedAppBundleId(appPath) {
  return execFileSync('/usr/libexec/PlistBuddy', [
    '-c',
    'Print :CFBundleIdentifier',
    join(appPath, 'Contents', 'Info.plist'),
  ], { encoding: 'utf8' }).trim();
}

export async function waitForPackagedAppExit(child, requestQuit) {
  const assertCleanExit = () => {
    assert.equal(child.signalCode, null, 'packaged app must exit without a termination signal');
    assert.equal(child.exitCode, 0, 'packaged app must exit successfully');
  };
  if (child.exitCode !== null || child.signalCode !== null) { assertCleanExit(); return; }
  let onExit;
  let timer;
  const exited = new Promise((resolvePromise) => {
    onExit = resolvePromise;
    child.once('exit', onExit);
  });
  try {
    // Subscribe before requesting quit: CDP may close its socket only after
    // the process has already exited. Listening afterwards loses that event.
    await requestQuit();
    if (child.exitCode !== null || child.signalCode !== null) { assertCleanExit(); return; }
    await Promise.race([
      exited,
      new Promise((_, rejectPromise) => {
        // macOS Chromium can finish its native profile fsync after `quit` and
        // all application drains. An observed clean exit took 18.3 seconds;
        // keep a finite deadline without terminating those durable writes.
        timer = setTimeout(() => rejectPromise(new Error('packaged app did not exit after Browser.close within 30000ms')), 30_000);
      }),
    ]);
    assertCleanExit();
  } finally {
    clearTimeout(timer);
    child.removeListener('exit', onExit);
  }
}

async function closePackagedApp(browser, child, bundleId) {
  try {
    await waitForPackagedAppExit(child, async () => {
    try {
      execFileSync('/usr/bin/osascript', ['-e', `tell application id "${bundleId}" to quit`], { stdio: 'pipe' });
    } catch {
      try {
        await Promise.race([
          browser.send('Browser.close'),
          delay(2_000).then(() => { throw new Error('Browser.close timed out'); }),
        ]);
      } catch {
        // Best effort fallback when Apple Events are unavailable.
      }
    }
    });
  } catch (error) {
    if (process.platform === 'darwin' && child.exitCode === null && child.signalCode === null) {
      const samplePath = join(tmpdir(), `lingxi-smoke-shutdown-${child.pid}-${Date.now()}.sample.txt`);
      try {
        execFileSync('/usr/bin/sample', [String(child.pid), '2', '-file', samplePath], { timeout: 5_000, stdio: 'pipe' });
        log(`shutdown timeout sample: ${samplePath}`);
      } catch { /* Diagnostics must not replace the original failure. */ }
    }
    throw error;
  }
}


async function assertRendererContract(page, leakPatterns) {
  let lastDetails;
  const details = await waitFor(
    () => evaluate(page, `(() => ({
      hasLingxi: typeof window.lingxi === 'object' && window.lingxi !== null,
      isElectron: window.lingxi?.isElectron === true,
      hasWorkspaceFileSearch: typeof window.lingxi?.searchWorkspaceFiles === 'function',
      protocol: window.location.protocol,
      hasNodeRequire: typeof window.require !== 'undefined',
      hasNodeProcess: typeof window.process !== 'undefined',
      screenLabel: document.querySelector('.desktop-shell')?.getAttribute('data-screen-label'),
      hasPrompt: Boolean(document.querySelector('[aria-label="Prompt"]')),
      promptPlaceholder: document.querySelector('[aria-label="Prompt"]')?.getAttribute('data-placeholder'),
      appsNavigationCount: document.querySelectorAll('.desktop-nav-rail button[aria-label="Apps"]').length,
      scheduledNavigationCount: document.querySelectorAll('.desktop-nav-rail button[aria-label="Scheduled tasks"]').length,
      duplicateSidebarEntries: Array.from(document.querySelectorAll('.sidebar-primary-action')).filter(button => /^(Apps|Scheduled)$/.test(button.textContent.trim())).length,
      bodyText: document.body.innerText,
    }))()`).then((value) => {
      lastDetails = value;
      return value?.hasLingxi
      && value.screenLabel === 'LingXi Code Desktop Beta'
      && value.hasPrompt
      // The welcome shell now renders before persisted workspace hydration.
      // Wait for the seeded project before asserting its bootstrap contract.
      && /workspace/.test(value.bodyText)
        ? value
        : undefined;
    }),
    // Health, listing and optional OAuth metadata each use a bounded helper.
    { timeoutMs: 280_000, label: 'renderer bootstrap' },
  ).catch((error) => {
    log(`last renderer bootstrap: ${JSON.stringify(lastDetails)}`);
    throw error;
  });
  assert.equal(details.hasLingxi, true, 'window.lingxi must be exposed');
  assert.equal(details.isElectron, true, 'window.lingxi must identify Electron');
  assert.equal(details.hasWorkspaceFileSearch, true, 'preload must expose bounded workspace file search');
  assert.equal(details.protocol, 'file:', 'packaged app must load the bundled file renderer');
  assert.equal(details.hasNodeRequire, false, 'nodeIntegration must remain disabled');
  assert.equal(details.hasNodeProcess, false, 'process must not be exposed to the renderer');
  assert.equal(details.screenLabel, 'LingXi Code Desktop Beta');
  assert.equal(details.hasPrompt, true, 'desktop prompt composer must render');
  assert.match(details.bodyText, /LingXi/);
  assert.match(details.bodyText, /New chat/);
  assert.equal(details.appsNavigationCount, 1, 'Apps must have one main navigation entry');
  assert.equal(details.scheduledNavigationCount, 1, 'Scheduled tasks must have one main navigation entry');
  assert.equal(details.duplicateSidebarEntries, 0, 'the chat sidebar must not duplicate Apps or Scheduled');
  assert.match(details.bodyText, /Projects/);
  assert.match(details.bodyText, /workspace/);
  assert.equal(
    isExpectedBootstrapPromptPlaceholder(details.promptPlaceholder,
      details.promptPlaceholder === 'Do anything' ? await evaluate(page, 'window.lingxi.bootstrap()') : undefined),
    true,
    `unexpected bootstrap prompt placeholder: ${details.promptPlaceholder}`,
  );
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

async function setupSmokeCollectors(page, sessionId = null) {
  await evaluate(page, `(() => {
    window.__lingxiSmoke?.unsubs?.forEach((unsubscribe) => unsubscribe());
    const state = { events: [], states: [], permissions: [], unsubs: [] };
    state.unsubs.push(window.lingxi.onEvent((envelope) => {
      if (${JSON.stringify(sessionId)} === null || envelope.sessionId === ${JSON.stringify(sessionId)}) state.events.push(envelope.event);
    }));
    state.unsubs.push(window.lingxi.onConnectionStateChanged((connection) => { state.states.push(connection); }));
    state.unsubs.push(window.lingxi.onPermission((permission) => { state.permissions.push(permission); }));
    window.__lingxiSmoke = state;
    return true;
  })()`);
}

async function assertBundledSidecar(page, appPath, tempRoot, workspace) {
  await setupSmokeCollectors(page);
  // Startup can own a global draft whose cwd is not a Project. Exercise the
  // explicitly registered fixture Project instead of deriving ownership from cwd.
  const sessionBootstrap = await evaluate(page, `window.lingxi.newSession(${JSON.stringify(workspace)})`);
  const sessionId = sessionBootstrap?.activeSession?.sessionId;
  assert.equal(typeof sessionId, 'string', 'packaged smoke must create an active session before dispatching commands');
  const encodedSessionId = JSON.stringify(sessionId);
  await evaluate(page, `window.lingxi.command(${encodedSessionId}, { type: 'list_models' })`);

  const bootstrap = await waitFor(
    async () => {
      const value = await evaluate(page, `window.lingxi.bootstrap()`);
      return value?.connection?.status === 'connected' ? value : undefined;
    },
    { timeoutMs: 60_000, label: 'keyless bundled bridge connection' },
  );
  assert.equal(
    isSafeProviderAccountSnapshot(bootstrap.providerAccountSnapshot),
    true,
    'packaged credential snapshot must stay encrypted and metadata-only',
  );
  assert.equal(bootstrap.workspace.trusted, true, 'workspace should be pretrusted for automated smoke verification');

  const fileSearch = await evaluate(page, `window.lingxi.searchWorkspaceFiles('smoke-context')`);
  assert.deepEqual(fileSearch.files, ['src/smoke-context.ts'], 'workspace file search must return relative source paths and skip dependencies');
  assert.equal(fileSearch.truncated, false, 'small workspace file search should not be truncated');

  await evaluate(page, `window.lingxi.command(${encodedSessionId}, { type: 'list_sessions', limit: 5 })`);
  await evaluate(page, `window.lingxi.command(${encodedSessionId}, { type: 'set_permission_mode', mode: 'acceptEdits' })`);
  // bypassPermissions now requires explicit, persisted acceptance (security #34).
  // This profile pre-accepts it (createPackagedSettings sets
  // bypassPermissionsModeAccepted), so the raw command passes the gate without a
  // blocking dialog — the "already accepted, don't re-prompt" path. A fresh
  // profile with no acceptance would have this command rejected.
  await evaluate(page, `window.lingxi.command(${encodedSessionId}, { type: 'set_permission_mode', mode: 'bypassPermissions' })`);

  const readSmokeState = () => evaluate(page, `window.__lingxiSmoke && ({
    events: window.__lingxiSmoke.events,
    states: window.__lingxiSmoke.states,
    permissions: window.__lingxiSmoke.permissions,
  })`);
  let smokeState;
  try {
    smokeState = await waitFor(
      async () => {
        const value = await readSmokeState();
        const types = new Set((value?.events ?? []).map((event) => event.type));
        const modes = (value?.events ?? [])
          .filter((event) => event.type === 'permission_mode_changed')
          .map((event) => event.mode);
        if (
          types.has('model_list')
          && types.has('session_list')
          && modes.includes('acceptEdits')
          && modes.includes('bypassPermissions')
        ) return value;
        return undefined;
      },
      { timeoutMs: 20_000, label: 'bundled sidecar event flow' },
    );
  } catch (error) {
    const observed = await readSmokeState();
    const types = (observed?.events ?? []).map((event) => event.type);
    throw new Error(`${formatError(error)}; observed event types: ${JSON.stringify(types)}`);
  }
  assert.equal(smokeState.permissions.length, 0, 'packaged smoke must not trigger permission prompts during listing/session setup');
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

function canonicalPathInside(root, candidate, label) {
  const canonicalRoot = realpathSync.native(root);
  const canonicalCandidate = realpathSync.native(candidate);
  const relativePath = relative(canonicalRoot, canonicalCandidate);
  assert.ok(
    relativePath !== '' && relativePath !== '..' && !relativePath.startsWith(`..${sep}`) && !isAbsolute(relativePath),
    `${label} must resolve below the isolated smoke home`,
  );
  return canonicalCandidate;
}

function temporaryCredentialBrokerPaths(tempHome, channel) {
  assert.ok(['development', 'production'].includes(channel));
  const label = `com.lingxi.code.credential-broker${channel === 'development' ? '.development' : ''}`;
  const canonicalHome = realpathSync.native(tempHome);
  return {
    homePath: canonicalHome,
    label,
    target: `gui/${process.getuid()}/${label}`,
    plistPath: join(canonicalHome, 'Library', 'LaunchAgents', `${label}.plist`),
    programPath: join(canonicalHome, 'Library', 'Application Support', 'LingXi', 'CredentialBroker',
      channel, 'LingXiCredentialBroker.app', 'Contents', 'MacOS', 'LingXiCredentialBroker'),
  };
}

function assertCredentialBrokerChannelUnregistered(channel) {
  assert.ok(['development', 'production'].includes(channel));
  const label = `com.lingxi.code.credential-broker${channel === 'development' ? '.development' : ''}`;
  const target = `gui/${process.getuid()}/${label}`;
  const state = spawnSync('/bin/launchctl', ['print', target], { encoding: 'utf8', timeout: 10_000 });
  if (state.status === 113) return;
  if (state.status !== 0) {
    throw new Error(`cannot verify that the ${channel} credential Broker service is unregistered: ${state.stderr ?? state.error?.message ?? state.status}`);
  }
  throw new Error(`refusing packaged smoke because credential Broker service ${target} is already registered`);
}

function launchctlAssignment(output, name) {
  return output.match(new RegExp(`^\\s*${name}\\s*=\\s*(.+?)\\s*$`, 'm'))?.[1];
}

export function assertTemporaryBrokerLaunchdIdentity(output, paths) {
  const serviceTarget = output.match(/^\s*(\S+)\s*=\s*\{$/m)?.[1];
  assert.equal(serviceTarget, paths.target, 'launchd service label must match the isolated Broker target');
  const launchdPath = launchctlAssignment(output, 'path');
  const launchdProgram = launchctlAssignment(output, 'program');
  assert.ok(launchdPath, 'launchd must report the registered Broker plist path');
  assert.ok(launchdProgram, 'launchd must report the registered Broker program path');
  const plistPath = realpathSync.native(launchdPath);
  const programPath = realpathSync.native(launchdProgram);
  assert.equal(plistPath, paths.plistPath, 'refusing to operate on a Broker registered outside this isolated smoke home');
  assert.equal(programPath, paths.programPath, 'refusing to operate on a Broker program that does not match this isolated smoke home');
  const pidValue = launchctlAssignment(output, 'pid');
  const pid = pidValue === undefined ? undefined : Number.parseInt(pidValue, 10);
  assert.ok(pid === undefined || (Number.isSafeInteger(pid) && pid > 1), 'launchd Broker PID must be valid');
  return { plistPath, programPath, pid };
}

function temporaryBrokerLaunchdState(paths) {
  const result = spawnSync('/bin/launchctl', ['print', paths.target], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  if (result.status === 113) return null;
  assert.equal(result.status, 0, `cannot inspect isolated credential broker (${result.stderr ?? result.error?.message ?? result.status})`);

  assert.equal(existsSync(paths.plistPath), true, 'isolated credential Broker LaunchAgent plist must exist');
  const plistPath = canonicalPathInside(paths.homePath, paths.plistPath, 'credential Broker LaunchAgent plist');
  const programPath = canonicalPathInside(paths.homePath, paths.programPath, 'credential Broker executable');
  const identity = assertTemporaryBrokerLaunchdIdentity(result.stdout, { ...paths, plistPath, programPath });

  const plist = JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', plistPath], { encoding: 'utf8' }));
  assert.equal(plist.Label, paths.label, 'isolated Broker plist label must match the expected channel');
  assert.ok(Array.isArray(plist.ProgramArguments) && plist.ProgramArguments.length === 1,
    'isolated Broker plist must launch only its expected executable');
  assert.equal(realpathSync.native(plist.ProgramArguments[0]), programPath,
    'isolated Broker plist must launch only its expected executable');
  assert.equal(plist.MachServices?.[paths.label], true, 'isolated Broker plist must own its expected Mach service');

  const { pid } = identity;
  if (pid !== undefined) {
    const images = execFileSync('/usr/sbin/lsof', ['-a', '-p', String(pid), '-d', 'txt', '-Fn'], { encoding: 'utf8' })
      .split('\n')
      .filter((line) => line.startsWith('n'))
      .map((line) => line.slice(1))
      .filter((path) => path.startsWith('/'));
    assert.ok(images.some((image) => {
      try { return realpathSync.native(image) === programPath; } catch { return false; }
    }), 'launchd Broker PID executable must match the isolated Broker program');
  }
  return { ...paths, plistPath, programPath, pid };
}

async function waitForTemporaryCredentialBroker(tempHome, channel, protectedProcessIds = new Set()) {
  const paths = temporaryCredentialBrokerPaths(tempHome, channel);
  return await waitFor(() => {
    const state = temporaryBrokerLaunchdState(paths);
    if (!state?.pid) return undefined;
    protectedProcessIds.add(state.pid);
    return state;
  }, { timeoutMs: 10_000, intervalMs: 100, label: 'isolated credential Broker launch' });
}

async function bootoutTemporaryCredentialBroker(tempHome, channel, protectedProcessIds = new Set()) {
  const paths = temporaryCredentialBrokerPaths(tempHome, channel);
  let owned = temporaryBrokerLaunchdState(paths);
  if (!owned) return false;
  if (owned.pid !== undefined) protectedProcessIds.add(owned.pid);

  // Re-read the service immediately before bootout. The label alone is never
  // sufficient ownership evidence because a real user Broker may share it.
  const actionTimeState = temporaryBrokerLaunchdState(paths);
  assert.ok(actionTimeState, 'isolated Broker disappeared before guarded bootout');
  assert.equal(actionTimeState.target, owned.target);
  assert.equal(actionTimeState.plistPath, owned.plistPath);
  assert.equal(actionTimeState.programPath, owned.programPath);
  if (owned.pid !== undefined) assert.equal(actionTimeState.pid, owned.pid, 'isolated Broker PID changed before guarded bootout');
  owned = actionTimeState;

  const stopped = spawnSync('/bin/launchctl', ['bootout', owned.target], { encoding: 'utf8', timeout: 10_000 });
  assert.ok([0, 3].includes(stopped.status), `isolated Broker bootout failed (${stopped.stderr ?? stopped.error?.message ?? stopped.status})`);
  await waitFor(() => {
    const result = spawnSync('/bin/launchctl', ['print', owned.target], { encoding: 'utf8', timeout: 10_000 });
    if (result.status === 113) return true;
    assert.equal(result.status, 0, `launchctl failed while waiting for isolated Broker removal (${result.stderr ?? result.error?.message ?? result.status})`);
    const current = temporaryBrokerLaunchdState(paths);
    assert.ok(current, 'isolated Broker disappeared during launchd verification');
    assert.equal(current.plistPath, owned.plistPath, 'Broker service was replaced by a different LaunchAgent');
    assert.equal(current.programPath, owned.programPath, 'Broker service was replaced by a different executable');
    return undefined;
  }, { timeoutMs: 10_000, intervalMs: 100, label: 'isolated credential Broker bootout' });
  return true;
}

// A unique, disposable profile exercises the signed Broker rather than mocking
// Keychain. The server accepts only a synthetic test key and never leaves localhost.
async function customProviderProbe() {
  const providerId = `smoke-custom-${process.pid}-${Date.now()}`;
  const key = `test-only-${providerId}`;
  const requests = [];
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    requests.push({ authenticated: request.headers.authorization === `Bearer ${key}`, body });
    if (request.headers.authorization !== `Bearer ${key}`) {
      response.writeHead(401).end();
      return;
    }
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const payload = JSON.parse(body);
    const wantsApp = payload.messages?.some(message => message.role === 'user' && JSON.stringify(message.content).includes('LOCAL_APP_PACKAGED_SMOKE'));
    const preparationReturned = payload.messages?.some(message => message.tool_call_id === 'local-app-preparation');
    const appReturned = payload.messages?.some(message => message.tool_call_id === 'local-app-smoke');
    if (wantsApp && !preparationReturned && payload.tools?.some(tool => tool.function?.name === 'Bash')) {
      const manifest = JSON.parse(readFileSync(join(repoRoot, 'fixtures/local-apps/v2/static/lingxi-app.json'), 'utf8'));
      manifest.name = 'Packaged Local App'; manifest.summary = 'Created through the packaged model tool';
      const command = ['mkdir -p dist', "cat > lingxi-app.json <<'LINGXI_MANIFEST'", JSON.stringify(manifest), 'LINGXI_MANIFEST',
        "cat > dist/index.html <<'LINGXI_HTML'", '<!doctype html><meta charset="utf-8"><title>Packaged Local App</title><h1>LocalApp tool connected</h1>', 'LINGXI_HTML',
        "printf '%s' '/* Packaged acceptance */' > dist/client.js", "printf 'LocalApp package check\\n'"].join('\n');
      response.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, id: 'local-app-preparation', type: 'function', function: { name: 'Bash', arguments: JSON.stringify({ command }) } }] } }] })}\n\n`);
      response.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] })}\n\n`);
      response.end('data: [DONE]\n\n');
      return;
    }
    if (wantsApp && !appReturned && payload.tools?.some(tool => tool.function?.name === 'LocalApp')) {
      const args = { bundle_path: '.' };
      response.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, id: 'local-app-smoke', type: 'function', function: { name: 'LocalApp', arguments: JSON.stringify(args) } }] } }] })}\n\n`);
      response.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] })}\n\n`);
      response.end('data: [DONE]\n\n');
      return;
    }
    for (const chunk of [
      { choices: [{ index: 0, delta: { role: 'assistant', content: 'Custom provider verified.' } }] },
      { choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } },
    ]) response.write(`data: ${JSON.stringify(chunk)}\n\n`);
    response.end('data: [DONE]\n\n');
  });
  await new Promise((resolvePromise, rejectPromise) => {
    server.once('error', rejectPromise);
    server.listen(0, '127.0.0.1', resolvePromise);
  });
  return {
    providerId, key, requests,
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    close: () => new Promise((resolvePromise) => {
      server.close(resolvePromise);
      server.closeAllConnections();
    }),
  };
}

async function assertDesktopGit(page, workspace, tempRoot) {
  const scope = await evaluate(page, `(async () => {
    const state = await window.lingxi.bootstrap();
    return state.activeSession ?? { projectPath: state.workspace.path, sessionId: '__draft__' };
  })()`);
  const request = (operation) => evaluate(page, `window.lingxi.git.request(${JSON.stringify(scope)}, ${JSON.stringify(operation)})`);
  const localGit = (...args) => execFileSync('git', ['-C', workspace, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  await request({ kind: 'init' });
  localGit('config', 'user.name', 'Desktop Git Smoke');
  localGit('config', 'user.email', 'desktop-smoke@example.invalid');
  localGit('config', 'commit.gpgsign', 'false');
  writeFileSync(join(workspace, '.gitignore'), '*\n!/.gitignore\n!/git-smoke.txt\n');
  writeFileSync(join(workspace, 'git-smoke.txt'), 'initial\n');
  let status = (await request({ kind: 'status' })).status;
  await request({ kind: 'stage', token: status.token, paths: ['.gitignore', 'git-smoke.txt'] });
  status = (await request({ kind: 'status' })).status;
  await request({ kind: 'commit', token: status.token, message: 'Establish the local Git smoke fixture' });
  status = (await request({ kind: 'status' })).status;
  const originalBranch = status.branch;
  await request({ kind: 'checkout', branch: 'desktop-review-smoke', create: true, token: status.token });
  writeFileSync(join(workspace, 'git-smoke.txt'), 'initial\nDesktop Review 中文\n');
  status = (await request({ kind: 'status' })).status;
  assert.ok(status.files.some(file => file.path === 'git-smoke.txt'));
  const diff = (await request({ kind: 'diff', mode: 'working', path: 'git-smoke.txt' })).diff;
  assert.ok(diff.patch.includes('+Desktop Review 中文'));
  // Summary contains outputs/activity; Git Review opens from the desktop rail.
  // Check the closed state first so an existing panel cannot mask a broken entry.
  assert.equal(await evaluate(page, `document.querySelector('.git-review') !== null`), false);
  assert.equal(await evaluate(page, `document.querySelector('#runtime-center-overview') !== null`), false);
  await evaluate(page, `document.querySelector('[aria-label="Toggle pinned summary"]').click()`);
  await waitFor(() => evaluate(page, `document.querySelector('#runtime-center-overview [aria-label="Outputs"]') !== null`), { label: 'packaged summary outputs' });
  assert.equal(await evaluate(page, `document.querySelector('#runtime-center-overview .git-environment-content') !== null`), false);
  await evaluate(page, `document.querySelector('[aria-label="Toggle pinned summary"]').click()`);
  await waitFor(() => evaluate(page, `document.querySelector('#runtime-center-overview') === null`), { label: 'packaged summary closes' });
  assert.equal(await evaluate(page, `document.querySelector('.git-review') !== null`), false);
  await evaluate(page, `document.querySelector('.desktop-nav-rail [aria-label="Review changes"]').click()`);
  await waitFor(() => evaluate(page, `document.querySelector('.git-review') !== null`), { label: 'packaged Review tab' });
  await request({ kind: 'stage', paths: ['git-smoke.txt'], token: status.token });
  status = (await request({ kind: 'status' })).status;
  await request({ kind: 'commit', message: 'Verify staged changes reach a local remote', token: status.token });
  const remote = join(tempRoot, 'git-smoke-remote.git');
  execFileSync('git', ['init', '--bare', remote], { stdio: 'pipe' });
  localGit('remote', 'add', 'smoke-local', remote);
  status = (await request({ kind: 'status' })).status;
  await request({ kind: 'push', remote: 'smoke-local', branch: 'desktop-review-smoke', setUpstream: true, token: status.token });
  assert.equal(execFileSync('git', ['--git-dir', remote, 'rev-parse', 'refs/heads/desktop-review-smoke'], { encoding: 'utf8' }).trim(), localGit('rev-parse', 'HEAD'));
  await request({ kind: 'fetch', remote: 'smoke-local' });
  status = (await request({ kind: 'status' })).status;
  await request({ kind: 'pull', remote: 'smoke-local', branch: 'desktop-review-smoke', token: status.token });
  const comparison = await request({ kind: 'diff', mode: 'branch', base: originalBranch, target: 'desktop-review-smoke' });
  assert.ok(comparison.diff.patch.includes('Desktop Review 中文'));
  writeFileSync(join(workspace, 'git-smoke.txt'), 'initial\nDesktop Review 中文\nReview preview\n');
  await waitFor(() => evaluate(page, `Array.from(document.querySelectorAll('.git-file-select')).some(button => button.textContent.includes('git-smoke.txt'))`), { label: 'Git watcher updates Review' });
  await evaluate(page, `Array.from(document.querySelectorAll('.git-file-select')).find(button => button.textContent.includes('git-smoke.txt')).click()`);
  await waitFor(() => evaluate(page, `document.querySelector('.git-diff')?.textContent.includes('Review preview')`), { label: 'packaged diff rendering' });
  const screenshot = await page.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(tmpdir(), 'lingxi-git-packaged.png'), Buffer.from(screenshot.data, 'base64'));
  await waitFor(() => evaluate(page, `document.querySelector('.git-review-toolbar [aria-label="Switch branch"]')?.textContent.includes('desktop-review-smoke')`), { label: 'Review toolbar shows current branch' });
  await evaluate(page, `document.querySelector('.git-review-toolbar [aria-label="Switch branch"]').click()`);
  await waitFor(() => evaluate(page, `(() => {
    const current = document.querySelector('.git-branch-row[aria-current="true"]');
    const warning = document.querySelector('.git-branch-warning');
    return current?.textContent.includes('desktop-review-smoke')
      && current?.textContent.includes('Current')
      && warning?.textContent.includes('Changed files: 1.');
  })()`), { label: 'current branch and uncommitted change warning' });
  const branchScreenshot = await page.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(tmpdir(), 'lingxi-git-branch-packaged.png'), Buffer.from(branchScreenshot.data, 'base64'));
  await evaluate(page, `document.querySelector('dialog[open] [aria-label="Close dialog"]').click()`);
  log('packaged Git: init, branch, diff, stage, commit, local push/fetch/pull, summary outputs, Review rail entry, watcher and branch menu passed');
}

async function assertBottomTerminal(page) {
  // The sidecar fixture was created through Host IPC, outside the renderer's
  // navigation transaction. Observe its committed owner before testing scope.
  await page.send('Page.reload', { ignoreCache: true });
  await assertRendererContract(page, []);
  const scope = await evaluate(page, `(async () => {
    const state = await window.lingxi.bootstrap();
    return state.activeSession ?? { projectPath: state.workspace.path, sessionId: '__draft__' };
  })()`);
  await evaluate(page, `document.querySelector('[aria-label="Toggle terminal"]').click()`);
  try {
    await waitFor(() => evaluate(page, `document.querySelector('#desktop-terminal[data-open="true"] .xterm') !== null`), { label: 'packaged xterm renderer' });
  } catch (error) {
    log(`terminal failure state: ${JSON.stringify(await evaluate(page, `({ button: document.querySelector('[aria-label="Toggle terminal"]')?.outerHTML, panel: document.querySelector('#desktop-terminal')?.outerHTML, dialogs: [...document.querySelectorAll('dialog')].map(d=>({open:d.open,text:d.innerText})), body: document.body.innerText.slice(-6000) })`))}`);
    throw error;
  }
  const terminals = await evaluate(page, `window.lingxi.terminal.list(${JSON.stringify(scope)})`);
  assert.equal(terminals.length, 1);
  const id = terminals[0].id;
  const command = "printf '\\nTERMINAL_PACKAGED_中文\\n'; pwd\r";
  await evaluate(page, `window.lingxi.terminal.input(${JSON.stringify(id)}, ${JSON.stringify(command)})`);
  await waitFor(() => evaluate(page, `window.lingxi.terminal.list(${JSON.stringify(scope)}).then(rows => rows.some(row => row.output.includes('\\r\\nTERMINAL_PACKAGED_中文\\r\\n') && row.output.includes(${JSON.stringify(scope.projectPath)})))`), { label: 'packaged shell output and cwd' });
  await evaluate(page, `document.querySelector('[aria-label="Hide terminal panel"]').click()`);
  const hiddenCommand = "printf '\\nHIDDEN_SHELL_ALIVE\\n'\r";
  await evaluate(page, `window.lingxi.terminal.input(${JSON.stringify(id)}, ${JSON.stringify(hiddenCommand)})`);
  await waitFor(() => evaluate(page, `window.lingxi.terminal.list(${JSON.stringify(scope)}).then(rows => rows.some(row => row.output.includes('\\r\\nHIDDEN_SHELL_ALIVE\\r\\n')))`), { label: 'hidden terminal continues running' });
  await evaluate(page, `document.querySelector('[aria-label="Toggle terminal"]').click()`);
  await waitFor(() => evaluate(page, `document.querySelector('#desktop-terminal:not([hidden]) .xterm') !== null`), { label: 'terminal restored' });
  const screenshot = await page.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(tmpdir(), 'lingxi-terminal-packaged.png'), Buffer.from(screenshot.data, 'base64'));
  await evaluate(page, `window.lingxi.terminal.close(${JSON.stringify(id)})`);
  assert.equal((await evaluate(page, `window.lingxi.terminal.list(${JSON.stringify(scope)})`)).length, 0);
  await evaluate(page, `document.querySelector('[aria-label="Hide terminal panel"]').click()`);
  log('packaged bottom terminal: renderer, Unicode, project cwd, hide/resume and close passed');
}

async function assertScheduledPersistenceBeforeRestart(page, model) {
  const active = await evaluate(page, 'window.lingxi.bootstrap().then(value => value.activeSession)');
  const scopes = await evaluate(page, 'window.lingxi.scheduled.scopes()');
  assert(scopes.some(scope => scope.id === 'global'), 'No project scope is available');
  const records = [];
  for (const scope of scopes.filter(scope => scope.id === 'global' || scope.projectPath === active.projectPath)) {
    const request = {
      action: 'create', cron: '0 9 1 1 *', prompt: 'Packaged cron persistence probe',
      recurring: true, durable: true, no_expiry: true,
      automation: { version: 2, name: 'Packaged cron probe', status: 'active', model,
        reasoning: { type: 'automatic' }, runMode: 'new_session', notificationPolicy: 'none' },
    };
    const jobs = await evaluate(page, `window.lingxi.scheduled.manage(${JSON.stringify(scope.id)}, ${JSON.stringify(request)})`);
    const task = jobs.find(job => job.automation?.name === request.automation.name);
    assert(task, 'trusted host can create a scheduled task without a CLI trust record');
    assert.equal(task.automation.status, 'active');
    await evaluate(page, `window.lingxi.scheduled.manage(${JSON.stringify(scope.id)}, ${JSON.stringify({action: 'pause', id: task.id})})`);
    await evaluate(page, `window.lingxi.scheduled.manage(${JSON.stringify(scope.id)}, ${JSON.stringify({action: 'update', id: task.id, cron: request.cron, prompt: 'Updated packaged cron probe'})})`);
    records.push({ scopeId: scope.id, id: task.id, model });
  }
  assert.equal(records.length, 2, 'project and managed No project scopes were exercised');
  assert.deepEqual(await evaluate(page, 'window.lingxi.bootstrap().then(value => value.activeSession)'), active,
    'scheduled management must not switch foreground conversation');
  return records;
}

async function assertScheduledPersistenceAfterRestart(page, records, workspaceRoot) {
  for (const record of records) {
    const jobs = await evaluate(page, `window.lingxi.scheduled.manage(${JSON.stringify(record.scopeId)}, {action: 'list'})`);
    const task = jobs.find(job => job.id === record.id);
    assert.equal(task?.automation?.status, 'paused');
    assert.equal(task.automation.model, record.model);
    assert.deepEqual(task.automation.reasoning, { type: 'automatic' });
    assert.equal(task.automation.runMode, 'new_session');
    assert.equal(task.prompt, 'Updated packaged cron probe');
    await evaluate(page, `window.lingxi.scheduled.manage(${JSON.stringify(record.scopeId)}, ${JSON.stringify({action: 'history', id: record.id})})`);
    if (record.scopeId === 'global') {
      await evaluate(page, `window.lingxi.scheduled.manage('global', ${JSON.stringify({ action: 'update', id: record.id, cron: '* * * * *', prompt: task.prompt })})`);
      await evaluate(page, `window.lingxi.scheduled.manage('global', ${JSON.stringify({ action: 'resume', id: record.id })})`);
      const run = await waitFor(async () => {
        const jobs = await evaluate(page, `window.lingxi.scheduled.manage('global', {action: 'history', id: ${JSON.stringify(record.id)}})`);
        const latest = jobs.find(job => job.id === record.id)?.automation?.runs?.at(-1);
        if (latest && ['failed', 'cancelled', 'interrupted'].includes(latest.status)) {
          throw new Error(`Packaged cron failed: ${latest.error ?? latest.status}`);
        }
        return latest?.status === 'succeeded' ? latest : undefined;
      }, { timeoutMs: 95_000, label: 'No project cron executes in its generated workspace' });
      assert(run.sessionTranscriptPath, 'cron must retain its exact transcript source');
      const rows = readFileSync(run.sessionTranscriptPath, 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line));
      const cwd = rows.map(row => row.relocatedCwd ?? row.cwd).filter(value => typeof value === 'string').at(-1);
      const taskRoot = join(realpathSync.native(workspaceRoot), 'cron', createHash('sha256').update(record.id).digest('hex'));
      assert(cwd?.startsWith(join(taskRoot, 'runs') + sep), 'cron session cwd must be below the branded workspace sibling');
      assert(lstatSync(join(cwd, 'work')).isDirectory());
      assert(lstatSync(join(cwd, 'outputs')).isDirectory());
      assert(lstatSync(join(taskRoot, 'memory.md')).isFile());
      await evaluate(page, `window.lingxi.scheduled.manage('global', ${JSON.stringify({ action: 'pause', id: record.id })})`);
      log('No project cron execution, transcript cwd, work/outputs and task memory verified');
    }
    const remaining = await evaluate(page, `window.lingxi.scheduled.manage(${JSON.stringify(record.scopeId)}, ${JSON.stringify({action: 'delete', id: record.id})})`);
    assert(!remaining.some(job => job.id === record.id), 'scheduled task deletion persists');
  }
  log('scheduled project/No project create, pause, edit, restart, history and delete verified');
}

async function prepareCustomProvider(page, probe, restore = false) {
  const sessionId = await evaluate(page, `(async () => {
    const current = await window.lingxi.bootstrap();
    return (await window.lingxi.newSession(current.workspace.path)).activeSession.sessionId;
  })()`);
  await setupSmokeCollectors(page, sessionId);
  if (!restore) {
    const patch = { providers: { [probe.providerId]: {
      type: 'openai', baseUrl: probe.baseUrl, models: [{ id: 'smoke-model' }],
    } } };
    await evaluate(page, `(async () => { const snapshot = await window.lingxi.providerSettings(); await window.lingxi.providerSettings(${JSON.stringify(patch)}, snapshot.raw); })()`);
  }
  await evaluate(page, `window.lingxi.command(${JSON.stringify(sessionId)}, {type:'refresh_listings',which:[{type:'settings'}]})`);
  await waitFor(() => evaluate(page, `window.__lingxiSmoke.events.some(e => e.type === 'settings_snapshot' && JSON.parse(e.effective_json).providers?.[${JSON.stringify(probe.providerId)}])`), { label: 'custom provider settings snapshot' });
  log('custom provider settings confirmed');
  let metadata = await evaluate(page, 'window.lingxi.providerAccounts.snapshot()');
  if (!restore) {
    metadata = await evaluate(page, `window.lingxi.providerAccounts.command(${JSON.stringify({
      action: 'add', expectedRevision: metadata.configRevision, providerId: probe.providerId,
      displayName: 'Packaged smoke test account', executionBindingId: probe.providerId, authKind: 'api_key',
    })}, ${JSON.stringify(probe.key)})`);
    log('custom provider account stored');
  }
  const credential = metadata.accounts.find(entry => entry.accountId === probe.accountId)
    ?? metadata.accounts.find(entry => entry.providerId === probe.providerId);
  assert.ok(credential, 'custom account must survive a full packaged app restart');
  probe.accountId = credential.accountId;
  assert.equal(metadata.health.find(entry => entry.accountId === credential.accountId)?.status, 'ready');
  assert.equal(isSafeProviderAccountSnapshot(metadata), true);
  assert.equal(JSON.stringify(metadata).includes(probe.key), false, 'account status must contain metadata only');
  return sessionId;
}

async function removeCustomProviderAccount(page, probe) {
  if (!probe.accountId) return;
  const snapshot = await evaluate(page, 'window.lingxi.providerAccounts.snapshot()');
  if (!snapshot.accounts.some(entry => entry.accountId === probe.accountId)) return;
  const cleared = await evaluate(page, `window.lingxi.providerAccounts.command(${JSON.stringify({
    action: 'remove', expectedRevision: snapshot.configRevision, accountId: probe.accountId,
  })})`);
  assert.equal(cleared.accounts.some(entry => entry.accountId === probe.accountId), false);
}

export function assertCustomProviderTurn(events) {
  assert.equal(events.some((event) => event.type === 'error' || (event.type === 'system_notice' && event.is_error)), false, 'custom provider turn must not report an error');
  assert.equal(events.find((event) => event.type === 'turn_ended')?.outcome?.type, 'end_turn', 'custom provider turn must finish successfully');
  const answer = events.filter((event) => event.type === 'text_delta').map((event) => event.text).join('');
  assert.equal(answer.includes('Custom provider verified.'), true, 'custom provider response must reach the client');
}

async function assertCustomProviderRequest(page, probe, sessionId) {
  const previousRequestCount = probe.requests.length;
  await evaluate(page, `window.lingxi.command(${JSON.stringify(sessionId)}, ${JSON.stringify({ type: 'set_model', model: `${probe.providerId}/smoke-model` })})`);
  await evaluate(page, `window.lingxi.sendPrompt(${JSON.stringify(sessionId)}, 'Reply briefly, without calling tools.')`);
  await waitFor(() => probe.requests.length > previousRequestCount, { label: 'custom provider authenticated request' });
  assert.ok(probe.requests.every((request) => request.authenticated), 'custom model must resolve its own stored key');
  assert.equal(JSON.parse(probe.requests[0].body).model, 'smoke-model');
  await waitFor(() => evaluate(page, `window.__lingxiSmoke.events.some(e => e.type === 'turn_ended')`), { label: 'custom provider turn completion' });
  assertCustomProviderTurn(await evaluate(page, 'window.__lingxiSmoke.events'));
  const diagnostics = await evaluate(page, 'window.lingxi.diagnostics()');
  assert.equal(JSON.stringify(diagnostics).includes(probe.key), false);
}

export function assertPackagedLocalAppAuthoring(app, sourceSession) {
  assert(app && typeof app.id === 'string' && app.id.length > 0, 'the published app must have a stable native id');
  const authoring = app.authoring;
  assert(authoring && typeof authoring === 'object', 'new Local App publication must persist its host-verified authoring binding');
  assert.equal(typeof authoring.workspaceId, 'string');
  assert(authoring.workspaceId.length > 0, 'authoring workspace identity must be stable');
  assert.equal(authoring.sourceDirectory, sourceSession.projectPath, 'authoring source must be the dedicated app Project cwd');
  assert.deepEqual(authoring.projectRef, { kind: 'desktopPath', path: sourceSession.projectPath }, 'Desktop project identity must point to the dedicated source Project');
  assert.equal(authoring.authoringSessionId, sourceSession.sessionId, 'authoring must bind the real Code session that created the app');
  assert.equal(sourceSession.mode, 'code', 'Local App authoring must use a Code session');
  if (authoring.appId !== undefined) assert.equal(authoring.appId, app.id, 'authoring binding must be owned by the published app');
}

export function assertPackagedLocalAppPendingAuthoring(receipts, sourceSession) {
  assert(Array.isArray(receipts), 'Native Create must persist host-owned authoring receipts');
  const matches = receipts.filter(entry => entry?.authoring?.authoringSessionId === sourceSession.sessionId);
  assert.equal(matches.length, 1, 'Native Create must persist exactly one pending binding for its Code session');
  const entry = matches[0];
  assert.equal(entry.sessionMode, 'code', 'the native receipt must authorize a Code session');
  assert.equal(typeof entry.sourceId, 'string');
  assert(entry.sourceId.length > 0, 'the native receipt must identify its verified source');
  assert(entry.sourceIdentity && typeof entry.sourceIdentity === 'object', 'the native receipt must retain its rooted source identity');
  const authoring = entry.authoring;
  assert.equal(typeof authoring.workspaceId, 'string');
  assert(authoring.workspaceId.length > 0, 'pending authoring must have a stable workspace identity');
  assert.equal(authoring.sourceDirectory, sourceSession.projectPath, 'pending sourceDirectory must match the native Project');
  assert.deepEqual(authoring.projectRef, { kind: 'desktopPath', path: sourceSession.projectPath }, 'pending projectRef must match the native Project');
  assert.equal(authoring.authoringSessionId, sourceSession.sessionId, 'pending authoring must identify the real Code session');
  assert.equal(authoring.appId, undefined, 'a new pending binding cannot already belong to a published app');
  return authoring;
}

export function assertPackagedLocalAppCatalogSession(catalog, sourceSession) {
  assert(catalog && Array.isArray(catalog.sessions), 'the published Code session must be present in the persisted catalog');
  const row = catalog.sessions.find(session => session?.uuid === sourceSession.sessionId);
  assert(row, 'the first prompt must persist the authoring session before publication is accepted');
  assert.equal(row.mode, 'code', 'the persisted authoring session must be a Code session');
  // HostController deliberately removes the private engine cwd from this
  // renderer DTO. Publication revalidates it in the trusted host; the complete
  // AuthoringRef is checked separately against this exact source Project.
  assert.equal('workspace_path' in row, false, 'the public catalog must not expose private engine workspace paths');
  return row;
}

export async function enterPackagedProjectName(page, projectName) {
  const selector = '[aria-label="Project name"]';
  await waitFor(() => evaluate(page, `(() => {
    const input = document.querySelector(${JSON.stringify(selector)});
    return Boolean(input && !input.disabled && !input.readOnly);
  })()`), { label: 'editable native Local App Project name input' });
  await evaluate(page, `document.querySelector(${JSON.stringify(selector)}).focus()`);
  await waitFor(() => evaluate(page, `document.activeElement === document.querySelector(${JSON.stringify(selector)})`),
    { label: 'focused native Local App Project name input' });
  await page.send('Input.insertText', { text: projectName });
  const enteredValue = await waitFor(() => evaluate(page, `(() => {
    const input = document.querySelector(${JSON.stringify(selector)});
    return input?.value === ${JSON.stringify(projectName)} && !document.querySelector('.create-project-submit')?.disabled
      ? input.value : undefined;
  })()`), { label: 'React Project name input update' });
  assert.equal(enteredValue, projectName, 'native Project name must be entered through the renderer input');
}

/** Trust only the test-generated Local App source folder shown in the dialog. */
export async function confirmPackagedLocalAppSourceTrust(page, expectedPath) {
  const actualPath = await waitFor(() => evaluate(page,
    `document.querySelector('.trust-folder-dialog .trust-folder-path')?.textContent`),
  { label: 'native Local App source folder trust confirmation' });
  assert.equal(actualPath, expectedPath, 'smoke must never trust a different folder');
  const confirmed = await evaluate(page, `(() => {
    const dialog = document.querySelector('.trust-folder-dialog');
    const path = dialog?.querySelector('.trust-folder-path')?.textContent;
    const button = dialog?.querySelector('.trust-folder-confirm');
    if (path !== ${JSON.stringify(expectedPath)} || !button || button.disabled) return false;
    button.click();
    return true;
  })()`);
  assert.equal(confirmed, true, 'the exact test folder trust confirmation must remain available');
}

async function createPackagedLocalAppSourceSession(page, creatorSessionId, configHome) {
  const creator = await evaluate(page, 'window.lingxi.bootstrap().then(state => state.activeSession)');
  assert.equal(creator?.sessionId, creatorSessionId, 'the ordinary custom-provider chat must remain the creator chat');
  await showPackagedAppLibrary(page);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.local-app-library-header button'))`), { label: 'open Apps before Local App creation' });
  await evaluate(page, `document.querySelector('.local-app-library-header button').click()`);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('[aria-label="Project name"]'))`), { label: 'native Local App source Project dialog' });
  const projectName = `Packaged Local App Source ${process.pid}`;
  await enterPackagedProjectName(page, projectName);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.create-project-submit:not(:disabled)'))`), { label: 'native Local App source Project is ready to create' });
  await evaluate(page, `document.querySelector('.create-project-submit').click()`);
  await confirmPackagedLocalAppSourceTrust(page, join(configHome + '-workspaces', 'apps', projectName));
  const sourceSession = await waitFor(async () => {
    const state = await evaluate(page, 'window.lingxi.bootstrap()');
    const active = state.activeSession;
    if (!active || active.sessionId === creatorSessionId || !active.projectPath || active.projectPath === creator.projectPath
      || state.connection?.status !== 'connected' || !state.settings.projects.includes(active.projectPath)) return undefined;
    const prompt = await evaluate(page, `document.querySelector('[aria-label="Prompt"]')?.innerText ?? ''`);
    if (!prompt.includes('创建一个本地应用')) return undefined;
    return { projectPath: active.projectPath, sessionId: active.sessionId, mode: 'code',
      projectRegistered: state.settings.projects.includes(active.projectPath) };
  }, { label: 'native Local App source Code session, connected runtime and initial composer draft' });
  assert.notEqual(sourceSession.projectPath, creator.projectPath, 'Local App creation must leave the ordinary creator workspace');
  assert.equal(sourceSession.projectRegistered, true, 'the dedicated source Project must be registered in Desktop');
  assert.equal(sourceSession.mode, 'code', 'the source chat must be a real Code session');
  const pendingRoot = join(configHome, 'local-apps', 'v2', 'pending-authoring');
  const receipts = await waitFor(() => {
    try {
      const entries = readdirSync(pendingRoot).filter(name => name.startsWith('owner-') && name.endsWith('.json'))
        .map(name => JSON.parse(readFileSync(join(pendingRoot, name), 'utf8')));
      return entries.some(entry => entry?.authoring?.authoringSessionId === sourceSession.sessionId) ? entries : undefined;
    } catch { return undefined; }
  }, { label: 'Native Create host-persisted authoring receipt' });
  assertPackagedLocalAppPendingAuthoring(receipts, sourceSession);
  return sourceSession;
}

async function assertColdSessionOpen(page, sessionId, activateWindow) {
  const cycles = Number(process.env.LINGXI_COLD_RELOAD_CYCLES ?? 10);
  assert(Number.isSafeInteger(cycles) && cycles >= 10 && cycles <= 100, 'cold reload cycles must be an integer from 10 to 100');
  const evidence = [];
  const capture = () => evaluate(page, `(async () => {
    const feed = document.querySelector('.desktop-stage-feed');
    const rows = [...document.querySelectorAll('[data-message-role="assistant"]')];
    const recovery = (await window.lingxi.diagnostics()).flatMap(entry => {
      try {
        const event = JSON.parse(entry.message);
        return event.event === 'session_recovery' && (!event.sessionId || event.sessionId === ${JSON.stringify(sessionId)})
          ? [{ timestamp: entry.timestamp, ...event }] : [];
      } catch { return []; }
    }).slice(-60);
    return { documentVisibility: document.visibilityState, documentFocused: document.hasFocus(),
      sessionKey: feed?.getAttribute('data-session-key'), itemCount: feed?.getAttribute('data-item-count'), rowCount: feed?.getAttribute('data-row-count'),
      assistantRows: rows.length,
      domHasReply: rows.some(row => row.textContent.includes('Custom provider verified.')),
      paintedTextHasReply: document.body.innerText.includes('Custom provider verified.'),
      rows: rows.map(row => { const content = row.querySelector('.transcript-run-item') ?? row;
        const rect = content.getBoundingClientRect();
        return { contentVisibility: getComputedStyle(content).contentVisibility, width: rect.width, height: rect.height,
          top: rect.top, bottom: rect.bottom, visible: content.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true }) }; }),
      viewport: { width: innerWidth, height: innerHeight }, recoveryTrace: recovery };
  })()`);
  const save = () => {
    if (!process.env.LINGXI_LOCAL_APP_EVIDENCE) return;
    mkdirSync(process.env.LINGXI_LOCAL_APP_EVIDENCE, { recursive: true });
    writeFileSync(join(process.env.LINGXI_LOCAL_APP_EVIDENCE, 'cold-open-cycles.json'), JSON.stringify(evidence, null, 2));
  };
  const before = await evaluate(page, 'window.lingxi.bootstrap()');
  assert.equal(before.runtimes.some(runtime => runtime.sessionId === sessionId), false,
    'the saved session must have no live runtime before its first open');
  const result = await evaluate(page, `(async () => {
    const id = ${JSON.stringify(sessionId)};
    const events = [], commands = [];
    let connected = false;
    const offEvent = window.lingxi.onEvent(envelope => {
      if (envelope.sessionId === id) events.push(envelope.event);
    });
    const offState = window.lingxi.onConnectionStateChanged(envelope => {
      if (connected || envelope.sessionId !== id || envelope.event.status !== 'connected') return;
      connected = true;
      for (const type of ['list_models', 'get_conversation_controls', 'task_list', 'list_session_agents']) {
        commands.push(window.lingxi.command(id, {type}).then(() => ({type}), error => ({type, error: String(error)})));
      }
    });
    try {
      const opened = await window.lingxi.openSession(${JSON.stringify(before.workspace.path)}, id);
      return {connected, events, commands: await Promise.all(commands), active: opened.activeSession?.sessionId,
        remoteStatus: await window.lingxi.remoteControl.status(), remoteSessions: await window.lingxi.remoteControl.sessions()};
    } finally { offEvent(); offState(); }
  })()`);
  assert.equal(result.connected, true, 'the cold runtime must publish connected');
  assert.equal(result.active, sessionId);
  assert.equal(result.commands.length, 4, 'metadata must be requested at the handshake, before owned resume completes');
  assert.deepEqual(result.commands.filter(command => command.error), [], 'connected-time metadata must wait instead of reporting a resume error');
  assert(result.events.some(event => event.type === 'session_resumed' && event.session_id === sessionId && event.messages.length > 0),
    'the exact saved session must restore its persisted transcript');
  assert(result.events.some(event => event.type === 'model_changed'), 'the saved model must be restored');
  assert.equal(typeof result.remoteStatus.enabled, 'boolean', 'the packaged Remote Control status IPC must remain available');
  assert(Array.isArray(result.remoteSessions.sessions), 'the packaged Remote Control sessions IPC must remain available');
  // Opening a native session intentionally retains the Teams workspace. Select
  // Home explicitly before checking the ordinary Code transcript on reload.
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.desktop-nav-rail button[aria-label="Home"]')) && !document.querySelector('.desktop-nav-rail button[aria-label="Home"]').disabled && document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') !== 'true'`), { label: 'cold-open renderer session admission before Home navigation' });
  await evaluate(page, `document.querySelector('.desktop-nav-rail button[aria-label="Home"]').click()`);
  // useSidebarMode treats an absent sidebar preference as Home.
  try {
    await waitFor(() => evaluate(page, `window.lingxi.bootstrap().then(state => (state.settings.sidebar?.mode ?? 'home') === 'home' && document.querySelector('.desktop-nav-rail button[aria-label="Home"]')?.getAttribute('data-active') === 'true')`), { label: 'cold-open Home workspace persisted' });
  } catch (error) {
    log(`cold-open Home navigation state: ${JSON.stringify(await evaluate(page, `(async () => ({ body: document.body?.innerText, home: document.querySelector('.desktop-nav-rail button[aria-label="Home"]')?.outerHTML, rail: document.querySelector('.desktop-nav-rail')?.outerHTML, bootstrap: await window.lingxi.bootstrap() }))()`))}`);
    throw error;
  }
  for (let reload = 0; reload < cycles; reload++) {
    const started = Date.now();
    if (reload > 0) {
      if (reload % 2 === 0) {
        // /clear preserves saved history but closes its runtime. In this
        // disposable profile it creates a genuinely cold owner again, unlike
        // restarting the same cached SessionRuntime instance.
        await evaluate(page, `window.lingxi.clearSession(${JSON.stringify(sessionId)})`);
        const closed = await evaluate(page, 'window.lingxi.bootstrap()');
        assert(!closed.runtimes.some(runtime => runtime.sessionId === sessionId), 'cold cycle must dispose the previous runtime');
        await evaluate(page, `window.lingxi.openSession(${JSON.stringify(before.workspace.path)}, ${JSON.stringify(sessionId)})`);
      } else {
        await evaluate(page, `window.lingxi.restartBridge(${JSON.stringify(sessionId)})`);
      }
    }
    const beforeReload = (await page.send('Page.getFrameTree')).frameTree.frame;
    await page.send('Page.reload', { ignoreCache: true });
    await waitFor(async () => {
      const frame = (await page.send('Page.getFrameTree')).frameTree.frame;
      return frame.url === beforeReload.url && frame.loaderId !== beforeReload.loaderId;
    }, { label: 'cold-open renderer reload committed' });
    await waitFor(() => evaluate(page, `document.readyState === 'complete' && Boolean(document.body)`), { label: 'cold-open renderer document ready' });
    try {
      // Transcript rows use `content-visibility: auto`, and Chromium leaves off-screen (or hidden-window) rows out of
      // `innerText`; `textContent` still holds every rendered row.
      await waitFor(() => evaluate(page, `(() => {
        const feed = document.querySelector('.desktop-stage-feed');
        return feed && (!feed.hasAttribute('data-session-key') || feed.getAttribute('data-session-key') === ${JSON.stringify(sessionId)})
          && [...feed.querySelectorAll('[data-message-role="assistant"]')].some(row => row.textContent.includes('Custom provider verified.'));
      })()`), { label: 'cold-open saved transcript rendered' });
      evidence.push({ reload, recovery: reload === 0 || reload % 2 === 0 ? 'cold-owner' : 'restart',
        durationMs: Date.now() - started, ...await capture() });
      save();
    } catch (error) {
      const state = await evaluate(page, `(async () => ({ url: location.href, ready: document.readyState,
        body: document.body?.innerText, textContent: document.body?.textContent, bootstrap: await window.lingxi?.bootstrap(),
        replay: await new Promise(resolve => {
          const events = [];
          const off = window.lingxi.onEvent(envelope => {
            if (envelope.sessionId !== ${JSON.stringify(sessionId)}) return;
            const event = envelope.event;
            events.push({ sequence: envelope.sequence, type: event.type,
              ...(event.type === 'session_resumed' ? { messages: event.messages.map(message => ({
                role: message.role, blocks: message.blocks.map(block => ({ type: block.type,
                  ...(block.type === 'text' ? { text: block.text } : {}) })) })) } : {}) });
          });
          setTimeout(() => { off(); resolve(events); }, 300);
        }) }))()`);
      const rendering = await capture();
      log(`cold-open renderer state: ${JSON.stringify({ reload, ...state, rendering })}`);
      if (process.env.LINGXI_LOCAL_APP_EVIDENCE) {
        mkdirSync(process.env.LINGXI_LOCAL_APP_EVIDENCE, { recursive: true });
        writeFileSync(join(process.env.LINGXI_LOCAL_APP_EVIDENCE, 'cold-open-failure.json'), JSON.stringify({ reload, ...state, rendering }, null, 2));
      }
      throw error;
    }
  }
  // State restoration and visible painting are separate assertions. A real
  // foreground viewport must show the reply as well as retaining its DOM row.
  // CDP focuses WebContents, but cannot raise an occluded native macOS window.
  // Exercise the app's ordinary second-launch activation before asking a
  // content-visibility:auto row to paint. Focus emulation is not visibility.
  await activateWindow();
  await page.send('Page.bringToFront');
  try {
    await waitFor(() => evaluate(page, `document.visibilityState === 'visible'`), { label: 'cold-open native window visible' });
    await waitFor(() => evaluate(page, `document.body.innerText.includes('Custom provider verified.')`), { label: 'cold-open foreground transcript painted' });
  } catch (error) {
    evidence.push({ foreground: false, ...await capture() });
    save();
    throw error;
  }
  evidence.push({ foreground: true, ...await capture() });
  save();
  assert.equal(await evaluate(page, `document.body.textContent.includes('This chat is being resumed')`), false,
    'the cold-open renderer must not show the original error');
  log(`cold saved session: handshake-time metadata, transcript/model restore, ${cycles} renderer reloads alternating cold owners and runtime restarts, foreground painting and Remote Control IPC passed`);
}

async function openPackagedProjectDraft(page, projectPath) {
  // App source projects belong to the first-level Apps workspace and are
  // hidden from Home. Exercise Home's ordinary New chat action to prove its
  // Global draft is isolated from authoring.
  const active = await evaluate(page, 'window.lingxi.bootstrap().then(state => state.activeSession)');
  assert.equal(active?.projectPath, projectPath);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.desktop-nav-rail button[aria-label="Home"]')) && !document.querySelector('.desktop-nav-rail button[aria-label="Home"]').disabled && document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') !== 'true'`), { label: 'Home navigation admission before ordinary New chat' });
  await evaluate(page, `document.querySelector('.desktop-nav-rail button[aria-label="Home"]').click()`);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.sidebar-new-chat:not(:disabled)'))`), { label: 'ordinary Home New chat action' });
  await evaluate(page, `document.querySelector('.sidebar-new-chat').click()`);
  await waitFor(() => evaluate(page, `window.lingxi.bootstrap().then(state => state.activeSession?.projectId === null && state.activeSession?.sessionId !== ${JSON.stringify(active.sessionId)})`), { label: 'ordinary Global chat draft opened from Home' });
}

async function showPackagedAppLibrary(page) {
  await evaluate(page, `Array.from(document.querySelectorAll('.desktop-nav-rail button')).find(button => button.getAttribute('aria-label') === 'Apps')?.click()`);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.local-apps-library-link'))`), { label: 'Apps workspace library action' });
  await evaluate(page, `document.querySelector('.local-apps-library-link').click()`);
}

async function assertPackagedAppDraftWithoutReload(page, probe, created, sessionId) {
  const marker = `APP_DRAFT_NO_RELOAD_${Date.now()}`;
  const frame = (await page.send('Page.getFrameTree')).frameTree.frame;
  const requestCount = probe.requests.length;
  await evaluate(page, `document.querySelector('.desktop-nav-rail button[aria-label="Apps"]').click()`);
  const title = await waitFor(() => evaluate(page, `(() => {
    const group = Array.from(document.querySelectorAll('.local-apps-sidebar-group')).find(group => group.querySelector('.local-apps-sidebar-name')?.textContent === ${JSON.stringify(created.name)});
    return group?.querySelector('.local-apps-sidebar-chats button[aria-current="page"]')?.getAttribute('title');
  })()`), { label: 'authoring chat in Apps sidebar' });
  const draft = await evaluate(page, `(() => {
    const prompt = document.querySelector('[aria-label="Prompt"]');
    if (!prompt?.isContentEditable) throw new Error('App draft must be editable');
    window.__packagedDraftRendererMarker = ${JSON.stringify(marker)};
    prompt.focus();
    const range = document.createRange(); range.selectNodeContents(prompt); range.collapse(false);
    const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
    document.execCommand('insertText', false, ${JSON.stringify(marker)});
    return prompt.innerText;
  })()`);
  assert.ok(draft.includes(created.id) && draft.includes(marker));
  const newChat = async () => {
    await evaluate(page, `document.querySelector('.desktop-nav-rail button[aria-label="Home"]').click()`);
    await waitFor(() => evaluate(page, `Boolean(document.querySelector('.sidebar-new-chat:not(:disabled)'))`), { label: 'Home new-chat action' });
    await evaluate(page, `document.querySelector('.sidebar-new-chat').click()`);
    await waitFor(() => evaluate(page, `window.lingxi.bootstrap().then(state => typeof state.activeSession?.sessionId === 'string' && state.activeSession.sessionId !== ${JSON.stringify(sessionId)} && Boolean(document.querySelector('[aria-label="Prompt"][contenteditable="true"]')) && document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') !== 'true' && !document.querySelector('[aria-label="Prompt"]').innerText.includes(${JSON.stringify(created.id)}) && !document.querySelector('[aria-label="Prompt"]').innerText.includes(${JSON.stringify(marker)}) && !document.querySelector('.local-app-chat-card'))`), { label: 'unrelated live chat excludes App draft and transcript' });
  };
  const returnToAuthor = async () => {
    await evaluate(page, `document.querySelector('.desktop-nav-rail button[aria-label="Apps"]').click()`);
    await waitFor(() => evaluate(page, `(() => {
      const group = Array.from(document.querySelectorAll('.local-apps-sidebar-group')).find(group => group.querySelector('.local-apps-sidebar-name')?.textContent === ${JSON.stringify(created.name)});
      const disclosure = group?.querySelector('.local-apps-sidebar-disclosure');
      if (disclosure?.getAttribute('aria-expanded') === 'false') { disclosure.click(); return false; }
      const matches = Array.from(group?.querySelectorAll('.local-apps-sidebar-chats button') ?? []).filter(button => button.title === ${JSON.stringify(title)});
      if (matches.length !== 1 || matches[0].disabled) return false; matches[0].click(); return true;
    })()`), { label: 'return through original Apps chat without Modify' });
    await waitFor(() => evaluate(page, `window.lingxi.bootstrap().then(state => state.activeSession?.sessionId === ${JSON.stringify(sessionId)} && document.querySelector('[aria-label="Prompt"]')?.innerText === ${JSON.stringify(draft)} && Boolean(document.querySelector('.local-app-chat-card')))`), { label: 'original live App draft restored byte for byte' });
    assert.equal(await evaluate(page, `document.querySelector('[aria-label="Prompt"]').innerText.split(${JSON.stringify(marker)}).length - 1`), 1);
  };
  await newChat();
  await returnToAuthor();
  await newChat();
  await returnToAuthor();
  assert.equal((await page.send('Page.getFrameTree')).frameTree.frame.loaderId, frame.loaderId, 'App draft round trip must not reload');
  assert.equal(await evaluate(page, 'window.__packagedDraftRendererMarker'), marker);
  assert.equal(probe.requests.length, requestCount, 'draft navigation must not invoke a model');
  log('packaged App draft: live UI round trip, exact restoration, unrelated-chat isolation and zero model calls passed');
}

async function assertPackagedLocalApps(page, probe, creatorSessionId, cdpPort, configHome) {
  const sourceSession = await createPackagedLocalAppSourceSession(page, creatorSessionId, configHome);
  const sessionId = sourceSession.sessionId;
  await setupSmokeCollectors(page, sessionId);
  await evaluate(page, `window.lingxi.command(${JSON.stringify(sessionId)}, {type:'set_model',model:${JSON.stringify(`${probe.providerId}/smoke-model`)}})`);
  await evaluate(page, `window.lingxi.command(${JSON.stringify(sessionId)}, {type:'set_permission_mode',mode:'bypassPermissions'})`);
  await evaluate(page, `window.lingxi.sendPrompt(${JSON.stringify(sessionId)}, 'LOCAL_APP_PACKAGED_SMOKE: Create a simple local app using LocalApp.')`);
  await waitFor(() => evaluate(page, `window.__lingxiSmoke.events.some(e => e.type === 'turn_ended')`), { label: 'packaged LocalApp tool turn' });
  const sourceCatalog = await waitFor(async () => {
    const catalog = await evaluate(page, `window.lingxi.listProjectSessions(${JSON.stringify(sourceSession.projectPath)}, 'current')`);
    return catalog.sessions?.some(row => row.uuid === sessionId) ? catalog : undefined;
  }, { label: 'post-prompt persisted authoring Code session' });
  assertPackagedLocalAppCatalogSession(sourceCatalog, sourceSession);
  const apps = await evaluate(page, 'window.lingxi.localApps.list()');
  const created = apps.find(app => app.name === 'Packaged Local App');
  assert(created, 'the real packaged model dispatcher must publish into the Desktop library');
  const preparation = probe.requests.flatMap(request => JSON.parse(request.body).messages ?? []).find(message => message.tool_call_id === 'local-app-preparation');
  assert(preparation, 'a real Bash preparation must precede the publication card');
  assert.match(typeof preparation.content === 'string' ? preparation.content : JSON.stringify(preparation.content), /LocalApp package check/, 'packaged Bash preparation must actually execute successfully');
  assertPackagedLocalAppAuthoring(created, sourceSession);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.local-app-chat-card button'))`), { label: 'LocalApp chat result card' });
  await evaluate(page, `document.querySelector('.local-app-chat-card button').click()`);
  const target = await waitFor(async () => (await fetchJson(`http://127.0.0.1:${cdpPort}/json/list`)).find(target => target.url.startsWith(`lingxi-app://${created.id}/`)), { label: 'independent packaged LocalApp window' });
  const guest = new CdpConnection(target.webSocketDebuggerUrl);
  try {
    await waitFor(() => evaluate(guest, `document.querySelector('h1')?.textContent === 'LocalApp tool connected'`), { label: 'packaged app rendered' });
    assert.equal(await evaluate(guest, `typeof require + ':' + typeof window.lingxi`), 'undefined:undefined');
    await evaluate(guest, `localStorage.setItem('progress','7')`);
    await evaluate(page, `window.lingxi.localApps.stop(${JSON.stringify(created.id)})`);
  } finally { await guest.close(); }
  await evaluate(page, `window.lingxi.localApps.open(${JSON.stringify(created.id)})`);
  const reopened = await waitFor(async () => (await fetchJson(`http://127.0.0.1:${cdpPort}/json/list`)).find(target => target.url.startsWith(`lingxi-app://${created.id}/`)), { label: 'reopened packaged app' });
  const reopenedGuest = new CdpConnection(reopened.webSocketDebuggerUrl);
  try { assert.equal(await evaluate(reopenedGuest, `localStorage.getItem('progress')`), '7'); }
  finally { await reopenedGuest.close(); }
  await evaluate(page, `window.lingxi.localApps.stop(${JSON.stringify(created.id)})`);
  await openPackagedProjectDraft(page, sourceSession.projectPath);
  const otherSession = await waitFor(async () => {
    const current = await evaluate(page, 'window.lingxi.bootstrap()');
    return current.activeSession?.projectId === null
      && current.activeSession?.sessionId !== sessionId && current.activeSession?.sessionId;
  }, { label: 'second chat for LocalApp modification' });
  assert.notEqual(otherSession, sessionId, 'the pre-Modify chat must differ from the saved authoring session');
  // Host bootstrap changes before the renderer completes session admission.
  // Navigation is intentionally blocked until that admission commits.
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('[aria-label="Prompt"][contenteditable="true"]')) && document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') !== 'true'`), { label: 'second chat renderer admission before Apps navigation' });
  await showPackagedAppLibrary(page);
  try {
    await waitFor(() => evaluate(page, `Boolean(document.querySelector('.local-app-library .local-app-tile'))`), { label: 'packaged Apps library' });
  } catch (error) {
    const state = await evaluate(page, `(async () => ({ bootstrap: await window.lingxi.bootstrap(), apps: await window.lingxi.localApps.list(), body: document.body.innerText, buttons: Array.from(document.querySelectorAll('button')).map(button => ({ label: button.getAttribute('aria-label'), disabled: button.disabled, text: button.textContent })) }))()`);
    const evidence = process.env.LINGXI_LOCAL_APP_EVIDENCE;
    if (evidence) {
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, 'packaged-apps-library-failure.json'), JSON.stringify(state, null, 2));
    }
    throw error;
  }
  await evaluate(page, `(() => {
    window.__packagedModifyEvents = []; window.__packagedModifyAlerts = []; window.__packagedModifyDomClicks = 0;
    window.__packagedModifyOff = window.lingxi.onLocalAppEdit(event => window.__packagedModifyEvents.push(event));
    const collect = () => { for (const node of document.querySelectorAll('[role="alert"], .local-app-error, .error-banner')) {
      const text = node.innerText?.trim();
      if (text && window.__packagedModifyAlerts.length < 20 && !window.__packagedModifyAlerts.includes(text)) window.__packagedModifyAlerts.push(text);
    } };
    collect(); window.__packagedModifyObserver = new MutationObserver(collect);
    window.__packagedModifyObserver.observe(document.body, { subtree: true, childList: true, characterData: true });
  })()`);
  try {
    await waitFor(() => evaluate(page, `(() => {
      const library = document.querySelector('.local-app-library');
      const tile = Array.from(library?.querySelectorAll('.local-app-tile') ?? []).find(tile => tile.querySelector('h2')?.textContent === ${JSON.stringify(created.name)});
      const modify = Array.from(tile?.querySelectorAll('button') ?? []).find(button => button.textContent.trim() === 'Modify');
      const visible = node => Boolean(node && node.getClientRects().length && node.getBoundingClientRect().width > 0 && node.getBoundingClientRect().height > 0);
      return document.querySelector('.local-apps-library-link')?.getAttribute('aria-current') === 'page'
        && document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') !== 'true'
        && visible(library) && visible(tile) && visible(modify) && !modify.disabled;
    })()`), { label: 'visible Apps library and enabled Modify action' });
    await evaluate(page, `(() => {
    const tile = Array.from(document.querySelectorAll('.local-app-tile')).find(tile => tile.querySelector('h2')?.textContent === ${JSON.stringify(created.name)});
    const modify = Array.from(tile?.querySelectorAll('button') ?? []).find(button => button.textContent.trim() === 'Modify');
    if (!modify || modify.disabled) throw new Error('The published app must expose an enabled Modify action');
    window.__packagedModifyClickState = { libraryCurrent: document.querySelector('.local-apps-library-link')?.getAttribute('aria-current'), visible: modify.getClientRects().length > 0, railDisabled: document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') };
    modify.addEventListener('click', () => window.__packagedModifyDomClicks++, { capture: true, once: true });
    modify.click();
  })()`);
    await waitFor(() => evaluate(page, `window.lingxi.bootstrap().then(state => state.activeSession?.projectPath === ${JSON.stringify(sourceSession.projectPath)} && state.activeSession?.sessionId === '${sessionId}' && document.querySelector('[aria-label="Prompt"]')?.innerText.includes('${created.id}') && Boolean(document.querySelector('.local-app-chat-card')))`), { label: 'Modify restores the dedicated authoring Code session, transcript and app draft' });
  } catch (error) {
    // Structural evidence only: no page text, prompt text, transcript or bootstrap payload, which carry account content.
    const state = await evaluate(page, `(async () => {
      const bootstrap = await window.lingxi.bootstrap();
      const prompt = document.querySelector('[aria-label="Prompt"]');
      return { sessionMatches: bootstrap.activeSession?.sessionId === ${JSON.stringify(sessionId)}, projectMatches: bootstrap.activeSession?.projectPath === ${JSON.stringify(sourceSession.projectPath)},
        activeMode: bootstrap.conversationSpaces?.activeMode, connectionStatus: bootstrap.connection?.status, sidebarMode: bootstrap.settings.sidebar?.mode,
        hasPrompt: Boolean(prompt), draftMatches: Boolean(prompt?.innerText.includes(${JSON.stringify(created.id)})), promptEditable: prompt?.isContentEditable,
        cardCount: document.querySelectorAll('.local-app-chat-card').length,
        railDisabled: document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled'),
        alertCount: document.querySelectorAll('[role="alert"], .local-app-error, .error-banner').length,
        editEvents: window.__packagedModifyEvents, clickState: window.__packagedModifyClickState,
        domClicks: window.__packagedModifyDomClicks, observedAlertCount: window.__packagedModifyAlerts?.length, url: location.protocol + '//' + location.host + location.pathname, title: document.title };
    })()`);
    const evidence = process.env.LINGXI_LOCAL_APP_EVIDENCE;
    if (evidence) {
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, 'packaged-modify-failure.json'), JSON.stringify({ actual: state }, null, 2), { mode: 0o600 });
    }
    log(`packaged Modify failure: active session matches=${state.sessionMatches}, project matches=${state.projectMatches}, draft matches=${state.draftMatches}, card count=${state.cardCount}`);
    throw error;
  } finally {
    await evaluate(page, 'window.__packagedModifyObserver?.disconnect(); window.__packagedModifyOff?.(); window.__packagedModifyOff = undefined');
  }
  await assertPackagedAppDraftWithoutReload(page, probe, created, sessionId);
  // Open an ordinary Global Code draft and verify its composer is isolated from Apps.
  await openPackagedProjectDraft(page, sourceSession.projectPath);
  await waitFor(() => evaluate(page, `window.lingxi.bootstrap().then(state => state.activeSession?.projectId === null && state.activeSession?.sessionId !== '${sessionId}' && !document.querySelector('[aria-label="Prompt"]')?.innerText.includes('${created.id}') && !document.querySelector('.local-app-chat-card'))`), { label: 'ordinary Global chat has its own transcript and draft' });
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('[aria-label="Prompt"][contenteditable="true"]')) && document.querySelector('.desktop-nav-rail')?.getAttribute('aria-disabled') !== 'true'`), { label: 'unrelated chat renderer admission before returning to Apps' });
  await showPackagedAppLibrary(page);
  await waitFor(() => evaluate(page, `Boolean(document.querySelector('.local-app-library .local-app-tile'))`), { label: 'return to application library' });
  const screenshot = await page.send('Page.captureScreenshot', { format: 'png' });
  const evidence = process.env.LINGXI_LOCAL_APP_EVIDENCE;
  if (evidence) { mkdirSync(evidence, { recursive: true }); writeFileSync(join(evidence, 'packaged-local-app-library.png'), Buffer.from(screenshot.data, 'base64')); }
  log('packaged LocalApp: Bash/tool dispatch, visible delivery card, Apps library, isolated window, durable state and source-chat modification passed');
}

export async function unregisterCredentialBrokerForColdStart(channel, run = spawnSync) {
  assert.ok(['development', 'production'].includes(channel));
  const suffix = channel === 'development' ? '.development' : '';
  const target = `gui/${process.getuid()}/com.lingxi.code.credential-broker${suffix}`;
  const stopped = run('/bin/launchctl', ['bootout', target], { encoding: 'utf8', timeout: 10_000 });
  assert.ok([0, 3].includes(stopped.status), 'cold-start broker bootout failed');
  const deadline = Date.now() + 10_000;
  for (;;) {
    const state = run('/bin/launchctl', ['print', target], { encoding: 'utf8', timeout: 10_000 });
    if (state.status === 113) return;
    assert.equal(state.status, 0, 'launchctl failed while waiting for broker removal');
    if (Date.now() >= deadline) throw new Error('credential broker remained registered after bootout');
    await delay(100);
  }
}

/** Isolated production Keychain check; never reads or copies a user's provider key. */
export async function runPackagedCredentialStability(appPath) {
  assert.equal(process.platform, 'darwin');
  appPath = realpathSync(appPath);
  const rendererUrl = pathToFileURL(join(appPath, 'Contents', 'Resources', 'app.asar', 'out', 'renderer', 'index.html')).href;
  const manifest = JSON.parse(readFileSync(join(appPath, 'Contents', 'Resources', 'credential-broker', 'broker-manifest.json'), 'utf8'));
  assert.equal(manifest.channel, 'production', 'production signed package required');
  assert.equal(packagedAppBundleId(appPath), 'com.lingxi.code');
  execFileSync('/usr/bin/codesign', ['--verify', '--deep', '--strict', appPath], { stdio: 'pipe' });
  // A running development app owns a different Broker and Keychain namespace.
  // Fail closed for any production or unidentifiable instance.
  for (const command of runningAppCommands()) {
    const match = command.match(/^\d+\s+(.+?\.app)\/Contents\/MacOS\/LingXi Code(?:\s|$)/);
    assert.ok(match, 'unidentifiable running Desktop instance');
    const other = JSON.parse(readFileSync(join(match[1], 'Contents', 'Resources', 'credential-broker', 'broker-manifest.json'), 'utf8'));
    assert.equal(other.channel, 'development', 'another production Desktop instance is running');
    assert.equal(packagedAppBundleId(match[1]), 'com.lingxi.code.development');
  }
  const tempRoot = mkdtempSync(join('/tmp', 'lingxi-credential-stability-'));
  log(`credential stability evidence: ${tempRoot}`);
  const home = join(tempRoot, 'home'), temporary = join(tempRoot, 'tmp');
  mkdirSync(home, { recursive: true, mode: 0o700 });
  mkdirSync(temporary, { recursive: true, mode: 0o700 });
  const runtime = runtimePathsForHome(home);
  writePackagedProfile(runtime, createAdversarialWorkspace(tempRoot));
  const env = sanitizePackagedAppEnvironment(process.env, {
    HOME: home, CFFIXED_USER_HOME: home, __CFPREFERENCES_AVOID_DAEMON: '1', TMPDIR: temporary, LINGXI_CONFIG_DIR: join(home, '.lingxi'),
  });
  const port = await reserveLoopbackPort();
  const probe = await customProviderProbe();
  const checks = [];
  let child, browser, page, output;
  const stopOwnedBroker = async () => {
    const target = `gui/${process.getuid()}/com.lingxi.code.credential-broker`;
    const state = spawnSync('/bin/launchctl', ['print', target], { encoding: 'utf8', timeout: 10_000 });
    if (state.status === 113) return;
    assert.equal(state.status, 0, 'cannot inspect production Broker ownership');
    assert.ok(state.stdout.includes(join(home, 'Library', 'Application Support', 'LingXi',
      'CredentialBroker', 'production', 'LingXiCredentialBroker.app', 'Contents', 'MacOS', 'LingXiCredentialBroker')),
    'refusing to stop a Broker outside this isolated test');
    await unregisterCredentialBrokerForColdStart('production');
  };
  const close = async () => {
    if (child && child.exitCode === null && child.signalCode === null) {
      // Chromium may close the CDP socket before acknowledging a normal quit.
      try { await browser.send('Browser.close'); } catch {}
      await waitFor(() => child.exitCode !== null || child.signalCode !== null, { label: 'credential stability normal exit' });
      assert.equal(child.exitCode, 0, 'Desktop must exit normally');
    }
    await page?.close(); await browser?.close();
    child = browser = page = null;
  };
  try {
    for (let launch = 1; launch <= 3; launch++) {
      ({ child, output } = spawnPackagedApp(appPath, env, port, runtime.userDataDir));
      ({ browser, page } = await connectToDebugger(port, rendererUrl));
      await page.send('Page.enable');
      await page.send('Runtime.enable');
      await waitFor(() => evaluate(page, 'window.lingxi?.bootstrap().then(b => b.connection?.status === "connected")'), { label: 'production credential bootstrap' });
      let sessionId = await prepareCustomProvider(page, probe, launch > 1);
      await assertCustomProviderRequest(page, probe, sessionId);
      checks.push({ launch, type: 'launch', credentialConfigured: true, authenticatedRequest: true });
      for (let refresh = 1; refresh <= 5; refresh++) {
        const previousLoader = (await page.send('Page.getFrameTree')).frameTree.frame.loaderId;
        await page.send('Page.reload', { ignoreCache: true });
        await waitFor(async () => {
          const frame = (await page.send('Page.getFrameTree')).frameTree.frame;
          return frame.loaderId && frame.loaderId !== previousLoader && frame.url.startsWith('file://');
        },
          { label: 'credential refresh main-frame navigation' });
        await waitFor(() => evaluate(page, 'window.lingxi?.bootstrap().then(b => b.connection?.status === "connected")'), { label: 'production credential refresh' });
        sessionId = await prepareCustomProvider(page, probe, true);
        await assertCustomProviderRequest(page, probe, sessionId);
        checks.push({ launch, refresh, type: 'refresh', credentialConfigured: true, authenticatedRequest: true });
      }
      if (launch === 3) {
        await removeCustomProviderAccount(page, probe);
      }
      await close();
      await stopOwnedBroker();
      await assertRuntimeCleanup(tempRoot, runtime.bridgeRuntimeDir);
    }
    assert.ok(probe.requests.length >= checks.length, 'every step requires a fresh authenticated request');
    assert.ok(probe.requests.every(request => request.authenticated));
    return { result: 'PASS', channel: 'production', appPath, tempRoot, checks, launches: 3, refreshes: 15,
      requests: probe.requests.length, testCredentialDeleted: true, realProviderCredentialsCopied: false };
  } finally {
    writeFileSync(join(tempRoot, 'request-checks.json'), JSON.stringify({
      requests: probe.requests.map(request => ({ authenticated: request.authenticated })), checks,
    }, null, 2), { mode: 0o600 });
    if (page) {
      try {
        const events = await evaluate(page, `window.__lingxiSmoke?.events.filter(e =>
          ['error', 'model_changed', 'provider_accounts_updated', 'settings_snapshot'].includes(e.type)).map(e =>
          e.type === 'settings_snapshot' ? { type: e.type, testProfile: JSON.parse(e.effective_json).providers?.[${JSON.stringify(probe.providerId)}] } : e)`);
        writeFileSync(join(tempRoot, 'credential-events.json'), JSON.stringify(events, null, 2), { mode: 0o600 });
      } catch {}
    }
    if (page) {
      try { await removeCustomProviderAccount(page, probe); } catch {}
    }
    try { await close(); } catch {
      if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    }
    if (output) writeFileSync(join(tempRoot, 'last-process.log'), output.stdout + output.stderr, { mode: 0o600 });
    try { await stopOwnedBroker(); } finally { await probe.close(); }
  }
}

export async function runPackagedAppSmoke(root = packageRoot, outputRoot = process.env[PACKAGE_OUTPUT_ROOT_ENV]) {
  if (process.platform !== 'darwin') {
    throw new Error(`packaged smoke verification requires macOS; received ${process.platform}`);
  }

  const metadata = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const { appPath } = artifactPaths(root, metadata, outputRoot);
  if (!existsSync(appPath)) {
    throw new Error(`packaged app is missing: ${appPath}`);
  }
  const brokerManifest = JSON.parse(readFileSync(join(appPath, 'Contents', 'Resources',
    'credential-broker', 'broker-manifest.json'), 'utf8'));
  assert.ok(['development', 'production'].includes(brokerManifest.channel), 'packaged Broker channel is invalid');
  assertCredentialBrokerChannelUnregistered(brokerManifest.channel);
  const preexistingAppCommands = runningAppCommands();
  if (preexistingAppCommands.length > 0) {
    throw new Error(`refusing packaged smoke launch while another ${APP_NAME} instance is running: ${preexistingAppCommands.join(' | ')}`);
  }

  if (process.env.LINGXI_CREDENTIAL_BROKER_COLD_START === '1') {
    const manifest = JSON.parse(readFileSync(join(appPath, 'Contents', 'Resources',
      'credential-broker', 'broker-manifest.json'), 'utf8'));
    // The first real packaged client must register and start the service.
    // launchd removal is asynchronous even after bootout returns successfully.
    await unregisterCredentialBrokerForColdStart(manifest.channel);
    log(`cold-start credential broker unregistered channel=${manifest.channel}; no readiness probe performed`);
  }

  // Keep nested supervisor Unix sockets below macOS SUN_LEN; the default
  // per-user temp root plus this fixture hierarchy exceeds that OS limit.
  const tempRoot = mkdtempSync(join('/tmp', 'lingxi-packaged-smoke-'));
  const tempHome = join(tempRoot, 'home');
  const tempTmp = join(tempRoot, 'tmp');
  mkdirSync(tempHome, { recursive: true, mode: 0o700 });
  mkdirSync(tempTmp, { recursive: true, mode: 0o700 });
  const workspace = createAdversarialWorkspace(tempRoot);
  const runtimePaths = runtimePathsForHome(tempHome);
  writePackagedProfile(runtimePaths, workspace);
  let copiedAppPath;
  try {
    copiedAppPath = realpathSync.native(copyPackagedApp(appPath, join(tempRoot, 'app-copy')));
  } catch (error) {
    rmSync(tempRoot, { recursive: true, force: true });
    throw error;
  }
  const rendererUrl = pathToFileURL(join(copiedAppPath, 'Contents', 'Resources', 'app.asar', 'out', 'renderer', 'index.html')).href;
  const copiedAppBundleId = packagedAppBundleId(copiedAppPath);
  const cdpPort = await reserveLoopbackPort();
  const env = sanitizePackagedAppEnvironment(process.env, {
    HOME: tempHome,
    CFFIXED_USER_HOME: tempHome,
    // A disposable CFFIXED_USER_HOME must not wait on the real user's
    // cfprefsd domain. Keep native Firebase preferences in this test home.
    __CFPREFERENCES_AVOID_DAEMON: '1',
    TMPDIR: tempTmp,
    LINGXI_CONFIG_DIR: join(tempHome, ".lingxi"),
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
  let customProbe;
  let coldSession;
  let scheduledRecords;
  const protectedBrokerProcessIds = new Set();
  const consoleErrors = [];
  const runtimeExceptions = [];
  const logErrors = [];
  const monitorRenderer = async target => {
    target.on('Runtime.consoleAPICalled', params => { if (params.type === 'error') consoleErrors.push(params); });
    target.on('Runtime.exceptionThrown', params => runtimeExceptions.push(params));
    target.on('Log.entryAdded', params => { if (params.entry?.level === 'error') logErrors.push(params.entry); });
    await target.send('Runtime.enable');
    await target.send('Log.enable');
  };
  const assertNoRendererErrors = () => {
    assert.equal(consoleErrors.length, 0, `renderer console errors detected: ${JSON.stringify(consoleErrors)}`);
    assert.equal(runtimeExceptions.length, 0, `renderer exceptions detected: ${JSON.stringify(runtimeExceptions)}`);
    assert.equal(logErrors.length, 0, `renderer log errors detected: ${JSON.stringify(logErrors)}`);
  };
  try {
    const launch = spawnPackagedApp(copiedAppPath, env, cdpPort, runtimePaths.userDataDir);
    child = launch.child;
    output = launch.output;
    const debuggerConnection = await connectToDebugger(cdpPort, rendererUrl);
    browser = debuggerConnection.browser;
    page = debuggerConnection.page;

    await monitorRenderer(page);

    log('checking renderer bootstrap');
    await assertRendererContract(page, leakPatterns);
    log('checking renderer security');
    await assertSecurityBehavior(page);
    log('checking bundled sidecar');
    await assertBundledSidecar(page, copiedAppPath, tempRoot, workspace);
    log('checking terminal');
    await assertBottomTerminal(page);
    log('checking Desktop Git');
    await assertDesktopGit(page, workspace, tempRoot);

    log('checking credential and scheduled-task persistence');
    customProbe = await customProviderProbe();
    coldSession = await prepareCustomProvider(page, customProbe);
    // Provider routes are assembled when the sidecar boots. This turn is now
    // exercised before the full app restart, so first load the saved route.
    await evaluate(page, `window.lingxi.restartBridge(${JSON.stringify(coldSession)})`);
    await assertCustomProviderRequest(page, customProbe, coldSession);
    // Commit another foreground session so that only that session is restored
    // at relaunch. The earlier saved UUID must then take the true cold-open path.
    const otherSession = await prepareCustomProvider(page, customProbe, true);
    assert.notEqual(otherSession, coldSession);
    await assertCustomProviderRequest(page, customProbe, otherSession);
    scheduledRecords = await assertScheduledPersistenceBeforeRestart(page, `${customProbe.providerId}/smoke-model`);

    assertNoRendererErrors();
    await waitForTemporaryCredentialBroker(tempHome, brokerManifest.channel, protectedBrokerProcessIds);

    await closePackagedApp(browser, child, copiedAppBundleId);
    child = null;
    await browser.close();
    await page.close();
    browser = null;
    page = null;

    await bootoutTemporaryCredentialBroker(tempHome, brokerManifest.channel, protectedBrokerProcessIds);
    await assertRuntimeCleanup(tempRoot, runtimePaths.bridgeRuntimeDir);

    // Relaunch the same signed app and isolated settings home, then prove both
    // encrypted persistence and real request credential resolution end to end.
    const relaunch = spawnPackagedApp(copiedAppPath, env, cdpPort, runtimePaths.userDataDir);
    child = relaunch.child;
    output = relaunch.output;
    ({ browser, page } = await connectToDebugger(cdpPort, rendererUrl));
    await monitorRenderer(page);
    await waitFor(() => evaluate(page, 'window.lingxi?.bootstrap().then(b => b.connection?.status === "connected")'), { label: 'restarted custom provider app' });
    await assertScheduledPersistenceAfterRestart(page, scheduledRecords, join(tempHome, '.lingxi-workspaces'));
    await assertColdSessionOpen(page, coldSession, async () => {
      const activation = spawnPackagedApp(copiedAppPath, env, cdpPort, runtimePaths.userDataDir);
      try {
        await waitForPackagedAppExit(activation.child, async () => {});
        assert.equal(activation.child.exitCode, 0, 'second launch must hand off to the existing instance');
        assert.equal(child.exitCode, null, 'second launch must preserve the original packaged process');
      } finally {
        if (activation.child.exitCode === null && activation.child.signalCode === null) activation.child.kill('SIGTERM');
      }
    });
    const customSession = await prepareCustomProvider(page, customProbe, true);
    await assertCustomProviderRequest(page, customProbe, customSession);
    await assertPackagedLocalApps(page, customProbe, customSession, cdpPort, join(tempHome, '.lingxi'));
    await removeCustomProviderAccount(page, customProbe);
    assertNoRendererErrors();
    await waitForTemporaryCredentialBroker(tempHome, brokerManifest.channel, protectedBrokerProcessIds);
    await closePackagedApp(browser, child, copiedAppBundleId);
    child = null;
    await browser.close();
    await page.close();
    browser = null;
    page = null;
    await bootoutTemporaryCredentialBroker(tempHome, brokerManifest.channel, protectedBrokerProcessIds);
    await assertRuntimeCleanup(tempRoot, runtimePaths.bridgeRuntimeDir);

    return {
      appPath: copiedAppPath,
      cdpPort,
      runtimePaths,
      notes: [
        'Automated: bundled renderer and sidecar, custom Provider Keychain persistence across app restart, authenticated localhost model request, credential deletion, live permission modes, security checks, and exact temp cleanup.',
        'Manual-only: Gatekeeper transfer prompts, ad-hoc signature approval on a different Mac, full Developer ID notarization/stapling, and native workspace-trust dialog text on physical user interaction.',
      ],
    };
  } finally {
    if (consoleErrors.length) log(`renderer console errors: ${JSON.stringify(consoleErrors.slice(-3))}`);
    if (runtimeExceptions.length) log(`renderer exceptions: ${JSON.stringify(runtimeExceptions.slice(-3))}`);
    if (logErrors.length) log(`renderer log errors: ${JSON.stringify(logErrors.slice(-3))}`);
    if (existsSync(runtimePaths.diagnosticsPath)) {
      const diagnosticLines = readFileSync(runtimePaths.diagnosticsPath, 'utf8').split('\n');
      if (process.env.LINGXI_LOCAL_APP_EVIDENCE) {
        mkdirSync(process.env.LINGXI_LOCAL_APP_EVIDENCE, { recursive: true });
        cpSync(runtimePaths.diagnosticsPath, join(process.env.LINGXI_LOCAL_APP_EVIDENCE, 'packaged-runtime-diagnostics.jsonl'));
      }
      const shutdownPhases = diagnosticLines.flatMap(line => {
        try {
          const event = JSON.parse(JSON.parse(line).message);
          return event.event === 'shutdown_phase' ? [{ phase: event.phase, status: event.status, durationMs: event.durationMs }] : [];
        } catch { return []; }
      });
      if (shutdownPhases.length) log(`shutdown phases: ${JSON.stringify(shutdownPhases)}`);
      log(`app diagnostics: ${diagnosticLines.slice(-8).join('\n')}`);
    }
    if (page && customProbe) {
      try { await removeCustomProviderAccount(page, customProbe); } catch {}
    }
    if (customProbe) await customProbe.close();
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
    let survivors = [];
    let profileCleanupFailed = false;
    try {
      await bootoutTemporaryCredentialBroker(tempHome, brokerManifest.channel, protectedBrokerProcessIds);
    } catch (error) {
      profileCleanupFailed = true;
      log(`refusing to remove isolated smoke home after Broker ownership/bootout verification failed: ${formatError(error)}`);
    }
    if (!profileCleanupFailed) {
      survivors = await reapTrackedCommands(tempRoot, protectedBrokerProcessIds);
      if (survivors.length === 0) rmSync(tempRoot, { recursive: true, force: true });
      else log(`retaining isolated smoke home because processes survived normal cleanup: ${survivors.join(' | ')}`);
    } else {
      survivors = trackedCommands(tempRoot);
    }
    if (output.stdout.trim()) log(`app stdout: ${output.stdout.trim()}`);
    if (output.stderr.trim()) log(`app stderr: ${output.stderr.trim()}`);
    if (profileCleanupFailed) {
      throw new Error(`isolated smoke home retained at ${tempRoot} because Broker cleanup could not be verified`);
    }
    if (survivors.length > 0) {
      throw new Error(`tracked packaged processes survived cleanup: ${survivors.join(' | ')}`);
    }
    assert.doesNotMatch(output.stderr, /No handler registered for ['"]lingxi:sessionRecovery['"]/,
      'late recovery diagnostics must be harmless after IPC teardown');
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
