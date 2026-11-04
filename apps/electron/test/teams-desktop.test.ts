import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import { AccountSignInContext } from '../src/renderer/bridge/useAccountState';
import { unreadBadgeText } from '../src/renderer/bridge/teamsUnread';
import { teamBotAvatarId } from '../src/renderer/components/TeamAvatar';
import { TeamsSignIn } from '../src/renderer/components/TeamsSignIn';
import { tr } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { mixColors, teamsThemeVariables } from '../src/renderer/theme/teamsTheme';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

test('the rail badge counts up to 99 and says 99+ beyond, and says nothing for no unread messages', () => {
  assert.equal(unreadBadgeText(0), '');
  assert.equal(unreadBadgeText(1), '1');
  assert.equal(unreadBadgeText(9), '9');
  assert.equal(unreadBadgeText(10), '10');
  assert.equal(unreadBadgeText(99), '99');
  assert.equal(unreadBadgeText(100), '99+');
  assert.equal(unreadBadgeText(12_345), '99+');
  assert.equal(unreadBadgeText(Number.POSITIVE_INFINITY), '99+');
});

test('the rail badge never shows what is not a count of messages', () => {
  for (const count of [-1, -100, Number.NaN, Number.NEGATIVE_INFINITY, 0.4, 0.99]) assert.equal(unreadBadgeText(count), '', String(count));
  assert.equal(unreadBadgeText(2.9), '2', 'whole messages only');
});

test('the button the badge sits on is named with the exact count, not the capped one', () => {
  assert.equal(tr('desktop_teams_rail_unread', 3), 'Teams, 3 unread');
  assert.equal(tr('desktop_teams_rail_unread', 250), 'Teams, 250 unread');
  assert.equal(tr('teams_title'), 'Teams', 'and with none unread, by its name alone');
});

test('a Bot has the avatar of its id in the Teams views as in the Bots sidebar, whether the workspace names it by id or by key', () => {
  assert.equal(teamBotAvatarId('bot:owner-uid:host-1:planner'), 'planner');
  assert.equal(teamBotAvatarId('bot:owner-uid:host-1:3f2b8c10-1c0e-4c7f-9d2e-5a1b6e7f8a90'), '3f2b8c10-1c0e-4c7f-9d2e-5a1b6e7f8a90');
  assert.equal(teamBotAvatarId('planner'), 'planner', 'a card id is its own');
  assert.equal(teamBotAvatarId(teamBotAvatarId('bot:u:h:planner')), 'planner', 'and the same Bot is the same either way');
});

// --- theming ---------------------------------------------------------------------------------------------------

const workspaceCss = readFileSync(new URL('../../../packages/teams/workspace.css', import.meta.url), 'utf8');

