import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { CommandIdentity } from '../src/renderer/components/CommandIdentity';
import { CommandOutput } from '../src/renderer/components/CommandOutput';
import { CommandResultPanel } from '../src/renderer/components/CommandResultPanel';
import { Stage } from '../src/renderer/components/Stage';
import { commandPaletteColor, commandPaletteIcon } from '../src/renderer/components/commandPaletteIcons';
import { parseSlashCommandMessage, parseSlashCommandPrefix } from '../src/renderer/components/slashCommandMessage';

(globalThis as { React?: typeof React }).React = React;

test('command decoration preserves source whitespace and multiline arguments exactly', () => {
  for (const text of ['/plan', '  /plan  检查输入框\n保留换行  ', '/plugin:review --fix\n\n第二段', '/custom.command\targuments']) {
    const parsed = parseSlashCommandPrefix(text);
    assert.ok(parsed);
    assert.equal(parsed.prefix + parsed.rest, text);
  }
  for (const text of ['/', '/path/to/file', '//server', 'please use /plan', '/plan?query']) {
    assert.equal(parseSlashCommandPrefix(text), null);
  }
  assert.deepEqual(parseSlashCommandMessage('/plugin:review --fix\n第二段'), { name: 'plugin:review', arguments: '--fix\n第二段' });
});

test('command aliases and full invocations retain their identity in both themes', () => {
  for (const [alias, canonical] of [['cost', 'usage'], ['stats', 'usage'], ['settings', 'config'], ['plugins', 'plugin'], ['allowed-tools', 'permissions']] as const) {
    assert.equal(commandPaletteIcon(alias), commandPaletteIcon(canonical));
    for (const dark of [false, true]) assert.equal(commandPaletteColor(`/${alias} arguments`, dark), commandPaletteColor(canonical, dark));
  }
  for (const name of ['constructor', '__proto__', 'third-party-command']) {
    assert.equal(commandPaletteIcon(name), 'terminal');
    assert.match(commandPaletteColor(name, false), /^#[a-f0-9]{6}$/i);
  }
});

test('message badges, result cards, and result dialogs share command icons and colors', () => {
  for (const dark of [false, true]) {
    for (const command of ['plan', 'model', 'permissions', 'cron', 'plugin:review']) {
      const item = { type: 'command' as const, id: command, name: `/${command}`, output: 'Done', isError: false };
      const elements = [
        React.createElement(CommandIdentity, { command }),
        React.createElement(CommandOutput, { item, open: true, onSetOpen: () => {} }),
        React.createElement(CommandResultPanel, { item, onClose: () => {} }),
      ];
      for (const element of elements) {
        const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, element));
        assert.ok(html.includes(`data-command-icon="${commandPaletteIcon(command)}"`));
        assert.ok(html.includes(`--command-identity-color:${commandPaletteColor(command, dark)}`));
      }
    }
  }
});

test('command errors keep the command identity as well as an explicit failure state', () => {
  const item = { type: 'command' as const, id: 'failed-model', name: '/model missing', output: 'Unknown model: missing', isError: true };
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) },
    React.createElement(CommandOutput, { item, open: true, onSetOpen: () => {} })));
  assert.match(html, /data-command-icon="box"/);
  assert.match(html, /role="alert"/);
  assert.match(html, /Command failed/);
  assert.match(html, /Unknown model: missing/);
});

test('command messages keep context mentions as links', () => {
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) },
    React.createElement(Stage, { running: false, liveItems: [{ type: 'narration', id: 'mention-command', role: 'user',
      text: '/plan Use [@review](lingxi-mention://skill?name=review&target=review)' }] })));
  assert.match(html, /data-command-icon="bulb"/);
  assert.match(html, /href="lingxi-mention:\/\/skill\?name=review&amp;target=review"/);
});
