import { createHash } from 'node:crypto';
import { z } from 'zod';

import { authorizeDataBinding, beforeRecordId, createPinnedDataClient, dataReference, LingxiDataError } from './lingxi-data.mjs';
import { dataId, generation } from './lingxi-data-sources.mjs';
import { SharedBotError, SharedBotsStore } from './shared-bots.mjs';

const cursor = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const scope = z.object({ kind: z.literal('team'), teamId: dataId, conversationId: dataId, generation }).strict();
const definition = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('created') }).strict(),
  z.object({ kind: z.literal('transition'), field: dataId, equals: z.union([z.string().max(1024), z.boolean(), z.number(), z.null()]) }).strict(),
]);
const request = z.object({
  action: z.literal('setDataRule'), scope, dataset: dataId, rule: dataId, expectedVersion: cursor,
  config: z.object({ sharedBotId: dataId, dailyLimit: z.number().int().min(1).max(100), definition }).strict().nullable(),
}).strict();
const eventPage = z.object({ cursor, events: z.array(z.object({
  cursor, kind: z.string(), reference: dataReference.nullable(),
})).max(50) });

const hash = parts => createHash('sha256').update(JSON.stringify(parts)).digest('hex');
export const ruleHeadId = (dataset, record) => hash([dataset, record]);

function deny(status = 403, code = 'DATA_ACCESS_REQUIRED') {
  throw new LingxiDataError(status, code);
}

