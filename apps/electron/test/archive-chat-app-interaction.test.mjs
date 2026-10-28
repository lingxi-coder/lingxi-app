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
const fixtureMain = join(fixtureRoot, 'archive-chat-app-main.tsx');
const betaDesktop = join(electronRoot, 'src', 'renderer', 'components', 'BetaDesktop.tsx');
const stubImports = new Set([
  './components/LocalApps', './bridge/useSideChats', './components/SideChatPanel',
  './components/ComputerAccessPrompt', './components/AskUserQuestionPrompt', './components/PermissionPrompt',
  './components/settings/SettingsScreen', './components/ScheduledTasks', './components/Stage',
  './components/modUiAbovePrompt', './components/CommandResultPanel', './components/RuntimeCenter',
  './components/GitReview', './components/TerminalPanel', './components/SessionPicker', './components/CreateProjectDialog',
]);

test('App deep links invalidate old archive ownership without stealing new session focus', { timeout: 45_000 }, async () => {
  const viteCacheDir = mkdtempSync(join(tmpdir(), 'lingxi-archive-app-vite-'));
  const buildDir = mkdtempSync(join(tmpdir(), 'lingxi-archive-app-build-'));
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-archive-app-user-'));
  let child;
  try {
    await build({
      root: fixtureRoot, base: './', cacheDir: viteCacheDir, configFile: false, logLevel: 'error',
      plugins: [{
        name: 'archive-app-fixture', enforce: 'pre',
        resolveId(id, importer) {
          if (!importer?.endsWith('/src/renderer/App.tsx')) return null;
          if (id === './bridge/useBridge.js' || stubImports.has(id)) return fixtureMain;
          if (id === './components/BetaDesktop') return '\0archive-app-beta-wrapper';
          return null;
        },
        load(id) {
          if (id !== '\0archive-app-beta-wrapper') return null;
          return `export { BetaSidebar } from ${JSON.stringify(betaDesktop)};\nexport { BetaComposer, BetaTopBar, ErrorBanner } from ${JSON.stringify(fixtureMain)};`;
        },
        transform(source, id) {
          if (process.env.LINGXI_TEST_ARCHIVE_DISABLE_EXTERNAL_FENCE !== '1') return null;
          if (id.endsWith('/src/renderer/App.tsx')) {
            const notify = '    externalSessionNavigationRef.current?.(projectPath, sessionId);';
            assert.ok(source.includes(notify), 'negative control must disable synchronous session invalidation');
            return source.replace(notify, '');
          }
          if (id.endsWith('/src/renderer/components/BetaDesktop.tsx')) {
            const guard = '    if (!owner || !archiveTarget || (archiving && !bridge.sessionLoading)) return;';
            assert.ok(source.includes(guard), 'negative control must disable session-change fallback');
            return source.replace(guard, '    if (true) return;');
          }
          return null;
        },
      }, react()],
      resolve: { alias: { '@renderer': join(electronRoot, 'src', 'renderer') }, dedupe: ['react', 'react-dom'] },
      build: { outDir: buildDir, emptyOutDir: true, rollupOptions: { input: join(fixtureRoot, 'archive-chat-app-fixture.html') } },
    });
    child = spawn(process.execPath, [join(electronRoot, 'node_modules', 'electron', 'cli.js'),
      join(fixtureRoot, 'archive-chat-app-electron.mjs'), pathToFileURL(join(buildDir, 'archive-chat-app-fixture.html')).href], {
      cwd: electronRoot, env: { ...process.env, ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: userData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const output = [], errors = [];
    child.stdout.on('data', chunk => output.push(String(chunk)));
    child.stderr.on('data', chunk => errors.push(String(chunk)));
    const result = await new Promise((resolveResult, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Electron archive App fixture timed out\n${errors.join('')}`)); }, 35_000);
      child.once('error', error => { clearTimeout(timeout); reject(error); });
      child.once('exit', (code, signal) => {
        clearTimeout(timeout);
        if (code !== 0) { reject(new Error(`Electron archive App fixture exited ${code ?? signal}\n${errors.join('')}`)); return; }
        try { resolveResult(JSON.parse(output.join('').trim().split('\n').at(-1))); }
        catch (error) { reject(new Error('Invalid archive App fixture output', { cause: error })); }
      });
    });
    assert.equal(result.ok, true);
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    rmSync(viteCacheDir, { recursive: true, force: true });
    rmSync(buildDir, { recursive: true, force: true });
    rmSync(userData, { recursive: true, force: true });
  }
});
