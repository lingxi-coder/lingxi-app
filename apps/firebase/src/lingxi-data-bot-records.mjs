import { createHash, createHmac } from 'node:crypto';
import { z } from 'zod';

import { authorizeDataBinding, createPinnedDataClient, dataReference, LingxiDataError } from './lingxi-data.mjs';
import { dataId, generation } from './lingxi-data-sources.mjs';
import { SharedBotError, SharedBotsStore } from './shared-bots.mjs';
import { effectiveScope } from './bot-scope.mjs';
import { deliverReport, readReport, reviewReport } from './lingxi-data-reports.mjs';

const cursor = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const scope = z.object({ kind: z.literal('team'), teamId: dataId, generation, conversationId: dataId }).strict();
const accessRequest = z.object({
  action: z.literal('setBotDataAccess'), scope, dataset: dataId, sharedBotId: dataId,
  expectedVersion: cursor, access: z.enum(['read', 'edit']).nullable(),
  reportReview: z.literal(true).optional(),
  handoff: z.object({ targets: z.array(dataId).min(1).max(8).refine(ids => new Set(ids).size === ids.length),
    dailyLimit: z.number().int().min(1).max(100) }).strict().optional(),
}).strict();
const hostProof = { teamId: dataId, generation, deploymentId: dataId, hostId: dataId, accountEpoch: generation,
  taskId: dataId, permitId: dataId, expectedLastChangeSeq: cursor };
const offerRequest = z.object({ action: z.literal('botDataOffers'), ...hostProof }).strict();
const handoffRequest = z.object({ action: z.literal('botDataHandoff'), ...hostProof, accessVersion: generation,
  sharedBotId: dataId, reference: dataReference, stepId: dataId }).strict();
const settingsRequest = z.object({ scope, command: z.object({ action: z.literal('dataAutomation'), dataset: dataId }).strict() }).strict();
const operationRequest = z.object({
  action: z.literal('botDataOperation'), ...hostProof, accessVersion: generation,
  operation: z.discriminatedUnion('action', [
    readReport, reviewReport,
    z.object({ action: z.literal('describe'), dataset: dataId }).strict(),
    z.object({ action: z.literal('list'), dataset: dataId, after: dataId.optional(), limit: z.number().int().min(1).max(10) }).strict(),
    z.object({ action: z.literal('get'), reference: dataReference }).strict(),
    z.object({ action: z.literal('put'), dataset: dataId, record: dataId, expectedRevision: cursor,
      schemaVersion: generation, value: z.record(z.string(), z.json()) }).strict(),
  ]),
  stepId: dataId.optional(),
}).strict();

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  }
  return value;
}

const hash = value => createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex');

function deny(status = 403, code = 'DATA_ACCESS_REQUIRED') {
  throw new LingxiDataError(status, code);
}

