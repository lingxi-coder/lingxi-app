import { createHash, randomBytes } from 'node:crypto';
import { admissionId, sharedBotPermits } from './team-execution-contract.mjs';
import { sharedBotCommand } from './shared-bot-commands.mjs';
import { messagePreview } from './team-message-preview.mjs';
import { prepareSharedConversation } from './team-conversations.mjs';

const canonical = value => Array.isArray(value) ? value.map(canonical) : value !== null && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const hash = value => createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex');
const same = (left, right) => hash(left) === hash(right);
const randomId = () => randomBytes(24).toString('base64url');
const nowDay = now => new Date(now).toISOString().slice(0, 10);
const memberIdentity = (uid, member) => ({ uid, generation: member.generation });
const readActions = new Set(['getTask', 'listTasks', 'readArtifact', 'listSharedBots', 'listHostTasks', 'prepareTask', 'validateTask']);
const contains = (audience, identity) => audience.mode === 'team'
  || (audience.mode === 'listed' && audience.members.some(member => same(member, identity)));
const deny = (status, code) => { throw new SharedBotError(status, code); };
export class SharedBotError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
function identityPermitted(bot, identity) { return bot && sharedBotPermits(bot.identity, identity); }
function ownerOf(bot, identity) { return bot && same(bot.identity.maintainer, identity); }
function taskView(task, now) {
  const state = task.deadline <= now && task.state === 'queued' ? 'expired'
    : task.deadline <= now && task.state === 'running' ? 'interrupted' : task.state;
  return { taskId: task.taskId, conversationId: task.conversationId, sharedBotId: task.sharedBotId,
    requester: task.requester, state, createdAt: task.createdAt, deadline: task.deadline,
    ...(task.replyId ? { replyId: task.replyId } : {}), ...(task.artifactId ? { artifactId: task.artifactId } : {}),
    ...(task.usage ? { usage: task.usage } : {}) };
}

/** Execution 2 only. This store reuses the Team membership and message database;
 * all admission/receipt/quota changes commit with their human message. It never
 * interprets old requests or reads old run/trigger collections. */
