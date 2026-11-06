import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as settings from '../src/shared/productConfig.js';
import { MarketApiClient } from '../src/main/local-apps/marketClient.js';

const root = 'https://marketgateway-pacws5zsoa-uc.a.run.app/api/v3/';
test('Bot API uses its explicit trusted root while App v2 and website stay on Hosting', async () => {
  assert.equal((settings as any).botMarketApiRoot, root);
  assert.equal(settings.localAppMarketApiRoot, 'https://lingxi-5e4b2-market.web.app/api/v2');
  assert.equal(settings.localAppMarketUrl, 'https://lingxi-5e4b2-market.web.app/');
  let seen: URL | undefined;
  const api = new MarketApiClient(async () => 'fixture-token', (settings as any).botMarketApiRoot,
    async (url, options) => { seen = new URL(String(url)); assert.equal(options?.redirect, 'error'); return Response.json({ data: {} }); });
  await api.request('bot-install-intents/example', undefined, true);
  assert.equal(seen?.href, root + 'bot-install-intents/example');
});

test('Bot API configuration rejects credential-bearing, ambiguous and non-v3 roots', () => {
  const parse = (settings as any).parseBotMarketApiRoot;
  assert.equal(typeof parse, 'function');
  assert.equal(parse(root), root);
  for (const value of [undefined, '', '/api/v3/', 'http://market.test/api/v3/',
    'https://user:password@market.test/api/v3/', 'https://market.test/api/v3/?token=x',
    'https://market.test/api/v3/#fragment', 'https://market.test/api/v2/',
    'https://market.test/api/v3', 'https://market.test/other/api/v3/']) assert.throws(() => parse(value));
});
