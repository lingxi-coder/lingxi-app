import { createHash } from 'node:crypto';
import { z } from 'zod';
import { displayName } from './teams.mjs';

/**
 * A Bot offered to people outside its publisher's Teams (detailed design 17, HostedBotService). Not the local BotService.
 *
 * A service is a scope of its own, `services/{serviceId}`. It holds nothing of any Team: one published Bot, the people who
 * may use it, and one conversation per session. A caller is let in by the service's audience, not by an invitation, and is
 * never a member of the publisher's Team. Inside the scope the Team execution ledger (`shared-bots.mjs`) runs unchanged on
 * the same record shapes, so admission, quotas, permits, waits and operations are the same code as in a Team; what a
 * service adds is here: who may enter, the off switch, concurrency, reports, and which ledger commands the scope takes.
 *
 * Records under `services/{serviceId}`:
 * - the service itself: publisher, name, audience, limits, `online` (off until the publisher turns it on);
 * - `members/{uid}`: the publisher (`role: 'owner'`) and each caller's entitlement (`role: 'member'`, with its generation);
 * - `entitlements/{uid}`: the last generation a caller held, so that coming back after losing access is a new generation;
 * - `active/{uid}` and `active/_service`: the tasks counted against the concurrency limits;
 * - what the ledger writes: `sharedBots`, `deployments`, `conversations` (the sessions), `tasks`, `requestQuotas`, …
 * Clients read none of this directly (the rules deny it): everything goes through the API.
 */
const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const positive = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
const unique = values => new Set(values).size === values.length;
const audience = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('public') }).strict(),
  z.object({ mode: z.literal('users'), uids: z.array(uid).max(200).refine(unique) }).strict(),
]);
/** How many of one caller's requests, and of everyone's, may be under way at once. */
const limits = z.object({ userConcurrent: z.number().int().min(1).max(5), serviceConcurrent: z.number().int().min(1).max(50) }).strict();
const name = z.string().trim().min(1).max(80), description = z.string().max(500);
/** What the publisher says is kept, and for how long. Shown to a caller before they enter. */
const retention = z.string().trim().min(1).max(500);
const scope = { serviceId: id };
export const serviceCommand = z.discriminatedUnion('action', [
  z.object({ action: z.literal('createService'), ...scope, sharedBotId: id, name, description, audience, limits, retention }).strict(),
  z.object({ action: z.literal('updateService'), ...scope, expectedRevision: positive, name: name.optional(), description: description.optional(),
    audience: audience.optional(), limits: limits.optional(), retention: retention.optional() }).strict(),
  z.object({ action: z.literal('setServiceOnline'), ...scope, online: z.boolean() }).strict(),
  z.object({ action: z.literal('withdrawService'), ...scope }).strict(),
  z.object({ action: z.literal('getService'), ...scope }).strict(),
  // The caller has read who can see the session, who pays and what is kept.
  z.object({ action: z.literal('enterService'), ...scope, acknowledged: z.literal(true) }).strict(),
  z.object({ action: z.literal('leaveService'), ...scope }).strict(),
  z.object({ action: z.literal('reportService'), ...scope, reportId: id, reason: z.enum(['harmful', 'deceptive', 'privacy', 'spam', 'other']),
    text: z.string().max(1000), sessionId: id.optional() }).strict(),
  z.object({ action: z.literal('listServices') }).strict(),
]);

export class HostedBotServiceError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
const deny = (status, code) => { throw new HostedBotServiceError(status, code); };
const hash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
const day = now => new Date(now).toISOString().slice(0, 10);
const includes = (a, user) => a.mode === 'public' || a.uids.includes(user);
const publisherLimit = 20, reportsPerDay = 5, sessionLimit = 50;

/**
 * What each ledger command is in a service: the caller's, the publisher's (its host), or not offered. A command that is
 * not listed is refused: nothing made for a Team reaches a service by default, and none falls back to the publisher's
 * authority (design 17).
 */
