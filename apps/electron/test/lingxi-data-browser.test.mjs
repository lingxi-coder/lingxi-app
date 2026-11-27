import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import { build } from 'esbuild';

test('Desktop Datasets lists datasets in the sidebar and opens their records', {
  skip: !process.env.LINGXI_TEST_PLAYWRIGHT_MODULE, timeout: 30000,
}, async t => {
  const bundle = await build({ write: false, outfile: 'workspace.js', bundle: true, jsx: 'automatic', platform: 'browser', format: 'esm',
    nodePaths: [resolve('node_modules')], stdin: { sourcefile: 'workspace.tsx', resolveDir: resolve('.'), loader: 'tsx', contents: `
      import { useState } from 'react';
      import { createRoot } from 'react-dom/client';
      import '../../packages/teams/styles.ts';
      import './src/renderer/components/LocalApps.css';
      import { LocalAppDatasets } from './src/renderer/components/LocalAppDatasets.tsx';
      import { Theme } from './src/renderer/theme/ThemeContext.tsx';
      import { tokens } from './src/renderer/theme/tokens.ts';
      import { teamsThemeVariables } from './src/renderer/theme/teamsTheme.ts';
      const dark = matchMedia('(prefers-color-scheme: dark)').matches;
      const palette = tokens(dark);
      document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
      const api = { data: async body => {
        const response = await fetch('/data', { method: 'POST', body: JSON.stringify(body) });
        if (!response.ok) throw new Error('Service unavailable');
        return response.json();
      }};
      function Workspace() {
        const [sidebarTarget, setSidebarTarget] = useState(null);
        const [collapsed, setCollapsed] = useState(false);
        return <Theme.Provider value={palette}>
          <button onClick={() => setCollapsed(value => !value)}>Toggle sidebar</button>
          <div className="workspace">
            {!collapsed && <aside ref={setSidebarTarget} />}
            <LocalAppDatasets workspace={{ uid: 'user', api, teams: [] }} locale="en"
              teamsTheme={teamsThemeVariables(palette)} sidebarTarget={sidebarTarget} />
          </div>
        </Theme.Provider>;
      }
      createRoot(document.querySelector('#app')).render(<Workspace />);
    ` } });
  const scripts = new Map(bundle.outputFiles.map(file => ['/' + file.path.split('/').at(-1), file.text]));
  const { chromium } = await import(pathToFileURL(process.env.LINGXI_TEST_PLAYWRIGHT_MODULE).href);
  const browser = await chromium.launch({ executablePath: process.env.LINGXI_TEST_CHROME });
  t.after(() => browser.close());
  const page = await browser.newPage({ locale: 'en-US', viewport: { width: 1050, height: 760 } });
  page.setDefaultTimeout(5000);
  await page.addInitScript(() => {
    window.lingxi = { teams: { localDataSource: async (uid, request) => {
      const response = await fetch('/sources', { method: 'POST', body: JSON.stringify({ uid, ...request }) });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error);
      return result;
    } } };
  });
  let catalogReads = 0;
  let descriptions = 0;
  const createOperations = [];
  let schema;
  const dataset = 'inventory';
  const otherDataset = 'recipes';
  const records = [];
  const sourceRequests = [];
  const postgres = { source: 'orders', kind: 'postgres', fields: ['id', 'title'] };
  const sources = [{
    source: 'cache', kind: 'redis', name: 'Cache', scope: { kind: 'personal' },
    enabled: true, version: 1, requesters: [], grants: [], running: false,
  }];
  let sourceLists = 0;
  let sourceImports = 0;
  let releaseCatalog;
  const pendingCatalog = new Promise(resolve => { releaseCatalog = resolve; });
  await page.route('https://datasets.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/') return route.fulfill({ contentType: 'text/html', body: `<!doctype html><meta charset="utf-8">
      <style>*{box-sizing:border-box;margin:0;padding:0}body{font:13px/1.5 system-ui}dialog{margin:auto}.workspace{display:flex}aside{flex:0 0 220px}</style>
      <link rel="stylesheet" href="/workspace.css"><div id="app"></div><script type="module" src="/workspace.js"></script>` });
    if (scripts.has(path)) return route.fulfill({ contentType: path.endsWith('.css') ? 'text/css' : 'text/javascript', body: scripts.get(path) });
    if (path === '/sources') {
      const request = route.request().postDataJSON();
      sourceRequests.push(request);
      switch (request.action) {
        case 'list':
          if (++sourceLists === 1) return route.fulfill({ status: 503, json: { error: 'DATA_UNAVAILABLE' } });
          return route.fulfill({ json: sources });
        case 'import':
          if (++sourceImports === 1) return route.fulfill({ status: 400, json: { error: 'DATA_SOURCE_KIND_MISMATCH' } });
          return route.fulfill({ json: postgres });
        case 'configure':
          sources.push({ ...postgres, ...request.input, running: false });
          return route.fulfill({ json: { running: false } });
        case 'start': return route.fulfill({ json: { running: true } });
        default: return route.fulfill({ status: 400, json: { error: 'INVALID_INPUT' } });
      }
    }
    if (path !== '/data') return route.fulfill({ status: 404 });
    const { command, operation } = route.request().postDataJSON();
    switch (command.action) {
      case 'datasets':
        if (++catalogReads === 1) { await pendingCatalog; return route.fulfill({ status: 503 }); }
        return route.fulfill({ json: { datasets: schema ? [dataset, otherDataset].map(id => ({ dataset: id, access: 'manage' })) : [] } });
      case 'create':
        createOperations.push(operation);
        schema = command.schema;
        if (createOperations.length === 1) return route.fulfill({ status: 503 });
        return route.fulfill({ json: { dataset } });
      case 'describe':
        if (++descriptions === 1) return route.fulfill({ status: 503 });
        return route.fulfill({ json: { schema, epoch: 1, schemaVersion: 1, rules: {} } });
      case 'list': return route.fulfill({ json: { records: command.dataset === dataset ? records : [
        { id: 'bread', record: { revision: 1, schemaVersion: 1, value: { title: 'Sourdough' } } },
      ], cursor: null } });
      case 'templateViews': return route.fulfill({ json: { views: [] } });
      case 'put':
        records.push({ id: command.record, record: { revision: 1, schemaVersion: 1, value: command.value } });
        return route.fulfill({ json: { dataset, record: command.record, revision: 1, schemaVersion: 1 } });
      default: return route.fulfill({ status: 400 });
    }
  });

  await page.goto('https://datasets.test/');
  const sidebar = page.getByRole('complementary').getByRole('navigation', { name: 'Datasets' });
  await sidebar.getByRole('status').filter({ hasText: 'Loading datasets' }).waitFor();
  releaseCatalog();
  await sidebar.getByRole('alert').filter({ hasText: 'Could not load datasets.' }).waitFor();
  assert.equal(await page.getByRole('combobox', { name: 'Dataset' }).count(), 0);
  await sidebar.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.getByRole('heading', { name: 'No datasets yet' }).waitFor();

  const trigger = page.getByRole('button', { name: 'New dataset' });
  const modal = page.getByRole('dialog', { name: 'New dataset' });
  await trigger.focus();
  await page.keyboard.press('Enter');
  await modal.waitFor();
  const provider = modal.getByRole('combobox', { name: 'Data provider' });
  const input = modal.getByRole('textbox', { name: 'JSON Schema' });
  assert.equal(await provider.evaluate(element => element === document.activeElement), true);
  assert.deepEqual(await provider.locator('option').allTextContents(), [
    'LingXi managed', 'MySQL', 'PostgreSQL', 'SQLite', 'MongoDB', 'Redis', 'CSV', 'JSON',
  ]);
  const initialProviderPosition = await provider.boundingBox();
  await input.fill('{"type":"object"}');
  await provider.selectOption('csv');
  await modal.getByText('Use a host configuration to read an existing file on this Mac.').waitFor();
  assert.deepEqual(await provider.boundingBox(), initialProviderPosition);
  assert.equal(await input.count(), 0);
  await provider.selectOption('managed');
  assert.equal(await input.inputValue(), '{"type":"object"}');
  await page.keyboard.press('Escape');
  await modal.waitFor({ state: 'hidden' });
  assert.equal(await trigger.evaluate(element => element === document.activeElement), true);
  await trigger.click();
  await provider.selectOption('postgres');
  await modal.getByRole('button', { name: 'Set up data source' }).click();
  await modal.waitFor({ state: 'hidden' });
  const sourceDialog = page.getByRole('dialog', { name: 'PostgreSQL data sources' });
  await sourceDialog.getByRole('alert').waitFor();
  await sourceDialog.getByRole('button', { name: 'Retry' }).click();
  const choose = sourceDialog.getByRole('button', { name: 'Choose host configuration' });
  await choose.click();
  await sourceDialog.getByText('Choose a host configuration for PostgreSQL.').waitFor();
  await choose.click();
  await sourceDialog.getByRole('textbox', { name: 'Name' }).fill('Orders');
  await sourceDialog.getByRole('button', { name: 'Save source' }).click();
  await sourceDialog.getByText('Orders', { exact: true }).waitFor();
  assert.equal(await sourceDialog.getByText('Cache', { exact: true }).count(), 0);
  assert.deepEqual(sourceRequests.filter(request => request.action === 'import'), [
    { uid: 'user', action: 'import', kind: 'postgres' }, { uid: 'user', action: 'import', kind: 'postgres' },
  ]);
  assert.equal(createOperations.length, 0);
  await sourceDialog.getByRole('button', { name: 'Close', exact: true }).click();
  assert.equal(await trigger.evaluate(element => element === document.activeElement), true);
  await page.getByRole('button', { name: 'Data sources', exact: true }).click();
  const sourceManager = page.getByRole('dialog', { name: 'Data sources', exact: true });
  await sourceManager.getByText('Orders', { exact: true }).waitFor();
  await sourceManager.getByText('Cache', { exact: true }).waitFor();
  await sourceManager.getByRole('button', { name: 'Close', exact: true }).click();

  await trigger.click();
  await provider.selectOption('managed');
  await input.fill('[]');
  await modal.getByRole('button', { name: 'Create dataset' }).click();
  await modal.getByRole('alert').filter({ hasText: 'Enter a valid JSON object.' }).waitFor();
  assert.equal(createOperations.length, 0);
  await input.fill(JSON.stringify({ type: 'object', additionalProperties: false, properties: { title: { type: 'string' } } }));
  await modal.getByRole('button', { name: 'Create dataset' }).click();
  await modal.getByRole('alert').filter({ hasText: 'Could not create the dataset.' }).waitFor();
  await modal.getByRole('button', { name: 'Create dataset' }).click();
  await page.getByRole('alert').filter({ hasText: 'Could not open this dataset.' }).waitFor();
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.getByText('This dataset has no records yet.').waitFor();
  assert.equal(createOperations.length, 2);
  assert.equal(createOperations[0], createOperations[1]);
  assert.equal(await sidebar.getByRole('button', { name: dataset }).getAttribute('aria-current'), 'page');
  assert.equal(await page.getByRole('combobox', { name: 'Dataset' }).count(), 0);

  await page.getByText('Add record', { exact: true }).click();
  await page.getByRole('textbox', { name: 'Record ID' }).fill('sample');
  await page.getByRole('textbox', { name: 'Record JSON' }).fill('{"title":"Field notebook"}');
  await page.getByRole('button', { name: 'Save record' }).click();
  await page.getByRole('cell', { name: 'Field notebook' }).waitFor();
  assert.equal(await page.getByRole('textbox', { name: 'Record ID' }).isVisible(), false);
  await page.getByText('Publish to Team', { exact: true }).click();
  await page.getByRole('button', { name: 'Preview publication audience' }).waitFor();
  await page.reload();
  await page.getByRole('cell', { name: 'Field notebook' }).waitFor();
  await page.getByRole('heading', { name: dataset }).waitFor();
  const otherRow = sidebar.getByRole('button', { name: otherDataset });
  await otherRow.focus();
  await page.keyboard.press('Enter');
  await page.getByRole('cell', { name: 'Sourdough' }).waitFor();
  assert.equal(await otherRow.getAttribute('aria-current'), 'page');
  assert.equal(await otherRow.evaluate(element => element === document.activeElement), true);
  assert.equal(await page.getByRole('cell', { name: 'Field notebook' }).count(), 0);

  await page.getByRole('button', { name: 'Toggle sidebar' }).click();
  await sidebar.waitFor({ state: 'hidden' });
  await page.setViewportSize({ width: 340, height: 660 });
  assert.equal(await page.locator('body').evaluate(element => element.scrollWidth === element.clientWidth), true);
  const picker = page.getByRole('combobox', { name: 'Dataset' });
  assert.equal(await picker.inputValue(), otherDataset);
  await picker.selectOption(dataset);
  await page.getByRole('cell', { name: 'Field notebook' }).waitFor();
  await page.getByRole('button', { name: 'Toggle sidebar' }).click();
  await sidebar.waitFor();
  assert.equal(await sidebar.getByRole('button', { name: dataset }).getAttribute('aria-current'), 'page');
});

