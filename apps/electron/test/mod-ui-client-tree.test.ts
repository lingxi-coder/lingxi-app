import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import reactPlugin from '@vitejs/plugin-react';
import { build } from 'vite';
import {
  buildNativeUiClientPressRequest,
  ModUiClientTree,
  parseModUiClientTree,
  type ModUiClientSurfaceEvent,
  type ModUiSurfaceNode,
} from '../src/renderer/components/modUiClientTree';
import {
  collectModUiClientElements,
  gridSizeFromPixels,
  heldHandleForElement,
  invalidationTargetsRenderSite,
  isClientControlAddressable,
} from '../src/renderer/components/modUiAbovePrompt';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

const site = { component: 'AbovePrompt', instanceId: 'parent-render-1' };
const client = { plugin: 'status-plugin', key: 'status', module: 'surface/status.tsx' };
const noOp = () => undefined;

function render(tree: unknown, interactionsEnabled = true): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(ModUiClientTree, {
    tree,
    site,
    client,
    onPress: noOp,
    interactionsEnabled,
  })));
}

test('renders the eight Native Client surface node kinds as inert JSON-backed React nodes', () => {
  const tree: ModUiSurfaceNode = {
    type: 'Box',
    props: { flexDirection: 'column', columnGap: 8 },
    children: [
      { type: 'Text', props: { bold: true }, children: ['Status'] },
      { type: 'Button', props: { key: 'refresh', label: 'Refresh', variant: 'primary', hotkey: 'r' }, press: { plugin: 'status-plugin', handle: 1 },
        children: [{ type: 'Text', children: ['Refresh child'] }] },
      { type: 'Input', props: { key: 'filter', label: 'Filter', placeholder: 'name', value: '' }, press: { plugin: 'status-plugin', handle: 2 } },
      { type: 'Select', props: { key: 'period', label: 'Period', options: [{ value: 'day', label: 'Today' }, { value: 'week' }], value: 'day' }, press: { plugin: 'status-plugin', handle: 3 } },
      { type: 'Link', props: { href: 'https://example.com', label: 'Details' } },
      { type: 'Code', props: { source: 'const ready = true;', language: 'typescript', path: 'status.ts' } },
      { type: 'Markdown', props: { key: 'status-markdown', text: '**All good**', dimColor: true } },
    ],
  };

  const html = render(tree);
  assert.match(html, /class="mod-ui-client-box"/);
  assert.match(html, /class="mod-ui-client-text"[^>]*><strong>Status|class="mod-ui-client-text"[^>]*>Status/);
  assert.match(html, /class="mod-ui-client-button"[^>]*aria-label="Refresh"[^>]*>[\s\S]*Refresh child/);
  assert.match(html, /class="mod-ui-client-input"[^>]*placeholder="name"/);
  assert.match(html, /class="mod-ui-client-select"/);
  assert.match(html, /<option value="day" selected="">Today<\/option>/);
  assert.match(html, /class="mod-ui-client-link"[^>]*href="https:\/\/example.com\/?"/);
  assert.match(html, /data-language="typescript"/);
  assert.match(html, /ready = /);
  assert.match(html, /class="mod-ui-client-markdown"[^>]*[\s\S]*All good/);
  assert.doesNotMatch(html, /<Client|plugin source/);
});

