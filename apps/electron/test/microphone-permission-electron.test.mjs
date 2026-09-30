import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';

import react from '@vitejs/plugin-react';
import ts from 'typescript';
import { createServer } from 'vite';

import { microphonePermissionFromMediaAccessStatus } from '../src/shared/microphoneAccess.ts';

const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(electronRoot, 'test', 'fixtures');
const electronBinary = resolve(electronRoot, 'node_modules/electron/cli.js');
const electronDriver = join(fixtureRoot, 'microphone-permission-electron.mjs');

/**
 * Transpiles production TypeScript into a temp ESM tree the Electron MAIN
 * process can import. Electron's main process cannot load `.ts`, and this
 * test's whole value is that the module under test is the real one rather
 * than a copy — so it is compiled, not reimplemented. The relative import
 * `../shared/microphoneAccess.js` resolves inside the temp tree because the
 * layout is preserved.
 */
function transpileForElectron(outputRoot, relativePaths) {
  writeFileSync(join(outputRoot, 'package.json'), JSON.stringify({ type: 'module' }), 'utf8');
  for (const relativePath of relativePaths) {
    const source = readFileSync(join(electronRoot, relativePath), 'utf8');
    const { outputText } = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
      fileName: relativePath,
    });
    const outputPath = join(outputRoot, relativePath.replace(/^src\//, '').replace(/\.ts$/, '.js'));
    mkdirSync(dirname(outputPath), { recursive: true });
    writeFileSync(outputPath, outputText, 'utf8');
  }
}

/**
 * Proves BY EXECUTION, on the real machine's real microphone grant, that the
 * value the voice settings page shows in its 麦克风权限 row comes from the OS.
 *
 * Final review, Defects 7 and 9: `probePlatform`'s only source of
 * `microphonePermission` was `navigator.permissions.query({name:'microphone'})`,
 * which in this app is answered by `main/index.ts`'s
 * `session.setPermissionCheckHandler` — `permission === 'media' &&
 * isLocalRendererUrl(origin)` — a value computed with no reference to macOS
 * TCC. The reviewer measured `{"permissionsApiState":"granted",
 * "macOsTccStatus":"not-determined"}` on real Electron 43 with this app's
 * exact handlers. No unit test can tell the two sources apart, because both
 * are only distinguishable against a real OS grant; this one runs them side
 * by side in one real Electron process.
 *
 * The `rendererProbeWhenOsDenies` leg keeps the test honest on a machine
 * whose TCC grant happens to match the page permission: it swaps the OS
 * answer for a denial while the page permission stays granted, so a probe
 * that reads the page permission is caught on ANY machine.
 */
