// Regenerates test/fixtures/remote/{signing,pairing,tunnel,framing}.json.
//
//   node test/gen-remote-fixtures.mjs
//
// Deliberately independent of src/: everything is derived with node:crypto from
// fixed seeds and the formats in the plan, so the vectors are an outside check
// on the implementation (and the contract the Rust client is tested against).
// The output is deterministic; test/remote-fixtures-sync.test.ts fails when the
// committed files and this script disagree.

import {
  createCipheriv,
  createHash,
  createPrivateKey,
  createPublicKey,
  diffieHellman,
  hkdfSync,
  sign as edSign,
} from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const sha256 = (...parts) => {
  const hash = createHash('sha256');
  for (const part of parts) hash.update(part);
  return hash.digest();
};
const hex = (buffer) => Buffer.from(buffer).toString('hex');
const unhex = (text) => Buffer.from(text, 'hex');
const b64u = (buffer) => Buffer.from(buffer).toString('base64url');
const b64uText = (text) => b64u(Buffer.from(text, 'utf8'));
const seedOf = (label) => sha256(Buffer.from(label, 'utf8'));

const ED25519_PKCS8 = unhex('302e020100300506032b657004220420');
const X25519_PKCS8 = unhex('302e020100300506032b656e04220420');

function edKey(seed) {
  const privateKey = createPrivateKey({ key: Buffer.concat([ED25519_PKCS8, seed]), format: 'der', type: 'pkcs8' });
  const publicKey = Buffer.from(createPublicKey(privateKey).export({ format: 'jwk' }).x, 'base64url');
  return { seed, privateKey, publicKey, id: b64u(sha256(publicKey)).slice(0, 22), sign: (data) => edSign(null, data, privateKey) };
}

function xKey(seed) {
  const privateKey = createPrivateKey({ key: Buffer.concat([X25519_PKCS8, seed]), format: 'der', type: 'pkcs8' });
  const publicKey = Buffer.from(createPublicKey(privateKey).export({ format: 'jwk' }).x, 'base64url');
  return { seed, privateKey, publicKey };
}

const dh = (mine, theirs) => {
  const publicKey = createPublicKey({
    key: Buffer.concat([unhex('302a300506032b656e032100'), theirs.publicKey]),
    format: 'der',
    type: 'spki',
  });
  return diffieHellman({ privateKey: mine.privateKey, publicKey });
};

// ── signing.json ─────────────────────────────────────────────────────────────

const SDP = [
  'v=0',
  'o=- 4611731400430051336 2 IN IP4 127.0.0.1',
  's=-',
  't=0 0',
  'a=group:BUNDLE 0',
  'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
  'c=IN IP4 0.0.0.0',
  'a=ice-ufrag:aBcD',
  'a=ice-pwd:0123456789abcdefghijklmn',
  'a=fingerprint:sha-256 4A:5D:9D:5B:A4:CE:2D:E1:72:8E:3B:F4:80:35:0F:25:E0:7E:21:C9:47:D1:9E:33:76:F0:9B:3E:1C:16:17:42',
  'a=setup:actpass',
  'a=mid:0',
  'a=sctp-port:5000',
  '',
].join('\r\n');

function offerInput(ctx, offer) {
  return [
    'lingxi-rc-offer/1',
    ctx.uid,
    ctx.host_id,
    ctx.conn_id,
    offer.device.id,
    offer.device.key,
    b64uText(offer.device.name),
    offer.device.platform,
    b64uText(offer.device.app_version),
    offer.transport,
    offer.sdp === undefined ? '-' : b64u(sha256(Buffer.from(offer.sdp, 'utf8'))),
    offer.eph,
    offer.nonce,
  ].join('\n');
}