export function createDataRules({ db, endpoint, projectActor, botToken, clock = Date.now }) {
  const forward = createPinnedDataClient(endpoint);
  const store = new SharedBotsStore(db, clock);
  const rules = db.collection('lingxiDataRules');
  let lastRule;

  async function authority(tx, entry) {
    const { scope: target, dataset, config, admin } = entry;
    const team = db.collection('teams').doc(target.teamId);
    const [member, thread] = (await tx.getAll(team.collection('members').doc(admin), team.collection('conversations').doc(target.conversationId)))
      .map(doc => doc.data());
    if (!['owner', 'admin'].includes(member?.role) || member.generation !== target.generation) deny();
    await authorizeDataBinding(tx, db, admin, target, dataset, true, true);
    if (!config) return { team, member };
    if (!thread?.participants?.includes('shared_bot:' + config.sharedBotId)) deny();
    const bot = team.collection('sharedBots').doc(config.sharedBotId);
    const [grant, published] = (await tx.getAll(bot.collection('dataGrants').doc(dataset), bot)).map(doc => doc.data());
    if (!grant?.access || published?.identity?.state !== 'published' || grant.botRevision !== published.identity.revision
      || grant.conversationId !== target.conversationId || (entry.grantVersion && grant.version !== entry.grantVersion)) deny();
    const issuer = (await tx.get(team.collection('members').doc(grant.issuedBy.uid))).data();
    if (!['owner', 'admin'].includes(issuer?.role)) deny();
    await authorizeDataBinding(tx, db, grant.issuedBy.uid, { ...target, generation: grant.issuedBy.generation }, dataset, true, true);
    return { team, member, grant };
  }

  async function read(entry, input, authorize) {
    const grant = await authorize();
    const token = await botToken(grant.principalUid);
    return projectActor(grant.principalUid, { ...entry.scope, generation: grant.version }, async () => {
      const live = await authorize();
      if (live.principalUid !== grant.principalUid || live.version !== grant.version) deny();
    }, () => forward(token, null, input));
  }

  async function events(entry, after, authorize) {
    const raw = await read(entry, { action: 'events', dataset: entry.dataset, after }, authorize);
    const parsed = eventPage.safeParse(raw);
    if (!parsed.success || parsed.data.cursor < after
      || parsed.data.events.some(({ cursor }, index, events) => cursor <= (events[index - 1]?.cursor ?? after) || cursor > parsed.data.cursor)) {
      deny(503, 'DATA_UNAVAILABLE');
    }
    return parsed.data;
  }

  async function setRule(uid, input) {
    const parsed = request.safeParse(input);
    if (!parsed.success) deny(400, 'INVALID_DATA_REQUEST');
    const { action: _action, expectedVersion, ...entry } = parsed.data;
    const ref = rules.doc(hash([entry.scope.teamId, entry.dataset, entry.rule]));
    const version = expectedVersion + 1;
    if (!generation.safeParse(version).success) deny(409, 'DATA_CAPACITY');
    const signature = hash([uid, parsed.data]);
    const desired = { ...entry, admin: uid, enabled: entry.config !== null, version, signature };
    async function check(tx) {
      const live = await authority(tx, desired);
      const existing = (await tx.get(ref)).data();
      if (existing?.signature === signature) return { ...live, exists: true };
      if ((existing?.version ?? 0) !== expectedVersion) deny(409, 'DATA_CONFLICT');
      return live;
    }
    const { exists, grant } = await db.runTransaction(check);
    if (exists) return { version, enabled: desired.enabled };
    let after = 0;
    if (desired.enabled) {
      desired.grantVersion = grant.version;
      const authorize = () => db.runTransaction(async tx => (await check(tx)).grant);
      const description = await read(desired, { action: 'describe', dataset: entry.dataset }, authorize);
      if (entry.config.definition.kind === 'transition'
        && !Object.hasOwn(description.schema?.properties ?? {}, entry.config.definition.field)) deny(400, 'INVALID_DATA_REQUEST');
      let settled = false;
      for (let index = 0; index < 100; index++) {
        const page = await events(desired, after, authorize);
        after = page.cursor;
        if (page.events.length < 50) { settled = true; break; }
      }
      if (!settled) deny(409, 'DATA_CAPACITY');
    }
    await db.runTransaction(async tx => {
      if (!(await check(tx)).exists) tx.set(ref, { ...desired, cursor: after });
    });
    return { version, enabled: desired.enabled };
  }

  async function evaluate(entry, reference, authorize) {
    const newest = await read(entry, { action: 'list', dataset: reference.dataset, after: beforeRecordId(reference.record), limit: 1 }, authorize);
    const latest = newest.records?.[0];
    if (latest?.id !== reference.record) return { revision: null, actorTask: null, matches: false };
    const row = latest.record;
    const head = { revision: row.revision, actorTask: row.actor?.task ?? null, matches: false };
    if (row.revision !== reference.revision || row.schemaVersion !== reference.schemaVersion) return head;
    const predicate = entry.config.definition;
    if (predicate.kind === 'created') return { ...head, matches: reference.revision === 1 };
    const value = row.value?.[predicate.field];
    if (value !== predicate.equals || reference.revision === 1) return head;
    for (let schemaVersion = reference.schemaVersion; schemaVersion >= Math.max(1, reference.schemaVersion - 15); schemaVersion--) {
      try {
        const prior = await read(entry, { action: 'get', reference: { ...reference, revision: reference.revision - 1, schemaVersion } }, authorize);
        return { ...head, matches: prior.record.value?.[predicate.field] !== predicate.equals };
      } catch (error) {
        if (!(error instanceof LingxiDataError) || error.status !== 404) throw error;
      }
    }
    return head;
  }

  async function pollRule(ref, entry) {
    async function check(tx) {
      const fresh = (await tx.get(ref)).data();
      if (!fresh?.enabled || fresh.version !== entry.version) deny();
      return { ...await authority(tx, fresh), cursor: fresh.cursor };
    }
    const authorize = () => db.runTransaction(async tx => (await check(tx)).grant);
    const page = await events(entry, entry.cursor, authorize);
    for (const event of page.events) {
      if (event.kind !== 'record' || event.reference?.dataset !== entry.dataset) continue;
      const reference = event.reference;
      const head = await evaluate(entry, reference, authorize);
      await db.runTransaction(async tx => {
        const { team, member, cursor: previous } = await check(tx);
        if (previous >= event.cursor) return;
        const state = team.collection('dataRuleHeads').doc(ruleHeadId(reference.dataset, reference.record));
        const existing = (await tx.get(state)).data();
        const origin = head.actorTask ? (await tx.get(team.collection('tasks').doc(head.actorTask))).data() : null;
        if (head.matches && (!head.actorTask || (origin && !origin.dataCause))) {
          const key = hash([ref.id, entry.version, reference.dataset, reference.record, reference.revision]);
          const identity = { uid: entry.admin, generation: entry.scope.generation };
          const cause = { kind: 'rule', ruleId: ref.id, ruleVersion: entry.version, quotaOwner: ref.id,
            dailyLimit: entry.config.dailyLimit, targetGrantVersion: entry.grantVersion, reference };
          try {
            await store.submit({ tx, team, self: member, identity, now: clock(), dataCause: cause,
              command: { payload: { conversationId: entry.scope.conversationId, messageId: 'bot_' + key.slice(0, 60),
                changeId: 'bot_' + hash(['change', key]).slice(0, 60), text: 'Data rule matched this record revision.',
                targets: [entry.config.sharedBotId] } },
            });
          } catch (error) {
            if (!(error instanceof SharedBotError) || ![403, 409, 429].includes(error.status)) throw error;
          }
        }
        if (!existing || existing.cursor < event.cursor) tx.set(state, { cursor: event.cursor, revision: head.revision, actorTask: head.actorTask });
        tx.update(ref, { cursor: event.cursor });
      });
    }
    if (page.cursor > entry.cursor) {
      await db.runTransaction(async tx => {
        const live = await check(tx);
        if (live.cursor < page.cursor) tx.update(ref, { cursor: page.cursor });
      });
    }
  }

  async function poll() {
    let query = rules.where('enabled', '==', true).orderBy('__name__').limit(16);
    if (lastRule) query = query.startAfter(lastRule);
    const batch = await query.get();
    lastRule = batch.size === 16 ? batch.docs.at(-1).id : undefined;
    for (const doc of batch.docs) {
      try { await pollRule(doc.ref, doc.data()); }
      catch (error) {
        if (!(error instanceof LingxiDataError) || ![403, 404, 409].includes(error.status)) throw error;
      }
    }
  }

  return { setRule, poll };
}
