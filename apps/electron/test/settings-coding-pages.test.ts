import { LANGUAGE_STORAGE_KEY } from '../src/renderer/i18n/locale';

// Behavior assertions use an explicit interface language across developer machines.
Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? 'en' : null } });
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
import { decidePublicMarketplaceSourceAction, findConflictingPublicMarketplace, findRegisteredPublicMarketplace, pluginDependencyCaveat } from '../src/renderer/components/settings/pages/Plugins';
import { decidePublicMcpDirectoryAction, matchesPublicMcpDirectoryEntry, mcpRuntimeStatusLabel } from '../src/renderer/components/settings/pages/McpServers';
import { publicExtensionItems } from '../src/shared/publicExtensionDirectory';
import { adminRecordCommand } from '../src/renderer/components/settings/pages/configurationAdmin';
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
  // `assert.equal(sent.type, 'update_permission_rules')` is the whole claim:
  // a second `assert.notEqual(sent.type, 'update_settings')` used to sit here
  // and could never fail independently of it.
  assert.equal(
    sent.type, 'update_permission_rules',
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
// Hooks: read-only, with a configuration-file shortcut.
// ---------------------------------------------------------------------------

test('hooks expose the configuration-file shortcut while the page itself is editable', () => {
  const page = hooksPageModel({ hooks: { PreToolUse: [] } } as never);
  assert.equal(page.editable, true);
  assert.equal(page.escapeHatch, 'diagnostics');
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

test('parseJsonObjectInput names what kind of config failed, when a caller says so', () => {
  // The interface language controls the sentence; supplied labels remain intact.
  const generic = parseJsonObjectInput('[1,2,3]');
  assert.ok('error' in generic);
  assert.match((generic as { error: string }).error, /^Configuration must be a JSON object/);

  const server = parseJsonObjectInput('[1,2,3]', '服务器配置');
  assert.ok('error' in server);
  assert.match((server as { error: string }).error, /^服务器配置 must be a JSON object/);

  const plugin = parseJsonObjectInput('[1,2,3]', '插件配置');
  assert.match((plugin as { error: string }).error, /^插件配置 must be a JSON object/);
});

test('public MCP configured state matches both the catalog name and endpoint identity', () => {
  const devtools = publicExtensionItems.find((item) => item.id === 'chrome-devtools');
  const context7 = publicExtensionItems.find((item) => item.id === 'context7');
  assert.ok(devtools?.mcp);
  assert.ok(context7?.mcp);
  assert.equal(matchesPublicMcpDirectoryEntry({ name: 'chrome-devtools', config: devtools.mcp.config }, devtools), true);
  assert.equal(matchesPublicMcpDirectoryEntry({ name: 'chrome-devtools', config: { command: 'other-command', args: devtools.mcp.config.args } }, devtools), false);
  assert.equal(matchesPublicMcpDirectoryEntry({ name: 'chrome-devtools', config: { ...devtools.mcp.config, url: 'https://different.example/mcp' } }, devtools), false);
  assert.equal(matchesPublicMcpDirectoryEntry({ name: 'context7', config: { ...context7.mcp.config, url: 'https://other.example/mcp' } }, context7), false);
  assert.equal(matchesPublicMcpDirectoryEntry({ name: 'renamed', config: devtools.mcp.config }, devtools), false);

  const sameNameDifferentEndpoint = { id: 'custom-devtools', scope: 'user' as const, name: 'chrome-devtools', config: { command: 'other-command' }, path: '/user', revision_sha256: 'r' };
  assert.equal(decidePublicMcpDirectoryAction(devtools, [sameNameDifferentEndpoint], 'user').kind, 'configure');
  const matchingPreset = { ...sameNameDifferentEndpoint, config: devtools.mcp.config };
  assert.equal(decidePublicMcpDirectoryAction(devtools, [matchingPreset], 'user').kind, 'configured');
});

test('MCP runtime status labels preserve disconnected and error DTOs without an available fallback', () => {
  assert.equal(mcpRuntimeStatusLabel('connected'), 'Connected');
  assert.equal(mcpRuntimeStatusLabel({ type: 'connected' }), 'Connected');
  assert.equal(mcpRuntimeStatusLabel({ type: 'disconnected' }), 'Disconnected');
  assert.equal(mcpRuntimeStatusLabel({ type: 'error', reason: 'connection refused' }), 'Connection error: connection refused');
  assert.equal(mcpRuntimeStatusLabel({ type: 'unrecognized', reason: 'future status' }), null);
});

test('a curated marketplace is considered registered only when both its name and source match', () => {
  const curated = publicExtensionItems.find((item) => item.id === 'ora-webmcp');
  assert.ok(curated?.marketplace);
  assert.equal(findRegisteredPublicMarketplace(curated.marketplace, [
    { name: 'ora', source_json: JSON.stringify({ source: 'github', repo: 'ora/webmcp' }) },
  ])?.name, 'ora');
  assert.equal(findRegisteredPublicMarketplace(curated.marketplace, [
    { name: 'ora', source_json: JSON.stringify({ source: 'git', url: 'https://github.com/ora/webmcp.git' }) },
  ])?.name, 'ora');
  const wrongSourceRegistry = [
    { name: 'ora', source_json: JSON.stringify({ source: 'github', repo: 'another/webmcp' }) },
  ];
  assert.equal(findRegisteredPublicMarketplace(curated.marketplace, wrongSourceRegistry), undefined);
  assert.equal(findConflictingPublicMarketplace(curated.marketplace, wrongSourceRegistry)?.name, 'ora');
  assert.equal(decidePublicMarketplaceSourceAction(curated.marketplace, wrongSourceRegistry).kind, 'source-conflict');
  assert.equal(findRegisteredPublicMarketplace(curated.marketplace, [
    { name: 'ora', source_json: JSON.stringify({ source: 'url', url: 'https://github.com/ora/webmcp/custom/marketplace.json' }) },
  ]), undefined);
  assert.equal(findRegisteredPublicMarketplace(curated.marketplace, [
    { name: 'ora', source_json: JSON.stringify({ source: 'directory', path: 'ora/webmcp' }) },
  ]), undefined);
  assert.equal(decidePublicMarketplaceSourceAction(curated.marketplace, [
    { name: 'ora', source_json: JSON.stringify({ source: 'directory', path: 'ora/webmcp' }) },
  ]).kind, 'source-conflict');
  assert.equal(findRegisteredPublicMarketplace(curated.marketplace, [
    { name: 'ora', source_json: JSON.stringify({ source: 'github', repo: 'ora/webmcp', ref: 'stable' }) },
  ]), undefined);
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

// ---------------------------------------------------------------------------
// Plugins: Desktop delegates dependency-aware lifecycle mutations to the same
// service used by the CLI and explains the guard in the page.
// ---------------------------------------------------------------------------

test('the plugins page discloses the dependency-aware enable and disable rules', () => {
  const caveat = pluginDependencyCaveat('zh-Hans');
  assert.match(caveat, /递归启用.*依赖/);
  assert.match(caveat, /停用.*会被拒绝/);
});

test('the plugins lifecycle explains its preflight confirmation boundary', () => {
  const caveat = pluginDependencyCaveat('zh-Hans');
  assert.match(caveat, /预检/);
  assert.match(caveat, /确认执行/);
});

test('configuration admin commands contain only protocol fields that were supplied', () => {
  assert.deepEqual(adminRecordCommand('get_catalog'), { action: 'get_catalog' });
  assert.deepEqual(
    adminRecordCommand('save_config', {
      operation_id: 7,
      scope: 'user',
      revision: 'a'.repeat(64),
      payload_json: '{}',
    }),
    {
      action: 'save_config',
      operation_id: 7,
      scope: 'user',
      revision: 'a'.repeat(64),
      payload_json: '{}',
    },
  );
});
