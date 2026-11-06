/** Explicit, offline maintenance for retired execution data. Never called by a reader.
 * Inspect first; apply only the reviewed digest after old workers/writers stop.
 * Each mutation is durable; interruption may leave a smaller candidate to inspect again.
 */
import { createHash, randomUUID } from 'node:crypto';
import { constants, closeSync, fchmodSync, fchownSync, fstatSync, fsyncSync, lstatSync, openSync, readSync, readdirSync, renameSync, rmdirSync, unlinkSync, writeFileSync, realpathSync, type BigIntStats } from 'node:fs';
import { dirname, isAbsolute, join, parse, relative, resolve } from 'node:path';

const BOT_ID = /^[A-Za-z0-9_-]{22}$/u;
const MAX_RECORD = 1024 * 1024;
const MAX_ENTRIES = 20000;
function fail(reason: string): never { throw new Error(`RETIRED_TEAMS_${reason}`); }
const hash = (value: string | Buffer) => createHash('sha256').update(value).digest('hex');
interface Stamp { dev: string; ino: string; uid: string; gid: string; mode: string; size: string; mtimeNs: string; ctimeNs: string; nlink: string }
interface Entry { path: string; kind: 'directory' | 'file'; stamp: Stamp; digest?: string }
export interface RetiredTeamsCleanupPlan {
  readonly schemaVersion: 1;
  readonly userData: string;
  readonly digest: string;
  readonly actions: readonly { readonly kind: 'delete_retired_host' | 'remove_teams_field'; readonly relativePath: string; readonly files: number; readonly bytes: number }[];
}
export interface RetiredTeamsCleanupReceipt { reviewedDigest: string; removedFiles: number; removedDirectories: number; rewrittenBots: number }
interface Scan { plan: RetiredTeamsCleanupPlan; root: Entry; parents: Entry[]; host: Entry[]; records: { entry: Entry; replacement: string }[] }
const stamp = (s: BigIntStats): Stamp => ({ dev: String(s.dev), ino: String(s.ino), uid: String(s.uid), gid: String(s.gid), mode: String(s.mode), size: String(s.size), mtimeNs: String(s.mtimeNs), ctimeNs: String(s.ctimeNs), nlink: String(s.nlink) });
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const sameIdentity = (s: BigIntStats, expected: Stamp) => ['dev', 'ino', 'uid', 'gid', 'mode'].every(key => String(s[key as keyof BigIntStats]) === expected[key as keyof Stamp]);
function owner(): bigint { if (!process.getuid) fail('OWNER_CHECK_UNSUPPORTED'); return BigInt(process.getuid()); }
function info(path: string): BigIntStats | undefined {
  try { return lstatSync(path, { bigint: true }); } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
}
function checked(path: string, kind: Entry['kind']): BigIntStats {
  const s = info(path);
  if (!s || s.uid !== owner() || (s.mode & 0o022n) !== 0n || s.isSymbolicLink()
    || (kind === 'directory' ? !s.isDirectory() : !s.isFile() || s.nlink !== 1n)) fail('UNSAFE_PATH');
  return s;
}
function canonicalRoot(userData: string): string {
  if (!isAbsolute(userData) || resolve(userData) !== userData || userData === parse(userData).root) fail('UNSAFE_ROOT');
  // Reject links in every existing ancestor, including a symlink spelling of /tmp.
  let cursor = userData;
  while (cursor !== parse(cursor).root) { const s = info(cursor); if (!s?.isDirectory() || s.isSymbolicLink() || ((s.mode & 0o022n) !== 0n && ((s.mode & 0o1000n) === 0n || (s.uid !== 0n && s.uid !== owner())))) fail('UNSAFE_PATH'); cursor = dirname(cursor); }
  if (realpathSync.native(userData) !== userData) fail('UNSAFE_ROOT');
  checked(userData, 'directory'); return userData;
}
function read(path: string, maxBytes?: number): { bytes: Buffer; entry: Entry } {
  const before = checked(path, 'file');
  if (maxBytes !== undefined && before.size > BigInt(maxBytes)) fail('RECORD_TOO_LARGE');
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = fstatSync(fd, { bigint: true }); if (!same(stamp(before), stamp(opened))) fail('CHANGED');
    if (opened.size > BigInt(Number.MAX_SAFE_INTEGER)) fail('TREE_TOO_LARGE');
    const expectedSize = Number(opened.size), digest = createHash('sha256'), parts: Buffer[] = []; const chunk = Buffer.alloc(64 * 1024); let offset = 0;
    while (offset < expectedSize) { const length = readSync(fd, chunk, 0, Math.min(chunk.length, expectedSize - offset), offset); if (!length) fail('CHANGED'); offset += length; if (maxBytes !== undefined && offset > maxBytes) fail('RECORD_TOO_LARGE'); digest.update(chunk.subarray(0, length)); if (maxBytes !== undefined) parts.push(Buffer.from(chunk.subarray(0, length))); }
    if (!same(stamp(opened), stamp(fstatSync(fd, { bigint: true }))) || !same(stamp(opened), stamp(checked(path, 'file')))) fail('CHANGED');
    return { bytes: Buffer.concat(parts), entry: { path, kind: 'file', stamp: stamp(opened), digest: digest.digest('hex') } };
  } finally { closeSync(fd); }
}
/** Remove top-level teams properties as raw JSON spans. Other field bytes (including
 * large integer literals, whitespace, duplicate non-teams keys and escaped text)
 * remain unchanged rather than being coerced through a JSON reserializer. */
