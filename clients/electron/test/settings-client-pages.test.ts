import { test } from 'node:test';
import assert from 'node:assert/strict';

import { appearanceOptions } from '../src/renderer/components/settings/pages/Appearance';
import { projectRows } from '../src/renderer/components/settings/pages/Projects';

test('appearance offers exactly dark, light and system, in that order', () => {
  const options = appearanceOptions();
  assert.deepEqual(options.map((o) => o.id), ['system', 'light', 'dark']);
});

test('appearance options have no duplicate ids and every id has a non-empty label', () => {
  const options = appearanceOptions();
  assert.equal(new Set(options.map((o) => o.id)).size, options.length);
  for (const option of options) assert.ok(option.label.length > 0);
});

test('projects lists the persisted projects and marks the active one', () => {
  const rows = projectRows({ projects: ['/a', '/b'], activeProject: '/b' } as never);
  assert.deepEqual(rows.map((r) => r.path), ['/a', '/b']);
  assert.equal(rows.find((r) => r.path === '/b')?.active, true);
  assert.equal(rows.find((r) => r.path === '/a')?.active, false);
});

test('projects with no active project marks none active', () => {
  const rows = projectRows({ projects: ['/a', '/b'] } as never);
  assert.deepEqual(rows.map((r) => r.active), [false, false]);
});

test('projects is empty when there is nothing persisted, not a throw', () => {
  assert.deepEqual(projectRows(undefined), []);
  assert.deepEqual(projectRows({ projects: [] } as never), []);
});
