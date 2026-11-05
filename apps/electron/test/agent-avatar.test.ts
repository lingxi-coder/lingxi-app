import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import reactPlugin from '@vitejs/plugin-react';
import { build } from 'vite';
import { AgentAvatar, agentAvatarIndex } from '../src/renderer/components/AgentAvatar';
import { TranscriptAgents } from '../src/renderer/components/TranscriptAgents';
import { RuntimeCenterInspector } from '../src/renderer/components/RuntimeCenter';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
const render = (node: React.ReactElement, dark = false) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, node));
const identities = (html: string) => [...html.matchAll(/data-agent-avatar="(\d+)"/g)].map((match) => Number(match[1]));

test('all 28 stable identities have self-contained light and dark SVG assets', () => {
  const seen = new Set<number>();
  for (let i = 0; i < 300; i++) seen.add(agentAvatarIndex(`agent-${i}`));
  assert.equal(seen.size, 28);
  for (const index of seen) {
    const id = Array.from({ length: 300 }, (_, i) => `agent-${i}`).find((id) => agentAvatarIndex(id) === index)!;
    for (const dark of [false, true]) {
      const html = render(React.createElement(AgentAvatar, { agentId: id }), dark);
      const src = html.match(/src="([^"]+)"/)![1];
      assert.ok(src.endsWith(`-${dark ? 'dark' : 'light'}.svg`));
      const svg = readFileSync(new URL(src), 'utf8');
      assert.match(svg, /<svg\b/);
      assert.doesNotMatch(svg, /<script|<foreignObject|(?:href|src)=["']https?:/i);
      assert.match(html, /alt="" aria-hidden="true"/);
    }
  }
});

test('agent identity survives renames and terminal status and matches inspector rows', () => {
  const agent = { agent_id: 'agent-review-42', name: 'Review', agent_type: 'reviewer', status: 'running' };
  const expected = [agentAvatarIndex(agent.agent_id)];
  for (const status of ['running', 'completed', 'failed', 'cancelled']) {
    const row = { ...agent, name: `Renamed ${status}`, status };
    assert.deepEqual(identities(render(React.createElement(TranscriptAgents, { agents: [row] }))), expected);
  }
  const active = { kind: 'section', id: 'agents' } as const;
  const bridge = {
    runtimeCenter: { ...emptyRuntimeCenterState(), inspectorOpen: true, activeItem: active, tabs: [active], agents: { [agent.agent_id]: agent } },
    desktop: emptyDesktopState(), conversation: { plan: [] },
  };
  assert.deepEqual(identities(render(React.createElement(RuntimeCenterInspector, { bridge: bridge as never }))), expected);
});

test('inspector toolbar renders matching agent avatars and keeps side-question tabs selectable', { timeout: 45_000 }, async () => {
  // Tabs mount into the window toolbar in an effect, so server rendering cannot
  // exercise them. Mount the real component and its portal in Electron instead.
  const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const tempRoot = mkdtempSync(join(realpathSync(tmpdir()), 'lingxi-agent-tabs-'));
  const fixtureRoot = join(tempRoot, 'fixture');
  const buildDir = join(tempRoot, 'build');
  const userData = join(tempRoot, 'user-data');
  const fixtureHtml = join(fixtureRoot, 'index.html');
  const fixtureMain = join(fixtureRoot, 'main.tsx');
  const driver = join(tempRoot, 'electron-driver.mjs');
  let child: ReturnType<typeof spawn> | undefined;
  let childClosed: Promise<void> | undefined;
  let testError: unknown;
  try {
    mkdirSync(fixtureRoot, { recursive: true });
    mkdirSync(userData, { recursive: true });
    writeFileSync(fixtureHtml, '<!doctype html><html><body><div id="runtime-inspector-topbar-controls"></div><div id="root"></div><script type="module" src="/main.tsx"></script></body></html>');
    writeFileSync(fixtureMain, `
      import * as React from 'react';
      import { createRoot } from 'react-dom/client';
      import { RuntimeCenterInspector, RuntimeCenterOverview } from '@fixture/RuntimeCenter';
      import { emptyRuntimeCenterState, openRuntimeCenterItem } from '@fixture/runtimeCenterState';
      import { emptyDesktopState } from '@fixture/desktopState';
      import { beginSideQuestion, finishSideQuestion } from '@fixture/sideQuestion';
      import { Theme } from '@fixture/ThemeContext';
      import { tokens } from '@fixture/tokens';
      localStorage.setItem('lingxi.desktop.language', 'en');
      const agent = { agent_id: 'agent-review-42', name: 'Review', agent_type: 'reviewer', status: 'running' };
      const section = { kind: 'section', id: 'agents' };
      const initial = () => openRuntimeCenterItem({ ...emptyRuntimeCenterState(), agents: { [agent.agent_id]: agent } }, section);
      const noop = async () => {};
      function Fixture() {
        const [center, setCenter] = React.useState(initial);
        const bridge = {
          runtimeCenter: center, desktop: emptyDesktopState(), conversation: { sessionKey: 'fixture', plan: [] },
          refreshTasks: noop, loadSessionAgentTranscript: noop,
          openRuntimeItem: (item) => setCenter(current => openRuntimeCenterItem(current, item)),
          setRuntimeInspectorOpen: (open) => setCenter(current => ({ ...current, inspectorOpen: open })),
          setRuntimeCenterOverviewOpen: (open) => setCenter(current => ({ ...current, overviewOpen: open })),
        };
        window.fixture = {
          showSideQuestion() {
            let next = beginSideQuestion({ ...emptyRuntimeCenterState(), agents: { [agent.agent_id]: agent }, overviewOpen: true }, 'fixture', 1, '/btw What remains?');
            next = finishSideQuestion(next, 'fixture', 1, 'Finish verification.');
            setCenter(openRuntimeCenterItem(next, section));
          },
        };
        return <Theme.Provider value={tokens(false)}><RuntimeCenterOverview bridge={bridge} /><RuntimeCenterInspector bridge={bridge} /></Theme.Provider>;
      }
      createRoot(document.getElementById('root')).render(<Fixture />);
    `);
    writeFileSync(driver, `
      import { app, BrowserWindow } from 'electron';
      app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
      const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
      async function main() {
        await app.whenReady();
        const window = new BrowserWindow({ show: false, webPreferences: { sandbox: true, backgroundThrottling: false } });
        try {
          await window.loadURL(process.argv.at(-1));
          const run = source => window.webContents.executeJavaScript(source);
          const wait = async expression => {
            const deadline = Date.now() + 8_000;
            while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
            throw new Error('Timed out waiting for ' + expression);
          };
          await wait("Boolean(document.querySelector('.runtime-section-detail [data-agent-avatar]'))");
          const rowAvatar = await run("document.querySelector('.runtime-section-detail [data-agent-avatar]').dataset.agentAvatar");
          await run("[...document.querySelectorAll('.runtime-section-detail button')].find(button => button.textContent.includes('Review')).click()");
          await wait("Boolean(document.querySelector('[data-runtime-inspector-tab=\\\"agent:agent-review-42\\\"][aria-selected=\\\"true\\\"]'))");
          const agentTab = await run("(() => { const tab = document.querySelector('[data-runtime-inspector-tab=\\\"agent:agent-review-42\\\"]'); return { avatar: tab.querySelector('[data-agent-avatar]')?.dataset.agentAvatar, selected: tab.getAttribute('aria-selected'), inToolbar: Boolean(tab.closest('#runtime-inspector-topbar-controls')) }; })()");
          await run('window.fixture.showSideQuestion()');
          await wait("Boolean(document.querySelector('#runtime-center-overview [aria-label=\\\"Side chats\\\"]'))");
          const grouped = await run("({ ordinary: document.querySelector('.runtime-section-detail').textContent, side: document.querySelector('#runtime-center-overview [aria-label=\\\"Side chats\\\"]').textContent })");
          await run("document.querySelector('[data-runtime-inspector-tab=\\\"agent:btw:1\\\"]').click()");
          await wait("document.querySelector('[data-runtime-inspector-tab=\\\"agent:btw:1\\\"]')?.getAttribute('aria-selected') === 'true'");
          const sideTab = await run("(() => { const tab = document.querySelector('[data-runtime-inspector-tab=\\\"agent:btw:1\\\"]'); const panel = document.querySelector('#runtime-inspector-panel'); return { label: tab.textContent, selected: tab.getAttribute('aria-selected'), inToolbar: Boolean(tab.closest('#runtime-inspector-topbar-controls')), labelledBy: panel.getAttribute('aria-labelledby'), text: panel.textContent }; })()");
          console.log(JSON.stringify({ rowAvatar, agentTab, grouped, sideTab }));
        } finally { window.destroy(); await app.quit(); }
      }
      main().catch(error => { console.error(error); process.exitCode = 1; app.quit(); });
    `);
    await build({
      root: fixtureRoot, base: './', cacheDir: join(tempRoot, 'vite-cache'), configFile: false, logLevel: 'error',
      plugins: [reactPlugin()],
      resolve: { alias: [
        { find: 'react-dom/client', replacement: resolve(electronRoot, 'node_modules/react-dom/client.js') },
        { find: 'react-dom', replacement: resolve(electronRoot, 'node_modules/react-dom/index.js') },
        { find: 'react/jsx-runtime', replacement: resolve(electronRoot, 'node_modules/react/jsx-runtime.js') },
        { find: 'react', replacement: resolve(electronRoot, 'node_modules/react/index.js') },
        { find: '@fixture/RuntimeCenter', replacement: resolve(electronRoot, 'src/renderer/components/RuntimeCenter.tsx') },
        { find: '@fixture/runtimeCenterState', replacement: resolve(electronRoot, 'src/renderer/bridge/runtimeCenterState.ts') },
        { find: '@fixture/desktopState', replacement: resolve(electronRoot, 'src/renderer/bridge/desktopState.ts') },
        { find: '@fixture/sideQuestion', replacement: resolve(electronRoot, 'src/renderer/bridge/sideQuestion.ts') },
        { find: '@fixture/ThemeContext', replacement: resolve(electronRoot, 'src/renderer/theme/ThemeContext.tsx') },
        { find: '@fixture/tokens', replacement: resolve(electronRoot, 'src/renderer/theme/tokens.ts') },
      ], dedupe: ['react', 'react-dom'] },
      build: { outDir: buildDir, emptyOutDir: true, rollupOptions: { input: fixtureHtml } },
    });
    const output: string[] = [];
    const errors: string[] = [];
    const electronBinary = createRequire(import.meta.url)('electron') as string;
    child = spawn(electronBinary, [driver, pathToFileURL(join(buildDir, 'index.html')).href], {
      cwd: electronRoot, env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', LINGXI_TEST_USER_DATA: userData }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    childClosed = new Promise((resolveClosed) => child!.once('close', () => resolveClosed()));
    child.stdout.on('data', (chunk: Buffer | string) => output.push(String(chunk)));
    child.stderr.on('data', (chunk: Buffer | string) => errors.push(String(chunk)));
    const result = await new Promise<{ rowAvatar: string; agentTab: { avatar: string; selected: string; inToolbar: boolean }; grouped: { ordinary: string; side: string }; sideTab: { label: string; selected: string; inToolbar: boolean; labelledBy: string; text: string } }>((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => { child?.kill('SIGTERM'); rejectResult(new Error(`Electron agent tab fixture timed out\n${errors.join('')}`)); }, 35_000);
      child?.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child?.once('close', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) return rejectResult(new Error(`Electron agent tab fixture exited ${code ?? signal}\n${errors.join('')}`));
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron agent tab fixture result: ${line}`, { cause: error })); }
      });
    });
    assert.equal(Number(result.rowAvatar), agentAvatarIndex('agent-review-42'));
    assert.deepEqual(result.agentTab, { avatar: result.rowAvatar, selected: 'true', inToolbar: true });
    assert.match(result.grouped.ordinary, /Review/);
    assert.doesNotMatch(result.grouped.ordinary, /\/btw/);
    assert.match(result.grouped.side, /\/btw/);
    assert.equal(result.sideTab.label, '/btw');
    assert.equal(result.sideTab.selected, 'true');
    assert.equal(result.sideTab.inToolbar, true);
    assert.equal(result.sideTab.labelledBy, 'runtime-inspector-tab-agent%3Abtw%3A1');
    assert.match(result.sideTab.text, /What remains\?/);
    assert.match(result.sideTab.text, /Finish verification\./);
  } catch (error) {
    testError = error;
    throw error;
  } finally {
    try {
      if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
      if (childClosed) {
        const waitForClose = (milliseconds: number) => new Promise<boolean>((resolveClosed) => {
          const timer = setTimeout(() => resolveClosed(false), milliseconds);
          childClosed!.then(() => { clearTimeout(timer); resolveClosed(true); });
        });
        if (!await waitForClose(3_000)) {
          child?.kill('SIGKILL');
          if (!await waitForClose(3_000)) throw new Error('Electron agent tab fixture did not terminate');
        }
      }
      rmSync(tempRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    } catch (cleanupError) {
      if (!testError) throw cleanupError;
      console.error('Electron agent tab fixture cleanup failed:', cleanupError);
    }
  }
});
