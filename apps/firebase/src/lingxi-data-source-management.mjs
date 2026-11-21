import { createHash } from 'node:crypto';
import { z } from 'zod';

import { createPinnedDataClient, LingxiDataError } from './lingxi-data.mjs';
import { dataId, generation, sourceBotPrincipal } from './lingxi-data-sources.mjs';
import { sharedBotPermits } from './team-execution-contract.mjs';

const scope = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('personal') }).strict(),
  z.object({ kind: z.literal('team'), teamId: dataId, conversationId: dataId, generation }).strict(),
]);
const fields = z.array(dataId).min(1).max(64).refine(values => new Set(values).size === values.length);
const inspect = z.object({ action: z.literal('dataSources') }).strict();
const previewOffers = z.object({
  scope: z.object({ kind: z.literal('team'), teamId: dataId, conversationId: dataId, generation }).strict(),
  command: z.object({ action: z.literal('privateSourceOffers') }).strict(),
}).strict();
const configure = z.object({
  action: z.literal('configureDataSource'), source: dataId, scope,
  expectedVersion: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
  name: z.string().trim().min(1).max(120), kind: z.enum(['mysql', 'postgres', 'sqlite', 'mongodb', 'redis', 'csv', 'json']),
  key: z.string().regex(/^(?:[A-Za-z0-9_-]{44}|[A-Za-z0-9_-]{87})$/u),
  fields: fields.optional(),
  enabled: z.boolean(), requesters: z.array(dataId).max(100).refine(values => new Set(values).size === values.length),
  grants: z.array(z.object({ sharedBotId: dataId, fields, preset: dataId }).strict()).max(16)
    .refine(values => new Set(values.map(({ sharedBotId }) => sharedBotId)).size === values.length),
}).strict();

const hash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
const personalSignal = z.discriminatedUnion('action', [
  z.object({ action: z.literal('directory') }).strict(),
  z.object({ action: z.literal('host'), source: dataId }).strict(),
  z.object({ action: z.literal('offer'), source: dataId, id: dataId,
    requesterKey: z.string().max(87), hostKey: z.string().max(87), body: z.string().max(65536) }).strict(),
  z.object({ action: z.literal('status'), id: dataId }).strict(),
  z.object({ action: z.literal('answer'), id: dataId, body: z.string().max(65536) }).strict(),
  z.object({ action: z.literal('close'), id: dataId }).strict(),
]);

function deny(status = 403, code = 'DATA_ACCESS_REQUIRED') {
  throw new LingxiDataError(status, code);
}

