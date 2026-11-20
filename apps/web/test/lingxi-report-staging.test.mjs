import assert from 'node:assert/strict';
import test from 'node:test';
import { pathToFileURL } from 'node:url';

const { LINGXI_DATA_STAGING_WEB: url, LINGXI_TEST_PLAYWRIGHT_MODULE: playwright, LINGXI_TEST_CHROME: executablePath } = process.env;
test('published report in Chat renders the viewer projection and opens a real review case', { skip: !url || !playwright, timeout: 90000 }, async t => {
  const { chromium } = await import(pathToFileURL(playwright).href);
  const browser = await chromium.launch({ executablePath });
  t.after(() => browser.close());
  for (const account of ['owner', 'reader']) {
    const context = await browser.newContext({ locale: 'en-US' });
    await context.addInitScript(() => localStorage.setItem('lingxi.web.locale', JSON.stringify('en')));
    const page = await context.newPage();
    await page.goto(url);
    await page.getByRole('textbox', { name: 'Email' }).fill(account + '@fixture.invalid');
    await page.locator('input[type=password]').fill('AccountFixture123');
    await page.locator('form button').first().click();
    await page.getByRole('button', { name: 'Data Staging', exact: true }).waitFor();
    await page.getByRole('button', { name: /^Orders/u }).first().click();
    const opened = page.waitForResponse(response => response.request().method() === 'POST'
      && response.request().postDataJSON()?.command?.action === 'getReport');
    await page.getByRole('button', { name: 'Open published report', exact: true }).last().click();
    const response = await opened; assert.equal(response.status(), 200);
    const report = await response.json();
    await page.getByRole('heading', { name: 'Source report', exact: true }).waitFor();
    const table = page.getByRole('table', { name: 'Source report' });
    assert.equal(await table.getByRole('row').count(), report.points.length);
    if (account === 'reader') {
      assert.deepEqual(report.points, [{ group: 'a', value: '1' }]);
      assert.equal(await page.getByText('Review a partition', { exact: true }).count(), 0);
    } else {
      assert.ok(report.points.length > 1);
      await page.getByText('Review a partition', { exact: true }).click();
      await page.getByRole('button', { name: 'Open review case', exact: true }).first().click();
      await page.getByText('Review case open', { exact: true }).waitFor();
    }
    await context.close();
  }
});
