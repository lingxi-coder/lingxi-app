/**
 * Tests for the ONE client-side tool-view fallback (`src/toolview.ts`).
 *
 * These pin the DEGRADED path only — what a client shows when an older engine
 * sends no `header`/`display`. The non-degraded path is the engine's derivation
 * and is pinned in Rust; nothing here should grow into a second implementation
 * of it.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  composeToolTitle,
  fallbackToolBody,
  fallbackToolHeader,
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

test('fallbackToolHeader derives verb, label and primary for table tools', () => {
  const edit = fallbackToolHeader('Edit', '{"file_path":"src/host.rs"}');
  assert.equal(edit.verb, 'update');
  assert.equal(edit.label, 'Update');
  assert.equal(edit.primary, 'src/host.rs');
  assert.equal(edit.title, 'Update(src/host.rs)');

  const write = fallbackToolHeader('Write', '{"file_path":"new.rs"}');
  assert.equal(write.verb, 'create');
  assert.equal(write.title, 'Write(new.rs)');

  const grep = fallbackToolHeader('Grep', '{"pattern":"fn main","path":"src"}');
  assert.equal(grep.verb, 'search');
  assert.equal(grep.title, 'Search(fn main)');
});

test('fallbackToolHeader gives Bash its shell label and $ sub-line, newlines collapsed', () => {
  const header = fallbackToolHeader('Bash', JSON.stringify({ command: 'cargo test\n  --all' }));
  assert.equal(header.verb, 'shell');
  assert.equal(header.label, 'Running 1 shell command…');
  assert.equal(header.primary, undefined);
  assert.deepEqual(header.sub_line, { prefix: '$', text: 'cargo test --all' });
});

test('fallbackToolHeader names an MCP call by its tool with a server qualifier', () => {
  const direct = fallbackToolHeader('mcp__github__create_issue', '{"name":"Bug"}');
  assert.equal(direct.label, 'create_issue');
  assert.equal(direct.qualifier, ' (github MCP)');
  assert.equal(direct.title, 'create_issue(Bug) (github MCP)');

  // The generic dispatcher carries the namespaced name in the INPUT.
  const dispatched = fallbackToolHeader('MCP', '{"full_name":"mcp__linear__list_issues"}');
  assert.equal(dispatched.label, 'list_issues');
  assert.equal(dispatched.title, 'list_issues (linear MCP)');
});

test('an unknown tool keeps its own name and probes the generic key order', () => {
  const header = fallbackToolHeader('SomeFutureTool', '{"query":"q","file_path":"f.rs"}');
  assert.equal(header.verb, 'generic');
  assert.equal(header.label, 'SomeFutureTool');
  // file_path outranks query — the Rust GENERIC_PRIMARY_KEYS order.
  assert.equal(header.title, 'SomeFutureTool(f.rs)');
});

test('a subagent type becomes the Task label, except the two generic types', () => {
  assert.equal(fallbackToolHeader('Task', '{"subagent_type":"code-reviewer","description":"review"}').label, 'code-reviewer');
  assert.equal(fallbackToolHeader('Task', '{"subagent_type":"general-purpose","description":"review"}').label, 'Task');
  assert.equal(fallbackToolHeader('Task', '{"subagent_type":"worker","description":"review"}').label, 'Task');
});

test('malformed or empty payloads never throw and never invent a primary', () => {
  const broken = fallbackToolHeader('Read', 'not json at all');
  assert.equal(broken.title, 'Read');
  assert.equal(fallbackToolHeader('Read', '').title, 'Read');
  assert.equal(fallbackToolHeader('Read', '[1,2,3]').title, 'Read');
  assert.equal(fallbackToolHeader('', '{}').label, 'tool');
});

test('toolInputPreview follows one probe order for every caller', () => {
  // The reducer and the permission prompt previously disagreed here: one put
  // file_path first, the other command. There is now exactly one answer.
  assert.equal(toolInputPreview('{"command":"ls","file_path":"a.rs"}'), 'a.rs');
  assert.equal(toolInputPreview('{"command":"ls -al"}'), 'ls -al');
  assert.equal(toolInputPreview('{}'), undefined);
  assert.equal(toolInputPreview('nonsense'), undefined);
});

test('a tool named after an Object.prototype member never throws', () => {
  // `Object.freeze({…})` keeps the prototype, so `PRIMARY_KEYS[name]` used to
  // resolve to an inherited FUNCTION and `for (const key of keys)` threw
  // "keys is not iterable" — inside a React setState updater, from a module
  // that promises it never throws.
  for (const name of ['constructor', 'toString', 'valueOf', 'hasOwnProperty', '__proto__']) {
    const header = fallbackToolHeader(name, '{"file_path":"src/host.rs"}');
    assert.equal(header.verb, 'generic', `${name} should fall through to the generic verb`);
    assert.equal(header.label, name);
    // It falls through to GENERIC_PRIMARY_KEYS like any other unknown tool.
    assert.equal(header.title, `${name}(src/host.rs)`);
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

test('fallbackToolBody unwraps a JSON string, pretty-prints objects and clamps', () => {
  assert.equal(fallbackToolBody('"plain text"'), 'plain text');
  assert.equal(fallbackToolBody('{"a":1}'), '{\n  "a": 1\n}');
  assert.equal(fallbackToolBody('raw non-json'), 'raw non-json');
  assert.equal(fallbackToolBody(''), undefined);
  assert.equal(fallbackToolBody('"   "'), undefined);

  const huge = JSON.stringify('y'.repeat(5_000));
  const body = fallbackToolBody(huge);
  assert.equal(body?.length, 2_000);
  assert.ok(body?.endsWith('…'));
});

test('fallbackToolBody redacts secrets in tool output', () => {
  assert.doesNotMatch(fallbackToolBody('"token=super-secret-value"') ?? '', /super-secret-value/);
});