export function createBotDataRecords({ db, endpoint, secret, projectActor, botToken, clock = Date.now }) {
  const forward = createPinnedDataClient(endpoint);
  const store = new SharedBotsStore(db, clock);
  const grants = (team, sharedBotId) => team.collection('sharedBots').doc(sharedBotId).collection('dataGrants');

  async function setAccess(uid, token, raw) {
    const parsed = accessRequest.safeParse(raw);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const { scope: target, dataset, sharedBotId, expectedVersion, access, handoff, reportReview } = parsed.data;
    if (reportReview && access !== 'edit') deny(400, 'INVALID_DATA_REQUEST');
    if (handoff && (!access || handoff.targets.includes(sharedBotId))) deny(400, 'INVALID_DATA_REQUEST');
    const team = db.collection('teams').doc(target.teamId);
    const ref = grants(team, sharedBotId).doc(dataset);
    const requestDigest = hash([uid, raw]);
    async function authority(tx) {
      await authorizeDataBinding(tx, db, uid, target, dataset, true, true);
      const [member, thread, bot, current] = (await tx.getAll(team.collection('members').doc(uid),
        team.collection('conversations').doc(target.conversationId), team.collection('sharedBots').doc(sharedBotId),
        ref)).map(doc => doc.data());
      if (!['owner', 'admin'].includes(member?.role) || !thread?.participants?.includes('shared_bot:' + sharedBotId)) deny();
      if (handoff?.targets.some(id => !thread.participants.includes('shared_bot:' + id))) deny();
      if (current?.requestDigest === requestDigest) return { previous: current };
      if ((current?.version ?? 0) !== expectedVersion) deny(409, 'DATA_CONFLICT');
      if (access !== null && (bot?.identity?.state !== 'published' || !generation.safeParse(bot.identity.revision).success)) deny();
      const version = expectedVersion + 1;
      if (!generation.safeParse(version).success) deny(409, 'DATA_CAPACITY');
      const botRevision = bot?.identity?.revision ?? null;
      const principalUid = 'lx_data_bot_' + hash(['dataset', target, dataset, sharedBotId, version, access, botRevision]).slice(0, 40);
      return { entry: { version, access, botRevision, principalUid, requestDigest, conversationId: target.conversationId,
        issuedBy: { uid, generation: target.generation }, ...(handoff ? { handoff } : {}), ...(reportReview ? { reportReview } : {}) } };
    }
    const admitted = await db.runTransaction(authority);
    if (admitted.previous) return { version: admitted.previous.version, access };
    const { entry } = admitted;
    if (access !== null) {
      await projectActor(uid, target, async () => {
        const live = await db.runTransaction(authority);
        if (!live.previous && live.entry.principalUid !== entry.principalUid) deny(409, 'DATA_CONFLICT');
      }, () => forward(token, requestDigest, { action: 'share', dataset, recipient: entry.principalUid, access }),
      [{ uid: entry.principalUid, generation: entry.version }]);
    }
    await db.runTransaction(async tx => {
      const live = await authority(tx);
      if (live.previous) return;
      if (live.entry.principalUid !== entry.principalUid) deny(409, 'DATA_CONFLICT');
      tx.set(ref, entry);
    });
    return { version: entry.version, access };
  }

  async function taskAuthority(tx, team, uid, proof) {
    const [teamDoc, member] = await tx.getAll(team, team.collection('members').doc(uid));
    if (teamDoc.data()?.state !== 'active' || member.data()?.generation !== proof.generation) deny();
    const now = clock();
    try {
      const status = await store.taskContext({
        tx, team, identity: { uid, generation: proof.generation }, command: { action: 'validateTask', payload: proof }, now,
      });
      if (status.privateDataSource || !status.deployment.ready || status.deployment.onlineUntil <= now) deny();
      return status;
    } catch (error) {
      if (error instanceof SharedBotError) deny();
      throw error;
    }
  }

  async function grantScope(tx, team, status, dataset, grant, write) {
    const { task, deployment } = status;
    if (!grant?.access || grant.conversationId !== task.conversationId || grant.botRevision !== deployment.revision
      || (write && (grant.access !== 'edit' || !deployment.capabilities.writeOperations))) deny();
    const target = { kind: 'team', teamId: team.id, conversationId: task.conversationId };
    const issuer = (await tx.get(team.collection('members').doc(grant.issuedBy.uid))).data();
    if (!['owner', 'admin'].includes(issuer?.role)) deny();
    await authorizeDataBinding(tx, db, grant.issuedBy.uid, { ...target, generation: grant.issuedBy.generation }, dataset, true, true);
    await authorizeDataBinding(tx, db, task.requester.uid, { ...target, generation: task.requester.generation }, dataset, write);
    return target;
  }

  async function offers(uid, raw) {
    const parsed = offerRequest.safeParse(raw);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const proof = parsed.data;
    const team = db.collection('teams').doc(proof.teamId);
    return db.runTransaction(async tx => {
      const status = await taskAuthority(tx, team, uid, proof);
      const [found, parent] = await Promise.all([
        tx.get(grants(team, status.task.sharedBotId).where('conversationId', '==', status.task.conversationId)),
        tx.get(team.collection('tasks').doc(proof.taskId)),
      ]);
      const datasets = [];
      for (const doc of found.docs) {
        if (!dataId.safeParse(doc.id).success) continue;
        // A grant is not enough: the Bot published this dataset, and this Chat has not narrowed it away.
        const allowed = status.scope.datasets.find(row => row.dataset === doc.id);
        if (!allowed) continue;
        const grant = doc.data();
        let write = allowed.write && grant.access === 'edit' && status.deployment.capabilities.writeOperations;
        if (write) {
          try { await grantScope(tx, team, status, doc.id, grant, true); }
          catch (error) { if (!(error instanceof LingxiDataError) || ![403, 409].includes(error.status)) throw error; write = false; }
        }
        if (!write) {
          try { await grantScope(tx, team, status, doc.id, grant, false); }
          catch (error) { if (!(error instanceof LingxiDataError) || ![403, 409].includes(error.status)) throw error; continue; }
        }
        const canHandoff = write && parent.data()?.rootTaskId === proof.taskId && grant.handoff
          && (!parent.data()?.dataCause || parent.data().dataCause.reference.dataset === doc.id);
        datasets.push({ dataset: doc.id, version: grant.version, write,
          ...(write && grant.reportReview && allowed.reportReview ? { reportReview: true } : {}),
          ...(canHandoff ? { handoffTargets: grant.handoff.targets } : {}) });
        if (datasets.length === 32) break;
      }
      return { datasets };
    });
  }

  async function operate(uid, raw) {
    const parsed = operationRequest.safeParse(raw);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const { operation, stepId, accessVersion, ...proof } = parsed.data;
    const write = operation.action === 'put' || operation.action === 'openReportReview';
    if (write !== Boolean(stepId)) deny(400, 'INVALID_DATA_REQUEST');
    const dataset = operation.action === 'get' ? operation.reference.dataset : operation.dataset;
    const team = db.collection('teams').doc(proof.teamId);
    const taskRef = team.collection('tasks').doc(proof.taskId);
    const id = write ? hash(['data', proof.taskId, stepId]) : null;
    const receipt = id ? team.collection('dataBotOperations').doc(id) : null;
    const digest = hash([accessVersion, operation]);

    async function authorize(tx) {
      const status = await taskAuthority(tx, team, uid, proof);
      const { task } = status;
      // What the Bot published and this Chat leaves it comes first: outside it a live grant is not used, and a write or a report review needs its own flag.
      const allowed = status.scope.datasets.find(row => row.dataset === dataset);
      if (!allowed || (write && !allowed.write) || (operation.action === 'openReportReview' && !allowed.reportReview)) deny();
      const grant = (await tx.get(grants(team, task.sharedBotId).doc(dataset))).data();
      if (grant?.version !== accessVersion) deny();
      const target = await grantScope(tx, team, status, dataset, grant, write);
      const expires = Math.floor(task.deadline / 1000);
      if (expires * 1000 <= clock()) deny();
      return { grant, target, expires, epoch: task.epoch, requester: task.requester, sharedBotId: task.sharedBotId };
    }
    if (operation.action === 'getReport' || operation.action === 'openReportReview') {
      return db.runTransaction(async tx => {
        const { target, grant, requester, sharedBotId } = await authorize(tx);
        if (write && !grant.reportReview) deny();
        const previous = receipt ? (await tx.get(receipt)).data() : null;
        if (previous && previous.digest !== digest) deny(409, 'DATA_CONFLICT');
        const usage = receipt && !previous ? (await tx.get(taskRef)).data()?.operationCount : 0;
        if (!cursor.safeParse(usage).success || usage >= 32) deny(409, 'DATA_CAPACITY');
        const value = await deliverReport(tx, db, requester.uid, { ...target, generation: requester.generation }, operation,
          { uid: requester.uid, sharedBotId, task: proof.taskId });
        if (receipt && !previous) {
          tx.create(receipt, { digest, sharedBotId, taskId: proof.taskId, review: value.review });
          tx.update(taskRef, { operationCount: usage + 1 });
        }
        return value;
      });
    }
    const { grant, target, expires, epoch, answer } = await db.runTransaction(async tx => {
      const admitted = await authorize(tx);
      if (!receipt) return admitted;
      const [pending, task] = await Promise.all([tx.get(receipt), tx.get(taskRef)]);
      if (pending.exists) {
        if (pending.data().digest !== digest) deny(409, 'DATA_CONFLICT');
        return { ...admitted, answer: pending.data().reference };
      }
      const usage = task.data().operationCount;
      if (!cursor.safeParse(usage).success || usage >= 32) deny(409, 'DATA_CAPACITY');
      tx.create(receipt, { digest, taskId: proof.taskId, sharedBotId: task.data().sharedBotId });
      tx.update(taskRef, { operationCount: usage + 1 });
      return admitted;
    });
    if (answer) return answer;
    const token = createHmac('sha256', secret).update(JSON.stringify(['dataset', grant.principalUid, proof.taskId, epoch])).digest('base64url');
    const agent = { task: proof.taskId, expires, tokenDigest: createHash('sha256').update(token).digest('base64url') };
    const credential = await botToken(grant.principalUid);
    const result = await projectActor(grant.principalUid, { ...target, generation: grant.version },
      () => db.runTransaction(authorize), () => forward(credential, id, operation, token), [], agent);
    return db.runTransaction(async tx => {
      await authorize(tx);
      if (receipt) {
        const checked = dataReference.safeParse(result);
        if (!checked.success) deny(503, 'DATA_UNAVAILABLE');
        tx.update(receipt, { reference: checked.data });
      }
      return result;
    });
  }

  async function handoff(uid, raw) {
    const parsed = handoffRequest.safeParse(raw);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const { sharedBotId, accessVersion, reference, stepId, ...proof } = parsed.data;
    const team = db.collection('teams').doc(proof.teamId);
    async function authorize(tx) {
      const status = await taskAuthority(tx, team, uid, proof);
      const { task } = status;
      const parentRef = team.collection('tasks').doc(task.taskId);
      const senderRef = team.collection('sharedBots').doc(task.sharedBotId);
      const receiverRef = team.collection('sharedBots').doc(sharedBotId);
      const [parent, sender, senderGrant, receiver, receiverGrant, requester, room] = (await tx.getAll(
        parentRef, senderRef, grants(team, task.sharedBotId).doc(reference.dataset),
        receiverRef, grants(team, sharedBotId).doc(reference.dataset), team.collection('members').doc(task.requester.uid),
        team.collection('conversations').doc(task.conversationId),
      )).map(doc => doc.data());
      if (!parent || parent.rootTaskId !== task.taskId || parent.dataCause?.kind === 'handoff'
        || senderGrant?.version !== accessVersion || !senderGrant.handoff?.targets.includes(sharedBotId)) deny();
      if (parent.dataCause && (parent.dataCause.reference.dataset !== reference.dataset
        || parent.dataCause.reference.record !== reference.record)) deny();
      await grantScope(tx, team, status, reference.dataset, senderGrant, false);
      if (!receiver || receiver.identity.state !== 'published') deny();
      // The record goes to a task of the receiving Bot, which must be able to read that dataset here: the sender's own read was checked by `operate`.
      if (!effectiveScope(receiver.scope, room?.botScopes?.[sharedBotId]).datasets.some(row => row.dataset === reference.dataset)) deny();
      await grantScope(tx, team, { task, deployment: { revision: receiver.identity.revision } }, reference.dataset, receiverGrant, false);
      return { parent, sender, senderGrant, receiverGrant, requester };
    }
    await db.runTransaction(authorize);
    const { action: _action, ...fields } = proof;
    await operate(uid, { ...fields, action: 'botDataOperation', accessVersion, operation: { action: 'get', reference } });
    return db.runTransaction(async tx => {
      const { parent, sender, senderGrant, receiverGrant, requester } = await authorize(tx);
      const id = hash(['handoff', parent.rootTaskId]);
      const ref = team.collection('dataHandoffs').doc(id);
      const previous = (await tx.get(ref)).data();
      const digest = hash([stepId, reference, sharedBotId, accessVersion, receiverGrant.version]);
      if (previous) {
        if (previous.digest !== digest) deny(409, 'DATA_CONFLICT');
        return previous.result;
      }
      const cause = {
        kind: 'handoff', rootTaskId: parent.rootTaskId, parentTaskId: parent.taskId,
        quotaOwner: parent.sharedBotId, dailyLimit: senderGrant.handoff.dailyLimit, reference,
        targetGrantVersion: receiverGrant.version, author: { kind: 'shared_bot', sharedBotId: parent.sharedBotId, displayName: sender.name },
        ...(parent.dataCause?.ruleId ? { ruleId: parent.dataCause.ruleId, ruleVersion: parent.dataCause.ruleVersion } : {}),
      };
      let receipt;
      try {
        receipt = await store.submit({ tx, team, identity: parent.requester, self: requester, now: clock(), dataCause: cause,
          command: { payload: { conversationId: parent.conversationId, messageId: 'bot_' + id.slice(0, 60),
            changeId: 'bot_' + hash(['change', id]).slice(0, 60), text: 'Data record handed off for follow-up.', targets: [sharedBotId] } },
        });
      } catch (error) {
        if (error instanceof SharedBotError) deny(error.status, error.status === 404 ? 'DATA_NOT_FOUND' : error.status === 403 ? 'DATA_ACCESS_REQUIRED' : 'DATA_CONFLICT');
        throw error;
      }
      const result = { taskId: receipt.tasks[0].taskId, messageId: receipt.messageId, reference };
      tx.create(ref, { digest, result });
      return result;
    });
  }

  async function settings(uid, raw) {
    const parsed = settingsRequest.safeParse(raw);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const { scope: target, command: { dataset } } = parsed.data;
    const team = db.collection('teams').doc(target.teamId);
    return db.runTransaction(async tx => {
      await authorizeDataBinding(tx, db, uid, target, dataset, true, true);
      const [member, room] = (await tx.getAll(team.collection('members').doc(uid),
        team.collection('conversations').doc(target.conversationId))).map(doc => doc.data());
      if (!['owner', 'admin'].includes(member?.role)) deny();
      const bots = [];
      for (const participant of room.participants) {
        if (!participant.startsWith('shared_bot:')) continue;
        const sharedBotId = participant.slice(11);
        const [bot, grant] = (await tx.getAll(team.collection('sharedBots').doc(sharedBotId),
          grants(team, sharedBotId).doc(dataset))).map(doc => doc.data());
        if (bot?.identity?.state !== 'published') continue;
        bots.push({ sharedBotId, name: bot.name, version: grant?.version ?? 0, access: grant?.access ?? null,
          ...(grant?.reportReview ? { reportReview: true } : {}),
          ...(grant?.handoff ? { handoff: grant.handoff } : {}) });
      }
      const rows = await tx.get(db.collection('lingxiDataRules').where('dataset', '==', dataset));
      const rules = rows.docs.map(row => row.data()).filter(entry => entry.scope.teamId === target.teamId
        && entry.scope.conversationId === target.conversationId)
        .map(({ rule, version, config }) => ({ rule, version, config }));
      return { bots, rules };
    }, { readOnly: true });
  }

  return { setAccess, settings, offers, operate, handoff };
}