test('Desktop and Web open a custom record view, recover a conflict and import a Chat schema', {
  skip: !process.env.LINGXI_DATA_FIXTURE || !process.env.LINGXI_TEST_PLAYWRIGHT_MODULE, timeout: 45000,
}, async t => {
  const { createProductDataBrowserFixture } = await import('../../firebase/test/lingxi-data-browser-fixture.mjs');
  const fixture = JSON.parse(await readFile(process.env.LINGXI_DATA_FIXTURE, 'utf8'));
  const product = await createProductDataBrowserFixture(fixture);
  t.after(() => product.close());
  const scope = product.binding.scope, { dataset, record } = fixture.reference;
  const latest = await product.request({ scope, command: { action: 'latest', dataset, record } });
  assert.ok(latest.revision > fixture.reference.revision);
  const pinned = await product.request({ scope, command: { action: 'get', reference: fixture.reference } });
  const opened = await product.request({ scope, command: { action: 'get', reference: latest } });
  await product.request({ scope, command: { action: 'setTemplateView', dataset, view: 'order-summary', expectedVersion: 0,
    schemaVersion: latest.schemaVersion, definition: { version: 1, nodes: [{ kind: 'element', tag: 'h3',
      children: [{ kind: 'text', text: 'Order: ' }, { kind: 'field', field: 'title' }] }] } } });
  const bundle = await build({ write: false, bundle: true, jsx: 'automatic', platform: 'browser',
    nodePaths: [resolve('node_modules')], stdin: {
    resolveDir: resolve('.'), loader: 'tsx', contents: `
      import { createRoot } from 'react-dom/client';
      import { DataRecordButton } from '../../packages/teams/DataRecordButton.tsx';
      import { compileDataTemplate, DataTemplate } from '../../packages/teams/DataTemplate.tsx';
      import { PersonalChatData } from '../../packages/teams/PersonalChatData.tsx';
      const api = { data: async body => {
        const response = await fetch('/data', { method: 'POST', body: JSON.stringify(body) });
        const result = await response.json();
        if (!response.ok) throw new Error(result.error.code);
        return result;
      }};
      createRoot(document.querySelector('#app')).render(<>
        <DataRecordButton api={api} scope={${JSON.stringify(scope)}} reference={${JSON.stringify(fixture.reference)}} locale="en"
          onSaved={() => document.querySelector('#status').textContent = 'Saved'} />
        <DataTemplate spec={compileDataTemplate('<section class="summary"><h3>{{title}}</h3></section>',
          '.summary { color: #423190 }', ['title'])} value={{ title: '<em>Literal title</em>' }} />
        <PersonalChatData uid={${JSON.stringify(product.uid)}} api={api} teams={[]} locale="en"
          chatReply={${JSON.stringify('Here is the draft.\n```json\n{"type":"object","additionalProperties":false,"properties":{"subject":{"type":"string"}}}\n```')}} />
      </>);
    `,
  } });
  const { chromium } = await import(pathToFileURL(process.env.LINGXI_TEST_PLAYWRIGHT_MODULE).href);
  const browser = await chromium.launch({ executablePath: process.env.LINGXI_TEST_CHROME });
  t.after(() => browser.close());
  const page = await browser.newPage();
  await page.route('https://lingxi-data-ui.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/') return route.fulfill({ contentType: 'text/html', body: '<main id="app"></main><p id="status"></p><script src="/app.js"></script>' });
    if (path === '/app.js') return route.fulfill({ contentType: 'text/javascript', body: bundle.outputFiles[0].text });
    try {
      const request = route.request().postDataJSON();
      return route.fulfill({ json: await product.request(request) });
    } catch (error) {
      return route.fulfill({ status: error.message === 'DATA_CONFLICT' ? 409 : 503, json: { error: { code: error.message } } });
    }
  });
  await page.goto('https://lingxi-data-ui.test/');
  const template = page.getByRole('heading', { name: '<em>Literal title</em>', exact: true });
  await template.waitFor();
  assert.equal(await template.locator('em').count(), 0);
  assert.equal(await template.evaluate(element => getComputedStyle(element).color), 'rgb(66, 49, 144)');
  await page.getByRole('button', { name: 'Open record' }).click();
  await page.getByRole('combobox', { name: 'View' }).selectOption('order-summary');
  await page.getByRole('heading', { name: 'Order: ' + pinned.record.value.title }).waitFor();
  const title = page.getByRole('textbox', { name: 'title', exact: true });
  await title.fill('Stale attempt');
  await page.getByRole('button', { name: 'Save changes' }).click();
  await page.getByRole('alert').filter({ hasText: 'The record changed. Reload before editing.' }).waitFor();
  await page.getByRole('button', { name: 'Load latest revision' }).click();
  await title.waitFor();
  assert.equal(await title.inputValue(), opened.record.value.title);
  await title.fill('Saved after conflict');
  await page.getByRole('button', { name: 'Save changes' }).click();
  await page.getByText('Saved', { exact: true }).waitFor();
  const after = await product.request({ scope, command: { action: 'latest', dataset, record } });
  assert.equal(after.revision, latest.revision + 1);
  assert.equal((await product.request({ scope, command: { action: 'get', reference: after } })).record.value.title, 'Saved after conflict');
  await page.getByRole('button', { name: 'Personal data' }).click();
  await page.getByText('Create a dataset from JSON Schema').click();
  await page.getByRole('button', { name: 'Use schema from the latest reply' }).click();
  const schema = { type: 'object', additionalProperties: false, properties: { subject: { type: 'string' } } };
  assert.deepEqual(JSON.parse(await page.getByRole('textbox', { name: 'JSON Schema' }).inputValue()), schema);
  await page.getByRole('button', { name: 'Create dataset', exact: true }).click();
  const workspace = page.getByRole('dialog', { name: 'Personal data' });
  await workspace.getByText('Add record', { exact: true }).waitFor();
  const created = await workspace.getByRole('combobox', { name: 'Dataset' }).inputValue();
  assert.match(created, /^personal-/u);
  const description = await product.request({ scope: { kind: 'personal' }, command: { action: 'describe', dataset: created } });
  assert.deepEqual(description.schema.properties, schema.properties);
});