export class SharedBotsStore {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  async execute(user, raw) {
    const command = sharedBotCommand.parse(raw);
    const { teamId, generation, action, payload: p } = command;
    const requestDigest = hash(command), now = this.clock();
    return this.db.runTransaction(async tx => {
      const team = this.db.doc(`teams/${teamId}`);
      const receiptRef = team.collection('executionReceipts').doc(hash([user.uid, generation, command.commandId]));
      const [teamDoc, selfDoc, receiptDoc] = await tx.getAll(team, team.collection('members').doc(user.uid), receiptRef);
      const self = selfDoc.data(), receipt = receiptDoc.data();
      if (teamDoc.data()?.state !== 'active' || !self) deny(403, 'MEMBERSHIP_REQUIRED');
      if (self.generation !== generation) deny(409, 'STALE_MEMBERSHIP');
      const identity = memberIdentity(user.uid, self);
      if (receipt) {
        if (receipt.requestDigest !== requestDigest) deny(409, 'COMMAND_ID_CONFLICT');
        // A start response is an unrepeatable capability, never a replayable receipt.
        if (action === 'startTask' && receipt.startGranted === true) return { execute: false, taskId: p.taskId };
        // Reads recheck current ACL below; a cached read receipt must not bypass revocation.
        if (action !== 'startTask' && !readActions.has(action)) return receipt.response;
      }
      const context = { tx, team, self, identity, user, command, now };
      let response;
      if (action === 'createConversation') {
        // A friendship's pair team (docs/FRIENDS.md) holds its friend chat and one Bot DM per shared Bot, never a group or channel.
        if (teamDoc.data().kind === 'pair' && p.kind !== 'bot_dm') deny(400, 'PAIR_TEAM');
        const prepared = await prepareSharedConversation(tx, team, identity, p);
        prepared.write(); response = { conversationId: p.conversationId, readers: prepared.record.memberUids };
      }
      else if (action === 'publishSharedBot') response = await this.publish(context);
      else if (action === 'listSharedBots') response = await this.directory(context);
      else if (action === 'pauseSharedBot') response = await this.pause(context);
      else if (action === 'heartbeatDeployment') response = await this.heartbeat(context);
      else if (action === 'activateDeployment') response = await this.activate(context);
      else if (action === 'submit') response = await this.submit(context);
      else if (action === 'listHostTasks') response = await this.hostTasks(context);
      else if (action === 'startTask') response = await this.start(context);
      else if (action === 'prepareTask' || action === 'validateTask') response = await this.taskContext(context);
      else if (action === 'completeTask') response = await this.complete(context);
      else if (action === 'getTask' || action === 'listTasks') response = await this.tasks(context);
      else if (action === 'readArtifact') response = await this.artifact(context);
      else if (action === 'setConversationPolicy') response = await this.conversationPolicy(context);
      else deny(400, 'INVALID_INPUT');
      if (!readActions.has(action)) {
        const saved = { schemaVersion: 1, requestDigest,
          response: action === 'startTask' ? { execute: false, taskId: p.taskId } : response, at: now,
          ...(action === 'startTask' ? { startGranted: response.execute === true } : {}) };
        // A transient ordering/lock denial pins command identity, but it does
        // not consume the only start attempt. Only a committed grant does.
        if (receipt) tx.set(receiptRef, saved); else tx.create(receiptRef, saved);
      }
      return response;
    }, { maxAttempts: 20 });
  }
  async publish({ tx, team, identity, command: { payload: p }, now }) {
    const ref = team.collection('sharedBots').doc(p.sharedBotId), existing = (await tx.get(ref)).data();
    if (existing && !ownerOf(existing, identity)) deny(403, 'MAINTAINER_REQUIRED');
    if ((existing?.identity.revision ?? 0) !== p.expectedRevision) deny(409, 'BOT_REVISION_CONFLICT');
    const d = p.deployment;
    if (d.sharedBotId !== p.sharedBotId || !same(d.owner, identity) || !d.enabled) deny(400, 'INVALID_DEPLOYMENT');
    const deploymentRef = team.collection('deployments').doc(d.deploymentId);
    const previousDeployment = (await tx.get(deploymentRef)).data();
    if (previousDeployment && (!same(previousDeployment.owner, identity) || previousDeployment.sharedBotId !== p.sharedBotId
      || d.revision <= previousDeployment.revision)) deny(409, 'DEPLOYMENT_REVISION_CONFLICT');
    const members = (await tx.get(team.collection('members'))).docs.map(doc => memberIdentity(doc.id, doc.data()));
    if (p.usePolicy.mode === 'listed' && p.usePolicy.members.some(value => !members.some(member => same(value, member)))) deny(403, 'CURRENT_RECIPIENT_REQUIRED');
    const audience = p.usePolicy.mode === 'all' ? members : p.usePolicy.members;
    // Persona is newly authored for this publication. Named files/Skills retain
    // their narrower source audience; publication cannot declassify private data.
    const grants = p.inputs.map(input => {
      const permitted = input.kind === 'persona' ? audience : audience.filter(member =>
        input.sourceAudience.mode === 'owner' ? same(member, identity) : contains(input.sourceAudience, member));
      return { grantId: hash([p.sharedBotId, p.expectedRevision + 1, input.resourceId]), ...input,
        audience: permitted, usePolicy: p.usePolicy, publisher: identity };
    });
    if (grants.some(grant => audience.some(member => !this.grantPermits(grant, member)))) deny(403, 'INPUT_AUDIENCE_NARROWER');
    const identityRecord = { schemaVersion: 1, sharedBotId: p.sharedBotId, teamId: team.id, maintainer: identity,
      revision: p.expectedRevision + 1, state: 'published', usePolicy: p.usePolicy, activeDeploymentId: d.deploymentId };
    if (Buffer.byteLength(JSON.stringify({ identity: identityRecord, name: p.name, description: p.description, grants })) > 750000) deny(400, 'PUBLICATION_TOO_LARGE');
    tx.set(ref, { identity: identityRecord, name: p.name, description: p.description, grants, quotas: p.quotas, publishedAt: now });
    tx.set(deploymentRef, { ...d, ready: false, onlineUntil: 0 });
    tx.set(team.collection('signals').doc(p.sharedBotId), { updatedAt: new Date(now) });
    return { sharedBotId: p.sharedBotId, revision: identityRecord.revision, grantIds: grants.map(g => g.grantId) };
  }
  async directory({ tx, team, identity, now }) {
    const bots = await tx.get(team.collection('sharedBots').orderBy('__name__').limit(200));
    const deployments = bots.empty ? [] : await tx.getAll(...bots.docs.map(doc => team.collection('deployments').doc(doc.data().identity.activeDeploymentId)));
    const maintainers = bots.empty ? [] : await tx.getAll(...bots.docs.map(doc => team.collection('members').doc(doc.data().identity.maintainer.uid)));
    return { bots: bots.docs.flatMap((doc, index) => {
      const b = doc.data(), d = deployments[index].data();
      if (!identityPermitted(b, identity) && !ownerOf(b, identity)) return [];
      const activeOwner = maintainers[index].data()?.generation === b.identity.maintainer.generation;
      return [{ sharedBotId: doc.id, name: b.name, description: b.description, revision: b.identity.revision,
        permitted: activeOwner && identityPermitted(b, identity), ready: activeOwner && b.identity.state === 'published' && !!d?.ready && d.enabled && d.onlineUntil > now,
        maintainer: b.identity.maintainer, usePolicy: b.identity.usePolicy, state: b.identity.state,
        onlineUntil: d?.onlineUntil ?? 0, quotas: b.quotas,
        ...(ownerOf(b, identity) && d ? { instanceId: d.instanceId } : {}) }];
    }) };
  }
  async pause({ tx, team, identity, command: { payload: p }, now }) {
    const ref = team.collection('sharedBots').doc(p.sharedBotId), bot = (await tx.get(ref)).data();
    if (!ownerOf(bot, identity)) deny(403, 'MAINTAINER_REQUIRED');
    if (bot.identity.revision !== p.expectedRevision) deny(409, 'BOT_REVISION_CONFLICT');
    const revision = p.expectedRevision + 1;
    tx.update(ref, { 'identity.state': 'paused', 'identity.revision': revision });
    tx.set(team.collection('signals').doc(p.sharedBotId), { updatedAt: new Date(now) });
    return { sharedBotId: p.sharedBotId, revision };
  }
  async checkHost(tx, team, identity, p) {
    const ref = team.collection('deployments').doc(p.deploymentId), doc = await tx.get(ref), deployment = doc.data();
    if (!deployment || !same(deployment.owner, identity) || deployment.hostId !== p.hostId
      || deployment.accountEpoch !== p.accountEpoch || !deployment.enabled || deployment.executionVersion !== 2) deny(403, 'DEPLOYMENT_REQUIRED');
    const bot = (await tx.get(team.collection('sharedBots').doc(deployment.sharedBotId))).data();
    if (!bot || bot.identity.activeDeploymentId !== p.deploymentId || !ownerOf(bot, identity)
      || bot.identity.state !== 'published') deny(403, 'DEPLOYMENT_REQUIRED');
    return { ref, deployment, bot };
  }
  async heartbeat({ tx, team, identity, command: { payload: p }, now }) {
    const { ref, deployment } = await this.checkHost(tx, team, identity, p);
    tx.update(ref, { ready: p.ready, onlineUntil: p.ready ? now + 90000 : 0 });
    tx.set(team.collection('signals').doc(deployment.sharedBotId), { updatedAt: new Date(now) });
    return { onlineUntil: p.ready ? now + 90000 : 0 };
  }
  async activate({ tx, team, identity, command: { payload: p }, now }) {
    const { ref, deployment } = await this.checkHost(tx, team, identity, { ...p, accountEpoch: p.expectedAccountEpoch });
    tx.update(ref, { accountEpoch: p.newAccountEpoch, ready: false, onlineUntil: 0 });
    tx.set(team.collection('signals').doc(deployment.sharedBotId), { updatedAt: new Date(now) });
    return { accountEpoch: p.newAccountEpoch };
  }
  async conversation(tx, team, id, identity, mustParticipate = false) {
    const ref = team.collection('conversations').doc(id), doc = await tx.get(ref), value = doc.data();
    if (!value?.memberUids?.includes(identity.uid) || (mustParticipate && !value.participants.includes(`user:${identity.uid}`))) deny(403, 'CONVERSATION_REQUIRED');
    return { ref, value };
  }
  async submit({ tx, team, identity, self, command: { payload: p }, now }) {
    const { ref, value: c } = await this.conversation(tx, team, p.conversationId, identity, true);
    if (p.messageId.startsWith('bot_') || p.changeId.startsWith('bot_')) deny(400, 'INVALID_INPUT');
    if (p.targets.some(id => !c.participants.includes(`shared_bot:${id}`))) deny(403, 'BOT_PARTICIPANT_REQUIRED');
    if (c.kind === 'bot_dm' && (p.targets.length !== 1 || p.targets[0] !== c.sharedBotId)) deny(400, 'INVALID_BOT_TARGET');
    if (c.executionPolicy && p.targets.some(id => !c.executionPolicy.targets.includes(id))) deny(403, 'CONVERSATION_POLICY_REQUIRED');
    const [message, change] = await tx.getAll(ref.collection('messages').doc(p.messageId), ref.collection('changes').doc(p.changeId));
    const digest = hash({ requester: identity, ...p, targets: [...p.targets].sort() });
    if (message.exists || change.exists) {
      if (change.data()?.submissionDigest !== digest) deny(409, 'MESSAGE_ID_CONFLICT');
      return change.data().submissionReceipt;
    }
    if (p.replyTo && !(await tx.get(ref.collection('messages').doc(p.replyTo))).exists) deny(400, 'REPLY_REQUIRED');
    const bots = await tx.getAll(...p.targets.map(id => team.collection('sharedBots').doc(id)));
    const owners = await tx.getAll(...bots.map(doc => team.collection('members').doc(doc.data()?.identity.maintainer.uid ?? '_missing')));
    const deployments = await tx.getAll(...bots.map(doc => team.collection('deployments').doc(doc.data()?.identity.activeDeploymentId ?? '_missing')));
    for (const [index, snapshot] of bots.entries()) {
      const bot = snapshot.data(), d = deployments[index].data();
      if (!identityPermitted(bot, identity) || owners[index].data()?.generation !== bot.identity.maintainer.generation) deny(403, 'BOT_USE_REQUIRED');
      if (!d?.enabled || !d.ready || d.onlineUntil <= now) deny(409, 'HOST_UNAVAILABLE');
      if (bot.grants.some(grant => !this.grantPermits(grant, identity))) deny(403, 'INPUT_GRANT_REQUIRED');
    }
    const day = nowDay(now), quotaKeys = [hash([day, 'team']), hash([day, 'member', identity.uid, identity.generation]),
      ...p.targets.map(id => hash([day, 'bot', id]))];
    const quotaRefs = quotaKeys.map(id => team.collection('requestQuotas').doc(id));
    const quotas = await tx.getAll(...quotaRefs);
    const limits = [Math.min(...bots.map(doc => doc.data().quotas.team)), Math.min(...bots.map(doc => doc.data().quotas.member)),
      ...bots.map(doc => doc.data().quotas.bot)];
    const increments = [p.targets.length, p.targets.length, ...p.targets.map(() => 1)];
    for (const [i, snapshot] of quotas.entries()) if ((snapshot.data()?.used ?? 0) + increments[i] > limits[i]) deny(429, 'REQUEST_QUOTA_EXCEEDED');
    const seq = c.lastSeq + 1, changeSeq = c.lastChangeSeq + 1;
    if (!Number.isSafeInteger(seq) || !Number.isSafeInteger(changeSeq)) deny(409, 'CURSOR_LIMIT');
    const tasks = p.targets.map(sharedBotId => ({ sharedBotId, taskId: admissionId(p.messageId, sharedBotId) }));
    const response = { messageId: p.messageId, changeId: p.changeId, changeSeq, tasks };
    const author = { kind: 'user', uid: identity.uid, displayName: self.displayName || 'Member' }, at = new Date(now);
    tx.create(ref.collection('messages').doc(p.messageId), { seq, changeSeq, changeId: p.changeId, text: p.text, kind: 'text', author,
      createdAt: at, taskIds: tasks.map(task => task.taskId), mentions: p.targets.map(id => `shared_bot:${id}`), ...(p.replyTo ? { replyTo: p.replyTo } : {}) });
    tx.create(ref.collection('changes').doc(p.changeId), { seq: changeSeq, messageId: p.messageId, operation: 'create',
      actorUid: identity.uid, generation: identity.generation, textDigest: createHash('sha256').update(p.text).digest('hex'), at,
      submissionDigest: digest, submissionReceipt: response });
    tx.update(ref, { lastSeq: seq, lastChangeSeq: changeSeq, lastChange: { id: p.changeId, seq: changeSeq },
      lastMessage: { id: p.messageId, seq, at, author, preview: messagePreview(p.text) } });
    for (const [i, target] of tasks.entries()) {
      const b = bots[i].data();
      tx.create(team.collection('tasks').doc(target.taskId), { schemaVersion: 1, executionVersion: 2, ...target,
        conversationId: p.conversationId, messageId: p.messageId, changeId: p.changeId, messageSeq: seq,
        requester: identity, botRevision: b.identity.revision, deploymentId: b.identity.activeDeploymentId,
        state: 'queued', createdAt: now, deadline: now + 300000, inputGrantIds: b.grants.map(g => g.grantId) });
    }
    for (const [i, quotaRef] of quotaRefs.entries()) tx.set(quotaRef, { day, used: (quotas[i].data()?.used ?? 0) + increments[i], limit: limits[i] });
    return response;
  }
  grantPermits(grant, identity) {
    // all-current-member grants remain usable after unrelated new members join,
    // but the source restriction is still checked against each current identity.
    const use = grant.usePolicy.mode === 'all' || grant.usePolicy.members.some(member => same(member, identity));
    const source = grant.kind === 'persona' || (grant.sourceAudience.mode === 'owner'
      ? same(identity, grant.publisher) : contains(grant.sourceAudience, identity));
    return same(identity, grant.publisher) || (use && source);
  }
  async hostTasks({ tx, team, identity, command: { payload: p }, now }) {
    await this.checkHost(tx, team, identity, p);
    let query = team.collection('tasks').where('deploymentId', '==', p.deploymentId)
      .where('state', 'in', ['queued', 'running']).orderBy('__name__');
    if (p.after) query = query.startAfter(p.after);
    const tasks = await tx.get(query.limit(100));
    return { tasks: tasks.docs.filter(doc => doc.data().state === 'running' || doc.data().deadline > now).map(doc => ({ ...taskView(doc.data(), now),
      messageId: doc.data().messageId, messageSeq: doc.data().messageSeq })).sort((a, b) => a.messageSeq - b.messageSeq),
      nextCursor: tasks.size === 100 ? tasks.docs.at(-1).id : null };
  }
  async taskContext({ tx, team, identity, command: { action, payload: p }, now }) {
    const { bot, deployment } = await this.checkHost(tx, team, identity, p);
    const task = (await tx.get(team.collection('tasks').doc(p.taskId))).data();
    if (!task || task.deploymentId !== p.deploymentId) deny(404, 'TASK_REQUIRED');
    if (action === 'validateTask' && (task.state !== 'running' || task.permitDigest !== hash(p.permitId))) deny(403, 'EXECUTION_PERMIT_REQUIRED');
    if (action === 'prepareTask' && task.state !== 'queued') deny(409, 'TASK_ALREADY_STARTED');
    if (task.deadline <= now) deny(409, 'TASK_EXPIRED');
    if (!identityPermitted(bot, task.requester) || task.botRevision !== bot.identity.revision
      || bot.grants.some(grant => !this.grantPermits(grant, task.requester))) deny(403, 'TASK_REVOKED');
    const conversation = await this.validateTask(tx, team, task);
    if (action === 'validateTask' && !await this.sourceWatermarkCurrent(tx, conversation, task, p.expectedLastChangeSeq)) deny(409, 'SOURCE_CHANGED');
    return { task: taskView(task, now), messageId: task.messageId, messageSeq: task.messageSeq,
      inputs: bot.grants, deployment, sourceLastChangeSeq: conversation.data().lastChangeSeq };
  }
  async sourceWatermarkCurrent(tx, conversation, task, watermark) {
    const current = conversation.data().lastChangeSeq;
    if (!Number.isSafeInteger(watermark) || watermark < 0 || watermark > current) deny(409, 'INVALID_CHANGE_CURSOR');
    let after = watermark;
    while (after < current) {
      const size = Math.min(200, current - after);
      const page = await tx.get(conversation.ref.collection('changes').where('seq', '>', after).where('seq', '<=', current).orderBy('seq').limit(size));
      if (page.size !== size || page.docs.some((doc, index) => doc.data().seq !== after + index + 1)) deny(503, 'CHANGE_GAP');
      const mutations = page.docs.filter(doc => ['edit', 'recall'].includes(doc.data().operation));
      if (mutations.length) {
        const messages = await tx.getAll(...mutations.map(doc => conversation.ref.collection('messages').doc(doc.data().messageId)));
        if (messages.some(doc => !doc.exists)) deny(503, 'MESSAGE_GAP');
        if (messages.some(doc => doc.data().seq <= task.messageSeq)) return false;
      }
      after += size;
    }
    return true;
  }
  async validateTask(tx, team, task) {
    const [member, conversation, message] = await tx.getAll(team.collection('members').doc(task.requester.uid),
      team.collection('conversations').doc(task.conversationId), team.collection('conversations').doc(task.conversationId).collection('messages').doc(task.messageId));
    if (member.data()?.generation !== task.requester.generation || !conversation.data()?.memberUids.includes(task.requester.uid)
      || !conversation.data().participants.includes(`shared_bot:${task.sharedBotId}`)
      || (conversation.data().executionPolicy && !conversation.data().executionPolicy.targets.includes(task.sharedBotId))
      || message.data()?.recalled || message.data()?.changeId !== task.changeId) deny(403, 'TASK_REVOKED');
    return conversation;
  }
  async start({ tx, team, identity, command: { payload: p }, now }) {
    const { deployment, bot } = await this.checkHost(tx, team, identity, p);
    const ref = team.collection('tasks').doc(p.taskId), snapshot = await tx.get(ref), task = snapshot.data();
    if (!task || task.deploymentId !== p.deploymentId) deny(404, 'TASK_REQUIRED');
    if (task.state !== 'queued') return { execute: false, taskId: p.taskId };
    if (task.deadline <= now) { tx.update(ref, { state: 'expired' }); return { execute: false, taskId: p.taskId }; }
    if (!deployment.ready || deployment.onlineUntil <= now || task.botRevision !== bot.identity.revision
      || !identityPermitted(bot, task.requester) || bot.grants.some(grant => !this.grantPermits(grant, task.requester))) deny(403, 'TASK_REVOKED');
    const conversation = await this.validateTask(tx, team, task);
    if (!await this.sourceWatermarkCurrent(tx, conversation, task, p.expectedLastChangeSeq)) deny(409, 'SOURCE_CHANGED');
    const lockRef = team.collection('chatExecutionLocks').doc(hash([task.sharedBotId, task.conversationId]));
    const lock = (await tx.get(lockRef)).data();
    if (lock && lock.until > now) return { execute: false, taskId: p.taskId };
    const earlier = await tx.get(team.collection('tasks').where('sharedBotId', '==', task.sharedBotId)
      .where('conversationId', '==', task.conversationId).where('state', '==', 'queued')
      .where('deadline', '>', now).where('messageSeq', '<', task.messageSeq).limit(1));
    if (!earlier.empty) return { execute: false, taskId: p.taskId };
    const permitId = randomId();
    tx.set(lockRef, { taskId: p.taskId, until: task.deadline });
    tx.update(ref, { state: 'running', startedAt: now, permitDigest: hash(permitId), hostId: p.hostId, accountEpoch: p.accountEpoch,
      inputLastChangeSeq: p.expectedLastChangeSeq });
    tx.set(team.collection('signals').doc(task.sharedBotId), { updatedAt: new Date(now) });
    return { execute: true, permitId, task: taskView({ ...task, state: 'running' }, now), deployment,
      inputs: bot.grants, messageId: task.messageId, messageSeq: task.messageSeq };
  }
  async complete({ tx, team, identity, command: { payload: p }, now }) {
    const { bot } = await this.checkHost(tx, team, identity, p);
    const ref = team.collection('tasks').doc(p.taskId), task = (await tx.get(ref)).data();
    if (!task || task.deploymentId !== p.deploymentId || task.permitDigest !== hash(p.permitId)) deny(403, 'EXECUTION_PERMIT_REQUIRED');
    const { accountEpoch: _accountEpoch, ...stableResult } = p;
    const resultDigest = hash(stableResult);
    if (task.resultDigest) {
      if (task.resultDigest !== resultDigest) deny(409, 'TASK_RESULT_CONFLICT');
      return { taskId: task.taskId, state: task.state, replyId: task.replyId ?? null, artifactId: task.artifactId ?? null };
    }
    if (task.state !== 'running' || p.finishedAt < task.startedAt || p.finishedAt > task.deadline || p.finishedAt > now) deny(409, 'TASK_EXPIRED');
    const conversation = await this.validateTask(tx, team, task), c = conversation.data();
    const sourceCurrent = await this.sourceWatermarkCurrent(tx, conversation, task, task.inputLastChangeSeq);
    const members = c.memberUids.length ? await tx.getAll(...c.memberUids.map(uid => team.collection('members').doc(uid))) : [];
    const readers = members.filter(doc => doc.exists).map(doc => memberIdentity(doc.id, doc.data()));
    const grantsMatch = same([...p.inputGrantIds].sort(), [...task.inputGrantIds].sort()) && task.botRevision === bot.identity.revision;
    const allowed = sourceCurrent && p.outputAudience.mode === 'published_inputs' && grantsMatch && identityPermitted(bot, task.requester)
      && readers.every(reader => bot.grants.every(grant => this.grantPermits(grant, reader)));
    const replyId = `bot_${hash([p.taskId, 'reply']).slice(0, 60)}`, changeId = `bot_${hash([p.taskId, 'reply-change']).slice(0, 60)}`;
    const body = allowed ? p.text : sourceCurrent ? '结果需要授权，请联系助手维护者。' : '消息已更新，本次结果未发布。请重新提问。';
    const seq = c.lastSeq + 1, changeSeq = c.lastChangeSeq + 1, at = new Date(now);
    if (!Number.isSafeInteger(seq) || !Number.isSafeInteger(changeSeq)) deny(409, 'CURSOR_LIMIT');
    const author = { kind: 'shared_bot', sharedBotId: task.sharedBotId, displayName: bot.name };
    const artifactId = allowed && p.document ? hash([p.taskId, 'document']) : null;
    const lockRef = team.collection('chatExecutionLocks').doc(hash([task.sharedBotId, task.conversationId]));
    const lock = (await tx.get(lockRef)).data();
    tx.create(conversation.ref.collection('messages').doc(replyId), { seq, changeSeq, changeId, author, kind: 'text', text: body,
      taskId: task.taskId, deliveryId: hash([task.taskId, 'delivery']), replyTo: task.messageId, createdAt: at, ...(artifactId ? { artifactId } : {}) });
    tx.create(conversation.ref.collection('changes').doc(changeId), { seq: changeSeq, messageId: replyId, operation: 'create',
      actorUid: identity.uid, generation: identity.generation, at, textDigest: createHash('sha256').update(body).digest('hex') });
    tx.update(conversation.ref, { lastSeq: seq, lastChangeSeq: changeSeq, lastChange: { id: changeId, seq: changeSeq },
      lastMessage: { id: replyId, seq, at, author, preview: messagePreview(body) } });
    if (artifactId) tx.create(team.collection('artifacts').doc(artifactId), { schemaVersion: 1, artifactId, taskId: task.taskId,
      sharedBotId: task.sharedBotId, conversationId: task.conversationId, botRevision: task.botRevision,
      revision: 1, digest: hash(p.document), document: p.document, inputGrantIds: task.inputGrantIds,
      readers, createdAt: now });
    const state = allowed ? p.outcome : sourceCurrent ? 'authorization_required' : 'failed';
    // No restricted body, document or diagnostic trace is persisted in shared projections/receipts.
    tx.update(ref, { state, resultDigest, replyId, artifactId, usage: p.usage, finishedAt: p.finishedAt, deliveredAt: now });
    if (lock?.taskId === p.taskId) tx.delete(lockRef);
    return { taskId: task.taskId, state, replyId, artifactId };
  }
  async tasks({ tx, team, identity, command: { action, payload: p }, now }) {
    if (action === 'getTask') {
      const task = (await tx.get(team.collection('tasks').doc(p.taskId))).data();
      if (!task) deny(404, 'TASK_REQUIRED');
      await this.conversation(tx, team, task.conversationId, identity);
      return taskView(task, now);
    }
    if (p.conversationId) await this.conversation(tx, team, p.conversationId, identity);
    let query = team.collection('tasks');
    if (p.conversationId) query = query.where('conversationId', '==', p.conversationId);
    else query = query.where('requester.uid', '==', identity.uid).where('requester.generation', '==', identity.generation);
    query = query.orderBy('createdAt', 'desc').orderBy('__name__').limit(101);
    if (p.after) {
      const cursor = (await tx.get(team.collection('tasks').doc(p.after))).data();
      if (!cursor || (p.conversationId ? cursor.conversationId !== p.conversationId : !same(cursor.requester, identity))) deny(400, 'INVALID_TASK_CURSOR');
      query = query.startAfter(cursor.createdAt, p.after);
    }
    const page = await tx.get(query), visible = [];
    const docs = page.docs.slice(0, 100);
    const chats = docs.length ? await tx.getAll(...docs.map(doc => team.collection('conversations').doc(doc.data().conversationId))) : [];
    for (const [i, doc] of docs.entries()) if (chats[i].data()?.memberUids.includes(identity.uid)) visible.push(taskView(doc.data(), now));
    return { tasks: visible, next: page.size > 100 ? docs.at(-1).id : null };
  }
  async artifact({ tx, team, identity, command: { payload: p } }) {
    const artifact = (await tx.get(team.collection('artifacts').doc(p.artifactId))).data();
    if (!artifact || !artifact.readers.some(member => same(member, identity))) deny(403, 'ARTIFACT_READ_REQUIRED');
    await this.conversation(tx, team, artifact.conversationId, identity);
    const bot = (await tx.get(team.collection('sharedBots').doc(artifact.sharedBotId))).data();
    if ((!identityPermitted(bot, identity) && !ownerOf(bot, identity)) || artifact.botRevision !== bot.identity.revision
      || bot.grants.some(grant => !this.grantPermits(grant, identity))) deny(403, 'ARTIFACT_READ_REQUIRED');
    return { artifactId: p.artifactId, revision: artifact.revision, digest: artifact.digest, document: artifact.document };
  }
  async conversationPolicy({ tx, team, self, identity, command: { payload: p } }) {
    if (!['owner', 'admin'].includes(self.role)) deny(403, 'MANAGER_REQUIRED');
    const { ref, value } = await this.conversation(tx, team, p.conversationId, identity);
    if ((value.executionPolicy?.revision ?? 0) !== p.expectedRevision) deny(409, 'POLICY_REVISION_CONFLICT');
    if (p.targets.some(id => !value.participants.includes(`shared_bot:${id}`))) deny(403, 'BOT_PARTICIPANT_REQUIRED');
    const revision = p.expectedRevision + 1;
    tx.update(ref, { executionPolicy: { revision, targets: p.targets } });
    return { revision };
  }
}