/** Reading a session's messages (`team-messages.mjs`): the caller reads their own, the publisher every one, as each caller was told on entering. */
export const serviceMessageActions = new Set(['list', 'sync', 'read']);
const callerActions = new Set(['createConversation', 'submit', 'answerWait', 'decideWait', 'cancelTask', 'getTask', 'listTasks']);
const publisherActions = new Set(['publishSharedBot', 'pauseSharedBot', 'heartbeatDeployment', 'activateDeployment', 'listHostTasks',
  'prepareTask', 'validateTask', 'startTask', 'continueTask', 'pauseTask', 'completeTask',
  'recordOperation', 'settleOperation', 'resolveOperation', 'listOperations', 'getTask', 'listTasks']);
/** Commands that give a run its next piece of work: refused while the service is off. */
const admitting = new Set(['submit', 'startTask', 'continueTask']);
const live = new Set(['queued', 'running', 'waiting']);

/**
 * The service scope of the execution ledger. `enter` is called inside the ledger's transaction before a command is carried
 * out, with the service record and the caller's entitlement already read.
 */
export const serviceScope = {
  root: (db, serviceId) => db.doc(`services/${serviceId}`),
  /** Whether a root is in a state the ledger may work in, and the error when it is not. */
  check(service, self, user) {
    if (!service || service.kind !== 'service') deny(404, 'NOT_FOUND');
    // Someone the audience names has to enter first; to anyone else the service does not exist.
    if (!self) includes(service.audience, user) ? deny(403, 'SERVICE_ENTRY_REQUIRED') : deny(404, 'NOT_FOUND');
    if (service.state !== 'active') deny(410, 'SERVICE_WITHDRAWN');
    if (self.role !== 'owner' && !includes(service.audience, user)) deny(403, 'SERVICE_ACCESS_REQUIRED');
  },
  /**
   * Refuses what the scope does not take and applies the service's own gates. `liveState(task)` is the ledger's view of
   * a task's state now. Returns what to write once the command has been carried out.
   */
  async enter({ tx, root, service, self, identity, action, payload: p, now, liveState }) {
    const owner = self.role === 'owner';
    if (!(owner ? publisherActions : callerActions).has(action)) deny(403, 'SERVICE_SCOPE_UNSUPPORTED');
    if (admitting.has(action) && !service.online) deny(409, 'SERVICE_DISABLED');
    if (action === 'publishSharedBot' && (p.sharedBotId !== service.sharedBotId || p.usePolicy.mode !== 'all' || p.sharedList
      || p.inputs.some(input => input.kind !== 'persona' && input.sourceAudience.mode !== 'team'))) deny(400, 'INVALID_SERVICE_PUBLICATION');
    if (action === 'createConversation') {
      // A session is the caller's own conversation with the service's Bot, and nothing else.
      if (p.kind !== 'bot_dm' || p.sharedBotIds[0] !== service.sharedBotId) deny(400, 'INVALID_SERVICE_SESSION');
      const sessions = root.collection('conversations');
      if (!(await tx.get(sessions.doc(p.conversationId))).exists
        && (await tx.get(sessions.where('memberUids', 'array-contains', identity.uid).limit(sessionLimit))).size >= sessionLimit) deny(409, 'SERVICE_SESSION_LIMIT');
    }
    if (action !== 'submit' || !p.targets.length) return () => {};
    if (p.attachments?.length) deny(400, 'SERVICE_SCOPE_UNSUPPORTED');
    // A message the ledger already has was counted when it was first sent: sending it again asks for nothing new.
    if ((await tx.get(root.collection('conversations').doc(p.conversationId).collection('messages').doc(p.messageId))).exists) return () => {};
    // Concurrency: the tasks last counted for this caller and for the service, less those that have ended since.
    const mine = root.collection('active').doc(identity.uid), all = root.collection('active').doc('_service');
    const [mineDoc, allDoc] = await tx.getAll(mine, all);
    const stillLive = async ids => {
      const docs = ids.length ? await tx.getAll(...ids.map(taskId => root.collection('tasks').doc(taskId))) : [];
      return ids.filter((_, index) => docs[index].exists && live.has(liveState(docs[index].data())));
    };
    const [own, everyone] = [await stillLive(mineDoc.data()?.taskIds ?? []), await stillLive(allDoc.data()?.taskIds ?? [])];
    return response => {
      const added = response.tasks.map(task => task.taskId);
      if (own.length + added.length > service.limits.userConcurrent) deny(429, 'SERVICE_USER_BUSY');
      if (everyone.length + added.length > service.limits.serviceConcurrent) deny(429, 'SERVICE_BUSY');
      tx.set(mine, { taskIds: [...own, ...added], updatedAt: now });
      tx.set(all, { taskIds: [...everyone, ...added], updatedAt: now });
    };
  },
};