test('the voice page reads the real OS microphone grant, not the page permission this app grants itself', { skip: process.platform === 'linux' && 'Electron exposes native media access status on macOS and Windows' }, async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-microphone-vite-'));
  const transpiledRoot = mkdtempSync(join(tmpdir(), 'lingxi-microphone-main-'));
  transpileForElectron(transpiledRoot, ['src/shared/microphoneAccess.ts', 'src/main/microphoneAccess.ts']);

  const vite = await createServer({
    root: fixtureRoot,
    cacheDir: viteCacheDir,
    configFile: false,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false },
    plugins: [react()],
    resolve: { alias: { '@renderer': resolve(electronRoot, 'src/renderer') } },
  });
  const temporaryUserData = mkdtempSync(join(tmpdir(), 'lingxi-microphone-electron-'));
  const childOutput = [];
  const childError = [];
  let child;

  try {
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port, 'Vite fixture server did not bind a port');
    const fixtureUrl = `http://127.0.0.1:${address.port}/microphone-permission-fixture.html`;
    child = spawn(process.execPath, [
      electronBinary,
      electronDriver,
      fixtureUrl,
      join(transpiledRoot, 'main/microphoneAccess.js'),
    ], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: temporaryUserData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk) => childOutput.push(String(chunk)));
    child.stderr.on('data', (chunk) => childError.push(String(chunk)));

    const result = await new Promise((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child.kill('SIGTERM');
        rejectResult(new Error(`Electron microphone fixture timed out\n${childError.join('')}`));
      }, 60_000);
      child.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const output = childOutput.join('').trim().split('\n').at(-1);
        if (code !== 0 || !output) {
          rejectResult(new Error(`Electron microphone fixture exited ${code ?? signal}\n${childError.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(output)); } catch (error) { rejectResult(error); }
      });
    });

    // The measurement that makes this test necessary: this app's own session
    // handler answers the renderer's Permissions API `granted`, whatever the
    // OS grant is.
    assert.equal(
      result.permissionsApi,
      'granted',
      'setPermissionCheckHandler grants the app renderer `media` unconditionally — if this ever stops being true, '
      + 'the reasoning behind reading the grant in the main process needs revisiting',
    );

    // The production main-process reader agrees with the OS, on this machine,
    // right now — whatever this machine's grant happens to be.
    assert.equal(
      result.mainProcessRead,
      microphonePermissionFromMediaAccessStatus(result.mediaAccessStatus),
      `readMicrophoneAccess() disagreed with systemPreferences.getMediaAccessStatus (OS said `
      + `"${result.mediaAccessStatus}")`,
    );

    // And the page's own probe reports that OS value, over real IPC.
    assert.equal(
      result.rendererProbe,
      result.mainProcessRead,
      `the 麦克风权限 row would show "${result.rendererProbe}" while the OS grant is `
      + `"${result.mediaAccessStatus}" (${result.mainProcessRead}); the Permissions API said `
      + `"${result.permissionsApi}"`,
    );

    // The mounted page renders the OS answer, not the page permission.
    const labels = { granted: '已授权', denied: '未授权', prompt: '尚未询问', unavailable: '无法确定' };
    assert.ok(
      result.rowTextFromOs.includes(`当前状态：${labels[result.mainProcessRead]}`),
      `the 麦克风权限 row does not show the OS grant (${result.mainProcessRead}); it rendered:\n${result.rowTextFromOs}`,
    );

    // Machine-independent: with the OS denying and the page permission still
    // granted, a probe reading the page permission answers `granted`.
    assert.equal(
      result.rendererProbeWhenOsDenies,
      'denied',
      'the page probe ignored a denied OS grant — it is reading the page permission this app grants itself, '
      + `not the microphone grant (Permissions API said "${result.permissionsApi}")`,
    );

    // The grant changed in System Settings while this page sat open. Coming
    // back to the window is the only signal macOS gives, and the row has to
    // act on it — a row read once at mount is wrong from that moment on.
    assert.ok(
      result.rowTextAfterRefocus.includes('当前状态：未授权'),
      'the 麦克风权限 row did not follow the OS grant when the user came back to the window; it rendered:\n'
      + result.rowTextAfterRefocus,
    );
    // The v3 permissions card keeps the denied state and remedy inline with
    // the OS-backed row instead of rendering a separate permission banner.
    assert.ok(
      result.rowTextAfterRefocus.includes('当前状态：未授权'),
      `the denied OS state was not visible in the microphone row; the page said:\n${result.rowTextAfterRefocus}`,
    );
    assert.ok(
      !result.rowTextAfterRefocus.includes('录音与语音朗读功能不受影响'),
      'the banner still claimed recording is unaffected while the OS denies the microphone',
    );
    // The previously unreachable remedy, now reachable and wired to the right pane.
    assert.equal(result.clickedOpenSystemSettings, true, 'the 「打开系统设置」 button did not render in the denied state');
    assert.deepEqual(
      result.openedSystemSettingsPanes,
      ['microphone'],
      'the denied row\'s button did not open the microphone System Settings pane',
    );

    // And back: the row follows the OS in both directions, so this holds on
    // any machine whatever its real grant is.
    assert.ok(
      result.rowTextAfterGrant.includes('当前状态：已授权'),
      `the row did not follow the grant back; it rendered:\n${result.rowTextAfterGrant}`,
    );
    assert.equal(
      result.openSystemSettingsAfterGrant,
      false,
      'the 「打开系统设置」 button still rendered after the grant was given — it exists only for the denied state',
    );
  } finally {
    if (child && child.exitCode === null) child.kill('SIGKILL');
    await vite.close();
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(transpiledRoot, { recursive: true, force: true });
    rmSync(temporaryUserData, { recursive: true, force: true });
  }
});
