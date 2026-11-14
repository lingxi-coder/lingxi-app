import assert from 'node:assert/strict';
import test from 'node:test';
import {
  publicExtensionItems,
  publicExtensionsForPlatform,
  publicExtensionText,
  publicExtensionFromNavigation,
  publicExtensionNavigationId,
  publicExtensionNavigationAction,
  publicSkillRecommendations,
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

test('recommended skills link to complete upstream packages on every supported platform', () => {
  assert(publicSkillRecommendations.length >= 3);
  for (const item of publicSkillRecommendations) {
    assert.equal(item.kind, 'plugin');
    assert(item.marketplace?.plugin, `${item.id} needs an installable package`);
    assert.equal(new Set(item.skills).size, item.skills!.length);
    assert.equal(publicExtensionFromNavigation(publicExtensionNavigationId(item)), item);
    for (const platform of ['macos', 'windows', 'linux', 'android', 'ios'] as const) {
      assert(publicExtensionsForPlatform(platform).includes(item));
    }
  }
  assert.equal(publicExtensionFromNavigation('secure@acme'), undefined);
  assert.equal(publicExtensionFromNavigation('directory:missing'), undefined);
  assert.equal(publicExtensionFromNavigation('directory:context7'), undefined);
});

test('remote recommendations have usable HTTPS templates without placeholder credentials', () => {
  for (const id of ['notion', 'linear', 'figma', 'supabase']) {
    const item = publicExtensionItems.find((entry) => entry.id === id)!;
    assert.equal(item.mcp?.config.type, 'http');
    const url = new URL(String(item.mcp?.config.url));
    assert.equal(url.protocol, 'https:');
    assert.equal(url.username, '');
    assert.equal(url.password, '');
    assert(!url.href.includes('${'));
    assert(item.requirements?.en);
  }
});

test('extension details keep localized examples and explicit installation intents separate', () => {
  const item = publicSkillRecommendations[0]!;
  assert.equal(publicExtensionNavigationAction(publicExtensionNavigationId(item)), 'details');
  for (const action of ['details', 'add', 'uninstall'] as const) {
    const id = publicExtensionNavigationId(item, action);
    assert.equal(publicExtensionFromNavigation(id), item);
    assert.equal(publicExtensionNavigationAction(id), action);
  }
  assert.equal(publicExtensionNavigationAction(`directory:${item.id}`), 'details');
  assert.equal(publicExtensionNavigationAction(undefined), 'details');
  for (const entry of publicExtensionItems) {
    for (const example of entry.examples ?? []) {
      for (const locale of ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko']) {
        assert(example[locale]?.trim(), `${entry.id} example lacks ${locale}`);
      }
    }
  }
});
