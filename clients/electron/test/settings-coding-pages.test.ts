import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  bypassRowProvenance,
  capturePermissionEdit,
  permissionsFromLayer,
} from '../src/renderer/components/settings/pages/Permissions';
import {
  boolFromLayer,
  parseToolList,
  stringArrayFromLayer,
  stringFromLayer,
  stringMapFromLayer,
} from '../src/renderer/components/settings/pages/ToolsAgent';
import { skillsPageModel } from '../src/renderer/components/settings/pages/Skills';
import { parseJsonObjectInput } from '../src/renderer/components/settings/jsonInput';
import { hooksPageModel } from '../src/renderer/components/settings/pages/Hooks';
import { SETTINGS_NAV } from '../src/renderer/components/settings/nav';
import type { SettingsSnapshot } from '../src/renderer/components/settings/useEngineSettings';

function snap(overrides: Partial<SettingsSnapshot> = {}): SettingsSnapshot {
  return {
    files: [],
    effective: {},
    active: {},
    provenance: {},
    locked: [],
    layers: {},
    mergedKeys: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Permissions: the four decisions the brief names, plus the ones the code
// had to choose between two plausible behaviours for.
// ---------------------------------------------------------------------------

test('permission rule edits go through the dedicated command, never the generic patch', () => {
  const sent = capturePermissionEdit({ behavior: 'allow', add: ['Bash(ls:*)'] });
  assert.equal(sent.type, 'update_permission_rules');
  assert.notEqual(
    sent.type, 'update_settings',
    'permissions has a dedicated writer; the generic patch refuses the key anyway',
  );
  assert.deepEqual((sent as { add: string[] }).add, ['Bash(ls:*)']);
  assert.deepEqual((sent as { remove: string[] }).remove, [], 'an unspecified remove list must default to empty, not undefined');
});

test('capturePermissionEdit targets the given destination layer, defaulting to user', () => {
  const withDestination = capturePermissionEdit({ destination: 'project', behavior: 'deny', remove: ['Read'] });
  assert.equal((withDestination as { destination: string }).destination, 'project');
  const withoutDestination = capturePermissionEdit({ behavior: 'ask' });
  assert.equal((withoutDestination as { destination: string }).destination, 'user');
});

test('the bypass acceptance row is device-owned, not layered', () => {
  assert.equal(
    bypassRowProvenance(), 'device',
    'bypassPermissionsModeAccepted lives in the Electron store, so switching layers must not move it',
  );
});

test('permissionsFromLayer reads a layer\'s OWN permissions, never a different layer\'s', () => {
  const snapshot = snap({
    layers: {
      user: { permissions: { allow: ['Bash(ls:*)'], deny: [], ask: [], additionalDirectories: [] } },
      project: { permissions: { allow: [], deny: ['Read(./secrets/**)'], ask: [], additionalDirectories: ['/repo'] } },
    },
  });
  assert.deepEqual(permissionsFromLayer(snapshot, 'user').allow, ['Bash(ls:*)']);
  assert.deepEqual(permissionsFromLayer(snapshot, 'project').allow, []);
  assert.deepEqual(permissionsFromLayer(snapshot, 'project').deny, ['Read(./secrets/**)']);
  assert.deepEqual(permissionsFromLayer(snapshot, 'user').additionalDirectories, []);
  assert.deepEqual(permissionsFromLayer(snapshot, 'project').additionalDirectories, ['/repo']);
});

test('permissionsFromLayer defaults every field for a layer that set nothing', () => {
  const draft = permissionsFromLayer(snap(), 'local');
  assert.deepEqual(draft, { allow: [], deny: [], ask: [], defaultMode: undefined, additionalDirectories: [] });
});

// ---------------------------------------------------------------------------
// ToolsAgent: the same layer-read gate `CustomProviders` was fixed for
// (Task 17 fix round 1), applied to a fresh set of keys.
// ---------------------------------------------------------------------------

test('ToolsAgent reads enabledTools/modelOverrides from the editing layer\'s own map, not effective', () => {
  const snapshot = snap({
    effective: { enabledTools: ['Bash', 'Read', 'Edit'], outputStyle: 'Explanatory' },
    layers: {
      user: { enabledTools: ['Bash'] },
      project: { modelOverrides: { 'gpt-4': 'gpt-4-turbo' } },
    },
  });
  assert.deepEqual(stringArrayFromLayer(snapshot, 'user', 'enabledTools'), ['Bash']);
  assert.deepEqual(stringArrayFromLayer(snapshot, 'project', 'enabledTools'), [], 'project set nothing for this key; effective must not leak in');
  assert.deepEqual(stringMapFromLayer(snapshot, 'project', 'modelOverrides'), { 'gpt-4': 'gpt-4-turbo' });
  assert.deepEqual(stringMapFromLayer(snapshot, 'user', 'modelOverrides'), {});
  assert.equal(stringFromLayer(snapshot, 'user', 'outputStyle'), '', 'outputStyle was only ever written to effective in this fixture, never to any layer');
});

test('boolFromLayer is strict: only a literal true in that layer counts', () => {
  const snapshot = snap({ layers: { user: { alwaysThinkingEnabled: true, disableAllHooks: 'yes' } } });
  assert.equal(boolFromLayer(snapshot, 'user', 'alwaysThinkingEnabled'), true);
  assert.equal(boolFromLayer(snapshot, 'user', 'disableAllHooks'), false, 'a non-boolean value must not be coerced to true');
  assert.equal(boolFromLayer(snapshot, 'project', 'alwaysThinkingEnabled'), false);
});

test('parseToolList splits on commas and newlines and drops blanks', () => {
  assert.deepEqual(parseToolList('Bash, Read,\n Edit'), ['Bash', 'Read', 'Edit']);
  assert.deepEqual(parseToolList(''), []);
});

// ---------------------------------------------------------------------------
// Skills: the list is directory-discovered, not layered.
// ---------------------------------------------------------------------------

test('the skills list is not affected by the layer switcher', () => {
  const model = skillsPageModel({ layer: 'user' } as never);
  const other = skillsPageModel({ layer: 'project' } as never);
  assert.deepEqual(
    model.skills, other.skills,
    'skills are discovered from directories; only syncClaudeAiSkills is layered',
  );
});

test('skillsPageModel carries through whatever skills list it is given, regardless of layer', () => {
  const skills = [{ name: 'greet', source_dir: '/repo/.lingxi/skills/greet' }];
  const asUser = skillsPageModel({ layer: 'user', skills } as never);
  const asProject = skillsPageModel({ layer: 'project', skills } as never);
  assert.deepEqual(asUser.skills, skills);
  assert.deepEqual(asProject.skills, skills);
  assert.equal(asUser.layerAffects, 'syncClaudeAiSkills only');
});

// ---------------------------------------------------------------------------
// Hooks: read-only, with a raw-JSON escape hatch.
// ---------------------------------------------------------------------------

test('hooks are read-only and point at the raw JSON page', () => {
  const page = hooksPageModel({ hooks: { PreToolUse: [] } } as never);
  assert.equal(page.editable, false);
  assert.equal(page.escapeHatch, 'raw-json');
});

test('hooksPageModel summarizes each event\'s rule count from the EFFECTIVE (merged) value', () => {
  const page = hooksPageModel({
    hooks: {
      PreToolUse: [{ matcher: 'Bash', hooks: [{ type: 'command', command: 'echo hi' }] }],
      PostToolUse: [],
    },
  });
  assert.deepEqual(page.events, [
    { event: 'PreToolUse', count: 1 },
    { event: 'PostToolUse', count: 0 },
  ]);
});

test('hooksPageModel tolerates a missing or malformed hooks key rather than throwing', () => {
  assert.deepEqual(hooksPageModel({}).events, []);
  assert.deepEqual(hooksPageModel({ hooks: 'not-an-object' }).events, []);
  assert.deepEqual(hooksPageModel({ hooks: ['also-not-an-object'] }).events, []);
});

// ---------------------------------------------------------------------------
// McpServers: page-local scope selector, generic JSON-object parsing.
// ---------------------------------------------------------------------------

test('parseJsonObjectInput accepts a JSON object and refuses everything else', () => {
  const ok = parseJsonObjectInput('{"command":"npx","args":["-y","pkg"]}');
  assert.ok('config' in ok);
  assert.deepEqual((ok as { config: unknown }).config, { command: 'npx', args: ['-y', 'pkg'] });

  const notJson = parseJsonObjectInput('{not json');
  assert.ok('error' in notJson);

  const array = parseJsonObjectInput('[1,2,3]');
  assert.ok('error' in array, 'a JSON array is not a valid server/plugin config object');

  const scalar = parseJsonObjectInput('"just a string"');
  assert.ok('error' in scalar);
});

// ---------------------------------------------------------------------------
// Registration: all six pages must be reachable via the nav, and `nav.ts`'s
// own `layered`/`needsEngine` data properties must match what each page
// actually needs (a mismatch here would silently reintroduce the exact bug
// Task 15's own fix-round comment warns about — "layered is a data
// property, not a group property").
// ---------------------------------------------------------------------------

test('all six coding-group pages are declared implemented in nav.ts', () => {
  for (const id of ['permissions', 'tools-agent', 'skills', 'mcp', 'hooks', 'plugins']) {
    const page = SETTINGS_NAV.find((candidate) => candidate.id === id);
    assert.ok(page, `${id} must be declared in SETTINGS_NAV`);
    assert.equal(page?.implemented, true, `${id} must be marked implemented`);
    assert.equal(page?.needsEngine, true, `${id} needs a running engine`);
  }
});

test('mcp is the one 编码-group page that is NOT layered — it owns its own scope storage', () => {
  const mcp = SETTINGS_NAV.find((page) => page.id === 'mcp');
  assert.equal(mcp?.layered, false);
  const others = ['permissions', 'tools-agent', 'skills', 'hooks', 'plugins'];
  for (const id of others) {
    const page = SETTINGS_NAV.find((candidate) => candidate.id === id);
    assert.equal(page?.layered, true, `${id} must be layered`);
  }
});
