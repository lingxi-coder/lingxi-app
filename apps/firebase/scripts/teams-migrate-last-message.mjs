#!/usr/bin/env node
// One-off repair of Teams conversation `lastMessage` pointers that are not whole.
//
// Clients accept exactly two states of `teams/{tid}/conversations/{cid}.lastMessage`: null (no message yet) or the
// whole pointer { id, seq, at, author, preview, recalled? }. Conversations written before pointers carried `author`
// and `preview` hold { id, seq, at } (or part of the rest); clients stop at them with INVALID_DATA and the rules
// refuse to complete them. This rebuilds each such pointer from its message:
//
//   - the message the pointer names exists: author, preview (messagePreview of its text; a recalled message
//     previews as '' and carries recalled: true) and, unless the pointer has its own, the time come from it;
//   - it does not: the pointer is rebuilt from the conversation's newest message (highest seq);
//   - the conversation has no messages: lastMessage becomes null.
//
// Dry run by default: it prints a summary and the plan and writes nothing. With --apply it writes only `lastMessage`
// of the conversations in the plan, at most 400 writes per batch, each only if the conversation is unchanged since it
// was read. Running it again finds nothing to do. It never writes without a target it was told to write to:
//
//   FIRESTORE_EMULATOR_HOST=127.0.0.1:8080 node scripts/teams-migrate-last-message.mjs [--apply] [--project <id>]
//   node scripts/teams-migrate-last-message.mjs --project <id> --i-understand-this-writes-production [--apply]
//
// The second form uses Application Default Credentials against the project's (default) database.
import { realpathSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { applicationDefault, deleteApp, initializeApp } from 'firebase-admin/app';
import { FieldPath, Timestamp, getFirestore } from 'firebase-admin/firestore';
import { messagePreview } from '../src/team-message-preview.mjs';

export const BATCH_SIZE = 400;
const PAGE_SIZE = 200;
const EMULATOR_PROJECT = 'demo-lingxi-teams-t0';

const isRecord = value => Boolean(value) && typeof value === 'object' && !Array.isArray(value);
const present = value => typeof value === 'string' && value !== '';
const validAuthor = value => isRecord(value)
  && (value.kind === 'user' ? present(value.uid) : value.kind === 'bot' && present(value.uid) && present(value.botKey));

/** What the clients accept besides null: the same checks as `firestoreLastMessage` in packages/teams/firestore.ts. */
export function isWholePointer(value) {
  if (!isRecord(value)) return false;
  const { id, seq, at, author, preview, recalled } = value;
  return present(id) && Number.isSafeInteger(seq) && (at === null || at instanceof Timestamp) && validAuthor(author)
    && typeof preview === 'string' && (recalled === undefined || recalled === true);
}

/** The pointer a message stands for, or undefined when the message cannot be summarised (no usable seq or author). */
function pointerFrom(snapshot, at) {
  const message = snapshot.data();
  if (!message || !Number.isSafeInteger(message.seq) || !validAuthor(message.author)) return undefined;
  const author = message.author.kind === 'bot'
    ? { kind: 'bot', uid: message.author.uid, botKey: message.author.botKey }
    : { kind: 'user', uid: message.author.uid };
  const when = at instanceof Timestamp ? at : message.createdAt instanceof Timestamp ? message.createdAt : null;
  return message.recalled === true
    ? { id: snapshot.id, seq: message.seq, at: when, author, preview: '', recalled: true }
    : { id: snapshot.id, seq: message.seq, at: when, author, preview: messagePreview(message.text) };
}

/** A pointer for the conversation, decided from its messages: { after, source } or { unfixable }. */
async function rebuild(conversation) {
  const messages = conversation.ref.collection('messages');
  const current = conversation.get('lastMessage');
  const named = isRecord(current) && present(current.id) ? await messages.doc(current.id).get() : undefined;
  if (named?.exists) {
    const after = pointerFrom(named, isRecord(current) ? current.at : undefined);
    return after ? { after, source: 'the message it names' } : { unfixable: `message ${named.id} has no usable seq or author` };
  }
  const [newest] = (await messages.orderBy('seq', 'desc').limit(1).get()).docs;
  if (!newest) return { after: null, source: 'no messages' };
  const after = pointerFrom(newest);
  return after ? { after, source: named ? 'the newest message (the one it names is gone)' : 'the newest message' } : { unfixable: `message ${newest.id} has no usable seq or author` };
}

/**
 * Read every team's conversations and decide what to repair. Nothing is written.
 * Returns { scanned, whole, empty, repairs: [{ ref, path, updateTime, before, after, source, note? }], unfixable: [{ path, reason }], warnings: [{ path, reason }] }.
 */
export async function planLastMessageRepairs(db) {
  const plan = { scanned: 0, whole: 0, empty: 0, repairs: [], unfixable: [], warnings: [] };
  for (const team of await db.collection('teams').listDocuments()) {
    let after;
    for (;;) {
      let query = team.collection('conversations').orderBy(FieldPath.documentId()).limit(PAGE_SIZE);
      if (after) query = query.startAfter(after);
      const page = await query.get();
      for (const conversation of page.docs) {
        plan.scanned++;
        const before = conversation.get('lastMessage'), path = conversation.ref.path;
        if (before === null) {
          plan.empty++;
          if (conversation.get('lastSeq') > 0) plan.warnings.push({ path, reason: `lastSeq is ${conversation.get('lastSeq')} but lastMessage is null; left as it is` });
          continue;
        }
        if (isWholePointer(before)) { plan.whole++; continue; }
        const result = await rebuild(conversation);
        if (result.unfixable) { plan.unfixable.push({ path, reason: result.unfixable }); continue; }
        const lastSeq = conversation.get('lastSeq');
        plan.repairs.push({ ref: conversation.ref, path, updateTime: conversation.updateTime, before, after: result.after, source: result.source,
          ...(result.after && lastSeq !== result.after.seq ? { note: `lastSeq is ${lastSeq}, the pointer's seq is ${result.after.seq}` } : {}) });
      }
      if (page.size < PAGE_SIZE) break;
      after = page.docs.at(-1);
    }
  }
  return plan;
}

/** Write the repairs, at most `BATCH_SIZE` to a batch; a conversation changed since it was read fails its batch (run again). */
export async function applyLastMessageRepairs(db, repairs) {
  for (let from = 0; from < repairs.length; from += BATCH_SIZE) {
    const batch = db.batch();
    for (const repair of repairs.slice(from, from + BATCH_SIZE)) batch.update(repair.ref, { lastMessage: repair.after }, { lastUpdateTime: repair.updateTime });
    await batch.commit();
  }
  return repairs.length;
}

const show = value => JSON.stringify(value, (_key, item) => item instanceof Timestamp ? item.toDate().toISOString() : item);

/** The report: a summary, then every conversation in the plan. */
export function describePlan(plan, applied) {
  const lines = [];
  for (const repair of plan.repairs) {
    lines.push(`${applied ? 'repaired' : 'would repair'} ${repair.path} from ${repair.source}${repair.note ? ` (${repair.note})` : ''}`,
      `    before ${repair.before === undefined ? '(no lastMessage field)' : show(repair.before)}`, `    after  ${show(repair.after)}`);
  }
  for (const item of plan.warnings) lines.push(`warning ${item.path}: ${item.reason}`);
  for (const item of plan.unfixable) lines.push(`cannot repair ${item.path}: ${item.reason}`);
  lines.push(`${plan.scanned} conversations: ${plan.whole} whole, ${plan.empty} without messages, ${plan.repairs.length} ${applied ? 'repaired' : 'to repair'}, ${plan.unfixable.length} that cannot be repaired.`);
  if (!applied && plan.repairs.length) lines.push('Dry run: nothing was written. Run again with --apply to write.');
  return lines;
}

export class Refusal extends Error {}

/** Where the script may work. Without a target it was explicitly given, it refuses before it connects to anything. */
export function resolveTarget(argv, environment) {
  const options = { apply: false, project: undefined, production: false, help: false };
  for (let at = 0; at < argv.length; at++) {
    const argument = argv[at];
    if (argument === '--apply') options.apply = true;
    else if (argument === '--i-understand-this-writes-production') options.production = true;
    else if (argument === '--help' || argument === '-h') options.help = true;
    else if (argument === '--project') {
      options.project = argv[++at];
      if (!options.project || options.project.startsWith('--')) throw new Refusal('--project needs a project id.');
    } else throw new Refusal(`Unknown argument ${argument}. Try --help.`);
  }
  if (options.help) return options;
  const emulator = environment.FIRESTORE_EMULATOR_HOST;
  if (emulator) {
    if (!['127.0.0.1', 'localhost'].includes(emulator.split(':')[0])) throw new Refusal(`FIRESTORE_EMULATOR_HOST must be a local address, not ${emulator}.`);
    if (options.production) throw new Refusal('FIRESTORE_EMULATOR_HOST is set: this would not reach production. Unset it to write to production.');
    return { ...options, kind: 'emulator', projectId: options.project ?? environment.GCLOUD_PROJECT ?? EMULATOR_PROJECT };
  }
  if (options.project && options.production) return { ...options, kind: 'production', projectId: options.project };
  throw new Refusal('Refusing to run without a target. Set FIRESTORE_EMULATOR_HOST to a local emulator, or name a project with --project <id> and --i-understand-this-writes-production.');
}

const usage = `Usage: teams-migrate-last-message.mjs [--apply] [--project <id>] [--i-understand-this-writes-production]
  Dry run unless --apply. Needs FIRESTORE_EMULATOR_HOST (a local emulator), or --project <id> together with
  --i-understand-this-writes-production for that project's (default) database.`;

/** Run the script. Resolves to the exit code: 0 done, 1 refused, 2 some conversations cannot be repaired. */
export async function main(argv, environment = process.env, log = console.log) {
  let target;
  try { target = resolveTarget(argv, environment); }
  catch (error) { if (!(error instanceof Refusal)) throw error; log(error.message); return 1; }
  if (target.help) { log(usage); return 0; }
  log(target.kind === 'emulator' ? `Target: Firestore emulator ${environment.FIRESTORE_EMULATOR_HOST}, project ${target.projectId}` : `Target: PRODUCTION project ${target.projectId}, (default) database`);
  const app = initializeApp({ projectId: target.projectId, ...(target.kind === 'production' ? { credential: applicationDefault() } : {}) }, `teams-migrate-last-message-${Date.now()}`);
  try {
    const db = getFirestore(app);
    const plan = await planLastMessageRepairs(db);
    if (target.apply) await applyLastMessageRepairs(db, plan.repairs);
    for (const line of describePlan(plan, target.apply)) log(line);
    return plan.unfixable.length ? 2 : 0;
  } finally { await deleteApp(app); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) process.exitCode = await main(process.argv.slice(2));