test('ReactDOM retains an edited Client input across parent revisions and resets on site or client identity changes', { timeout: 45_000 }, async () => {
  const electronRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const tempRoot = mkdtempSync(join(realpathSync(tmpdir()), 'lingxi-mod-ui-client-revision-'));
  const fixtureRoot = join(tempRoot, 'fixture');
  const buildDir = join(tempRoot, 'build');
  const userData = join(tempRoot, 'user-data');
  const fixtureHtml = join(fixtureRoot, 'index.html');
  const fixtureMain = join(fixtureRoot, 'main.tsx');
  const electronDriver = join(tempRoot, 'electron-driver.mjs');
  let child: ReturnType<typeof spawn> | undefined;
  try {
    mkdirSync(fixtureRoot, { recursive: true });
    mkdirSync(userData, { recursive: true });
    writeFileSync(fixtureHtml, '<!doctype html><html><body><div id="root"></div><script type="module" src="/main.tsx"></script></body></html>');
    writeFileSync(fixtureMain, `
      import * as React from 'react';
      import { createRoot } from 'react-dom/client';
      import { ModUiClientTree } from '@regression/modUiClientTree';
      import { Theme } from '@regression/ThemeContext';
      import { tokens } from '@regression/tokens';

      function Fixture() {
        const [revision, setRevision] = React.useState(1);
        const [siteId, setSiteId] = React.useState('site-one');
        const [module, setModule] = React.useState('surface.tsx');
        const capturedInput = React.useRef(null);
        const client = { plugin: 'fixture-plugin', key: 'message', module };
        const site = { component: 'AbovePrompt', instanceId: siteId };
        const tree = {
          type: 'Input',
          props: { key: 'message', label: 'Message', value: 'seed' },
          press: { plugin: 'fixture-plugin', handle: 1 },
        };
        const input = () => document.querySelector('.mod-ui-client-input');
        const snapshot = () => {
          const current = input();
          return {
            revision,
            siteId,
            module,
            value: current?.value ?? null,
            focused: current !== null && document.activeElement === current,
            sameNode: capturedInput.current !== null && capturedInput.current === current,
          };
        };
        window.__modUiClientRevisionRegression = {
          snapshot,
          captureInput() { capturedInput.current = input(); },
          typeDraft(value) {
            const current = input();
            if (!current) throw new Error('Client input is missing');
            current.focus();
            Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(current, value);
            current.dispatchEvent(new Event('input', { bubbles: true }));
          },
          bumpParentRevision() { setRevision((current) => current + 1); },
          changeSite() { setSiteId('site-two'); },
          changeClient() { setModule('surface-two.tsx'); },
        };
        return <Theme.Provider value={tokens(false)}>
          <ModUiClientTree tree={tree} site={site} client={client} onPress={() => {}} renderRevision={revision} />
        </Theme.Provider>;
      }

      createRoot(document.getElementById('root')).render(<Fixture />);
    `);
    writeFileSync(electronDriver, `
      import { app, BrowserWindow } from 'electron';
      const url = process.argv.at(-1);
      app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
      const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const main = async () => {
        await app.whenReady();
        const window = new BrowserWindow({ show: false, width: 600, height: 400, webPreferences: { sandbox: true, backgroundThrottling: false } });
        try {
          await window.loadURL(url);
          const run = (source) => window.webContents.executeJavaScript(source);
          const wait = async (predicate) => {
            const deadline = Date.now() + 8_000;
            while (Date.now() < deadline) {
              if (await run(predicate)) return;
              await delay(20);
            }
            throw new Error('Timed out waiting for Client revision fixture');
          };
          await wait('Boolean(window.__modUiClientRevisionRegression?.snapshot().value)');
          await run("window.__modUiClientRevisionRegression.captureInput(); window.__modUiClientRevisionRegression.typeDraft('revision-draft')");
          await wait("window.__modUiClientRevisionRegression.snapshot().value === 'revision-draft'");
          await run('window.__modUiClientRevisionRegression.bumpParentRevision()');
          await wait('window.__modUiClientRevisionRegression.snapshot().revision === 2');
          const parentRevision = await run('window.__modUiClientRevisionRegression.snapshot()');

          await run('window.__modUiClientRevisionRegression.captureInput(); window.__modUiClientRevisionRegression.changeSite()');
          await wait("window.__modUiClientRevisionRegression.snapshot().siteId === 'site-two'");
          const siteChange = await run('window.__modUiClientRevisionRegression.snapshot()');

          await run("window.__modUiClientRevisionRegression.captureInput(); window.__modUiClientRevisionRegression.typeDraft('client-draft')");
          await wait("window.__modUiClientRevisionRegression.snapshot().value === 'client-draft'");
          await run('window.__modUiClientRevisionRegression.captureInput(); window.__modUiClientRevisionRegression.changeClient()');
          await wait("window.__modUiClientRevisionRegression.snapshot().module === 'surface-two.tsx'");
          const clientChange = await run('window.__modUiClientRevisionRegression.snapshot()');
          console.log(JSON.stringify({ parentRevision, siteChange, clientChange }));
        } finally {
          window.destroy();
          await app.quit();
        }
      };
      main().catch((error) => { console.error(error); process.exitCode = 1; app.quit(); });
    `);

    await build({
      root: fixtureRoot,
      base: './',
      cacheDir: join(tempRoot, 'vite-cache'),
      configFile: false,
      logLevel: 'error',
      plugins: [reactPlugin()],
      resolve: {
        alias: [
          { find: 'react-dom/client', replacement: resolve(electronRoot, 'node_modules/react-dom/client.js') },
          { find: 'react-dom', replacement: resolve(electronRoot, 'node_modules/react-dom/index.js') },
          { find: 'react/jsx-runtime', replacement: resolve(electronRoot, 'node_modules/react/jsx-runtime.js') },
          { find: 'react', replacement: resolve(electronRoot, 'node_modules/react/index.js') },
          { find: '@regression/modUiClientTree', replacement: resolve(electronRoot, 'src/renderer/components/modUiClientTree.tsx') },
          { find: '@regression/ThemeContext', replacement: resolve(electronRoot, 'src/renderer/theme/ThemeContext.tsx') },
          { find: '@regression/tokens', replacement: resolve(electronRoot, 'src/renderer/theme/tokens.ts') },
        ],
        dedupe: ['react', 'react-dom'],
      },
      build: { outDir: buildDir, emptyOutDir: true, rollupOptions: { input: fixtureHtml } },
    });

    const output: string[] = [];
    const errors: string[] = [];
    child = spawn(process.execPath, [resolve(electronRoot, 'node_modules/electron/cli.js'), electronDriver, pathToFileURL(join(buildDir, 'index.html')).href], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', LINGXI_TEST_USER_DATA: userData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk: Buffer | string) => output.push(String(chunk)));
    child.stderr.on('data', (chunk: Buffer | string) => errors.push(String(chunk)));
    const result = await new Promise<{ parentRevision: { value: string; focused: boolean; sameNode: boolean }; siteChange: { value: string; focused: boolean; sameNode: boolean }; clientChange: { value: string; focused: boolean; sameNode: boolean } }>((resolveResult, rejectResult) => {
      const timeout = setTimeout(() => {
        child?.kill('SIGTERM');
        rejectResult(new Error(`Electron Client revision fixture timed out\n${errors.join('')}`));
      }, 35_000);
      child?.once('error', (error) => { clearTimeout(timeout); rejectResult(error); });
      child?.once('exit', (code, signal) => {
        clearTimeout(timeout);
        const line = output.join('').trim().split('\n').at(-1);
        if (code !== 0 || !line) {
          rejectResult(new Error(`Electron Client revision fixture exited ${code ?? signal}\n${errors.join('')}`));
          return;
        }
        try { resolveResult(JSON.parse(line)); }
        catch (error) { rejectResult(new Error(`Invalid Electron Client revision result: ${line}`, { cause: error })); }
      });
    });

    assert.deepEqual(result.parentRevision, {
      revision: 2, siteId: 'site-one', module: 'surface.tsx', value: 'revision-draft', focused: true, sameNode: true,
    });
    assert.deepEqual(result.siteChange, {
      revision: 2, siteId: 'site-two', module: 'surface.tsx', value: 'seed', focused: false, sameNode: false,
    });
    assert.deepEqual(result.clientChange, {
      revision: 2, siteId: 'site-two', module: 'surface-two.tsx', value: 'seed', focused: false, sameNode: false,
    });
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    rmSync(tempRoot, { recursive: true, force: true });
  }
});

test('renders a root Client string as escaped text and preserves its UTF-16 value', () => {
  const source = '<root & text> \uD800';
  const parsed = parseModUiClientTree(source);
  assert.equal(parsed, source);

  const html = render(parsed);
  assert.match(html, /&lt;root &amp; text&gt;/);
  assert.doesNotMatch(html, /<root & text>/);

  assert.throws(() => parseModUiClientTree('x'.repeat(100_000)), /serialized size limit/);
  assert.throws(() => parseModUiClientTree(null), /nodes must be objects/);
  assert.throws(() => parseModUiClientTree([]), /nodes must be objects/);
});

test('keeps links inert unless they use a supported external URL scheme', () => {
  const html = render({
    type: 'Box',
    children: [
      { type: 'Link', props: { href: 'javascript:alert(1)', label: 'blocked' } },
      { type: 'Link', props: { href: 'mailto:support@example.com', label: 'email' } },
      { type: 'Link', props: { href: 'https://example.com/path', label: 'web' } },
      { type: 'Link', props: { href: 'http://localhost:3000/health', label: 'local' } },
      { type: 'Link', props: { href: 'https://user:pass@example.com/', label: 'credentials' } },
      { type: 'Link', props: { href: 'https://example.com/@team', label: 'raw at' } },
    ],
  });
  assert.match(html, /class="mod-ui-client-link-disabled"[^>]*>blocked/);
  assert.match(html, /class="mod-ui-client-link-disabled"[^>]*>email/);
  assert.doesNotMatch(html, /href="javascript:/);
  assert.match(html, /href="https:\/\/example.com\/path"/);
  assert.match(html, /href="http:\/\/localhost:3000\/health"/);
  assert.doesNotMatch(html, /href="https:\/\/user:pass/);
  assert.doesNotMatch(html, /href="https:\/\/example.com\/@team/);
});

test('keeps ordinary Client Markdown links on the normal navigation path', () => {
  const html = render({ type: 'Markdown', props: { text: '[Docs](https://example.test/docs)' } });
  assert.match(html, /<a[^>]*href="https:\/\/example\.test\/docs"[^>]*target="_blank"/);
  assert.doesNotMatch(html, /data-pressable-links/);
});

test('rejects a Client Markdown press handler but retains pressableLinks as plain metadata', () => {
  assert.throws(() => parseModUiClientTree({
    type: 'Markdown', props: { text: '[Docs](https://example.test/docs)' }, press: { plugin: 'status-plugin', handle: 4 },
  }), /unsupported properties/);
  assert.doesNotThrow(() => parseModUiClientTree({
    type: 'Markdown', props: { text: '[Docs](https://example.test/docs)', pressableLinks: ['https://example.test/docs'] },
  }));
});

test('rejects unknown element names and nested Client instances before React can create tags', () => {
  assert.throws(() => parseModUiClientTree({ type: 'script', props: { children: 'alert(1)' } }), /node type is unsupported/);
  assert.throws(() => parseModUiClientTree({ type: 'Box', children: [{ type: 'Client', props: { key: 'nested', module: 'nested.tsx' } }] }), /node type is unsupported/);
});

test('allows recursive Button children while rejecting children on input leaves', () => {
  assert.doesNotThrow(() => parseModUiClientTree({
    type: 'Button',
    props: { key: 'ok', label: 'Okay' },
    press: { plugin: 'status-plugin', handle: 1 },
    children: [{ type: 'Text', children: ['nested'] }],
  }));
  assert.throws(() => parseModUiClientTree({
    type: 'Input',
    props: { key: 'name' },
    press: { plugin: 'status-plugin', handle: 1 },
    children: ['unexpected'],
  }), /is a leaf/);
});

test('rejects repeated Select values and malformed held handles', () => {
  assert.throws(() => parseModUiClientTree({
    type: 'Select',
    props: { key: 'choice', options: [{ value: 'x' }, { value: 'x' }] },
    press: { plugin: 'status-plugin', handle: 1 },
  }), /must be unique/);
  assert.throws(() => parseModUiClientTree({
    type: 'Button', props: { key: 'ok', label: 'Okay' }, press: { plugin: 'status-plugin', handle: 0 },
  }), /positive safe integer/);
  assert.throws(() => parseModUiClientTree({ type: 'Box', props: { flexDirection: 'diagonal' } }), /flexDirection is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Box', props: { top: 1.5 } }), /top is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Text', props: { wrap: 'unknown' } }), /wrap is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Button', props: { key: 'ok', label: 'Okay' }, press: { plugin: 'other', handle: 1 } }, 'status-plugin'), /owning Client plugin/);
});

test('builds source-shaped press/input/select control bodies with the parent render identity', () => {
  const events: ModUiClientSurfaceEvent[] = [
    { type: 'press' },
    { type: 'input', kind: 'change', value: 'hello' },
    { type: 'input', kind: 'submit', value: 'hello' },
    { type: 'select', value: 'week' },
  ];
  assert.deepEqual(events.map((event) => buildNativeUiClientPressRequest(site, client, 'control', event)), [
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[0] },
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[1] },
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[2] },
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[3] },
  ]);
  assert.throws(() => buildNativeUiClientPressRequest(site, client, 'control', { type: 'input', kind: 'change', value: 'x'.repeat(16_385) }), /input event value is invalid/);
  assert.throws(() => buildNativeUiClientPressRequest(site, { ...client, module: 'm'.repeat(257) }, 'control', { type: 'press' }), /client.module is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Markdown', props: { text: 'docs' }, press: { plugin: 'status-plugin', handle: 4 } }), /unsupported properties/);
});