/** The `--teams-*` variables the stylesheet reads, which are what a host may set. */
const publicVariables = [...new Set([...workspaceCss.matchAll(/var\((--teams-[a-z-]+)/g)].map(match => match[1]!))].sort();

const channel = (value: number) => { const c = value / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
const luminance = (hex: string) => 0.2126 * channel(Number.parseInt(hex.slice(1, 3), 16)) + 0.7152 * channel(Number.parseInt(hex.slice(3, 5), 16)) + 0.0722 * channel(Number.parseInt(hex.slice(5, 7), 16));
const contrast = (a: string, b: string) => { const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number]; return (light + 0.05) / (dark + 0.05); };

test('the Desktop sets every public variable of the Teams stylesheet, and no other', () => {
  assert.ok(publicVariables.length >= 15, `the stylesheet declares its variables (${publicVariables.length})`);
  for (const dark of [false, true]) {
    assert.deepEqual(Object.keys(teamsThemeVariables(tokens(dark))).sort(), publicVariables, dark ? 'dark' : 'light');
  }
});

test('every color of the Teams theme that text, outlines or fills are measured on is a plain #rrggbb; only the hairline is translucent', () => {
  for (const dark of [false, true]) {
    for (const [name, value] of Object.entries(teamsThemeVariables(tokens(dark)))) {
      const label = `${dark ? 'dark' : 'light'} ${name}`;
      if (name === '--teams-font') assert.match(value, /^var\(--/, 'the font is the Desktop\'s');
      else if (name === '--teams-line') assert.match(value, /^(#[0-9a-f]{6}|rgba\([\d., ]+\))$/, `${label}: the Desktop\'s hairline, which only divides`);
      else assert.match(value, /^#[0-9a-f]{6}$/, label);
    }
  }
});

test('mixing colors weighs the first against the second', () => {
  assert.equal(mixColors('#ffffff', '#000000', 1), '#ffffff');
  assert.equal(mixColors('#ffffff', '#000000', 0), '#000000');
  assert.equal(mixColors('#ff0000', '#0000ff', 0.5), '#800080');
  assert.equal(mixColors('#102030', '#000000', 0.5), '#081018');
});

// What the stylesheet promises (workspace.css, "--teams-border outlines controls (3:1 against the surfaces)"; text 4.5:1), held under the
// Desktop's own palettes, which the stylesheet's fallbacks do not cover. Each pair is a color and a surface it is drawn on.
const TEXT_PAIRS: Array<[string, string]> = [
  ['fg', 'bg'], ['fg', 'sidebar-bg'], ['fg', 'surface'], ['fg', 'hover'], ['fg', 'selected'],
  ['muted', 'bg'], ['muted', 'sidebar-bg'], ['muted', 'surface'], ['muted', 'hover'], ['muted', 'selected'],
  ['accent-fg', 'accent'],
  ['accent-text', 'bg'], ['accent-text', 'sidebar-bg'], ['accent-text', 'surface'], ['accent-text', 'hover'], ['accent-text', 'selected'],
  ['danger', 'bg'], ['danger', 'sidebar-bg'], ['danger', 'surface'], ['danger', 'hover'],
  ['badge-fg', 'badge-bg'],
  ['online', 'bg'], ['online', 'surface'],
];
const PART_PAIRS: Array<[string, string]> = [
  ['border', 'bg'], ['border', 'sidebar-bg'], ['border', 'surface'], ['border', 'hover'],
  ['focus', 'bg'], ['focus', 'sidebar-bg'], ['focus', 'surface'], ['focus', 'hover'],
  ['online', 'hover'], ['accent', 'bg'], ['accent', 'surface'],
];

test('under the Desktop palettes text on every Teams surface reads at 4.5:1 and outlines and fills at 3:1, in the light and the dark one', () => {
  for (const dark of [false, true]) {
    const variables = teamsThemeVariables(tokens(dark));
    const color = (name: string) => variables[`--teams-${name}`]!;
    for (const [text, surface] of TEXT_PAIRS) {
      const ratio = contrast(color(text), color(surface));
      assert.ok(ratio >= 4.5, `${dark ? 'dark' : 'light'}: ${text} on ${surface} is ${ratio.toFixed(2)}:1`);
    }
    for (const [part, surface] of PART_PAIRS) {
      const ratio = contrast(color(part), color(surface));
      assert.ok(ratio >= 3, `${dark ? 'dark' : 'light'}: ${part} against ${surface} is ${ratio.toFixed(2)}:1`);
    }
  }
});

test('the sidebar of the Teams sits on the Desktop\'s sidebar panel, and its page on the Desktop\'s stage', () => {
  for (const dark of [false, true]) {
    const palette = tokens(dark);
    const variables = teamsThemeVariables(palette);
    assert.equal(variables['--teams-sidebar-bg'], palette.sidebarPanelBg);
    assert.equal(variables['--teams-bg'], palette.stageBg);
  }
});

// --- before sign-in ----------------------------------------------------------------------------------------------

const render = (element: React.ReactElement, signIn: (() => void) | null = null): string =>
  renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(AccountSignInContext.Provider, { value: signIn }, element)));

test('before sign-in Teams says what to do and offers the way, and without a way it only says it', () => {
  const sidebar = render(React.createElement(TeamsSignIn, { compact: true }), () => undefined);
  assert.match(sidebar, /<h2[^>]*>Teams<\/h2>/);
  assert.match(sidebar, /Sign in to your LingXi account to use Teams\./);
  assert.match(sidebar, /<button[^>]*>Sign In<\/button>/);
  assert.doesNotMatch(render(React.createElement(TeamsSignIn, { compact: true })), /<button/);

  const page = render(React.createElement(TeamsSignIn, {}), () => undefined);
  assert.match(page, /<h2[^>]*>Sign in to use Teams<\/h2>/);
  assert.match(page, /Sign in to your LingXi account to use Teams\./);
});
