import { createHash, createHmac } from 'node:crypto';
import { z } from 'zod';
import { SharedBotError, SharedBotsStore } from './shared-bots.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import { allowedPrivateSource, dataId, denyPreview, generation, sourceBinding } from './lingxi-data-sources.mjs';

const sha = value => createHash('sha256').update(value).digest('hex');
const tokenFormat = /^[A-Za-z0-9_-]{43}$/u;
const cursor = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const permitRequest = z.object({
  action: z.literal('privatePreviewPermit'), teamId: dataId, generation,
  source: dataId, taskId: dataId, deploymentId: dataId, hostId: dataId, accountEpoch: generation,
  permitId: dataId, expectedLastChangeSeq: cursor,
}).strict();
const ciphertext = z.string().min(38).max(65536).regex(/^[A-Za-z0-9_-]+$/u);
const publicKey = z.string().regex(/^(?:[A-Za-z0-9_-]{44}|[A-Za-z0-9_-]{87})$/u);
const signalRequest = z.discriminatedUnion('action', [
  z.object({ action: z.literal('directory') }).strict(),
  z.object({ action: z.literal('host'), source: dataId }).strict(),
  z.object({ action: z.literal('offer'), source: dataId, id: dataId, requesterKey: publicKey, hostKey: publicKey, body: ciphertext }).strict(),
  z.object({ action: z.literal('answer'), id: dataId, body: ciphertext }).strict(),
  z.object({ action: z.literal('status'), id: dataId }).strict(),
  z.object({ action: z.literal('close'), id: dataId }).strict(),
]);