function withoutTeams(source: string): string | undefined {
  let object: unknown;
  try { object = JSON.parse(source); } catch { fail('INVALID_RECORD'); }
  if (!object || typeof object !== 'object' || Array.isArray(object)) fail('INVALID_RECORD');
  if (!Object.prototype.hasOwnProperty.call(object, 'teams')) return undefined;
  const whitespace = (at: number) => { while (/\s/u.test(source[at] ?? '') && at < source.length) at++; return at; };
  const open = whitespace(0), pieces: { key: string; raw: string }[] = [];
  let start = open + 1, cursor = whitespace(start), close = cursor;
  while (source[cursor] !== '}') {
    const keyStart = cursor++; let escaped = false;
    for (; cursor < source.length; cursor++) { const ch = source[cursor]!; if (escaped) escaped = false; else if (ch === '\\') escaped = true; else if (ch === '"') { cursor++; break; } }
    const key = JSON.parse(source.slice(keyStart, cursor)) as string;
    cursor = whitespace(cursor) + 1; // The entire document was already validated: this consumes ':'.
    let depth = 0, quoted = false; escaped = false;
    for (; cursor < source.length; cursor++) {
      const ch = source[cursor]!;
      if (quoted) { if (escaped) escaped = false; else if (ch === '\\') escaped = true; else if (ch === '"') quoted = false; continue; }
      if (ch === '"') quoted = true;
      else if (ch === '{' || ch === '[') depth++;
      else if (ch === '}' || ch === ']') { if (depth === 0) break; depth--; }
      else if (ch === ',' && depth === 0) break;
    }
    pieces.push({ key, raw: source.slice(start, cursor) });
    if (source[cursor] === '}') { close = cursor; break; }
    start = cursor + 1; cursor = whitespace(start);
  }
  return source.slice(0, open + 1) + pieces.filter(piece => piece.key !== 'teams').map(piece => piece.raw).join(',') + source.slice(close);
}
function scan(userData: string): Scan {
  const rootPath = canonicalRoot(userData), root: Entry = { path: rootPath, kind: 'directory', stamp: stamp(checked(rootPath, 'directory')) };
  const hostPath = join(rootPath, 'team-bot-host'), host: Entry[] = [];
  const visit = (path: string, depth: number) => {
    if (host.length >= MAX_ENTRIES || depth > 64) fail('TREE_TOO_LARGE');
    const s = info(path); if (!s) fail('CHANGED');
    if (s.isDirectory()) {
      const before = checked(path, 'directory'), names = readdirSync(path).sort();
      host.push({ path, kind: 'directory', stamp: stamp(before) });
      for (const name of names) visit(join(path, name), depth + 1);
      if (!same(stamp(before), stamp(checked(path, 'directory')))) fail('CHANGED');
    } else host.push(read(path).entry);
  };
  if (info(hostPath)) visit(hostPath, 0);
  const parents: Entry[] = [];
  const records: Scan['records'] = [], botsRoot = join(rootPath, 'bots');
  if (info(botsRoot)) {
    parents.push({ path: botsRoot, kind: 'directory', stamp: stamp(checked(botsRoot, 'directory')) });
    for (const id of readdirSync(botsRoot).filter(id => BOT_ID.test(id)).sort()) {
      const directory = join(botsRoot, id); parents.push({ path: directory, kind: 'directory', stamp: stamp(checked(directory, 'directory')) }); const path = join(directory, 'bot.json'); if (!info(path)) continue;
      const { entry, bytes } = read(path, MAX_RECORD); let source: string;
      try { source = new TextDecoder('utf-8', { fatal: true }).decode(bytes); } catch { fail('INVALID_RECORD'); }
      const replacement = withoutTeams(source); if (replacement !== undefined) records.push({ entry, replacement });
    }
  }
  const actions: RetiredTeamsCleanupPlan['actions'][number][] = [];
  if (host.length) actions.push({ kind: 'delete_retired_host', relativePath: 'team-bot-host', files: host.filter(e => e.kind === 'file').length, bytes: host.reduce((sum, e) => sum + (e.kind === 'file' ? Number(e.stamp.size) : 0), 0) });
  for (const { entry } of records) actions.push({ kind: 'remove_teams_field', relativePath: relative(rootPath, entry.path), files: 1, bytes: Number(entry.stamp.size) });
  // Directory mtimes change when the maintenance lock/temp files are created;
  // root identity/permissions, full retired tree, and original Bot bytes form the CAS.
  const identity = ({ dev, ino, uid, gid, mode }: Stamp) => ({ dev, ino, uid, gid, mode });
  const digest = hash(JSON.stringify({ schemaVersion: 1, userData: rootPath, root: identity(root.stamp), parents: parents.map(e => ({ path: e.path, ...identity(e.stamp) })), host, records: records.map(r => r.entry) }));
  return { root, parents, host, records, plan: { schemaVersion: 1, userData: rootPath, digest, actions } };
}
export function inspectRetiredTeamsData(userData: string): RetiredTeamsCleanupPlan { return scan(userData).plan; }
function syncDirectory(path: string): void {
  const fd = openSync(path, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { fsyncSync(fd); } finally { closeSync(fd); }
}
function assertParents(path: string, scan: Scan): void {
  let parent = dirname(path);
  while (parent.length >= scan.root.path.length) {
    const current = checked(parent, 'directory'); const expected = parent === scan.root.path ? scan.root : [...scan.host, ...scan.parents].find(e => e.path === parent);
    if (expected && !sameIdentity(current, expected.stamp)) fail('CHANGED');
    if (parent === scan.root.path) return; parent = dirname(parent);
  }
  fail('UNSAFE_PATH');
}
/** Explicit apply only: caller must stop old workers AND all personal Bot writers.
 * Inspection plans are not trusted as paths or replacement content: apply re-derives
 * the exact allowlist and original-byte CAS under an exclusive maintenance lock.
 */
export function applyRetiredTeamsData(userData: string, reviewed: RetiredTeamsCleanupPlan,
  confirmation: { reviewedDigest: string; writersStopped: true }): RetiredTeamsCleanupReceipt {
  if (confirmation?.writersStopped !== true || reviewed?.schemaVersion !== 1 || confirmation.reviewedDigest !== reviewed.digest
    || reviewed.userData !== userData || !/^[a-f0-9]{64}$/u.test(reviewed.digest)) fail('REVIEW_REQUIRED');
  const canonical = canonicalRoot(userData), lock = join(canonical, '.teams-retired-cleanup.lock');
  const fd = openSync(lock, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  const lockStamp = fstatSync(fd, { bigint: true });
  try {
    writeFileSync(fd, JSON.stringify({ schemaVersion: 1, pid: process.pid, reviewedDigest: reviewed.digest })); fsyncSync(fd); syncDirectory(canonical);
    const current = scan(canonical); if (current.plan.digest !== reviewed.digest || !same(current.plan.actions, reviewed.actions)) fail('CHANGED');
    const result: RetiredTeamsCleanupReceipt = { reviewedDigest: reviewed.digest, removedFiles: 0, removedDirectories: 0, rewrittenBots: 0 };
    for (const { entry, replacement } of current.records) {
      assertParents(entry.path, current); if (!same(read(entry.path, MAX_RECORD).entry, entry)) fail('CHANGED');
      const temp = join(dirname(entry.path), `.retired-teams-${randomUUID()}.tmp`); let staged: BigIntStats | undefined;
      try {
        const target = openSync(temp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
        try { staged = fstatSync(target, { bigint: true }); writeFileSync(target, replacement); fchownSync(target, Number(entry.stamp.uid), Number(entry.stamp.gid)); fchmodSync(target, Number(entry.stamp.mode) & 0o777); fsyncSync(target); } finally { closeSync(target); }
        assertParents(entry.path, current); if (!same(read(entry.path, MAX_RECORD).entry, entry)) fail('CHANGED');
        renameSync(temp, entry.path); syncDirectory(dirname(entry.path)); result.rewrittenBots++;
      } finally { const leftover = info(temp); if (leftover) { if (!staged || leftover.dev !== staged.dev || leftover.ino !== staged.ino || !leftover.isFile() || leftover.nlink !== 1n) fail('UNSAFE_PATH'); unlinkSync(temp); syncDirectory(dirname(temp)); } }
    }
    // Reverse preorder: children first. Never use recursive rm, which could follow
    // an unreviewed replacement tree or erase files added after inspection.
    for (const entry of [...current.host].reverse()) {
      assertParents(entry.path, current);
      if (entry.kind === 'file') { if (!same(read(entry.path).entry, entry)) fail('CHANGED'); unlinkSync(entry.path); result.removedFiles++; }
      else { const s = checked(entry.path, 'directory'); if (!sameIdentity(s, entry.stamp) || readdirSync(entry.path).length) fail('CHANGED'); rmdirSync(entry.path); result.removedDirectories++; }
      syncDirectory(dirname(entry.path));
    }
    return result;
  } finally {
    closeSync(fd); const existing = info(lock);
    if (!existing || existing.dev !== lockStamp.dev || existing.ino !== lockStamp.ino || existing.uid !== owner() || !existing.isFile() || existing.nlink !== 1n) fail('UNSAFE_LOCK');
    unlinkSync(lock); syncDirectory(canonical);
  }
}