/** The commands about a service itself. Sessions and tasks go through the execution ledger with `serviceId` as their scope. */
export class HostedBotServices {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  async execute(user, raw) {
    const input = serviceCommand.parse(raw), now = this.clock();
    if (input.action === 'listServices') return this.list(user.uid, now);
    const root = serviceScope.root(this.db, input.serviceId);
    const response = await this.db.runTransaction(async tx => {
      const [serviceDoc, selfDoc] = await tx.getAll(root, root.collection('members').doc(user.uid));
      const service = serviceDoc.data(), self = selfDoc.data();
      if (input.action === 'createService') return this.create(tx, root, service, user, input, now);
      // A service a person may not use does not exist for them: a guessed id tells nothing.
      const mine = service?.publisher === user.uid;
      if (!service || service.kind !== 'service' || (!mine && !includes(service.audience, user.uid))) deny(404, 'NOT_FOUND');
      if (['updateService', 'setServiceOnline', 'withdrawService'].includes(input.action)) {
        if (!mine) deny(403, 'PUBLISHER_REQUIRED');
        if (service.state !== 'active') deny(410, 'SERVICE_WITHDRAWN');
        return this.manage(tx, root, service, input, now);
      }
      if (input.action === 'getService') return this.view(tx, root, service, self, user.uid, now);
      if (input.action === 'reportService') return this.report(tx, root, service, user, input, now);
      if (service.state !== 'active') deny(410, 'SERVICE_WITHDRAWN');
      if (input.action === 'leaveService') {
        if (mine) deny(400, 'PUBLISHER_REQUIRED');
        if (self) this.revoke(tx, root, user.uid, self);
        return { left: true };
      }
      // enterService
      if (mine) return { generation: self.generation, service: await this.view(tx, root, service, self, user.uid, now) };
      if (self) return { generation: self.generation, service: await this.view(tx, root, service, self, user.uid, now) };
      const before = (await tx.get(root.collection('entitlements').doc(user.uid))).data();
      const entitled = { role: 'member', displayName: displayName(user), generation: (before?.generation ?? 0) + 1, enteredAt: now };
      const entered = { generation: entitled.generation, service: await this.view(tx, root, service, entitled, user.uid, now) };
      tx.create(root.collection('members').doc(user.uid), entitled);
      tx.set(this.db.doc(`serviceUsers/${user.uid}/entered/${input.serviceId}`), { enteredAt: now });
      return entered;
    }, { maxAttempts: 20 });
    if (response.sweep) { await this.sweep(root); delete response.sweep; }
    return response;
  }

