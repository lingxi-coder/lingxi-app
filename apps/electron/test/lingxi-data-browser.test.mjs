import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import { createProductDataBrowserFixture } from '../../firebase/test/lingxi-data-browser-fixture.mjs';

test('Desktop and Web open a custom record view, recover a conflict and import a Chat schema', {
  skip: !process.env.LINGXI_DATA_FIXTURE || !process.env.LINGXI_TEST_PLAYWRIGHT_MODULE, timeout: 45000,
}, async t => {
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