function answerInput(ctx, offerNonce, answer) {
  return [
    'lingxi-rc-answer/1',
    ctx.uid,
    ctx.host_id,
    ctx.conn_id,
    offerNonce,
    answer.status,
    answer.reason ?? '-',
    answer.transport ?? '-',
    answer.sdp === undefined ? '-' : b64u(sha256(Buffer.from(answer.sdp, 'utf8'))),
    answer.eph ?? '-',
    answer.nonce,
  ].join('\n');
}

function buildSigning() {
  const rfc = edKey(unhex('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60'));
  const host = edKey(seedOf('lingxi-rc-test host identity'));
  const device = edKey(seedOf('lingxi-rc-test device identity'));
  const device2 = edKey(seedOf('lingxi-rc-test device identity 2'));
  const viewerEph = xKey(seedOf('lingxi-rc-test viewer ephemeral 1'));
  const viewerEph2 = xKey(seedOf('lingxi-rc-test viewer ephemeral 2'));
  const hostEph = xKey(seedOf('lingxi-rc-test host ephemeral'));
  const uid = 'FvTr8Kq2Wm9LxNc4Zp7DsJ1HbGe5';
  const createdAt = 1760000000000;
  const connId = (n) => b64u(sha256(Buffer.from(`lingxi-rc-test conn ${n}`)).subarray(0, 16));
  const nonce = (label) => b64u(sha256(Buffer.from(`lingxi-rc-test nonce ${label}`)).subarray(0, 16));

  const offers = [];
  const offerCase = (name, n, signer, offerFields) => {
    const ctx = { uid, host_id: host.id, conn_id: connId(n) };
    const offer = { v: 1, ...offerFields, created_at: createdAt };
    const input = offerInput(ctx, offer);
    offer.sig = b64u(signer.sign(Buffer.from(input, 'utf8')));
    offers.push({
      name,
      uid: ctx.uid,
      host_id: ctx.host_id,
      conn_id: ctx.conn_id,
      record: offer,
      sdp_sha256_b64u: offer.sdp === undefined ? null : b64u(sha256(Buffer.from(offer.sdp, 'utf8'))),
      signing_input: input,
    });
  };
  offerCase('webrtc offer, ASCII device name', 1, device, {
    transport: 'webrtc',
    sdp: SDP,
    device: { id: device.id, key: b64u(device.publicKey), name: 'Test Phone', platform: 'android', app_version: '1.2.3' },
    eph: b64u(viewerEph.publicKey),
    nonce: nonce('offer 1'),
  });
  offerCase('webrtc offer, non-ASCII device name and version', 2, device2, {
    transport: 'webrtc',
    sdp: SDP,
    device: { id: device2.id, key: b64u(device2.publicKey), name: '小明的 iPhone 📱', platform: 'ios', app_version: '2.0.0-β' },
    eph: b64u(viewerEph2.publicKey),
    nonce: nonce('offer 2'),
  });
  offerCase('tunnel offer without sdp', 3, device, {
    transport: 'tunnel',
    device: { id: device.id, key: b64u(device.publicKey), name: 'Browser', platform: 'web', app_version: '0.9.0' },
    eph: b64u(viewerEph.publicKey),
    nonce: nonce('offer 3'),
  });

  const answers = [];
  const answerCase = (name, n, offerNonce, answerFields) => {
    const ctx = { uid, host_id: host.id, conn_id: connId(n) };
    const answer = { v: 1, ...answerFields, host: { id: host.id, key: b64u(host.publicKey) }, created_at: createdAt };
    const input = answerInput(ctx, offerNonce, answer);
    answer.sig = b64u(host.sign(Buffer.from(input, 'utf8')));
    answers.push({
      name,
      uid: ctx.uid,
      host_id: ctx.host_id,
      conn_id: ctx.conn_id,
      offer_nonce: offerNonce,
      record: answer,
      signing_input: input,
    });
  };
  answerCase('accepted, webrtc', 1, nonce('offer 1'), { status: 'accepted', transport: 'webrtc', sdp: SDP.replace('actpass', 'active'), nonce: nonce('answer 1') });
  answerCase('accepted, tunnel with host ephemeral key', 3, nonce('offer 3'), {
    status: 'accepted',
    transport: 'tunnel',
    eph: b64u(hostEph.publicKey),
    nonce: nonce('answer 3'),
  });
  answerCase('rejected, revoked', 4, nonce('offer 4'), { status: 'rejected', reason: 'revoked', nonce: nonce('answer 4') });
  answerCase('rejected, upgrade_required', 5, nonce('offer 5'), { status: 'rejected', reason: 'upgrade_required', nonce: nonce('answer 5') });

  const keyEntry = (key) => ({ seed_hex: hex(key.seed), public_hex: hex(key.publicKey), public_b64u: b64u(key.publicKey), id: key.id });
  return {
    _comment:
      'Ed25519 identity keys, ids and the signing inputs/signatures of offers and answers. Generated by test/gen-remote-fixtures.mjs from fixed seeds with node:crypto. ' +
      'Signing input = the listed lines joined with "\\n", no trailing newline, UTF-8; free text is b64u(UTF-8), an absent field is "-", the sdp enters as b64u(SHA-256(UTF-8 sdp)). ' +
      'Ed25519 signatures are deterministic, so "sig" is reproducible. created_at is not signed.',
    protocol: 'lingxi-rc/1',
    id_rule: 'id = first 22 characters of b64u(SHA-256(raw 32-byte public key))',
    rfc8032_test1: {
      seed_hex: hex(rfc.seed),
      public_hex: hex(rfc.publicKey),
      message_hex: '',
      signature_hex: hex(rfc.sign(Buffer.alloc(0))),
      id: rfc.id,
    },
    keys: { host: keyEntry(host), device: keyEntry(device), device2: keyEntry(device2) },
    ephemeral_public_b64u: { viewer_1: b64u(viewerEph.publicKey), viewer_2: b64u(viewerEph2.publicKey), host: b64u(hostEph.publicKey) },
    now_for_verification: createdAt,
    offers,
    answers,
  };
}

