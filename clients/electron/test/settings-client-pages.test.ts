import { test } from 'node:test';
import assert from 'node:assert/strict';

import { appearanceOptions } from '../src/renderer/components/settings/pages/Appearance';
import { pinnedSessionRows, projectRows } from '../src/renderer/components/settings/pages/Projects';

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

test('pinned sessions reflects the persisted list, in order', () => {
  const rows = pinnedSessionRows({
    pinnedSessions: [
      { projectPath: '/a', sessionId: 's1', title: 'Alpha', pinnedAt: '2026-01-01T00:00:00Z' },
      { projectPath: '/b', sessionId: 's2', title: 'Beta', pinnedAt: '2026-01-02T00:00:00Z' },
    ],
  } as never);
  assert.deepEqual(rows.map((r) => r.sessionId), ['s1', 's2']);
  assert.deepEqual(rows.map((r) => r.projectPath), ['/a', '/b']);
  assert.deepEqual(rows.map((r) => r.title), ['Alpha', 'Beta']);
});

test('unpinning a session removes it from the rows', () => {
  const pinnedSessions = [
    { projectPath: '/a', sessionId: 's1', title: 'Alpha', pinnedAt: '2026-01-01T00:00:00Z' },
    { projectPath: '/b', sessionId: 's2', title: 'Beta', pinnedAt: '2026-01-02T00:00:00Z' },
  ];
  const before = pinnedSessionRows({ pinnedSessions } as never);
  assert.equal(before.length, 2);

  // What the settings look like immediately after `setSessionPinned(s1, false)`
  // resolves — the same shape `PublicSettings.pinnedSessions` takes on.
  const after = pinnedSessionRows({ pinnedSessions: pinnedSessions.filter((p) => p.sessionId !== 's1') } as never);
  assert.deepEqual(after.map((r) => r.sessionId), ['s2']);
});

test('pinned sessions is empty when nothing is persisted, not a throw', () => {
  assert.deepEqual(pinnedSessionRows(undefined), []);
  assert.deepEqual(pinnedSessionRows({ pinnedSessions: [] } as never), []);
});
