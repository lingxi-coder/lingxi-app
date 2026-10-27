import assert from 'node:assert/strict';
import test from 'node:test';
import {
  publicExtensionItems,
  publicExtensionsForPlatform,
  publicExtensionText,
} from '../src/shared/publicExtensionDirectory';

test('public directory links are HTTPS and configuration drafts contain no credentials', () => {
  const ids = new Set<string>();
  for (const item of publicExtensionItems) {
    assert(!ids.has(item.id), `Duplicate directory item: ${item.id}`);
    ids.add(item.id);
    for (const raw of [item.sourceUrl, item.documentationUrl]) {
      const url = new URL(raw);
      assert.equal(url.protocol, 'https:');
      assert.equal(url.username, '');
      assert.equal(url.password, '');
    }
    for (const locale of ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko']) {
      assert(item.description[locale]?.trim(), `${item.id} lacks ${locale}`);
      if (item.requirements) assert(item.requirements[locale]?.trim());
    }
    if (item.mcp) {
      assert.equal(item.kind, 'mcp');
      assert.equal(item.mcp.config.headers, undefined);
      assert.equal(item.mcp.config.env, undefined);
      assert.equal(item.mcp.config.disabled, undefined);
      assert(!JSON.stringify(item.mcp.config).includes('@latest'));
    }
  }
});

test('desktop process recipes are pinned argument arrays, never shell scripts', () => {
  const recipes = publicExtensionItems.flatMap((item) => item.mcp?.config.command ? [item.mcp.config] : []);
  assert(recipes.length > 0);
  for (const recipe of recipes) {
    assert.equal(recipe.command, 'npx');
    assert(Array.isArray(recipe.args));
    assert.equal(recipe.args[0], '-y');
    assert.match(String(recipe.args[1]), /@\d+\.\d+\.\d+$/);
    assert(recipe.args.every((arg) => typeof arg === 'string' && !/[\r\n]/.test(arg)));
  }
});

test('mobile directory never offers a desktop process configuration', () => {
  for (const platform of ['android', 'ios'] as const) {
    const entries = publicExtensionsForPlatform(platform);
    assert(entries.some((item) => item.kind === 'plugin'));
    assert(entries.some((item) => item.mcp?.config.type === 'http'));
    assert(entries.every((item) => !item.mcp?.config.command));
  }
});

test('directory text follows explicit Chinese script and falls back to English', () => {
  const text = { en: 'English', 'zh-Hans': '简体', 'zh-Hant': '繁體', ja: '日本語' };
  assert.equal(publicExtensionText(text, 'zh-Hans-TW'), '简体');
  assert.equal(publicExtensionText(text, 'zh-TW'), '繁體');
  assert.equal(publicExtensionText(text, 'zh-Hant-HK'), '繁體');
  assert.equal(publicExtensionText(text, 'ja-JP'), '日本語');
  assert.equal(publicExtensionText(text, 'de-DE'), 'English');
  assert.equal(publicExtensionText(undefined), '');
});