export function createDataSourceManagement({ db, endpoint, projectActor, clock = Date.now }) {
  const forward = createPinnedDataClient(endpoint);
  const registry = db.collection('lingxiManagedDataSources');
  const bindings = db.collection('lingxiDataSourceBindings');
  const personalSessions = db.collection('lingxiDataPersonalSessions');
  const signalEndpoint = new URL('/v1/data/signaling', endpoint);

  async function teamBinding(tx, uid, input) {
    const { scope, source, requesters, grants, enabled } = input;
    const team = db.collection('teams').doc(scope.teamId);
    const member = id => team.collection('members').doc(id);
    const [active, owner, room] = (await tx.getAll(team, member(uid), team.collection('conversations').doc(scope.conversationId))).map(doc => doc.data());
    if (active?.state !== 'active' || !['owner', 'admin'].includes(owner?.role) || owner.generation !== scope.generation
      || !room?.participants?.includes('user:' + uid)) deny();
    const botIds = grants.map(grant => grant.sharedBotId);
    if (requesters.some(id => !room.participants.includes('user:' + id))
      || botIds.some(id => !room.participants.includes('shared_bot:' + id))) deny();
    const roster = requesters.length ? await tx.getAll(...requesters.map(member)) : [];
    if (roster.some(doc => !generation.safeParse(doc.data()?.generation).success)) deny();
    const bots = botIds.length ? await tx.getAll(...botIds.map(id => team.collection('sharedBots').doc(id))) : [];
    if (enabled && bots.some(doc => doc.data()?.identity.state !== 'published')) deny();
    return { teamId: scope.teamId, conversationId: scope.conversationId, enabled,
      owner: { uid, generation: scope.generation }, requesters: roster.map(doc => ({ uid: doc.id, generation: doc.data().generation })),
      bots: botIds.map(sharedBotId => ({ sharedBotId, principalUid: sourceBotPrincipal(scope.teamId, source, sharedBotId), generation: 1 })),
    };
  }

  async function manage(uid, token, raw) {
    const preview = previewOffers.safeParse(raw);
    if (preview.success) return listPreview(uid, preview.data.scope);
    if (inspect.safeParse(raw).success) {
      const found = await registry.where('uid', '==', uid).limit(100).get();
      return { sources: found.docs.map(doc => {
        const { pending, desiredEnabled, summary } = doc.data();
        return pending ? { ...summary, pending: true, desiredEnabled } : summary;
      }) };
    }
    const parsed = configure.safeParse(raw);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const input = parsed.data;
    const { source, scope, expectedVersion, kind, name, key, enabled, grants, requesters } = input;
    if (scope.kind === 'personal' && (grants.length || requesters.length || !input.fields)) deny(400, 'INVALID_DATA_REQUEST');
    const ref = registry.doc(source), bindingRef = bindings.doc(source);
    const request = hash([uid, input]), version = expectedVersion + 1;
    if (!generation.safeParse(version).success) deny(409, 'DATA_CAPACITY');

    async function check(tx) {
      const [old, bound] = (await tx.getAll(ref, bindingRef)).map(doc => doc.data());
      if (old?.uid !== undefined && (old.uid !== uid || hash(old.scope) !== hash(scope) || old.key !== key
        || old.summary.kind !== kind || old.summary.name !== name)) deny();
      if (!old && bound) deny();
      const retry = old?.request === request;
      if (!retry && ((old?.summary.version ?? 0) !== expectedVersion || old?.pending)) deny(409, 'DATA_CONFLICT');
      const binding = scope.kind === 'team' ? await teamBinding(tx, uid, input) : null;
      if (retry && hash(binding) !== old.bindingDigest) deny();
      return { old, binding, retry };
    }

    const admitted = await db.runTransaction(async tx => {
      const checked = await check(tx);
      if (!checked.retry) {
        const previousBots = checked.old?.bots ?? [];
        tx.set(ref, { uid, scope, key, request, pending: true, desiredEnabled: enabled, bindingDigest: hash(checked.binding), previousBots,
          bots: checked.binding?.bots ?? [], summary: { source, scope, name, kind, enabled: false, version, requesters, grants },
          expires: Math.floor(clock() / 1000) + 30 * 86400 - 120 });
        if (checked.binding) tx.set(bindingRef, { ...checked.binding, enabled: false, version });
      }
      return checked;
    });
    if (admitted.retry && !admitted.old.pending) return admitted.old.summary;
    const live = await ref.get();
    const { previousBots, expires } = live.data();
    const authorize = () => db.runTransaction(check);
    const owner = { uid, scope };
    async function apply(operation, input, recipients = []) {
      return projectActor(owner.uid, owner.scope, authorize, () => forward(token, hash([request, operation]), input), recipients);
    }
    if (expectedVersion === 0) await apply('register', { action: 'register_source', source, name, kind: kind === 'mysql' ? 'my_sql' : kind,
      team: scope.kind === 'team' ? scope.teamId : null, hostKey: key });
    if (scope.kind === 'personal') {
      await apply('personal', enabled
        ? { action: 'grant_source', source, recipient: uid, fields: input.fields, preset: 'main', expires, publish: false, audience: [] }
        : { action: 'revoke_source', source, recipient: uid });
    }
    const nextBots = admitted.binding?.bots ?? [];
    const allow = new Set(enabled ? nextBots.map(bot => bot.principalUid) : []);
    const toRevoke = previousBots.filter(bot => !allow.has(bot.principalUid));
    for (const bot of toRevoke) {
      await apply(['revoke', bot.principalUid], { action: 'revoke_source', source, recipient: bot.principalUid });
    }
    if (enabled && scope.kind === 'team') for (const [index, grant] of grants.entries()) {
      const bot = nextBots[index];
      await apply(['grant', bot.principalUid], { action: 'grant_source', source, recipient: bot.principalUid, fields: grant.fields,
        preset: grant.preset, expires, publish: false, audience: [] }, [{ uid: bot.principalUid, generation: bot.generation }]);
    }
    return db.runTransaction(async tx => {
      const { old, binding } = await check(tx);
      if (!old?.pending) return old.summary;
      const summary = { ...old.summary, enabled };
      if (binding) tx.set(bindingRef, { ...binding, version });
      tx.update(ref, { pending: false, summary });
      return summary;
    });
  }

  async function signalOwner(uid, token, raw) {
    const parsed = personalSignal.safeParse(raw);
    if (!parsed.success) return undefined;
    const message = parsed.data;

    async function forwardSignal() {
      try {
        const response = await fetch(signalEndpoint, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(12000),
          headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: JSON.stringify(message) });
        if (response.ok) return await response.json();
        if ([401, 403, 404].includes(response.status)) deny();
        if ([400, 409].includes(response.status)) deny(response.status, 'DATA_CONFLICT');
      } catch (error) { if (error instanceof LingxiDataError) throw error; }
      deny(503, 'DATA_UNAVAILABLE');
    }

    if (message.action === 'directory') {
      const listed = async () => {
        const rows = await registry.where('uid', '==', uid).limit(100).get();
        return new Set(rows.docs.filter(doc => {
          const entry = doc.data();
          return entry.scope.kind === 'personal' && entry.summary.enabled && !entry.pending;
        }).map(doc => doc.id));
      };
      const before = await listed();
      if (!before.size) return {};
      const data = await projectActor(uid, { kind: 'personal' }, async () => {}, forwardSignal);
      const live = await listed();
      return Object.fromEntries(Object.entries(data).filter(([source]) => before.has(source) && live.has(source)));
    }
    const sessionRef = 'id' in message ? personalSessions.doc(message.id) : null;
    const stored = sessionRef ? (await sessionRef.get()).data() : null;
    const source = 'source' in message ? message.source : stored?.source;
    if (!source) return undefined;
    async function authority(version) {
      const entry = (await registry.doc(source).get()).data();
      if (entry?.scope.kind !== 'personal') return undefined;
      if (entry.uid !== uid || !entry.summary.enabled || entry.pending
        || (version !== undefined && entry.summary.version !== version)) deny();
      return entry.summary.version;
    }
    const version = await authority(stored?.version);
    if (version === undefined) return undefined;
    if (stored && (stored.source !== source || stored.uid !== uid || stored.expires <= clock())) deny();
    if (message.action === 'offer') {
      if (stored) deny(409, 'DATA_CONFLICT');
      await sessionRef.create({ source, uid, version, expires: clock() + 90_000 });
    }
    const check = async () => { if (await authority(version) === undefined) deny(); };
    const result = await projectActor(uid, { kind: 'personal' }, check, forwardSignal);
    await check();
    return result;
  }

  async function listPreview(uid, target) {
    const team = db.collection('teams').doc(target.teamId);
    return db.runTransaction(async tx => {
      const identity = { uid, generation: target.generation };
      const [active, member, room] = (await tx.getAll(team, team.collection('members').doc(uid),
        team.collection('conversations').doc(target.conversationId))).map(doc => doc.data());
      if (active?.state !== 'active' || member?.generation !== identity.generation || !room?.participants?.includes('user:' + uid)) deny();
      const rows = await tx.get(bindings.where('conversationId', '==', target.conversationId));
      const sources = [];
      for (const doc of rows.docs) {
        const binding = doc.data();
        if (!binding.enabled || binding.teamId !== target.teamId
          || !binding.requesters.some(user => user.uid === uid && user.generation === identity.generation)) continue;
        const owner = (await tx.get(team.collection('members').doc(binding.owner.uid))).data();
        if (owner?.generation !== binding.owner.generation) continue;
        const source = (await tx.get(registry.doc(doc.id))).data();
        if (!source?.summary.enabled) continue;
        for (const entry of binding.bots) {
          if (!room.participants.includes('shared_bot:' + entry.sharedBotId)
            || (room.executionPolicy && !room.executionPolicy.targets.includes(entry.sharedBotId))) continue;
          const bot = (await tx.get(team.collection('sharedBots').doc(entry.sharedBotId))).data();
          if (bot?.identity.state !== 'published' || !sharedBotPermits(bot.identity, identity)) continue;
          sources.push({ source: doc.id, name: source.summary.name, sharedBotId: entry.sharedBotId, botName: bot.name });
          if (sources.length === 32) return { sources };
        }
      }
      return { sources };
    }, { readOnly: true });
  }

  return { manage, signalOwner };
}