test('extracts only unique, leaf Client identities from a parent ui.render tree', () => {
  assert.deepEqual(collectModUiClientElements({
    type: 'Box',
    children: [{ type: 'Client', props: { key: 'status', module: 'surface.tsx', props: { value: ['ready'] }, width: '80%' }, client: { plugin: 'status-plugin' } }],
  }), [{ plugin: 'status-plugin', key: 'status', module: 'surface.tsx', props: { value: ['ready'] }, width: '80%' }]);
  assert.throws(() => collectModUiClientElements({
    type: 'Client', props: { key: 'status', module: 'surface.tsx' }, client: { plugin: 'status-plugin' }, children: 'not a leaf',
  }), /leaves/);
  assert.throws(() => collectModUiClientElements({
    type: 'Box', children: [
      { type: 'Client', props: { key: 'status', module: 'one.tsx' }, client: { plugin: 'status-plugin' } },
      { type: 'Client', props: { key: 'status', module: 'two.tsx' }, client: { plugin: 'status-plugin' } },
    ],
  }), /more than once/);
  assert.throws(() => collectModUiClientElements({
    type: 'Client', props: { key: 'status', module: 'surface.tsx', props: { unsafe: Number.NaN } }, client: { plugin: 'status-plugin' },
  }), /not finite/);
  const longIdentity = collectModUiClientElements({
    type: 'Box', children: [
      { type: 'Client', props: { key: 'k'.repeat(257), module: 'm'.repeat(257) }, client: { plugin: 'status-plugin' } },
      { type: 'Client', props: { key: 'sibling', module: 'surface.tsx' }, client: { plugin: 'status-plugin' } },
    ],
  });
  assert.equal(longIdentity.length, 2);
  assert.equal(isClientControlAddressable(longIdentity[0]), false);
  assert.equal(isClientControlAddressable(longIdentity[1]), true);
});