  async create(tx, root, existing, user, input, now) {
    const { action: _action, serviceId, ...fields } = input;
    const requestDigest = hash([user.uid, serviceId, fields.sharedBotId]);
    if (existing) {
      // The same publisher asking again for the same service gets it back; the id is otherwise taken, and says nothing more.
      if (existing.requestDigest !== requestDigest) deny(409, 'SERVICE_ID_CONFLICT');
      return publisherView(existing);
    }
    const owned = await tx.get(this.db.collection('services').where('publisher', '==', user.uid).limit(publisherLimit + 1));
    if (owned.docs.filter(doc => doc.data().state === 'active').length >= publisherLimit) deny(409, 'SERVICE_LIMIT');
    // `state: 'active'` is what the ledger asks of the scope it works in. `online` is the publisher's switch, off at first:
    // a listing is not an online service until its publisher says so.
    const service = { schemaVersion: 1, kind: 'service', serviceId, state: 'active', online: false, publisher: user.uid,
      publisherName: displayName(user), ...fields, cost: 'publisher', visibility: 'requester_and_publisher', revision: 1,
      requestDigest, createdAt: now, updatedAt: now };
    tx.create(root, service);
    tx.create(root.collection('members').doc(user.uid), { role: 'owner', displayName: displayName(user), generation: 1, enteredAt: now });
    return publisherView(service);
  }

  manage(tx, root, service, input, now) {
    if (input.action === 'setServiceOnline') {
      if (service.online !== input.online) tx.update(root, { online: input.online, revision: service.revision + 1, updatedAt: now });
      return publisherView({ ...service, online: input.online, revision: service.revision + (service.online !== input.online ? 1 : 0) });
    }
    if (input.action === 'withdrawService') {
      // Withdrawn for good: no session is opened, read or answered any more. What people already received is theirs.
      tx.update(root, { state: 'withdrawn', online: false, revision: service.revision + 1, updatedAt: now });
      return publisherView({ ...service, state: 'withdrawn', online: false, revision: service.revision + 1 });
    }
    if (service.revision !== input.expectedRevision) deny(409, 'SERVICE_REVISION_CONFLICT');
    const { action: _action, serviceId: _serviceId, expectedRevision: _expected, ...changes } = input;
    const next = { ...service, ...changes, revision: service.revision + 1, updatedAt: now };
    tx.set(root, next);
    // Everyone the new audience leaves out loses their entitlement, and with it every task of theirs that has not ended.
    const narrowed = changes.audience && (service.audience.mode === 'public' ? changes.audience.mode !== 'public'
      : changes.audience.mode !== 'public' && service.audience.uids.some(user => !changes.audience.uids.includes(user)));
    return { ...publisherView(next), ...(narrowed ? { sweep: true } : {}) };
  }

  /** Ends an entitlement. The generation is remembered: entering again is a new one, and nothing admitted under the old one continues. */
  revoke(tx, root, user, member) {
    tx.delete(root.collection('members').doc(user));
    tx.set(root.collection('entitlements').doc(user), { generation: member.generation });
    tx.delete(this.db.doc(`serviceUsers/${user}/entered/${root.id}`));
  }

  /** After the audience was narrowed: removes the entitlements it no longer covers. The gate already refuses them; this ends their tasks. */
  async sweep(root) {
    for (;;) {
      const done = await this.db.runTransaction(async tx => {
        const service = (await tx.get(root)).data();
        if (!service || service.audience.mode === 'public') return true;
        const members = await tx.get(root.collection('members').where('role', '==', 'member').limit(500));
        const out = members.docs.filter(doc => !service.audience.uids.includes(doc.id)).slice(0, 100);
        for (const doc of out) this.revoke(tx, root, doc.id, doc.data());
        // At most 200 people can be named, so a page of 500 with nobody to remove is the end.
        return out.length === 0;
      });
      if (done) return;
    }
  }

