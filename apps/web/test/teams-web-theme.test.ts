import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// The Teams workspace takes its colors from --teams-* variables. The Web page sets every one of them from the
// site's own tokens (src/teams/teams-web.css), so the workspace follows the site's light/dark switch and not the
// system's. These tests read the two stylesheets and check the resulting pairs, in both palettes.

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//gu, '');

/** The custom properties declared by the rule that starts with `selector {`. */
function declarations(source: string, selector: string): Map<string, string> {
  const start = source.indexOf(`${selector} {`);
  assert.notEqual(start, -1, `${selector} is declared`);
  const block = source.slice(start, source.indexOf('}', start));
  return new Map([...block.matchAll(/(--[\w-]+):\s*([^;]+);/gu)].map(match => [match[1]!, match[2]!.trim()] as const));
}

type Rgb = [number, number, number];
/** A color expression of the stylesheets: `#rrggbb`, `var(--x)` or `color-mix(in srgb, A p%, B)`. */
function resolve(expression: string, scope: Map<string, string>): Rgb {
  const value = expression.trim();
  const hex = /^#([0-9a-f]{6})$/iu.exec(value);
  if (hex) return [0, 2, 4].map(at => parseInt(hex[1]!.slice(at, at + 2), 16)) as Rgb;
  const variable = /^var\((--[\w-]+)\)$/u.exec(value);
  if (variable) {
    const next = scope.get(variable[1]!);
    assert.ok(next, `${variable[1]} is defined`);
    return resolve(next, scope);
  }
  const mix = /^color-mix\(in srgb,\s*(.+?)\s+(\d+(?:\.\d+)?)%,\s*(.+)\)$/u.exec(value);
  if (mix) {
    const [first, weight, second] = [resolve(mix[1]!, scope), Number(mix[2]) / 100, resolve(mix[3]!, scope)];
    return first.map((channel, at) => Math.round(channel * weight + second[at]! * (1 - weight))) as Rgb;
  }
  throw new Error(`cannot evaluate ${value}`);
}
const luminance = (rgb: Rgb) => {
  const [r, g, b] = rgb.map(channel => channel / 255).map(v => v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4) as Rgb;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a: Rgb, b: Rgb) => { const [x, y] = [luminance(a), luminance(b)].sort((m, n) => n - m) as [number, number]; return (x + 0.05) / (y + 0.05); };

const site = read('../src/theme.css'), teams = read('../src/teams/teams-web.css'), workspaceCss = read('../../../packages/teams/workspace.css');
const lightTokens = declarations(site, ':root'), darkTokens = declarations(site, ':root[data-theme="dark"]');
const mapping = declarations(teams, '.teams-web'), darkMapping = declarations(teams, ":root[data-theme='dark'] .teams-web");
const palettes = {
  light: new Map([...lightTokens, ...mapping]),
  dark: new Map([...lightTokens, ...darkTokens, ...mapping, ...darkMapping]),
};

test('the Web page sets every public variable the workspace reads, so none falls back to the system palette', () => {
  const read = [...new Set([...workspaceCss.matchAll(/var\((--teams-[\w-]+),/gu)].map(match => match[1]!))];
  assert.ok(read.length >= 17, `the workspace reads ${read.length} variables`);
  assert.deepEqual(read.filter(name => !mapping.has(name)), []);
});

for (const [name, scope] of Object.entries(palettes)) test(`${name} palette: text meets 4.5:1 and outlines and fills 3:1 against the surfaces they sit on`, () => {
  const color = (variable: string) => resolve(`var(${variable})`, scope);
  const ratio = (foreground: string, background: string) => contrast(color(foreground), color(background));
  const surfaces = ['--teams-bg', '--teams-sidebar-bg', '--teams-surface', '--teams-hover', '--teams-selected'];
  const plain = ['--teams-bg', '--teams-sidebar-bg', '--teams-surface'];
  const check = (foreground: string, against: string[], least: number) => {
    for (const background of against) assert.ok(ratio(foreground, background) >= least, `${foreground} on ${background} is ${ratio(foreground, background).toFixed(2)}, below ${least}`);
  };
  check('--teams-fg', surfaces, 4.5);
  check('--teams-muted', surfaces, 4.5);
  check('--teams-accent-text', surfaces, 4.5);
  check('--teams-danger', plain, 4.5);
  check('--teams-online', plain, 4.5);
  check('--teams-border', plain, 3);
  check('--teams-focus', [...plain, '--teams-selected'], 3);
  check('--teams-accent', plain, 3);
  assert.ok(ratio('--teams-accent-fg', '--teams-accent') >= 4.5, 'the label of a primary button');
  assert.ok(ratio('--teams-badge-fg', '--teams-badge-bg') >= 4.5, 'the count in an unread badge');
  assert.ok(ratio('--teams-badge-bg', '--teams-sidebar-bg') >= 3, 'the badge against the sidebar it sits in');
  // Rows can be told apart: hover and selection are visible against the sidebar, and from each other.
  assert.ok(ratio('--teams-hover', '--teams-sidebar-bg') > 1.05 && ratio('--teams-selected', '--teams-sidebar-bg') > 1.05 && ratio('--teams-selected', '--teams-hover') > 1.02);
});
