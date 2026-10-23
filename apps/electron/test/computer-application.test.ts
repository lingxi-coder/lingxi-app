import assert from 'node:assert/strict';
import test from 'node:test';
import { computerApplication, groupComputerApplication } from '../src/renderer/components/computerApplication';
import { validBundleId } from '../src/shared/applicationIcon';
import type { ToolRunItem } from '../src/renderer/model/runItem';

const tool = (bundleId?: string): ToolRunItem => ({
  type: 'tool', id: 'computer-1', tool: 'computer', status: 'done',
  view: { verb: 'generic', label: 'computer', title: 'Clicked' },
  nativeOutput: bundleId ? { ok: true, application: { bundle_id: bundleId, name: 'TextEdit' } } : { ok: true },
});

test('computer action uses its persisted application, with no attribution for old or unrelated rows', () => {
  const item = tool('com.apple.TextEdit');
  assert.deepEqual(computerApplication(item), { bundleId: 'com.apple.TextEdit', name: 'TextEdit' });
  assert.equal(computerApplication(tool()), null);
  assert.equal(computerApplication({ ...item, tool: 'Read' }), null);
  assert.equal(computerApplication(tool('/Applications/TextEdit.app')), null);
});

test('opening an identified app can show its icon before the action completes', () => {
  assert.deepEqual(computerApplication({ ...tool(), status: 'running', nativeInput: { action: 'open_application', bundle_id: 'com.apple.TextEdit' } }), { bundleId: 'com.apple.TextEdit', name: 'com.apple.TextEdit' });
  assert.equal(computerApplication({ ...tool(), nativeInput: { action: 'request_access', apps: ['com.apple.TextEdit'] } }), null);
});

test('batch and folded activity retain a single app identity only when unambiguous', () => {
  const textEdit = tool('com.apple.TextEdit');
  const safari = tool('com.apple.Safari');
  const batch = (items: ToolRunItem[]): ToolRunItem => ({ ...tool(), nativeOutput: { results: items.map(item => ({ action: 'left_click', result: item.nativeOutput! })) } });
  assert.equal(computerApplication(batch([textEdit, textEdit]))?.bundleId, 'com.apple.TextEdit');
  assert.equal(computerApplication(batch([textEdit, safari])), null);
  assert.equal(computerApplication(batch([textEdit, tool()])), null);
  assert.equal(groupComputerApplication([textEdit, textEdit]), textEdit);
  assert.equal(groupComputerApplication([textEdit, safari]), undefined);
});

test('native icon lookup accepts only bundle identities, never paths or scripts', () => {
  for (const value of ['', '../TextEdit.app', '/System/Applications/TextEdit.app', 'com.apple.TextEdit\n', 'com.apple; do shell script', null, 42, 'a'.repeat(256)]) assert.equal(validBundleId(value), false);
  assert.equal(validBundleId('com.apple.TextEdit'), true);
});
