import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { DragDropProvider } from '@dnd-kit/react';

import { SessionRow } from '../src/renderer/components/BetaDesktop';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

/** WCAG relative luminance of `#rrggbb`. */
function luminance(hex: string): number {
  const channels = [1, 3, 5].map((index) => {
    const value = Number.parseInt(hex.slice(index, index + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
}

function contrast(foreground: string, background: string): number {
  const [light, dark] = [luminance(foreground), luminance(background)].sort((left, right) => right - left);
  return (light! + 0.05) / (dark! + 0.05);
}

test('secondary text on a selected row reaches 4.5:1 in both palettes, which the ordinary secondary text does not', () => {
  for (const dark of [true, false]) {
    const t = tokens(dark);
    assert.ok(contrast(t.text3OnActive, t.surfaceActive) >= 4.5, `${dark ? 'dark' : 'light'} ${contrast(t.text3OnActive, t.surfaceActive).toFixed(2)}`);
    assert.ok(contrast(t.text4, t.surfaceActive) < 4.5, `${dark ? 'dark' : 'light'}: the token exists because text4 falls short`);
  }
  assert.equal(tokens(false).text3OnActive, '#636366');
  assert.equal(tokens(true).text3OnActive, '#b0b0b5');
});

function metadataColor(dark: boolean, active: boolean): string | undefined {
  const row = React.createElement(SessionRow, {
    projectPath: '/work/app', session: { uuid: 's1', title: 'Plan', modified_rfc3339: new Date().toISOString(), message_count: 2 },
    active, pinned: false, opening: false, onClick: () => undefined,
  });
  const error = console.error;
  console.error = () => undefined;
  try {
    const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, React.createElement(DragDropProvider, null, row)));
    return /class="sidebar-session-metadata" style="[^"]*color:(#[0-9a-f]{6})/.exec(html)?.[1];
  } finally {
    console.error = error;
  }
}

test('only a selected row\'s secondary text takes the contrast color, in either palette', () => {
  for (const dark of [true, false]) {
    const t = tokens(dark);
    assert.equal(metadataColor(dark, true), t.text3OnActive, `${dark ? 'dark' : 'light'} selected`);
    assert.equal(metadataColor(dark, false), t.text4, `${dark ? 'dark' : 'light'} not selected`);
  }
});