// ── pairing.json ─────────────────────────────────────────────────────────────

function pairingVector(name, hostKey, deviceKey, offerNonce, answerNonce) {
  const digest = sha256(Buffer.from('lingxi-rc-pair/1'), hostKey, deviceKey, offerNonce, answerNonce);
  const value = digest.readUInt32BE(0);
  return {
    name,
    host_key_hex: hex(hostKey),
    device_key_hex: hex(deviceKey),
    offer_nonce_hex: hex(offerNonce),
    answer_nonce_hex: hex(answerNonce),
    sha256_hex: hex(digest),
    u32_be: value,
    code: String(value % 1_000_000).padStart(6, '0'),
  };
}

function buildPairing() {
  const host = edKey(seedOf('lingxi-rc-test host identity')).publicKey;
  const device = edKey(seedOf('lingxi-rc-test device identity')).publicKey;
  const device2 = edKey(seedOf('lingxi-rc-test device identity 2')).publicKey;
  const offerNonce = sha256(Buffer.from('lingxi-rc-test offer nonce')).subarray(0, 16);
  const counter = (n) => {
    const out = Buffer.alloc(16);
    out.writeUInt32BE(n, 12);
    return out;
  };
  // Found by searching the answer-nonce counter space (a u32 in the last four bytes) with the fixed keys and offer nonce above.
  const vectors = [
    pairingVector('ordinary', host, device, offerNonce, sha256(Buffer.from('lingxi-rc-test answer nonce a')).subarray(0, 16)),
    pairingVector('another device and nonces', host, device2, sha256(Buffer.from('b')).subarray(0, 16), sha256(Buffer.from('c')).subarray(0, 16)),
    pairingVector('one leading zero', host, device, offerNonce, counter(13)),
    pairingVector('three leading zeros', host, device, offerNonce, counter(1058)),
    pairingVector('all zeros', host, device, offerNonce, counter(122045)),
  ];
  return {
    _comment:
      'Pairing codes. code = SHA-256("lingxi-rc-pair/1" || host_key || device_key || offer_nonce || answer_nonce) -> first 4 bytes as u32 big-endian -> mod 1000000 -> zero-padded to 6 digits. ' +
      'All four inputs are raw bytes (32, 32, 16, 16). Generated by test/gen-remote-fixtures.mjs.',
    domain: 'lingxi-rc-pair/1',
    vectors,
  };
}