export function createPrivatePreview({ db, endpoint, secret, projectActor, botToken, clock = Date.now, fetchRequest = fetch }) {
  if (typeof secret !== 'string' || !tokenFormat.test(secret)) throw new Error('Expected a Data preview signing key');
  const url = new URL('/v1/data/signaling', endpoint);
  const store = new SharedBotsStore(db, clock);
  const leases = db.collection('lingxiDataPreviewLeases');
  const sessions = db.collection('lingxiDataPreviewSessions');
  const sourceSlot = (source, uid) => db.collection('lingxiDataSourceBindings').doc(source).collection('previewSlots').doc(uid);

  async function authority(tx, proof) {
    const team = db.collection('teams').doc(proof.teamId), now = clock();
    const [teamDoc, member] = await tx.getAll(team, team.collection('members').doc(proof.uid));
    if (teamDoc.data()?.state !== 'active' || member.data()?.generation !== proof.generation) denyPreview();
    let task;
    try {
      task = await store.taskContext({
        tx, team, identity: { uid: proof.uid, generation: proof.generation },
        command: { action: 'validateTask', payload: proof }, now, storedPermitDigest: proof.permitDigest,
      });
    } catch (error) {
      if (error instanceof SharedBotError) denyPreview();
      throw error;
    }
    if (task.privateDataSource !== proof.source || task.task.epoch !== (proof.epoch ?? task.task.epoch)) denyPreview();
    const source = await allowedPrivateSource(tx, db, team, proof.source, task.task.conversationId, task.task.sharedBotId, task.task.requester);
    return { ...source, epoch: task.task.epoch, deadline: task.task.deadline };
  }

  async function checkLease(tx, lease) {
    if (!lease || lease.expires <= clock()) denyPreview();
    const live = await authority(tx, lease.proof);
    if (live.binding.version !== lease.version || live.bot.principalUid !== lease.principalUid
      || live.bot.generation !== lease.generation) denyPreview();
    return live;
  }

  async function permit(uid, raw) {
    const parsed = permitRequest.safeParse(raw);
    if (!parsed.success) denyPreview(400, 'INVALID_DATA_REQUEST');
    const { action: _action, permitId, ...request } = parsed.data;
    const proof = { ...request, uid, permitDigest: sha(JSON.stringify(permitId)) };
    return db.runTransaction(async tx => {
      const live = await authority(tx, proof);
      proof.epoch = live.epoch;
      // No bearer or execution permit is persisted. Reissuing the same task
      // lease after a restart derives the same token from its bound authority.
      const token = createHmac('sha256', secret).update(JSON.stringify([
        proof.uid, proof.teamId, proof.generation, proof.deploymentId, proof.hostId, proof.accountEpoch,
        proof.taskId, proof.epoch, proof.permitDigest, proof.source, live.binding.version, live.bot.principalUid,
      ])).digest('base64url');
      const digest = sha(token), ref = leases.doc(digest), slot = sourceSlot(request.source, live.bot.principalUid);
      const [previous, indexed] = await tx.getAll(ref, slot);
      const expires = Math.floor(live.deadline / 1000) * 1000;
      if (expires <= clock()) denyPreview();
      if (indexed.exists && indexed.data().digest !== digest) {
        const other = (await tx.get(leases.doc(indexed.data().digest))).data();
        let valid = false;
        try { await checkLease(tx, other); valid = true; }
        catch (error) { if (!(error instanceof LingxiDataError) || error.status !== 403) throw error; }
        // d185636 has one signaling actor per Firebase principal.
        if (valid) denyPreview(409, 'DATA_CONFLICT');
      }
      if (!previous.exists) tx.create(ref, { proof, digest, expires, version: live.binding.version,
        principalUid: live.bot.principalUid, generation: live.bot.generation });
      if (!indexed.exists || indexed.data().digest !== digest) tx.set(slot, { digest });
      return { token, expires };
    });
  }

  function peer(lease) {
    return { uid: lease.principalUid, generation: lease.generation, agent: {
      task: lease.proof.taskId, expires: lease.expires / 1000, tokenDigest: Buffer.from(lease.digest, 'hex').toString('base64url'),
    } };
  }

  async function forward(signal, bearer, taskToken) {
    try {
      const result = await fetchRequest(url, { method: 'POST', redirect: 'error',
        signal: AbortSignal.timeout(15_000), headers: { Authorization: 'Bearer ' + bearer,
          'Content-Type': 'application/json', ...(taskToken ? { 'X-Lingxi-Task-Token': taskToken } : {}) },
        body: JSON.stringify(signal) });
      if (result.ok) return await result.json();
      if (result.status === 400) denyPreview(400, 'INVALID_DATA_REQUEST');
      if ([401, 403].includes(result.status)) denyPreview();
      if (result.status === 404) denyPreview(404, 'DATA_NOT_FOUND');
      if (result.status === 409) denyPreview(409, 'DATA_CONFLICT');
    } catch (error) {
      if (error instanceof LingxiDataError) throw error;
    }
    denyPreview(503, 'DATA_UNAVAILABLE');
  }

  async function activeLeases(tx, source, binding) {
    if (!binding.bots.length) return [];
    const slots = await tx.getAll(...binding.bots.map(actor => sourceSlot(source, actor.principalUid)));
    const refs = slots.filter(doc => doc.exists).map(doc => leases.doc(doc.data().digest));
    const candidates = refs.length ? await tx.getAll(...refs) : [];
    const active = [];
    for (const doc of candidates) {
      const lease = doc.data();
      if (lease?.proof.source !== source) continue;
      try { await checkLease(tx, lease); active.push(lease); }
      catch (error) { if (!(error instanceof LingxiDataError) || error.status !== 403) throw error; }
    }
    return active;
  }

  async function signal(uid, bearer, token, raw) {
    const parsed = signalRequest.safeParse(raw);
    if (!parsed.success) denyPreview(400, 'INVALID_DATA_REQUEST');
    const request = parsed.data;
    if (token !== undefined && !tokenFormat.test(token)) denyPreview();
    const offered = request.action === 'offer';
    const requester = token !== undefined;
    if (requester ? ['host', 'answer'].includes(request.action) : ['directory', 'offer'].includes(request.action)) denyPreview();
    const digest = requester ? sha(token) : null;
    const sessionRef = 'id' in request ? sessions.doc(request.id) : null;

    const read = async tx => {
        const session = sessionRef ? (await tx.get(sessionRef)).data() : null;
        const lease = digest ? (await tx.get(leases.doc(digest))).data() : null;
        const source = requester ? lease?.proof.source : (request.action === 'host' ? request.source : session?.source);
        if (!source) denyPreview();
        if (requester && (lease.proof.uid !== uid || (offered && request.source !== source))) denyPreview();
        if (!offered && sessionRef && (!session || session.expires <= clock()
          || (requester && (session.digest !== digest || session.source !== source)))) denyPreview();
        const { binding } = await sourceBinding(tx, db, source);
        if (!requester && binding.owner.uid !== uid) denyPreview();
        if (requester) await checkLease(tx, lease);
        const active = requester ? [lease] : request.action === 'host' ? await activeLeases(tx, source, binding)
          : [(await tx.get(leases.doc(session.digest))).data()];
        if (!requester && request.action !== 'host') await checkLease(tx, active[0]);
        if (offered) {
          if (session) denyPreview(409, 'DATA_CONFLICT');
          tx.create(sessionRef, { source, digest, expires: lease.expires });
        }
        return { source, binding, active };
    };
    const admitted = await db.runTransaction(read, offered ? undefined : { readOnly: true });
    const { source, binding, active } = admitted;
    const scope = { kind: 'team', teamId: binding.teamId, generation: binding.owner.generation };
    const recheck = () => db.runTransaction(tx => readCurrent(tx, source, uid, requester, active, binding), { readOnly: true });
    const participants = active.map(peer);
    const call = () => forward(request, bearer, token);
    let result;
    if (requester) {
      const [actor] = participants;
      const credential = await botToken(actor.uid);
      result = await projectActor(actor.uid, { ...scope, generation: actor.generation }, recheck,
        () => forward(request, credential, token), [binding.owner], actor.agent);
    } else {
      result = await projectActor(uid, scope, recheck, call, participants);
    }
    await recheck();
    if (request.action === 'directory') return Object.hasOwn(result, source) ? { [source]: result[source] } : {};
    return result;
  }

  async function readCurrent(tx, source, uid, requester, active, binding) {
    const current = (await sourceBinding(tx, db, source)).binding;
    if (current.version !== binding.version || current.owner.uid !== binding.owner.uid
      || current.owner.generation !== binding.owner.generation || (!requester && uid !== current.owner.uid)) denyPreview();
    for (const lease of active) await checkLease(tx, lease);
  }

  return { permit, signal };
}
