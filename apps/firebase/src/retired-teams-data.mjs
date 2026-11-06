import { createHash } from 'node:crypto';

const segment = value => typeof value === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(value);
const oldPrincipal = value => typeof value === 'string' && /^bot:[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{22}:[A-Za-z0-9_-]{22}$/u.test(value);
const retiredChildren = ['triggers', 'botRuns', 'botQuestions', 'botWaits', 'botCursors'];
const digest = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
const stamp = snapshot => snapshot.updateTime?.toDate().toISOString() + ':' + snapshot.updateTime?.nanoseconds;
function retiredConversation(value) {
  return Array.isArray(value?.participants) && value.participants.some(oldPrincipal);
}
/** Explicit cutover tool only. No runtime fallback reader, identity conversion,
 * binding recreation or invocation is performed. The caller selects Team IDs. */
export async function planRetiredTeamCleanup(db, teamIds) {
  if (!Array.isArray(teamIds) || !teamIds.length || teamIds.some(id => !segment(id)) || new Set(teamIds).size !== teamIds.length) throw new Error('EXPLICIT_TEAM_IDS_REQUIRED');
  const entries = [];
  for (const teamId of [...teamIds].sort()) {
    const team = db.doc(`teams/${teamId}`);
    const [bots, chats] = await Promise.all([
      team.collection('bots').select().get(),
      team.collection('conversations').select('participants').get(),
    ]);
    for (const bot of bots.docs) {
      if (!oldPrincipal(bot.id)) throw new Error('UNRECOGNIZED_RETIRED_BOT_ID');
      entries.push({ kind: 'binding', path: bot.ref.path, updateStamp: stamp(bot) });
    }
    for (const chat of chats.docs) {
      if (retiredConversation(chat.data())) {
        entries.push({ kind: 'conversation', path: chat.ref.path, updateStamp: stamp(chat) });
      } else {
        for (const child of retiredChildren) {
          const page = await chat.ref.collection(child).select().get();
          for (const doc of page.docs) entries.push({ kind: 'orphan', path: doc.ref.path, updateStamp: stamp(doc) });
        }
      }
    }
  }
  const plan = { schemaVersion: 1, teamIds: [...teamIds].sort(), entries: entries.sort((a, b) => a.path.localeCompare(b.path)) };
  return { ...plan, digest: digest(plan) };
}
/** Re-scans and compares the exact plan before deleting. Stop the old clients
 * and workers before invoking this maintenance-only operation. Human-only
 * conversations and all current SharedBot collections are outside the plan. */
export async function applyRetiredTeamCleanup(db, plan, { oldExecutionStopped } = {}) {
  if (oldExecutionStopped !== true) throw new Error('CUTOVER_NOT_STOPPED');
  const current = await planRetiredTeamCleanup(db, plan.teamIds);
  if (current.digest !== plan.digest) throw new Error('CLEANUP_PLAN_CHANGED');
  let deleted = 0;
  for (const entry of current.entries) {
    const ref = db.doc(entry.path), value = await ref.get();
    if (!value.exists) continue;
    if (stamp(value) !== entry.updateStamp) throw new Error('CLEANUP_PLAN_CHANGED');
    if (entry.kind === 'conversation' && !retiredConversation(value.data())) throw new Error('CLEANUP_SCOPE_CHANGED');
    await db.recursiveDelete(ref); deleted++;
  }
  return { deletedRoots: deleted, digest: plan.digest };
}
