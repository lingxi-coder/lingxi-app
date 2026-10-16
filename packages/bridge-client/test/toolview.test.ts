/**
 * Tests for tool labels and input detail shown on permission surfaces, plus the
 * minimal header used when a result arrives without its start event.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  composeToolTitle,
  orphanToolHeader,
  redactSensitiveText,
  toolInputDetail,
  toolInputPreview,
  VERB_LABEL,
} from '../src/toolview.js';

test('composeToolTitle reproduces the Rust ToolHeader::title() shape', () => {
  assert.equal(composeToolTitle('Update', 'src/host.rs'), 'Update(src/host.rs)');
  assert.equal(composeToolTitle('Update', 'a.rs', ' (3 edits)'), 'Update(a.rs) (3 edits)');
  assert.equal(composeToolTitle('Update Todos'), 'Update Todos');
});

test('the verb table matches the engine english() labels', () => {
  assert.equal(VERB_LABEL.update, 'Update');
  // Rust maps Create → "Write", not "Create". Guard the surprising one.
  assert.equal(VERB_LABEL.create, 'Write');
  assert.equal(VERB_LABEL.todo, 'Update Todos');
  assert.equal(VERB_LABEL.generic, null);
});

test('orphanToolHeader uses only the tool name when a result has no paired start', () => {
  assert.deepEqual(orphanToolHeader('Read'), {
    verb: 'read', label: 'Read', title: 'Read',
  });
  assert.deepEqual(orphanToolHeader('Bash'), {
    verb: 'shell', label: 'Running shell command', title: 'Running shell command',
  });
  assert.deepEqual(orphanToolHeader(''), {
    verb: 'generic', label: 'tool', title: 'tool',
  });
});

test('toolInputPreview follows one probe order for every caller', () => {
  // The reducer and the permission prompt previously disagreed here: one put
  // file_path first, the other command. There is now exactly one answer.
  assert.equal(toolInputPreview('{"command":"ls","file_path":"a.rs"}'), 'a.rs');
  assert.equal(toolInputPreview('{"command":"ls -al"}'), 'ls -al');
  assert.equal(toolInputPreview('{}'), undefined);
  assert.equal(toolInputPreview('nonsense'), undefined);
});

test('an orphan tool named after an Object.prototype member remains literal', () => {
  for (const name of ['constructor', 'toString', 'valueOf', 'hasOwnProperty', '__proto__']) {
    const header = orphanToolHeader(name);
    assert.equal(header.verb, 'generic', `${name} should fall through to the generic verb`);
    assert.equal(header.label, name);
    assert.equal(header.title, name);
  }
});

test('the lookup tables carry no inherited members', () => {
  const verbs = VERB_LABEL as unknown as Record<string, unknown>;
  assert.equal(verbs['constructor'], undefined);
  assert.equal(verbs['toString'], undefined);
  assert.equal(verbs['valueOf'], undefined);
  assert.equal(Object.getPrototypeOf(VERB_LABEL), null);
});

test('previews clamp long values and redact credential shapes', () => {
  const long = 'x'.repeat(400);
  const preview = toolInputPreview(JSON.stringify({ command: long }));
  assert.equal(preview?.length, 160);
  assert.ok(preview?.endsWith('…'));

  const secret = toolInputPreview(JSON.stringify({ command: 'curl -H "Authorization: Bearer sk-ant-abcdefghijklmnop"' }));
  assert.doesNotMatch(secret ?? '', /sk-ant-abcdefghijklmnop/);
});

test('toolInputDetail keeps the WHOLE command — a permission dialog may not truncate', () => {
  // A permission prompt is a security decision. The preview exists for the
  // transcript's one-line header; showing it in the dialog hid every line but
  // the first of a script the user was being asked to approve.
  const script = Array.from({ length: 40 }, (_, i) => `line ${i} of the script`).join('\n');
  const json = JSON.stringify({ command: script });
  assert.equal(toolInputDetail(json), script);
  assert.equal(toolInputDetail(json)?.split('\n').length, 40);
  // The preview really does drop all of it — that is why the two differ.
  assert.equal(toolInputPreview(json)?.includes('\n'), false);
  assert.ok((toolInputPreview(json)?.length ?? 0) <= 160);

  const long = 'x'.repeat(400);
  assert.equal(toolInputDetail(JSON.stringify({ command: long })), long);
});

test('toolInputDetail redacts, and shares the preview probe order', () => {
  assert.doesNotMatch(
    toolInputDetail(JSON.stringify({ command: 'curl -H "Authorization: Bearer sk-ant-abcdefghijklmnop"' })) ?? '',
    /sk-ant-abcdefghijklmnop/,
  );
  // Same table, same order, same "nothing recognizable" answer as the preview.
  assert.equal(toolInputDetail('{"command":"ls","file_path":"a.rs"}'), 'a.rs');
  assert.equal(toolInputDetail('{}'), undefined);
  assert.equal(toolInputDetail('nonsense'), undefined);
});

test('redactSensitiveText masks keys, bearer tokens and inline assignments', () => {
  assert.match(redactSensitiveText('sk-ant-abcdefghijklmn'), /\[REDACTED\]/);
  assert.equal(redactSensitiveText('Bearer abc.def-ghi='), 'Bearer [REDACTED]');
  assert.equal(redactSensitiveText('token=hunter2'), 'token=[REDACTED]');
});
