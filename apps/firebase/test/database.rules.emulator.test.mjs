import assert from 'node:assert/strict';
import test from 'node:test';
import { randomBytes, randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';

// Realtime Database rules for /remote-control, exercised through the emulator's REST API (firebase-tools 15 needs Java 21+):
//   firebase emulators:exec --only database --project demo-lingxi-backend "node --test test/database.rules.emulator.test.mjs"
const emulator = process.env.FIREBASE_DATABASE_EMULATOR_HOST, opts = { skip: !emulator };
const rules = () => readFileSync(new URL('../../../infra/firebase/database.rules.json', import.meta.url), 'utf8');
const b64u = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
// The emulator trusts the claims of an unsigned ID token (alg none, empty signature).
const idToken = (uid) => {
  const iat = Math.floor(Date.now() / 1000);
  return [b64u({ alg: 'none', typ: 'JWT' }), b64u({ sub: uid, user_id: uid, aud: 'demo-lingxi-backend',
    iss: 'https://securetoken.google.com/demo-lingxi-backend', iat, exp: iat + 3600, auth_time: iat, firebase: { sign_in_provider: 'password' } }), ''].join('.');
};

let ready;
const namespace = `demo-lingxi-backend-rules-${randomUUID()}`;
// `as`: a uid (ID token in ?auth=), 'owner' (the emulator's admin bypass), or nothing (unauthenticated);
// `token` sends that string as the token; `header` moves the ID token into an Authorization header.
async function rest(method, path, { as, body, raw, header, token } = {}) {
  ready ??= fetch(`http://${emulator}/.settings/rules.json?ns=${namespace}`, { method: 'PUT', headers: { Authorization: 'Bearer owner' }, body: rules() })
    .then((response) => assert.equal(response.status, 200, 'the emulator accepts the rules file'));
  await ready;
  const query = new URLSearchParams({ ns: namespace });
  if (token) query.set('auth', token);
  else if (as && as !== 'owner' && !header) query.set('auth', idToken(as));
  const response = await fetch(`http://${emulator}/${path}.json?${query}`, { method,
    headers: as === 'owner' ? { Authorization: 'Bearer owner' } : as && header ? { Authorization: `Bearer ${idToken(as)}` } : {},
    ...(body === undefined ? {} : { body: raw ? body : JSON.stringify(body) }) });
  const text = await response.text();
  return { status: response.status, body: text ? JSON.parse(text) : undefined };
}

const SV = { '.sv': 'timestamp' };
const bytes = (n) => randomBytes(n).toString('base64url'); // 16 bytes = 22 characters, 32 = 43, 64 = 86
const p = (uid, ...parts) => ['remote-control', uid, ...parts].join('/');
const hostRecord = () => ({ v: 1, name: 'Studio Mac', platform: 'darwin', app_version: '0.1.0', client_protocol_version: 29,
  host_key: bytes(32), online: true, last_seen: SV, started_at: SV });
const offerRecord = () => ({ v: 1, transport: 'webrtc', sdp: 'v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\n',
  device: { id: bytes(16), key: bytes(32), name: '小米 14', platform: 'android', app_version: '0.1.0' },
  eph: bytes(32), nonce: bytes(16), created_at: SV, sig: bytes(64) });
const answerRecord = () => ({ v: 1, status: 'accepted', transport: 'webrtc', sdp: 'v=0\r\n', host: { id: bytes(16), key: bytes(32) },
  eph: bytes(32), nonce: bytes(16), created_at: SV, sig: bytes(64) });
// A copy of `record` with the value at a dotted path replaced (undefined removes it).
const set = (record, path, value) => {
  const copy = structuredClone(record), keys = path.split('.'), last = keys.pop(), parent = keys.reduce((o, k) => o[k], copy);
  if (value === undefined) delete parent[last]; else parent[last] = value;
  return copy;
};
const DENIED = { error: 'Permission denied' };
// Every rule failure, `.validate` included, is HTTP 401 with this body.
const denied = (response, label) => assert.deepEqual([response.status, response.body], [401, DENIED], label);
const ok = (response, label) => assert.equal(response.status, 200, `${label}: ${JSON.stringify(response.body)}`);
// A write the rules must refuse, and that must leave nothing behind.
async function refuses(uid, method, path, body, label) {
  const before = await rest('GET', path, { as: 'owner' });
  denied(await rest(method, path, { as: uid, body }), label);
  assert.deepEqual((await rest('GET', path, { as: 'owner' })).body, before.body, `${label}: unchanged`);
}

test('rules: the owner can write and read every path of the layout', opts, async () => {
  const uid = `u-${randomUUID()}`, host = bytes(16), conn = bytes(16), tunnel = bytes(16);
  const write = async (path, body, method = 'PUT') => ok(await rest(method, p(uid, path), { as: uid, body }), `${method} ${path}`);
  await write(`hosts/${host}`, hostRecord());
  await write(`hosts/${host}`, { last_seen: SV }, 'PATCH'); // heartbeat
  await write(`hosts/${host}`, { online: false }, 'PATCH');
  await write(`hosts/${host}`, { online: true, name: 'Renamed', started_at: SV }, 'PATCH');
  await write(`to_host/${host}/${conn}/offer`, offerRecord());
  await write(`to_host/${host}/${tunnel}/offer`, set(set(offerRecord(), 'transport', 'tunnel'), 'sdp', undefined));
  await write(`to_host/${host}/${conn}/up/f0`, bytes(64));
  await write(`to_host/${host}/${conn}/up`, { f1: bytes(8), f2: bytes(8), f4294967295: bytes(8), f9999999999: bytes(8) }, 'PATCH');
  await write(`to_viewer/${host}/${conn}/answer`, answerRecord());
  await write(`to_viewer/${host}/${tunnel}/answer`, set(set(answerRecord(), 'transport', 'tunnel'), 'sdp', undefined));
  for (const reason of ['revoked', 'disabled', 'capacity', 'upgrade_required', 'bad_signature', 'expired', 'busy']) {
    await write(`to_viewer/${host}/${bytes(16)}/answer`, { v: 1, status: 'rejected', reason, host: { id: host, key: bytes(32) }, nonce: bytes(16), created_at: SV, sig: bytes(64) });
  }
  await write(`to_viewer/${host}/${conn}/down/f0`, bytes(64));
  await write(`to_viewer/${host}/${conn}/down`, { f1: bytes(8), f2: bytes(8) }, 'PATCH');
  for (const [path, check] of [
    [`hosts/${host}`, (v) => v.name === 'Renamed' && v.online === true && typeof v.last_seen === 'number' && v.host_key.length === 43],
    [`hosts`, (v) => Object.keys(v).join() === host],
    [`to_host/${host}/${conn}/offer`, (v) => v.transport === 'webrtc' && v.device.name === '小米 14' && typeof v.created_at === 'number'],
    [`to_host/${host}/${tunnel}/offer`, (v) => v.transport === 'tunnel' && v.sdp === undefined],
    [`to_host/${host}/${conn}/up`, (v) => !Array.isArray(v) && Object.keys(v).length === 5],
    [`to_host/${host}`, (v) => Object.keys(v).length === 2],
    [`to_viewer/${host}/${conn}/answer`, (v) => v.status === 'accepted' && v.host.key.length === 43],
    [`to_viewer/${host}/${conn}/down`, (v) => !Array.isArray(v) && Object.keys(v).length === 3],
    [`to_viewer/${host}`, (v) => Object.keys(v).length === 9],
    [``, (v) => ['hosts', 'to_host', 'to_viewer'].every((k) => k in v)],
  ]) {
    const response = await rest('GET', path ? p(uid, path) : p(uid), { as: uid });
    ok(response, `GET ${path}`);
    assert.ok(check(response.body), `GET ${path}: ${JSON.stringify(response.body).slice(0, 200)}`);
  }
  // Unsigned tokens are also accepted as a bearer header.
  ok(await rest('GET', p(uid, 'hosts'), { as: uid, header: true }), 'Authorization: Bearer <id token>');
});

test('rules: another uid can neither read nor write, and neither can an unauthenticated caller', opts, async () => {
  const owner = `u-${randomUUID()}`, other = `${owner}x`, host = bytes(16), conn = bytes(16);
  const prefix = owner.slice(0, -1);
  ok(await rest('PUT', p(owner, 'hosts', host), { as: owner, body: hostRecord() }), 'seed host');
  ok(await rest('PUT', p(owner, 'to_host', host, conn, 'offer'), { as: owner, body: offerRecord() }), 'seed offer');
  ok(await rest('PUT', p(owner, 'to_viewer', host, conn, 'answer'), { as: owner, body: answerRecord() }), 'seed answer');
  const before = (await rest('GET', p(owner), { as: 'owner' })).body;
  const paths = [p(owner), p(owner, 'hosts'), p(owner, 'hosts', host), p(owner, 'hosts', host, 'host_key'), p(owner, 'to_host', host),
    p(owner, 'to_host', host, conn, 'offer'), p(owner, 'to_viewer', host, conn, 'answer'), p(owner, 'to_viewer', host, conn, 'down', 'f0')];
  for (const [who, as] of [['another uid', other], ['a uid that is a prefix of the owner\'s', prefix], ['an unauthenticated caller', undefined]]) {
    for (const path of paths) {
      denied(await rest('GET', path, { as }), `${who} reads ${path}`);
      denied(await rest('PUT', path, { as, body: { v: 1 } }), `${who} writes ${path}`);
      denied(await rest('PATCH', path, { as, body: { name: 'x' } }), `${who} patches ${path}`);
      denied(await rest('DELETE', path, { as }), `${who} deletes ${path}`);
    }
    denied(await rest('PUT', p(owner, 'to_host', host, bytes(16), 'up', 'f0'), { as, body: bytes(8) }), `${who} injects a frame`);
  }
  assert.equal((await rest('GET', p(owner, 'hosts'), { token: 'not-a-jwt' })).status, 401, 'a malformed token');
  assert.deepEqual((await rest('GET', p(owner), { as: 'owner' })).body, before, 'nothing changed');
  // The denial is about the uid, not a general failure: each of them owns their own subtree.
  ok(await rest('PUT', p(other, 'hosts', host), { as: other, body: hostRecord() }), 'another uid writes its own host');
  ok(await rest('GET', p(other, 'hosts'), { as: other }), 'another uid reads its own hosts');
  // The root and the shared parent stay closed, even to a signed-in owner.
  for (const path of ['', 'remote-control', 'other-root-key']) {
    denied(await rest('GET', path, { as: owner }), `reads /${path}`);
    denied(await rest('PUT', path, { as: owner, body: { [owner]: { hosts: { [host]: hostRecord() } } } }), `writes /${path}`);
    denied(await rest('GET', path), `unauthenticated reads /${path}`);
  }
});

test('rules: invalid shapes are rejected and leave nothing behind', opts, async () => {
  const uid = `u-${randomUUID()}`, host = bytes(16), conn = bytes(16), offer = offerRecord(), answer = answerRecord();
  const offerPath = p(uid, 'to_host', host, conn, 'offer'), answerPath = p(uid, 'to_viewer', host, conn, 'answer');
  const bad = async (path, body, label, method = 'PUT') => refuses(uid, method, path, body, label);
  // Tunnel frames: key f{seq}, a string of at most 65536 characters, in the right direction.
  for (const [parent, dir, wrong] of [['to_host', 'up', 'down'], ['to_viewer', 'down', 'up']]) {
    const frames = p(uid, parent, host, conn, dir);
    for (const key of ['0', '7', 'f', 'F1', 'x1', 'f1x', 'f12345678901', 'f-1', 'frame0', '-1', 'f%201']) await bad(`${frames}/${key}`, bytes(8), `${dir} key ${key}`);
    await bad(`${frames}/f0`, 'A'.repeat(65537), `oversize ${dir} frame`);
    for (const value of [5, true, { a: 1 }]) await bad(`${frames}/f0`, value, `${dir} frame ${JSON.stringify(value)}`);
    await bad(frames, 'junk', `${dir} as a primitive`);
    await bad(frames, { f0: bytes(8), 1: bytes(8) }, `${dir} with one bad key among good ones`);
    await bad(p(uid, parent, host, conn, wrong, 'f0'), bytes(8), `${wrong} frame under ${parent}`);
    ok(await rest('PUT', `${frames}/f0`, { as: uid, body: 'A'.repeat(65536) }), `${dir} frame of exactly 65536 characters`);
  }
  await bad(p(uid, 'to_viewer', host, conn, 'offer'), offer, 'offer under to_viewer');
  await bad(p(uid, 'to_host', host, conn, 'answer'), answer, 'answer under to_host');
  // Unknown children at every level.
  for (const path of [p(uid, 'other'), p(uid, 'hosts_extra', host), p(uid, 'to_host', host, conn, 'extra'), p(uid, 'to_viewer', host, conn, 'extra')]) await bad(path, { a: 1 }, `unknown child ${path}`);
  await bad(offerPath, { ...offer, extra: 1 }, 'offer with an unknown field');
  await bad(offerPath, set(offer, 'device.extra', 1), 'offer device with an unknown field');
  await bad(answerPath, { ...answer, extra: 1 }, 'answer with an unknown field');
  await bad(answerPath, set(answer, 'host.extra', 1), 'answer host with an unknown field');
  ok(await rest('PUT', p(uid, 'hosts', host), { as: uid, body: hostRecord() }), 'a valid host');
  await bad(p(uid, 'hosts', host), { bogus: 1 }, 'host with an unknown field', 'PATCH');
  // Missing required fields.
  for (const field of ['v', 'transport', 'device', 'eph', 'nonce', 'created_at', 'sig', 'device.id', 'device.key', 'device.name', 'device.platform']) await bad(offerPath, set(offer, field, undefined), `offer without ${field}`);
  await bad(offerPath, set(offer, 'sdp', undefined), 'webrtc offer without sdp');
  for (const field of ['v', 'status', 'host', 'nonce', 'created_at', 'sig', 'host.id', 'host.key']) await bad(answerPath, set(answer, field, undefined), `answer without ${field}`);
  for (const field of ['name', 'host_key', 'last_seen']) await bad(p(uid, 'hosts', host), { [field]: null }, `host loses ${field}`, 'PATCH');
  for (const field of ['name', 'host_key', 'last_seen']) await bad(p(uid, 'hosts', bytes(16)), set(hostRecord(), field, undefined), `new host without ${field}`);
  await bad(p(uid, 'hosts', bytes(16), 'last_seen'), 1, 'a heartbeat for a host that was never registered');
  // Wrong values.
  const o = (path, value) => [`offer ${path}=${JSON.stringify(value).slice(0, 30)}`, set(offer, path, value)];
  for (const [label, record] of [o('v', 2), o('v', '1'), o('transport', 'udp'), o('sdp', 'x'.repeat(16385)), o('sdp', 5), o('device.platform', 'windows'), o('device.platform', 'Android'),
    o('device.name', 'n'.repeat(129)), o('device.name', 7), o('device.app_version', 'v'.repeat(65)),
    o('device.id', bytes(16).slice(1)), o('device.id', `${bytes(16)}A`), o('device.id', `${bytes(16).slice(1)}+`), o('device.key', bytes(32).slice(1)), o('device.key', `${bytes(32)}A`), o('device.key', `${bytes(32).slice(1)}=`),
    o('eph', bytes(32).slice(1)), o('eph', `${bytes(32)}=`), o('nonce', bytes(16).slice(1)), o('nonce', `${bytes(16)}A`), o('sig', bytes(64).slice(1)), o('sig', `${bytes(64)}A`), o('sig', 12),
    o('created_at', 'now'), o('created_at', Date.now() + 600_000), o('device', 'phone')]) await bad(offerPath, record, label);
  const a = (path, value) => [`answer ${path}=${JSON.stringify(value).slice(0, 30)}`, set(answer, path, value)];
  for (const [label, record] of [a('v', 2), a('status', 'pending'), a('status', true), a('reason', 'r'.repeat(33)), a('transport', 'udp'), a('sdp', 'x'.repeat(16385)),
    a('host.id', bytes(16).slice(1)), a('host.id', `${bytes(16).slice(1)}+`), a('host.key', bytes(32).slice(1)), a('host.key', `${bytes(32)}A`), a('host', 'desktop'),
    a('eph', bytes(32).slice(1)), a('nonce', bytes(16).slice(1)), a('sig', bytes(64).slice(1)), a('created_at', Date.now() + 600_000), a('created_at', 'now')]) await bad(answerPath, record, label);
  const h = (path, value) => [`host ${path}=${JSON.stringify(value).slice(0, 30)}`, set(hostRecord(), path, value)];
  for (const [label, record] of [h('v', 2), h('name', 'n'.repeat(129)), h('name', 5), h('host_key', bytes(32).slice(1)), h('host_key', `${bytes(32)}A`), h('host_key', `${bytes(32).slice(1)}=`),
    h('last_seen', 'now'), h('online', 'true'), h('online', 1), h('platform', 'p'.repeat(33)), h('app_version', 'v'.repeat(65)), h('client_protocol_version', '29'), h('started_at', 'now')]) await bad(p(uid, 'hosts', bytes(16)), record, label);
  // Base64url fields need the right alphabet as well as the right length (no +, /, =, . or non-ASCII).
  for (const char of ['+', '/', '=', '.', ' ', 'é']) {
    const wrong = (length) => 'A'.repeat(length - 1) + char;
    for (const [path, length] of [['device.id', 22], ['device.key', 43], ['eph', 43], ['nonce', 22], ['sig', 86]]) await bad(offerPath, set(offer, path, wrong(length)), `offer ${path} ending in ${JSON.stringify(char)}`);
    for (const [path, length] of [['host.id', 22], ['host.key', 43], ['eph', 43], ['nonce', 22], ['sig', 86]]) await bad(answerPath, set(answer, path, wrong(length)), `answer ${path} ending in ${JSON.stringify(char)}`);
    await bad(p(uid, 'hosts', bytes(16)), set(hostRecord(), 'host_key', wrong(43)), `host_key ending in ${JSON.stringify(char)}`);
  }
  // Keys of the layout: host and connection ids are 22 base64url characters.
  for (const id of [bytes(16).slice(1), `${bytes(16)}A`, `${bytes(16).slice(1)}+`, `${bytes(16).slice(1)}%20`, 'x']) {
    await bad(p(uid, 'hosts', id), hostRecord(), `host id ${id}`);
    await bad(p(uid, 'to_host', id, conn, 'offer'), offer, `to_host host id ${id}`);
    await bad(p(uid, 'to_host', host, id, 'offer'), offer, `to_host conn id ${id}`);
    await bad(p(uid, 'to_viewer', id, conn, 'answer'), answer, `to_viewer host id ${id}`);
    await bad(p(uid, 'to_viewer', host, id, 'answer'), answer, `to_viewer conn id ${id}`);
  }
  // Containers cannot be primitives.
  for (const path of [p(uid), p(uid, 'hosts'), p(uid, 'hosts', bytes(16)), p(uid, 'to_host'), p(uid, 'to_host', host), p(uid, 'to_host', host, conn), p(uid, 'to_viewer'), p(uid, 'to_viewer', host, conn)]) await bad(path, 'junk', `primitive at ${path}`);
  // The same records are valid when they are shaped right, and at their size limits.
  ok(await rest('PUT', offerPath, { as: uid, body: set(set(set(offer, 'sdp', 's'.repeat(16384)), 'device.name', 'n'.repeat(128)), 'device.app_version', 'v'.repeat(64)) }), 'offer at its limits');
  ok(await rest('PUT', answerPath, { as: uid, body: set(set(answer, 'sdp', 's'.repeat(16384)), 'reason', 'r'.repeat(32)) }), 'answer at its limits');
  ok(await rest('PUT', p(uid, 'hosts', bytes(16)), { as: uid, body: set(set(hostRecord(), 'name', 'n'.repeat(128)), 'app_version', 'v'.repeat(64)) }), 'host at its limits');
});

test('rules: deletion is allowed for the owner whatever the stored data looks like, and only for the owner', opts, async () => {
  const uid = `u-${randomUUID()}`, host = bytes(16), conn = bytes(16);
  ok(await rest('PUT', p(uid, 'hosts', host), { as: uid, body: hostRecord() }), 'host');
  ok(await rest('PUT', p(uid, 'to_host', host, conn, 'offer'), { as: uid, body: offerRecord() }), 'offer');
  ok(await rest('PUT', p(uid, 'to_host', host, conn, 'up'), { as: uid, body: { f0: bytes(8), f1: bytes(8), f2: bytes(8) } }), 'frames');
  ok(await rest('PUT', p(uid, 'to_viewer', host, conn, 'answer'), { as: uid, body: answerRecord() }), 'answer');
  const intruder = `${uid}-intruder`;
  for (const path of [p(uid, 'hosts', host), p(uid, 'to_host', host, conn, 'up', 'f0'), p(uid, 'to_host', host), p(uid)]) denied(await rest('DELETE', path, { as: intruder }), `delete ${path}`);
  denied(await rest('PATCH', p(uid, 'to_host', host, conn, 'up'), { as: intruder, body: { f0: null } }), 'batch delete');
  assert.equal(Object.keys((await rest('GET', p(uid, 'to_host', host, conn, 'up'), { as: 'owner' })).body).length, 3, 'nothing was deleted');
  // Frames: one by one, then a batch of nulls; a connection node; a whole host's inbox.
  ok(await rest('DELETE', p(uid, 'to_host', host, conn, 'up', 'f0'), { as: uid }), 'delete one frame');
  ok(await rest('PATCH', p(uid, 'to_host', host, conn, 'up'), { as: uid, body: { f1: null, f2: null } }), 'batch delete frames');
  assert.equal((await rest('GET', p(uid, 'to_host', host, conn, 'up'), { as: uid })).body, null);
  ok(await rest('DELETE', p(uid, 'to_host', host, conn, 'offer'), { as: uid }), 'delete offer');
  ok(await rest('DELETE', p(uid, 'to_viewer', host, conn), { as: uid }), 'delete a connection node');
  ok(await rest('DELETE', p(uid, 'to_host', host), { as: uid }), 'delete a host inbox');
  ok(await rest('DELETE', p(uid, 'hosts', host), { as: uid }), 'delete the host record');
  assert.equal((await rest('GET', p(uid), { as: 'owner' })).body, null, 'everything is gone');
  // .validate does not run on deletes: data that could not be written today can still be removed.
  const junk = [[p(uid, 'legacy'), { x: 1 }], [p(uid, 'hosts', 'short'), { junk: true }], [p(uid, 'to_host', host, conn, 'up', 'bad'), 'x'],
    [p(uid, 'to_viewer', host, conn, 'offer'), offerRecord()], [p(uid, 'hosts', host), { name: 'no key' }]];
  for (const [path, value] of junk) {
    ok(await rest('PUT', path, { as: 'owner', body: value }), `seed ${path} past the rules`);
    denied(await rest('PUT', path, { as: uid, body: value }), `the owner cannot write ${path} again`);
  }
  for (const [path] of junk) {
    ok(await rest('DELETE', path, { as: uid }), `delete ${path}`);
    assert.equal((await rest('GET', path, { as: 'owner' })).body, null, `${path} is gone`);
  }
  ok(await rest('PUT', p(uid, 'legacy'), { as: 'owner', body: { x: 1 } }), 'seed again');
  ok(await rest('DELETE', p(uid), { as: uid }), 'delete the whole subtree');
  assert.equal((await rest('GET', p(uid), { as: 'owner' })).body, null);
});

test('rules: server timestamps are accepted and resolved before validation', opts, async () => {
  const uid = `u-${randomUUID()}`, host = bytes(16), start = Date.now();
  ok(await rest('PUT', p(uid, 'hosts', host), { as: uid, body: hostRecord() }), 'host with .sv last_seen and started_at');
  const stamps = [];
  for (let i = 0; i < 25; i++) { // created_at <= now must hold for a server timestamp every single time
    const conn = bytes(16);
    ok(await rest('PUT', p(uid, 'to_host', host, conn, 'offer'), { as: uid, body: offerRecord() }), `offer ${i}`);
    ok(await rest('PUT', p(uid, 'to_viewer', host, conn, 'answer'), { as: uid, body: answerRecord() }), `answer ${i}`);
    ok(await rest('PATCH', p(uid, 'hosts', host), { as: uid, body: { last_seen: SV } }), `heartbeat ${i}`);
    stamps.push((await rest('GET', p(uid, 'to_host', host, conn, 'offer', 'created_at'), { as: uid })).body);
  }
  for (const stamp of stamps) assert.ok(typeof stamp === 'number' && Math.abs(stamp - start) < 120_000, `server time, got ${stamp}`);
  const stored = (await rest('GET', p(uid, 'hosts', host), { as: uid })).body;
  assert.ok(typeof stored.last_seen === 'number' && typeof stored.started_at === 'number' && stored.last_seen >= stored.started_at);
  // Client-supplied numbers are still checked against the server clock.
  const conn = bytes(16);
  ok(await rest('PUT', p(uid, 'to_host', host, conn, 'offer'), { as: uid, body: set(offerRecord(), 'created_at', Date.now() - 1000) }), 'a past created_at');
  denied(await rest('PUT', p(uid, 'to_host', host, bytes(16), 'offer'), { as: uid, body: set(offerRecord(), 'created_at', Date.now() + 600_000) }), 'a created_at ten minutes ahead');
  denied(await rest('PUT', p(uid, 'to_viewer', host, bytes(16), 'answer'), { as: uid, body: set(answerRecord(), 'created_at', Date.now() + 600_000) }), 'an answer created_at ten minutes ahead');
});