test('maps hook-adjusted elements to the current held handle and scopes invalidations by site', () => {
  const frame = {
    type: 'Box', children: [
      { type: 'Button', props: { key: 'run', label: 'Run' }, press: { plugin: 'status-plugin', handle: 7 } },
    ],
  } as unknown as ModUiSurfaceNode;
  assert.equal(heldHandleForElement(frame as unknown as never, 'run', 'status-plugin'), 7);
  assert.equal(heldHandleForElement(frame as unknown as never, 'run', 'other-plugin'), null);
  assert.equal(heldHandleForElement(frame as unknown as never, 'missing', 'status-plugin'), null);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's1', uuid: 'u1' }, 's1', 'above-prompt-x'), true);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's2', uuid: 'u1' }, 's1', 'above-prompt-x'), false);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's1', uuid: 'u1', instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: 'other' }] }, 's1', 'above-prompt-x'), false);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's1', uuid: 'u1', instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: 'above-prompt-x' }] }, 's1', 'above-prompt-x'), true);
});

test('keeps press controls disabled when the owning parent address is not representable', () => {
  const longClient = { ...client, key: 'k'.repeat(257) };
  assert.equal(isClientControlAddressable(longClient), false);
  const html = render({
    type: 'Box',
    children: [
      { type: 'Button', props: { key: 'run', label: 'Run' }, press: { plugin: 'status-plugin', handle: 1 } },
      { type: 'Input', props: { key: 'filter', value: '' }, press: { plugin: 'status-plugin', handle: 2 } },
      { type: 'Select', props: { key: 'period', options: [{ value: 'day' }] }, press: { plugin: 'status-plugin', handle: 3 } },
    ],
  }, isClientControlAddressable(longClient));
  assert.match(html, /<button[^>]*disabled=""[^>]*>Run/);
  assert.match(html, /<input[^>]*disabled=""/);
  assert.match(html, /<select[^>]*disabled=""/);
});

test('calculates VM character grids from measured pixels without a guessed cell size', () => {
  assert.deepEqual(gridSizeFromPixels(101, 38, 10, 19), { columns: 10, rows: 2 });
  assert.equal(gridSizeFromPixels(0, 38, 10, 19), null);
  assert.equal(gridSizeFromPixels(100, 38, Number.NaN, 19), null);
});