  /**
   * What a person who may use the service is told about it. Nothing of the publisher's Teams, members or connections is
   * here, because nothing of them is in the scope. `availability` says why a request would not be answered now.
   */
  async view(tx, root, service, self, user, now) {
    if (service.publisher === user) return publisherView(service);
    const botDoc = await tx.get(root.collection('sharedBots').doc(service.sharedBotId)), bot = botDoc.data();
    const deployment = bot ? (await tx.get(root.collection('deployments').doc(bot.identity.activeDeploymentId))).data() : undefined;
    // The ledger's own daily counters: one for the whole scope, one per person and generation.
    const today = day(now), used = async key => (await tx.get(root.collection('requestQuotas').doc(hash(key)))).data()?.used ?? 0;
    const serviceUsed = bot ? await used([today, 'team']) : 0;
    const userUsed = bot && self ? await used([today, 'member', user, self.generation]) : 0;
    const availability = service.state !== 'active' ? 'withdrawn' : !service.online ? 'disabled'
      : !bot || bot.identity.state !== 'published' ? 'not_ready'
      : !deployment?.enabled || !deployment.ready || deployment.onlineUntil <= now ? 'host_offline'
      : serviceUsed >= bot.quotas.team ? 'service_quota_exhausted' : userUsed >= bot.quotas.member ? 'user_quota_exhausted' : 'available';
    return { serviceId: service.serviceId, name: service.name, description: service.description, publisherName: service.publisherName,
      sharedBotId: service.sharedBotId, availability, entered: !!self, ...(self ? { generation: self.generation } : {}),
      // Who pays for the model and the tools, who can read a session, and what is kept.
      cost: service.cost, visibility: service.visibility, retention: service.retention,
      limits: { userConcurrent: service.limits.userConcurrent, ...(bot ? { userDaily: bot.quotas.member, userDailyLeft: Math.max(0, bot.quotas.member - userUsed) } : {}) } };
  }

  /** A report goes to the platform, not to the publisher: it may be about the publisher. It is kept even after the service is withdrawn. */
  async report(tx, root, service, user, input, now) {
    const ref = this.db.doc(`serviceReports/${hash([input.serviceId, user.uid, input.reportId])}`), existing = (await tx.get(ref)).data();
    const record = { serviceId: input.serviceId, publisher: service.publisher, reporter: user.uid, reason: input.reason, text: input.text,
      ...(input.sessionId ? { sessionId: input.sessionId } : {}) };
    if (existing) {
      if (hash(record) !== existing.requestDigest) deny(409, 'REPORT_ID_CONFLICT');
      return { reported: true };
    }
    if (input.sessionId) {
      const session = (await tx.get(root.collection('conversations').doc(input.sessionId))).data();
      if (!session?.participants.includes(`user:${user.uid}`)) deny(403, 'CONVERSATION_REQUIRED');
    }
    const counterRef = this.db.doc(`serviceUsers/${user.uid}/reportDays/${day(now)}`), count = (await tx.get(counterRef)).data()?.count ?? 0;
    if (count >= reportsPerDay) deny(429, 'REPORT_LIMIT');
    tx.create(ref, { schemaVersion: 1, ...record, requestDigest: hash(record), state: 'open', at: now });
    tx.set(counterRef, { count: count + 1 });
    return { reported: true };
  }

  /** The services a person publishes and the ones they have entered. A service they may no longer use is left out. */
  async list(user, now) {
    const [published, entered] = await Promise.all([this.db.collection('services').where('publisher', '==', user).limit(publisherLimit * 2).get(),
      this.db.collection(`serviceUsers/${user}/entered`).limit(200).get()]);
    const services = [];
    for (const doc of entered.docs) {
      const root = serviceScope.root(this.db, doc.id);
      const view = await this.db.runTransaction(async tx => {
        const [serviceDoc, selfDoc] = await tx.getAll(root, root.collection('members').doc(user));
        const service = serviceDoc.data();
        if (!service || !selfDoc.exists || service.state !== 'active' || !includes(service.audience, user)) return null;
        return this.view(tx, root, service, selfDoc.data(), user, now);
      });
      if (view) services.push(view);
    }
    return { published: published.docs.map(doc => publisherView(doc.data())), entered: services };
  }
}

/** The publisher's own view: everything they set. */
function publisherView(service) {
  const { requestDigest: _digest, schemaVersion: _schema, kind: _kind, ...view } = service;
  return view;
}
