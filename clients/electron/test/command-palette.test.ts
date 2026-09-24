import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

import { commandPaletteIcon } from '../src/renderer/components/commandPaletteIcons';

test('slash command popup gives known commands stable semantic icons', () => {
  assert.equal(commandPaletteIcon('model'), 'box');
  assert.equal(commandPaletteIcon('mcp'), 'mcp');
  assert.equal(commandPaletteIcon('cron'), 'clock');
  assert.equal(commandPaletteIcon('permissions'), 'shield');
  assert.equal(commandPaletteIcon('plan'), 'bulb');
  assert.equal(commandPaletteIcon('rename'), 'pencil');
  assert.equal(commandPaletteIcon('share'), 'share');
  assert.equal(commandPaletteIcon('open-provider-credentials'), 'key');
  assert.equal(commandPaletteIcon('copy-last-response'), 'copy');
  assert.equal(commandPaletteIcon('new'), 'chatPlus');
  assert.equal(commandPaletteIcon('pet'), 'user');
  assert.equal(commandPaletteIcon('status'), 'gauge');
  assert.equal(commandPaletteIcon('autocompact'), 'compact');
  assert.equal(commandPaletteIcon('brief'), 'summary');
  assert.equal(commandPaletteIcon('btw'), 'chatPlus');
});

test('slash command popup categorizes extensions and keeps a visible fallback', () => {
  assert.equal(commandPaletteIcon('team-agents'), 'users');
  assert.equal(commandPaletteIcon('deep-research'), 'search');
  assert.equal(commandPaletteIcon('format-code'), 'code');
  assert.equal(commandPaletteIcon('third-party-command'), 'terminal');
});

test('the composer owns the only command popup', () => {
  const app = readFileSync(join(process.cwd(), 'src/renderer/App.tsx'), 'utf8');
  const composer = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');

  assert.doesNotMatch(app, /DesktopCommandPalette|commandPaletteOpen|isCommandPaletteShortcut/);
  assert.equal(existsSync(join(process.cwd(), 'src/renderer/components/DesktopCommandPalette.tsx')), false);
  assert.match(composer, /<CommandIcon command=\{entry\.name\}/);
  assert.match(composer, /className="slash-command-row"/);
});

test('slash command rows keep neutral labels and a visible selection without a slash prefix', () => {
  const composer = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  const styles = readFileSync(join(process.cwd(), 'src/renderer/components/SlashCommandMenu.css'), 'utf8');

  assert.doesNotMatch(composer, /\/{entry\.name}/);
  assert.match(composer, /className="slash-command-name"/);
  assert.match(composer, /className="slash-command-description"/);
  assert.match(styles, /\.slash-command-row\[aria-selected='true'\] \{ background: var\(--slash-hover\); \}/);
});
