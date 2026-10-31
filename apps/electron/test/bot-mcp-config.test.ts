import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, existsSync, readFileSync, readdirSync, statSync, symlinkSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { ensureRouterEntry, routerServerEntry } from '../src/main/bots/mcpConfig.js';
import { botFixture } from './bot-helpers.js';

function setup(t: { after(fn: () => void): void }) {
  const f = botFixture(t);
  const dir = f.folder('config');
  const path = join(dir, 'config.json');
  return { ...f, dir, path, read: () => JSON.parse(readFileSync(path, 'utf8')) };
}

test('the entry holds no secret: it names the variables the session\'s environment fills in', () => {
  assert.deepEqual(routerServerEntry(), {
    type: 'http',
    url: '${BOT_ROUTER_URL}',
    headers: { Authorization: 'Bearer ${BOT_ROUTER_TOKEN}' },
  });
});

test('a configuration that does not exist is made with the entry alone', (t) => {
  const s = setup(t);
  ensureRouterEntry(s.path);
  assert.deepEqual(s.read(), { mcpServers: { bots: routerServerEntry() } });
  assert.equal(statSync(s.path).mode & 0o777, 0o600);
  assert.deepEqual(readdirSync(s.dir), ['config.json'], 'no temporary file is left');
});

test('every other key of the file, and every other server, stays as it was', (t) => {
  const s = setup(t);
  const existing = { numStartups: 4, projects: { '/x': { allowedTools: ['Bash'] } }, mcpServers: { other: { command: 'node', args: ['server.js'] } }, theme: 'dark' };
  writeFileSync(s.path, JSON.stringify(existing));
  ensureRouterEntry(s.path);
  assert.deepEqual(s.read(), { ...existing, mcpServers: { other: existing.mcpServers.other, bots: routerServerEntry() } });
  assert.deepEqual(Object.keys(s.read()), ['numStartups', 'projects', 'mcpServers', 'theme'], 'the order of the keys is kept');
});

test('an entry that is wrong is put right, and an empty file is a file with nothing in it', (t) => {
  const s = setup(t);
  writeFileSync(s.path, JSON.stringify({ mcpServers: { bots: { type: 'http', url: 'http://evil.example/mcp' }, keep: {} } }));
  ensureRouterEntry(s.path);
  assert.deepEqual(s.read(), { mcpServers: { bots: routerServerEntry(), keep: {} } });
  writeFileSync(s.path, '  \n');
  ensureRouterEntry(s.path);
  assert.deepEqual(s.read(), { mcpServers: { bots: routerServerEntry() } });
});

test('a file that already has the entry is not written again', (t) => {
  const s = setup(t);
  ensureRouterEntry(s.path);
  const before = statSync(s.path);
  utimesSync(s.path, 1_700_000_000, 1_700_000_000);
  ensureRouterEntry(s.path);
  ensureRouterEntry(s.path);
  assert.equal(statSync(s.path).mtimeMs, 1_700_000_000_000, 'untouched');
  assert.equal(statSync(s.path).ino, before.ino, 'and not replaced');
  // Pretty-printing, key order and a read-only file do not matter to a file that is right.
  writeFileSync(s.path, JSON.stringify({ mcpServers: { bots: routerServerEntry() } }));
  chmodSync(s.path, 0o400);
  ensureRouterEntry(s.path);
  assert.equal(statSync(s.path).mode & 0o777, 0o400);
});

test('a configuration that is a link is not followed or replaced', (t) => {
  const s = setup(t);
  const target = join(s.dir, 'target.json');
  writeFileSync(target, '{"a":1}');
  symlinkSync(target, s.path);
  assert.throws(() => ensureRouterEntry(s.path), /is a symbolic link/);
  assert.equal(readFileSync(target, 'utf8'), '{"a":1}');
  const dangling = join(s.dir, 'dangling.json');
  symlinkSync(join(s.dir, 'nowhere.json'), dangling);
  assert.throws(() => ensureRouterEntry(dangling), /is a symbolic link/);
  assert.equal(existsSync(join(s.dir, 'nowhere.json')), false);
});

test('a configuration that is not an object with an object of servers is refused and left as it is', (t) => {
  const s = setup(t);
  for (const [text, message] of [
    ['{broken', /is not valid JSON/],
    ['[]', /is not a JSON object/],
    ['"text"', /is not a JSON object/],
    ['null', /is not a JSON object/],
    ['{"mcpServers": []}', /mcpServers must be an object/],
    ['{"mcpServers": "x"}', /mcpServers must be an object/],
  ] as Array<[string, RegExp]>) {
    writeFileSync(s.path, text);
    assert.throws(() => ensureRouterEntry(s.path), message, text);
    assert.equal(readFileSync(s.path, 'utf8'), text);
  }
});
