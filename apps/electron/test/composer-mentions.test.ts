import { test } from 'node:test';
import assert from 'node:assert/strict';
import React, { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MarkdownContent } from '../src/renderer/components/MarkdownContent';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { activeFileMention } from '../src/renderer/bridge/fileMentions';
import { contextMentionHref, contextMentionMarkdown, mentionCatalog, mentionMenuEntries, parseContextMentionHref, promptWithMentionLinks } from '../src/renderer/bridge/composerMentions';

Object.assign(globalThis, { React });

test('mention catalog uses installed, enabled plugins and discovered skills, never marketplace or disabled skill catalog rows', () => {
  const catalog = mentionCatalog({
    skills: [{ name: 'review', source_dir: '/skills/review' }, { name: 'review', source_dir: '/skills/review' }],
    skillCatalog: JSON.stringify({ entries: [{ name: 'review', directory: '/skills/review', description: 'Review code' }, { name: 'disabled', directory: '/skills/disabled' }] }),
    pluginCatalog: JSON.stringify({ installed: [{ id: 'browser', name: 'Browser', description: 'Control browser' }, { id: 'disabled', name: 'Disabled' }, { id: 'opt-in', name: 'Opt in', default_enabled: false }], available: [{ id: 'uninstalled', name: 'Uninstalled' }] }),
    effectiveSettings: JSON.stringify({ enabledPlugins: { disabled: false, 'opt-in': true } }),
  });
  assert.deepEqual(catalog.map((entry) => entry.id), ['plugin:browser', 'plugin:opt-in', 'skill:/skills/review/SKILL.md']);
  assert.equal(catalog[2]?.description, 'Review code');
  assert.deepEqual(mentionCatalog({ pluginCatalog: '{', skillCatalog: '{}' }), []);
  assert.deepEqual(mentionCatalog({ pluginCatalog: '{"installed":[null,2,{}]}' }), []);
});

test('empty @ opens grouped actions and references; searching and file drilldown preserve distinct identities', () => {
  const input = { query: '', filesOnly: false, files: ['src/main.ts', 'src/'], catalog: mentionCatalog({ skills: [{ name: 'review', source_dir: '/a' }, { name: 'review', source_dir: '/b' }] }), running: false, planActive: false, goalAvailable: true };
  assert.deepEqual(mentionMenuEntries(input).map((entry) => entry.group), ['Add', 'Add', 'Add', 'Add', 'Skills', 'Skills']);
  assert.deepEqual(mentionMenuEntries({ ...input, filesOnly: true }).map((entry) => entry.icon), ['file', 'folder']);
  assert.equal(mentionMenuEntries({ ...input, query: 'review', files: [] }).length, 2);
  assert(mentionMenuEntries({ ...input, running: true }).find((entry) => entry.action === 'goal')?.disabled);
  assert(mentionMenuEntries({ ...input, planActive: true }).find((entry) => entry.action === 'plan')?.disabled);
});

test('full mention links round-trip Unicode and reserved characters through safe Markdown rendering', () => {
  const mention = { kind: 'skill' as const, name: 'Review [代码]', target: '/My Files/review #1 (new)/SKILL.md' };
  assert.deepEqual(parseContextMentionHref(contextMentionHref(mention)), mention);
  const html = renderToStaticMarkup(createElement(Theme.Provider, { value: tokens(false) }, createElement(MarkdownContent, { text: contextMentionMarkdown(mention) })));
  assert.match(html, /<a[^>]*href="lingxi-mention:\/\/skill/);
  assert.match(html, /@Review \[代码\]/);
  assert.match(html, /title="\/My Files\/review #1 \(new\)\/SKILL.md"/);
  for (const href of ['javascript:alert(1)', 'lingxi-mention://unknown?name=a&target=b', 'lingxi-mention://plugin?name=a', 'lingxi-mention://user:password@plugin?name=a&target=b']) assert.equal(parseContextMentionHref(href), null);
});

test('file history reconstructs hyperlinks without changing the body or losing full paths', () => {
  const markdown = promptWithMentionLinks('@src/main.ts @"My Folder/"\n\nInspect this');
  assert.match(markdown, /lingxi-mention:\/\/file/);
  assert.match(markdown, /lingxi-mention:\/\/folder/);
  assert.match(markdown, /"src\/main.ts"/);
  assert(markdown.endsWith('\n\nInspect this'));
  assert.equal(promptWithMentionLinks('user@example.com'), 'user@example.com');
});

test('@ detection supports adjacent rich tokens, punctuation, escaped quotes, and rejects invalid caret positions', () => {
  assert.deepEqual(activeFileMention('\u200b@review', 8), { start: 1, end: 8, query: 'review' });
  assert.deepEqual(activeFileMention('(@src)', 5), { start: 1, end: 5, query: 'src' });
  const text = '@"My \\"Folder';
  assert.equal(activeFileMention(text, text.length)?.query, 'My "Folder');
  assert.equal(activeFileMention('mail@example.com', 16), null);
  assert.equal(activeFileMention('@"closed"', 9), null);
  assert.equal(activeFileMention('@src', -1), null);
  assert.equal(activeFileMention('@src', 5), null);
});