// ── tunnel.json ──────────────────────────────────────────────────────────────

function buildTunnel() {
  // RFC 7748 section 6.1 keys serve as the two ephemeral keys; their shared secret is the published one.
  const viewer = xKey(unhex('77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a'));
  const host = xKey(unhex('5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb'));
  const shared = dh(viewer, host);
  const connId = b64u(sha256(Buffer.from('lingxi-rc-test conn tunnel')).subarray(0, 16));
  const derive = (dir) => Buffer.from(hkdfSync('sha256', shared, Buffer.from(connId, 'utf8'), Buffer.from(`lingxi-rc-tunnel/1 ${dir}`, 'utf8'), 32));
  const keys = { up: derive('up'), down: derive('down') };

  const fragment = (kind, payload) => Buffer.concat([Buffer.from([kind]), payload]);
  const pack = (fragments) => {
    const parts = [];
    for (const f of fragments) {
      const length = Buffer.alloc(4);
      length.writeUInt32BE(f.length);
      parts.push(length, f);
    }
    return Buffer.concat(parts);
  };
  const seal = (dir, seq, plaintext) => {
    const nonce = Buffer.alloc(12);
    nonce.writeBigUInt64BE(BigInt(seq), 4);
    const aad = Buffer.from(`lingxi-rc-tunnel/1\n${connId}\n${dir}\n${seq}`, 'utf8');
    const cipher = createCipheriv('aes-256-gcm', keys[dir], nonce, { authTagLength: 16 });
    cipher.setAAD(aad);
    const sealed = Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]);
    return { nonce, aad, stored: b64u(sealed) };
  };

  const frames = [];
  const smallFrame = (name, dir, seq, fragments) => {
    const plaintext = pack(fragments);
    const { nonce, aad, stored } = seal(dir, seq, plaintext);
    frames.push({
      name,
      dir,
      seq,
      key: `f${seq}`,
      fragments_hex: fragments.map(hex),
      plaintext_hex: hex(plaintext),
      nonce_hex: hex(nonce),
      aad_utf8: aad.toString('utf8'),
      aad_hex: hex(aad),
      stored_b64u: stored,
    });
  };
  const hello = fragment(0, Buffer.from('{"method":"ping"}', 'utf8'));
  smallFrame('up, seq 0, one whole message', 'up', 0, [hello]);
  smallFrame('down, seq 0, same plaintext (key and AAD differ)', 'down', 0, [hello]);
  smallFrame('up, seq 1, three fragments of one split message', 'up', 1, [
    fragment(1, Buffer.from('{"id":"1","method":"s', 'utf8')),
    fragment(1, Buffer.from('essions.list","para', 'utf8')),
    fragment(2, Buffer.from('ms":{}}', 'utf8')),
  ]);
  smallFrame('down, seq 255', 'down', 255, [fragment(0, Buffer.from('{"method":"pong"}'))]);
  smallFrame('up, seq 4294967296 (nonce bytes beyond 32 bits)', 'up', 4294967296, [fragment(0, Buffer.from('{"method":"ping"}'))]);
  smallFrame('down, seq 9999999999 (largest key f9999999999)', 'down', 9999999999, [fragment(0, Buffer.from('{"method":"pong"}'))]);
  smallFrame('up, seq 4, the close notice: a frame with no fragments (empty plaintext, the stored value is just the tag)', 'up', 4, []);

  // Frames too big to inline: described by a recipe, checked by length and SHA-256 of the stored text.
  const large = [];
  const largeFrame = (name, dir, seq, recipe) => {
    const fragments = recipe.map(({ kind, payload_length, fill_hex }) => fragment(kind, Buffer.alloc(payload_length, unhex(fill_hex))));
    const plaintext = pack(fragments);
    const { stored } = seal(dir, seq, plaintext);
    large.push({
      name,
      dir,
      seq,
      fragments: recipe,
      plaintext_length: plaintext.length,
      plaintext_sha256_hex: hex(sha256(plaintext)),
      stored_length: stored.length,
      stored_sha256_hex: hex(sha256(Buffer.from(stored, 'utf8'))),
    });
  };
  largeFrame('up, three tunnel-sized fragments (16374 bytes each)', 'up', 2, [
    { kind: 1, payload_length: 16373, fill_hex: '61' },
    { kind: 1, payload_length: 16373, fill_hex: '62' },
    { kind: 2, payload_length: 16373, fill_hex: '63' },
  ]);
  largeFrame('down, plaintext of exactly 49136 bytes (stored value is exactly 65536 characters)', 'down', 3, [
    { kind: 1, payload_length: 16373, fill_hex: '64' },
    { kind: 1, payload_length: 16373, fill_hex: '65' },
    { kind: 2, payload_length: 16375, fill_hex: '66' },
  ]);

  const good = frames[0];
  // The first frame's plaintext sealed under other binding parameters: values that must fail to open as (up, 0).
  const sealed = (dir, seq, id = connId) => {
    const plaintext = unhex(good.plaintext_hex);
    const nonce = Buffer.alloc(12);
    nonce.writeBigUInt64BE(BigInt(seq), 4);
    const aad = Buffer.from(`lingxi-rc-tunnel/1\n${id}\n${dir}\n${seq}`, 'utf8');
    const cipher = createCipheriv('aes-256-gcm', keys.up, nonce, { authTagLength: 16 });
    cipher.setAAD(aad);
    return b64u(Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]));
  };
  const flipped = Buffer.from(good.stored_b64u, 'base64url');
  flipped[0] ^= 1;
  const open_failures = [
    { name: 'bit flipped in the ciphertext', dir: 'up', seq: 0, conn_id: connId, stored_b64u: b64u(flipped) },
    { name: 'sealed for seq 1, presented as seq 0', dir: 'up', seq: 0, conn_id: connId, stored_b64u: sealed('up', 1) },
    { name: 'sealed with the down label under the up key, presented as up', dir: 'up', seq: 0, conn_id: connId, stored_b64u: sealed('down', 0) },
    { name: 'sealed for another conn_id', dir: 'up', seq: 0, conn_id: connId, stored_b64u: sealed('up', 0, 'AAAAAAAAAAAAAAAAAAAAAA') },
    { name: 'the down-direction frame presented under the up key', dir: 'up', seq: 0, conn_id: connId, stored_b64u: frames[1].stored_b64u },
    { name: 'shorter than the tag', dir: 'up', seq: 0, conn_id: connId, stored_b64u: 'AAAA' },
  ];

  // plaintext_hex is given directly, or "fragments" lists [u32 length][fill bytes] entries to lay out.
  const negativeUnpack = [
    { name: 'length prefix cut off', plaintext_hex: '000000' },
    { name: 'body shorter than its length', plaintext_hex: '0000000500aabb' },
    { name: 'empty fragment', plaintext_hex: '00000000' },
    { name: 'fragment of 16385 bytes', fragments: [{ length: 16385, fill_hex: '61' }] },
  ];

  return {
    _comment:
      'RTDB tunnel cryptography. shared = X25519(viewer_eph, host_eph) (the RFC 7748 6.1 pair). ' +
      'k_dir = HKDF-SHA256(ikm = shared, salt = UTF-8(conn_id), info = UTF-8("lingxi-rc-tunnel/1 " + dir), 32 bytes). ' +
      'A frame is stored as b64u(AES-256-GCM(k_dir, nonce, plaintext, aad) = ciphertext || 16-byte tag) under key "f" + decimal seq; ' +
      'nonce = 4 zero bytes || u64 big-endian seq; aad = UTF-8("lingxi-rc-tunnel/1\\n" + conn_id + "\\n" + dir + "\\n" + seq). ' +
      'plaintext = repeated [u32 big-endian length][fragment], at most 49136 bytes. "large_frames" are given as recipes (fragment kind, payload length, fill byte) and checked by length and SHA-256. ' +
      'A frame whose plaintext holds no fragments is the close notice: the sender writes it after its last data frame and then deletes its direction\'s node; the receiver closes (peer_closed) once it has processed every earlier frame. ' +
      '"open_failures" are stored values that must fail authentication under k_up for the given conn_id, dir and seq. ' +
      '"unpack_failures" are plaintexts that must be rejected: either {plaintext_hex}, or {fragments: [{length, fill_hex}]} = each fragment is length copies of the fill byte, written with its length prefix. ' +
      'Generated by test/gen-remote-fixtures.mjs.',
    conn_id: connId,
    viewer_eph: { seed_hex: hex(viewer.seed), public_hex: hex(viewer.publicKey) },
    host_eph: { seed_hex: hex(host.seed), public_hex: hex(host.publicKey) },
    shared_hex: hex(shared),
    hkdf: {
      salt_utf8: connId,
      info_up_utf8: 'lingxi-rc-tunnel/1 up',
      info_down_utf8: 'lingxi-rc-tunnel/1 down',
      k_up_hex: hex(keys.up),
      k_down_hex: hex(keys.down),
    },
    frames,
    large_frames: large,
    open_failures,
    unpack_failures: negativeUnpack,
  };
}

