import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'prompt-focus-electron.mjs');
const composerDraftDriver = join(fixtureRoot, 'composer-draft-electron.mjs');

test('real Electron preserves Sidebar focus and supports native resize dragging', async () => {
  const vite = await createServer({
    root: fixtureRoot,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: {
      alias: {
        '@renderer': resolve(electronRoot, 'src/renderer'),
      },
    },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-prompt-focus-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/prompt-focus-fixture.html`;

    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron focus fixture timed out\n${childError.join('')}`));
      }, 20_000);
      child.once('error', (error) => {
        clearTimeout(timeout);
        rejectResult(error);
      });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron focus fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try {
          resolveResult(JSON.parse(output));
        } catch (error) {
          rejectResult(new Error(`Invalid Electron focus fixture output: ${output}`, { cause: error }));
        }
      });
    });

    assert.deepEqual(result.beforeTab, {
      promptOpen: true,
      activeLabel: 'Allow once',
      sidebarExists: true,
    });
    assert.deepEqual(result.afterTab, {
      activeText: 'New chat',
      activeIsSidebar: true,
    });
    assert.deepEqual(result.afterDismiss, {
      promptOpen: false,
      activeText: 'New chat',
      activeIsSidebar: true,
    });
    assert.deepEqual(result.resize, { before: 260, after: 360 });
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise((resolveExit) => {
        const timeout = setTimeout(resolveExit, 2000);
        child.once('exit', () => {
          clearTimeout(timeout);
          resolveExit();
        });
      });
    }
    await vite.close();
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});

test('real Electron restores an independent unsent composer draft for each session', async () => {
  const vite = await createServer({
    root: fixtureRoot,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-composer-draft-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/composer-draft-fixture.html`;
    child = spawn(process.execPath, [electronBinary, composerDraftDriver, fixtureUrl], {
      cwd: electronRoot,
      env: {
        ...process.env,
        ELECTRON_ENABLE_LOGGING: '0',
        ELECTRON_IS_DEV: '0',
        LINGXI_TEST_USER_DATA: temporaryUserData,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron composer draft fixture timed out\n${childError.join('')}`));
      }, 20_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron composer draft fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); }
        catch (error) { rejectResult(new Error(`Invalid Electron composer draft output: ${output}`, { cause: error })); }
      });
    });

    if (process.env.LINGXI_TEST_PRELOAD) {
      assert.deepEqual(result, { nativeFileAttachments: true });
      return;
    }

    assert.deepEqual(result, {
      audioInteraction: {
        dictationStartRequests: ['request_authorization', 'start_listening'],
        dictatedText: 'dictated text',
        flowStartRequests: ['request_authorization', 'start_listening'],
      },
      modelSettings: {
        resetToDefaultVisible: false,
      },
      initialPicker: {
        sections: ['Paid', 'Free'],
        models: ['OpenRouter Auto', 'Anthropic: Claude Opus Latest', 'OpenRouter Free', 'InclusionAI: Ling 3.0 Flash Fin (free)'],
      },
      filteredPicker: {
        focused: 'Search models',
        models: ['InclusionAI: Ling 3.0 Flash Fin (free)'],
        clearVisible: true,
      },
      restoredA: 'draft for A',
      restoredB: 'draft for B',
      survivingDraft: 'draft that must survive',
      richDraft: {
        text: 'app.ts inspect this',
        mention: 'src/app.ts',
        image: 'tiny.png',
      },
      runningInteraction: {
        editable: true,
        attachEnabled: true,
        goalEnabled: false,
        stopEnabled: false,
      },
      sentPending: 'pending follow-up',
    });
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise((resolveExit) => {
        const timeout = setTimeout(resolveExit, 2000);
        child.once('exit', () => { clearTimeout(timeout); resolveExit(); });
      });
    }
    await vite.close();
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
