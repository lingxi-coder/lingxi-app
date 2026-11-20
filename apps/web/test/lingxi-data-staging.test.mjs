import assert from 'node:assert/strict';
import test from 'node:test';
import { pathToFileURL } from 'node:url';

const { LINGXI_DATA_STAGING_WEB: endpoint, LINGXI_TEST_PLAYWRIGHT_MODULE: playwright,
  LINGXI_TEST_CHROME: chrome } = process.env;

test('the Web Team page opens a published view and enforces the reader and editor roles', {
  skip: !endpoint || !playwright, timeout: 75000,
}, async t => {
  const url = new URL(endpoint);
  assert.equal(url.protocol, 'http:');
  assert.equal(url.hostname, '127.0.0.1');
  assert.equal(url.pathname, '/teams');
  const { chromium } = await import(pathToFileURL(playwright).href);
  const browser = await chromium.launch({ executablePath: chrome });
  t.after(() => browser.close());

  async function openRecord(account) {
    const context = await browser.newContext({ locale: 'en-US' });
    await context.addInitScript(() => localStorage.setItem('lingxi.web.locale', JSON.stringify('en')));
    const page = await context.newPage();
    await page.goto(url.href);
    await page.getByRole('textbox', { name: 'Email' }).fill(account + '@fixture.invalid');
    await page.locator('input[type=password]').fill('AccountFixture123');
    await page.locator('form button').first().click();
    await page.getByRole('button', { name: 'Data Staging', exact: true }).waitFor();
    await page.getByRole('button', { name: /^Orders/u }).first().click();
    await page.getByRole('button', { name: 'Open record', exact: true }).click();
    await page.getByRole('combobox', { name: 'View' }).selectOption('order-summary');
    await page.getByRole('heading', { name: 'Prepare shipment', exact: true }).waitFor();
    return page;
  }

  const owner = await openRecord('owner');
  const reader = await openRecord('reader');
  const submit = { name: 'Save changes', exact: true };
  assert.equal(await reader.getByRole('button', submit).count(), 0);

  async function save() {
    const result = owner.waitForResponse(response => {
      const request = response.request();
      return request.method() === 'POST' && new URL(request.url()).pathname === '/v1/teams/data'
        && request.postDataJSON()?.command?.action === 'put';
    });
    await owner.getByRole('textbox', { name: 'title', exact: true }).fill('Updated in Web staging');
    await owner.getByRole('button', submit).click();
    return result;
  }

  let result = await save();
  if (result.status() === 409) {
    await owner.getByRole('button', { name: 'Load latest revision' }).click();
    result = await save();
  }
  assert.equal(result.status(), 200);
});
