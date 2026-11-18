#!/usr/bin/env node
// One-off backfill of `teams/{tid}/tasks/{taskId}.visibility`, which `listTasks` now queries.
//
// A task written before the field existed holds none, and a query on the field never returns it, so it would drop out of its Chat's
// list and its requester's. This sets the value the task would have been given:
//
//   - `asked`  somebody asked the Bot: every task that is not optional, and an optional one in a Bot DM;
//   - `shown`  an optional run in a group or channel that posted a reply, or whose action has to be checked (needs_reconciliation);
//   - `hidden` any other optional run in a group or channel.
//
// Dry run by default: it prints a summary and writes nothing. With --apply it writes only `visibility`, at most 400 writes per batch,
// each only if the task is unchanged since it was read. Running it again finds nothing to do. Targets, as teams-migrate-last-message.mjs:
//
//   FIRESTORE_EMULATOR_HOST=127.0.0.1:8080 node scripts/teams-backfill-task-visibility.mjs [--apply] [--project <id>]
//   node scripts/teams-backfill-task-visibility.mjs --project <id> --i-understand-this-writes-production [--apply]
import { realpathSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { applicationDefault, deleteApp, initializeApp } from 'firebase-admin/app';
import { FieldPath, getFirestore } from 'firebase-admin/firestore';
import { BATCH_SIZE, Refusal, resolveTarget } from './teams-migrate-last-message.mjs';

const PAGE_SIZE = 300;

/** The visibility a task is given when it is written (see `queuedTask` and `shown` in src/shared-bots.mjs). */
export function visibilityOf(task, conversationKind) {
  if (task.optional !== true || conversationKind === 'bot_dm') return 'asked';
  return task.replyId || task.state === 'needs_reconciliation' ? 'shown' : 'hidden';
}

/** Every task of every team without a visibility, with the value it gets. Nothing is written. */
export async function planVisibility(db) {
  const plan = { scanned: 0, set: 0, updates: [], counts: { asked: 0, shown: 0, hidden: 0 } };
  for (const team of await db.collection('teams').listDocuments()) {
    const kinds = new Map();
    let after;
    for (;;) {
      let query = team.collection('tasks').orderBy(FieldPath.documentId()).limit(PAGE_SIZE);
      if (after) query = query.startAfter(after);
      const page = await query.get();
      for (const task of page.docs) {
        plan.scanned++;
        if (task.get('visibility') !== undefined) { plan.set++; continue; }
        const conversationId = task.get('conversationId');
        if (!kinds.has(conversationId)) kinds.set(conversationId, (await team.collection('conversations').doc(conversationId).get()).get('kind'));
        const visibility = visibilityOf(task.data(), kinds.get(conversationId));
        plan.counts[visibility]++;
        plan.updates.push({ ref: task.ref, path: task.ref.path, updateTime: task.updateTime, visibility });
      }
      if (page.size < PAGE_SIZE) break;
      after = page.docs.at(-1);
    }
  }
  return plan;
}

/** Write the plan, at most `BATCH_SIZE` to a batch; a task changed since it was read fails its batch (run again). */
export async function applyVisibility(db, updates) {
  for (let from = 0; from < updates.length; from += BATCH_SIZE) {
    const batch = db.batch();
    for (const update of updates.slice(from, from + BATCH_SIZE)) batch.update(update.ref, { visibility: update.visibility }, { lastUpdateTime: update.updateTime });
    await batch.commit();
  }
  return updates.length;
}

export function describeVisibility(plan, applied) {
  const { asked, shown, hidden } = plan.counts;
  const lines = [`${plan.scanned} tasks: ${plan.set} already set, ${plan.updates.length} ${applied ? 'set' : 'to set'} (${asked} asked, ${shown} shown, ${hidden} hidden).`];
  if (!applied && plan.updates.length) lines.push('Dry run: nothing was written. Run again with --apply to write.');
  return lines;
}

const usage = `Usage: teams-backfill-task-visibility.mjs [--apply] [--project <id>] [--i-understand-this-writes-production]
  Dry run unless --apply. Needs FIRESTORE_EMULATOR_HOST (a local emulator), or --project <id> together with
  --i-understand-this-writes-production for that project's (default) database.`;

/** Run the script. Resolves to the exit code: 0 done, 1 refused. */
export async function main(argv, environment = process.env, log = console.log) {
  let target;
  try { target = resolveTarget(argv, environment); }
  catch (error) { if (!(error instanceof Refusal)) throw error; log(error.message); return 1; }
  if (target.help) { log(usage); return 0; }
  log(target.kind === 'emulator' ? `Target: Firestore emulator ${environment.FIRESTORE_EMULATOR_HOST}, project ${target.projectId}` : `Target: PRODUCTION project ${target.projectId}, (default) database`);
  const app = initializeApp({ projectId: target.projectId, ...(target.kind === 'production' ? { credential: applicationDefault() } : {}) }, `teams-backfill-task-visibility-${Date.now()}`);
  try {
    const db = getFirestore(app);
    const plan = await planVisibility(db);
    if (target.apply) await applyVisibility(db, plan.updates);
    for (const line of describeVisibility(plan, target.apply)) log(line);
    return 0;
  } finally { await deleteApp(app); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) process.exitCode = await main(process.argv.slice(2));
