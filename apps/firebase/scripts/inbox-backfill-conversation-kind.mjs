#!/usr/bin/env node
// One-off backfill of `users/{uid}/inbox/{id}.conversationKind` for team rows (`ref.kind === 'team'`).
//
// The fold sets the field (`dm`, `group`, `channel` or `bot_dm`, from the conversation) on its next run, so a row that has
// not changed since the field was added holds none, and clients group it with people's chats: a dormant Bot DM shows under
// Teams until it next gets a message or is read. This sets the value the fold would write, from the conversation's `kind`.
// A row whose conversation is gone, or of another kind, is left as it is.
//
// Dry run by default: it prints a summary and writes nothing. With --apply it writes only `conversationKind`, at most 400
// writes per batch, each only if the row is unchanged since it was read. Running it again finds nothing to do. Targets, as
// teams-migrate-last-message.mjs:
//
//   FIRESTORE_EMULATOR_HOST=127.0.0.1:8080 node scripts/inbox-backfill-conversation-kind.mjs [--apply] [--project <id>]
//   node scripts/inbox-backfill-conversation-kind.mjs --project <id> --i-understand-this-writes-production [--apply]
import { realpathSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { applicationDefault, deleteApp, initializeApp } from 'firebase-admin/app';
import { FieldPath, getFirestore } from 'firebase-admin/firestore';
import { BATCH_SIZE, Refusal, resolveTarget } from './teams-migrate-last-message.mjs';

const PAGE_SIZE = 300;
/** The conversation kinds the fold copies onto a row (`fold` in src/inbox-store.mjs). */
export const CONVERSATION_KINDS = ['dm', 'group', 'channel', 'bot_dm'];

/** Every team row without a conversation kind, with the kind it gets. Nothing is written. */
export async function planConversationKinds(db) {
  const plan = { scanned: 0, set: 0, missing: 0, updates: [], counts: Object.fromEntries(CONVERSATION_KINDS.map(kind => [kind, 0])) };
  const kinds = new Map();
  const kindOf = async (teamId, conversationId) => {
    const key = `${teamId}/${conversationId}`;
    if (!kinds.has(key)) kinds.set(key, (await db.collection('teams').doc(teamId).collection('conversations').doc(conversationId).get()).get('kind'));
    return kinds.get(key);
  };
  for (const user of await db.collection('users').listDocuments()) {
    let after;
    for (;;) {
      let query = user.collection('inbox').orderBy(FieldPath.documentId()).limit(PAGE_SIZE);
      if (after) query = query.startAfter(after);
      const page = await query.get();
      for (const row of page.docs) {
        const ref = row.get('ref');
        if (ref?.kind !== 'team' || typeof ref.sourceId !== 'string' || typeof ref.conversationId !== 'string') continue;
        plan.scanned++;
        if (row.get('conversationKind') !== undefined) { plan.set++; continue; }
        const kind = await kindOf(ref.sourceId, ref.conversationId);
        if (!CONVERSATION_KINDS.includes(kind)) { plan.missing++; continue; }
        plan.counts[kind]++;
        plan.updates.push({ ref: row.ref, path: row.ref.path, updateTime: row.updateTime, conversationKind: kind });
      }
      if (page.size < PAGE_SIZE) break;
      after = page.docs.at(-1);
    }
  }
  return plan;
}

/** Write the plan, at most `BATCH_SIZE` to a batch; a row changed since it was read fails its batch (run again). */
export async function applyConversationKinds(db, updates) {
  for (let from = 0; from < updates.length; from += BATCH_SIZE) {
    const batch = db.batch();
    for (const update of updates.slice(from, from + BATCH_SIZE)) batch.update(update.ref, { conversationKind: update.conversationKind }, { lastUpdateTime: update.updateTime });
    await batch.commit();
  }
  return updates.length;
}

export function describeConversationKinds(plan, applied) {
  const counts = CONVERSATION_KINDS.map(kind => `${plan.counts[kind]} ${kind}`).join(', ');
  const lines = [`${plan.scanned} team rows: ${plan.set} already set, ${plan.updates.length} ${applied ? 'set' : 'to set'} (${counts}), ${plan.missing} without a known conversation.`];
  if (!applied && plan.updates.length) lines.push('Dry run: nothing was written. Run again with --apply to write.');
  return lines;
}

const usage = `Usage: inbox-backfill-conversation-kind.mjs [--apply] [--project <id>] [--i-understand-this-writes-production]
  Dry run unless --apply. Needs FIRESTORE_EMULATOR_HOST (a local emulator), or --project <id> together with
  --i-understand-this-writes-production for that project's (default) database.`;

/** Run the script. Resolves to the exit code: 0 done, 1 refused. */
export async function main(argv, environment = process.env, log = console.log) {
  let target;
  try { target = resolveTarget(argv, environment); }
  catch (error) { if (!(error instanceof Refusal)) throw error; log(error.message); return 1; }
  if (target.help) { log(usage); return 0; }
  log(target.kind === 'emulator' ? `Target: Firestore emulator ${environment.FIRESTORE_EMULATOR_HOST}, project ${target.projectId}` : `Target: PRODUCTION project ${target.projectId}, (default) database`);
  const app = initializeApp({ projectId: target.projectId, ...(target.kind === 'production' ? { credential: applicationDefault() } : {}) }, `inbox-backfill-conversation-kind-${Date.now()}`);
  try {
    const db = getFirestore(app);
    const plan = await planConversationKinds(db);
    if (target.apply) await applyConversationKinds(db, plan.updates);
    for (const line of describeConversationKinds(plan, target.apply)) log(line);
    return 0;
  } finally { await deleteApp(app); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) process.exitCode = await main(process.argv.slice(2));
