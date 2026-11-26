export const reservationMs = 15 * 60 * 1000;
const cleanupQueue = 'teamVisualizationCleanup';
const entryId = (message, version) => `${message}_${version}`;
const cleanupRef = (db, message, version) => db.collection(cleanupQueue).doc(entryId(message, version));
const inventoryRef = team => team.collection('visualizationVersions');
export const rowRef = (db, message, version) => db.doc(`teamVisualizations/${message}/versions/${version}`);
export const visualizationFile = (bucket, page, version) => bucket.file(`team-visualizations/${page}/${version}.json`);
export const visualizationQuota = team => team.collection('privateSettings').doc('visualizations');

export function trackVisualization(tx, team, { page: message, version }) {
  tx.create(inventoryRef(team).doc(entryId(message, version)), { message, version });
}

export function scheduleVisualizationCleanup(tx, db, team, { page: message, version }, expiresAt, retired = false) {
  tx.set(cleanupRef(db, message, version), { team: team.id, message, version, expiresAt, retired });
}

export function cancelVisualizationCleanup(tx, db, { page, version }) {
  tx.delete(cleanupRef(db, page, version));
}

export function recallVisualization(tx, team, reference, now) {
  const db = team.firestore;
  tx.update(rowRef(db, reference.page, reference.version), { status: 'recalled' });
  scheduleVisualizationCleanup(tx, db, team, reference, now);
}

export function retireTeamVisualizations(tx, team, expiresAt) {
  tx.set(team.firestore.collection(cleanupQueue).doc(`team_${team.id}`), { team: team.id, expiresAt });
}

async function enqueueRetiredVersions(db, team, item, now) {
  await db.runTransaction(async tx => {
    const [job, root] = await tx.getAll(item.ref, team);
    if (!job.exists || root.data()?.state !== 'deleted') return;
    let query = inventoryRef(team).orderBy('__name__').limit(20);
    if (job.data().after) query = query.startAfter(job.data().after);
    const versions = (await tx.get(query)).docs;
    for (const version of versions) {
      const row = version.data();
      scheduleVisualizationCleanup(tx, db, team, { page: row.message, version: row.version }, now, true);
    }
    if (versions.length < 20) tx.delete(item.ref);
    else tx.update(item.ref, { after: versions.at(-1).id, expiresAt: now });
  });
}

export async function sweepVisualizations(db, bucket, now) {
  const due = await db.collection(cleanupQueue).where('expiresAt', '<=', now).orderBy('expiresAt').limit(20).get();
  const failures = [];
  for (const item of due.docs) {
    const { team, message } = item.data();
    const teamRef = db.collection('teams').doc(team);
    try {
      if (message) await reclaimVisualization(db, bucket, teamRef, item, now);
      else await enqueueRetiredVersions(db, teamRef, item, now);
    } catch (error) { failures.push(error); }
  }
  if (failures.length) throw new AggregateError(failures, 'Visualization cleanup failed');
}

async function reclaimVisualization(db, bucket, team, item, now) {
  const { message, version } = item.data();
  const ref = rowRef(db, message, version), quota = visualizationQuota(team);
  const inventory = inventoryRef(team).doc(entryId(message, version));
  const reclaim = await db.runTransaction(async tx => {
    const [job, publication] = await tx.getAll(item.ref, ref);
    const row = publication.data();
    if (!job.exists || job.data().expiresAt > now) return false;
    const status = row?.status;
    if (!['pending', 'recalled', 'deleting'].includes(status) && !(status === 'active' && job.data().retired)) {
      tx.delete(item.ref);
      tx.delete(inventory);
      return false;
    }
    if (!Number.isSafeInteger(row.bytes) || row.bytes < 0) throw new Error('Invalid visualization reservation size');
    tx.update(ref, { status: 'deleting', recalled: row.recalled === true || status === 'recalled' || status === 'active' });
    return true;
  });
  if (!reclaim) return;
  await visualizationFile(bucket, message, version).delete({ ignoreNotFound: true });
  await db.runTransaction(async tx => {
    const [publication, usage] = await tx.getAll(ref, quota);
    const row = publication.data();
    if (row?.status !== 'deleting') return;
    const states = (await tx.get(ref.collection('viewerState').limit(80))).docs;
    for (const state of states) tx.delete(state.ref);
    if (states.length === 80) {
      tx.update(item.ref, { expiresAt: now });
      return;
    }
    tx.set(quota, { bytes: Math.max(0, (usage.data()?.bytes ?? 0) - row.bytes) });
    tx.set(ref, { status: row.recalled ? 'recalled' : 'expired', uid: row.uid,
      destination: row.destination, intentDigest: row.intentDigest });
    tx.delete(inventory);
    tx.delete(item.ref);
  });
}