// ── framing.json ─────────────────────────────────────────────────────────────

function buildFraming() {
  const encode = (bytes, maxFragmentBytes = 16384) => {
    const payloadMax = maxFragmentBytes - 1;
    if (bytes.length <= payloadMax) return [fragment(0, bytes)];
    const out = [];
    for (let offset = 0; offset < bytes.length; offset += payloadMax) {
      const end = Math.min(offset + payloadMax, bytes.length);
      out.push(fragment(end === bytes.length ? 2 : 1, bytes.subarray(offset, end)));
    }
    return out;
  };
  const fragment = (kind, payload) => Buffer.concat([Buffer.from([kind]), payload]);

  const cases = [];
  const addCase = (name, message, maxFragmentBytes = 16384) => {
    const text = 'text' in message ? message.text : (message.prefix ?? '') + message.unit.repeat(message.count) + (message.suffix ?? '');
    const bytes = Buffer.from(text, 'utf8');
    const fragments = encode(bytes, maxFragmentBytes);
    cases.push({
      name,
      message,
      max_fragment_bytes: maxFragmentBytes,
      message_utf8_length: bytes.length,
      message_sha256_hex: hex(sha256(bytes)),
      fragments: fragments.map((f) => ({
        kind: f[0],
        length: f.length,
        sha256_hex: hex(sha256(f)),
        ...(f.length <= 64 ? { hex: hex(f) } : {}),
      })),
    });
  };
  addCase('empty message', { text: '' });
  addCase('short ASCII message', { text: '{"method":"ping"}' });
  addCase('16383 bytes: the largest whole message', { unit: 'a', count: 16383 });
  addCase('16384 bytes: one byte too many for a whole message', { unit: 'a', count: 16384 });
  addCase('16385 bytes', { unit: 'a', count: 16385 });
  addCase('32766 bytes: exactly two full payloads', { unit: 'b', count: 32766 });
  addCase('50000 bytes in four fragments', { unit: 'abcdefghij', count: 5000 });
  addCase('2-byte characters split inside the last character', { unit: 'é', count: 8192 });
  addCase('4-byte characters split after three bytes', { unit: '😀', count: 4096 });
  addCase('3-byte characters misaligned by a one-byte prefix', { prefix: 'a', unit: '灵犀', count: 12000 });
  addCase('tunnel-sized fragments (16374 bytes)', { unit: 'x', count: 49000 }, 16374);
  addCase('tunnel-sized fragments, 16373 bytes: one whole fragment', { unit: 'y', count: 16373 }, 16374);

  const recipe = (kind, payload_length, fill_hex = '61') => ({ recipe: { kind, payload_length, fill_hex } });
  const invalid = [
    { name: 'empty fragment', fragments: [{ hex: '' }] },
    { name: 'unknown kind 3', fragments: [{ hex: '0361' }] },
    { name: 'unknown kind 255', fragments: [{ hex: 'ff61' }] },
    { name: 'continuation (kind 2) without a start', fragments: [{ hex: '0261' }] },
    { name: 'whole message (kind 0) inside a split message', fragments: [{ hex: '0161' }, { hex: '0062' }] },
    { name: 'invalid UTF-8 byte', fragments: [{ hex: '00ff' }] },
    { name: 'truncated multi-byte character at the end', fragments: [{ hex: '00c3' }] },
    { name: 'split character completed by a wrong byte', fragments: [{ hex: '01c3' }, { hex: '02c3' }] },
    { name: 'overlong encoding', fragments: [{ hex: '00c080' }] },
    { name: 'encoded surrogate', fragments: [{ hex: '00eda080' }] },
    { name: 'fragment of 16385 bytes', fragments: [recipe(0, 16384)] },
  ];
  const valid_split = [
    { name: 'a character split across two fragments is valid once reassembled', fragments: [{ hex: '01c3' }, { hex: '02a9' }], message_utf8_hex: 'c3a9' },
    { name: 'a byte order mark is kept, not stripped', fragments: [{ hex: '00efbbbf61' }], message_utf8_hex: 'efbbbf61' },
  ];

  return {
    _comment:
      'Message fragmentation. A message is UTF-8 text; fragments are [kind][payload], at most max_fragment_bytes (16384 unless stated) including the kind byte: ' +
      'kind 0 whole message, 1 more follows, 2 last. A message that fits one fragment is kind 0, otherwise the payload is cut every max_fragment_bytes - 1 bytes (inside a character if need be). ' +
      'Messages are given as {text} or {prefix?, unit, count, suffix?} = prefix + unit repeated count times + suffix; fragments are checked by length and SHA-256 (hex given when short). ' +
      'The receiver rejects: an empty fragment, a fragment above 16384 bytes, an unknown kind, kind 2 without a preceding kind 1, kind 0 during a split message, ' +
      'more than 16777216 bytes of message, and invalid UTF-8 after reassembly. ' +
      'A fragment too big to inline is given as {recipe: {kind, payload_length, fill_hex}} = the kind byte followed by payload_length copies of the fill byte. Generated by test/gen-remote-fixtures.mjs.',
    fragment_max_bytes: 16384,
    message_max_bytes: 16777216,
    cases,
    invalid_sequences: invalid,
    valid_sequences: valid_split,
  };
}

// ── main ─────────────────────────────────────────────────────────────────────

export function buildFixtures() {
  return { 'signing.json': buildSigning(), 'pairing.json': buildPairing(), 'tunnel.json': buildTunnel(), 'framing.json': buildFraming() };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = new URL('./fixtures/remote/', import.meta.url);
  for (const [name, value] of Object.entries(buildFixtures())) {
    writeFileSync(new URL(name, dir), `${JSON.stringify(value, null, 2)}\n`);
    console.log(`wrote test/fixtures/remote/${name}`);
  }
}
