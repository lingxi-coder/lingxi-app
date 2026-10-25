import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { runtimeSpecifiers } from './support/importGraph';

const here = path.dirname(fileURLToPath(import.meta.url));
const remoteDir = path.join(here, '../src/remote');
const sources = readdirSync(remoteDir).filter((name) => /\.tsx?$/u.test(name));

test('the Electron renderer is reached through reuse.ts only', () => {
  const offenders = sources.filter((name) => name !== 'reuse.ts').filter((name) => {
    const text = readFileSync(path.join(remoteDir, name), 'utf8');
    return /from\s+['"]@lingxi\/electron\//u.test(text) || /from\s+['"](?:\.\.\/)+electron\//u.test(text);
  });
  assert.deepEqual(offenders, []);
});

test('reuse.ts takes only logic from the Electron renderer: no component, no styling, no Electron API', () => {
  const specifiers = runtimeSpecifiers('reuse.ts', readFileSync(path.join(remoteDir, 'reuse.ts'), 'utf8'));
  const electron = specifiers.filter((specifier) => specifier.startsWith('@lingxi/electron/'));
  assert.ok(electron.length > 0);
  for (const specifier of electron) {
    assert.ok(!/\/components\/(?!transcriptRows$|transcriptAgentPlacement$)/u.test(specifier), `${specifier} is a component`);
    assert.ok(!/\.(?:tsx|css)$/u.test(specifier), specifier);
    assert.ok(!/\/(?:main|preload)\//u.test(specifier), specifier);
  }
});

test('nothing in the Remote Control sources sets HTML from a string', () => {
  for (const name of sources) {
    const text = readFileSync(path.join(remoteDir, name), 'utf8');
    assert.ok(!/dangerouslySetInnerHTML|\.innerHTML\s*=|insertAdjacentHTML|document\.write/u.test(text), name);
  }
});

test('the viewer never offers the permission mode that skips all prompts', () => {
  const controls = readFileSync(path.join(remoteDir, 'Controls.tsx'), 'utf8');
  assert.ok(!/MODE_LABELS[\s\S]{0,300}bypassPermissions/u.test(controls));
});
