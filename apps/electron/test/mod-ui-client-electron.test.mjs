import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import { build } from 'vite';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'mod-ui-client-electron.mjs');

test('Electron renders Client VM frames after draw commit and routes reached controls to current held handles', { timeout: 45_000 }, async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-mod-ui-client-vite-'));
  const fixtureBuildDir = mkdtempSync(join(tmpdir(), 'lingxi-mod-ui-client-build-'));
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-mod-ui-client-electron-'));
  const fixtureHtml = join(fixtureRoot, 'mod-ui-client-fixture.html');
  let child;
  try {
    await build({
      root: fixtureRoot,
      base: './',
      cacheDir: viteCacheDir,
      configFile: false,
      logLevel: 'error',
      plugins: [react()],
      resolve: {
        alias: { '@renderer': resolve(electronRoot, 'src/renderer') },
        dedupe: ['react', 'react-dom'],
      },
      build: { outDir: fixtureBuildDir, emptyOutDir: true, rollupOptions: { input: fixtureHtml } },
    });
    const fixtureUrl = pathToFileURL(join(fixtureBuildDir, 'mod-ui-client-fixture.html')).href;
    const output = [];
    const errors = [];
    child = spawn(process.execPath, [electronBinary, electronDriver, fixtureUrl], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => output.push(String(chunk)));
    child.stderr.on('data', (chunk) => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron Mod UI Client fixture timed out\n${errors.join('')}`));
      }, 35_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron Mod UI Client fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron Mod UI Client fixture output: ${line}`, { cause: error })); }
      });
    });

    assert.equal(result.parentElementsRendered, true);
    assert.equal(result.parentControlRequestsMatchNativeBodies, true, JSON.stringify({
      press: result.parentButtonRequest,
      inputChange: result.parentInputChangeRequest,
      inputSubmit: result.parentInputSubmitRequest,
      select: result.parentSelectRequest,
      markdown: result.parentMarkdownRequest,
    }));
    assert.equal(result.clientIdentityPropagationMatchesAttach, true, JSON.stringify(result.clientIdentitySnapshot));
    assert.equal(result.buttonPressHandled, true);
    assert.equal(result.buttonUsesCurrentHandle, true);
    assert.equal(result.inputUsesHookReachedValue, true);
    assert.equal(result.selectUsesHookReachedValue, true);
    assert.equal(result.preRegistrationFrameReplayed, true);
    assert.equal(result.staleOperationIgnoredWithoutFault, true);
    assert.equal(result.parentPropsUpdateRendered, true);
    assert.equal(result.staleRpcFrameIgnored, true);
    assert.equal(result.staleFrameIgnored, true);
    assert.equal(result.workerFaultRenderedWithoutDuplicateReport, true, JSON.stringify(result.workerFaultDiagnostics));
    assert.equal(result.workerFaultLeavesSiblingVisible, true);
    assert.equal(result.workerFaultNotClearedByParentRevision, true);
    assert.equal(result.workerFaultRecoveredFromEnvironmentRemount, true);
    assert.equal(result.asyncWorkerFaultRenderedWithoutDuplicateReport, true);
    assert.equal(result.bufferedAsyncWorkerFaultIsLeafLocal, true);
    assert.equal(result.statusReadyAfterAsyncClientRemoval, true, JSON.stringify(result.statusReadinessTimeline));
    assert.equal(result.adapterFaultReportedAsRun, true);
    assert.equal(result.faultLeavesSiblingVisible, true);
    assert.equal(result.adapterRunFaultSurvivedPassiveFrames, true);
    assert.equal(result.adapterRunFaultSurvivedNewRevisionSameIdentity, true);
    assert.equal(result.adapterRunFaultSurvivedSameIdentityPropsUpdate, true);
    assert.equal(result.adapterRunFaultRecoveredAfterStateTokenChange, true);
    assert.equal(result.adapterRunFaultRecoveredAfterClientRemount, true);
    assert.equal(result.pluginUnloadInvalidationRendered, true);
    assert.equal(result.emptyNoHostIsQuietAndCleansUp, true);
    assert.equal(result.runtimeLifecycleSnapshot.pluginEnvironmentRemountsAllClients, true, JSON.stringify(result.runtimeLifecycleSnapshot));
    assert.equal(result.unmountReleasedRuntimeAndDrawIdentity, true, JSON.stringify(result.runtimeLifecycleSnapshot));
    assert.equal(result.staleRenderCleanupPreservedNewerFrame, true);
    assert.equal(result.lateInitialRenderCleanedUp, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(fixtureBuildDir, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
