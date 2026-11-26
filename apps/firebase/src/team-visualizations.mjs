import { z } from 'zod';
import { DATA_PROTOCOL_VERSION } from './dataset-resource.mjs';
import { TeamMessagesStore } from './team-messages.mjs';
import { SharedBotsStore } from './shared-bots.mjs';
import { sharedBotCommand } from './shared-bot-commands.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import { reservationMs, rowRef, scheduleVisualizationCleanup, sweepVisualizations, trackVisualization, visualizationFile, visualizationQuota } from './team-visualization-storage.mjs';
import {
  bundle, canonical, channelAudience, confirmVisualizationBindings, dataRef, fail, hash, id, key, maxPackageBytes, messageId,
  mode, projection, sameAudience, scope, threadAccess, versionId, visualizationChannelEnabled,
} from './team-visualization-contract.mjs';

const cardLocator = { message: messageId, version: versionId };
const locator = { ...scope, ...cardLocator };
const alias = { alias: id };
const bindings = z.array(z.discriminatedUnion('kind', [
  z.object({ ...alias, kind: z.literal('reuse'), reference: dataRef, grant: id, mode }).strict(),
  z.object({ ...alias, kind: z.literal('share'), dataset: id, via: id.optional(),
    mode, projection: projection.default({}), reference: dataRef.optional() }).strict(),
])).max(4).refine(items => new Set(items.map(item => item.alias)).size === items.length);
const a2aArtifact = z.object({ artifactId: id, name: z.string(),
  parts: z.tuple([z.object({ mediaType: z.literal('application/json'), data: z.object({
    version: z.literal(1), bundle, bindings: z.array(z.object({ alias: id, messageId: id, sourceAlias: id.optional() }).strict())
      .max(4).refine(items => new Set(items.map(item => item.alias)).size === items.length),
  }).strict() }).strict()]),
}).strict().refine(value => value.name === value.parts[0].data.bundle.title);
const request = z.discriminatedUnion('action', [
  z.object({ action: z.literal('audience'), ...scope }).strict(),
  z.object({ action: z.literal('publish'), ...scope, operation: id, audienceDigest: id,
    bundle, bindings: bindings.default([]) }).strict(),
  z.object({ action: z.literal('completeBot'), completion: sharedBotCommand, artifact: a2aArtifact }).strict(),
  z.object({ action: z.literal('open'), ...locator }).strict(),
  z.object({ action: z.literal('checkBatch'), ...scope, cards: z.array(z.object(cardLocator).strict()).min(1).max(40) }).strict(),
  z.object({ action: z.literal('state'), ...locator, expected: z.number().int().nonnegative(),
    state: z.object({ modelContent: z.json(), privateContent: z.json() }).strict()
      .refine(value => Buffer.byteLength(JSON.stringify(value)) <= 16_384) }).strict(),
  z.object({ action: z.literal('data'), ...locator, ...alias, operation: id.optional(), request: z.discriminatedUnion('action', [
    z.object({ action: z.literal('read'), after: z.string().max(320).optional(), block: z.number().int().nonnegative().optional(), record: id.optional(),
      watch: z.boolean().optional() }).strict(),
    z.object({ action: z.literal('changes'), cursors: z.array(z.string().max(320)).length(16) }).strict(),
    z.object({ action: z.literal('patch'), record: id, expectedRevision: z.number().int().positive(),
      schemaVersion: z.number().int().positive(), partition: z.string().min(1).max(160), changes: z.record(id, z.json()) }).strict(),
  ]) }).strict(),
]);

const budget = 512 * 1024 * 1024;
const target = input => ({ team: input.teamId, conversation: input.conversationId });
const artifact = (page, version) => ({ kind: 'pageVersion', page, version });
const committed = row => row && (['active', 'recalled'].includes(row.status) || row.recalled === true);
const descriptor = (row, page, version) => ({ reference: artifact(page, version), title: row.title, bindings: row.bindings });

export function createTeamVisualizations({ db, bucket, core, channels = '', clock = Date.now }) {
  if (!bucket) return undefined;
  const canPublish = input => visualizationChannelEnabled(channels, input.teamId, input.conversationId);
  const stateRef = (input, uid) => rowRef(db, input.message, input.version).collection('viewerState')
    .doc(key([uid, input.generation]));
  const emptyState = { version: 0, modelContent: null, privateContent: null };

  const authorityRefs = (ref, input) => [rowRef(db, input.message, input.version), ref.collection('messages').doc(input.message)];
  function authorized(input, user, [source, document]) {
    const row = source.data(), message = document.data();
    if (row?.status === 'active' && Object.hasOwn(row.audience, user.uid) && row.audience[user.uid] === input.generation
        && row.destination.team === input.teamId && row.destination.conversation === input.conversationId
        && message && !message.recalled && message.visualization?.reference?.page === input.message
        && message.visualization.reference.version === input.version) return message.visualization;
  }

  async function authority(tx, input, user) {
    const { ref } = await threadAccess(tx, db, input, user.uid);
    const value = authorized(input, user, await tx.getAll(...authorityRefs(ref, input)));
    if (!value) fail(403, 'DATA_ACCESS_REQUIRED');
    return value;
  }

  async function checkBatch(tx, input, user) {
    const { ref } = await threadAccess(tx, db, input, user.uid);
    const paths = input.cards.map(card => authorityRefs(ref, card));
    const unique = new Map(paths.flat().map(doc => [doc.path, doc]));
    const snapshots = new Map((await tx.getAll(...unique.values())).map(doc => [doc.ref.path, doc]));
    return { active: input.cards.map((card, index) =>
      Boolean(authorized({ ...input, ...card }, user, paths[index].map(doc => snapshots.get(doc.path))))) };
  }

  function viewerState(row) {
    return row ? { version: row.version, ...JSON.parse(row.json) } : emptyState;
  }

  async function prepareBindings(input, token, page, version, audience) {
    if (!input.bindings.length) return [];
    if (!core) fail(503, 'DATA_UNAVAILABLE');
    const results = [];
    for (const spec of input.bindings) {
      if (spec.kind === 'reuse') {
        results.push({ alias: spec.alias, reference: spec.reference, grant: spec.grant, mode: spec.mode });
        continue;
      }
      const operation = key([page, spec.alias]);
      const call = (action, fields) => core(token, {
        protocol: DATA_PROTOCOL_VERSION, dataset: spec.dataset, via: spec.via, operation, command: { action, ...fields },
      });
      let reference = spec.reference;
      if (spec.mode === 'pinned' && !reference) {
        let snapshot = await call('startSnapshot', { projection: spec.projection });
        if (!snapshot.ready) snapshot = await call('advanceSnapshot', { snapshot: snapshot.reference.snapshot });
        if (!snapshot.ready) return null;
        reference = snapshot.reference;
      }
      const prepared = await call('prepareShare', { share: {
        destination: target(input), mode: spec.mode, projection: spec.projection, reference,
        audienceDigest: key([target(input), audience]), textDigest: hash(input.bundle.title, 'base64url'),
        visualization: { message: page, version, binding: spec.alias },
      } });
      if (prepared.messageId !== page || !dataRef.safeParse(prepared.reference).success || !id.safeParse(prepared.grant).success) {
        fail(503, 'DATA_UNAVAILABLE');
      }
      results.push({ alias: spec.alias, reference: prepared.reference, grant: prepared.grant, mode: spec.mode });
    }
    return results;
  }

  async function reserveAndUpload(input, user, audience, prepared, page, version, bytes, taskId) {
    const intentDigest = key([input, taskId ?? null]);
    const ref = rowRef(db, page, version);
    const row = await db.runTransaction(async tx => {
      const { team, thread, ref: channel } = await threadAccess(tx, db, input, user.uid);
      const operation = channel.collection('visualizationOperations').doc(page);
      const quota = visualizationQuota(team);
      const [previous, usage, receipt] = await tx.getAll(ref, quota, operation);
      if (receipt.exists && receipt.data().intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
      const existing = previous.data();
      if (existing && existing.intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
      if (committed(existing)) return existing;
      if (!taskId && !thread.participants.includes(`user:${user.uid}`)) fail(403, 'DATA_ACCESS_REQUIRED');
      if (!sameAudience(audience, await channelAudience(tx, team, thread))) fail(409, 'DATA_SHARE_CHANGED');
      if (existing && (existing.status !== 'pending' || !(existing.expiresAt > clock()))) {
        fail(409, 'DATA_SHARE_CHANGED');
      }
      if (existing) {
        const expiresAt = clock() + reservationMs;
        tx.update(ref, { expiresAt });
        scheduleVisualizationCleanup(tx, db, team, artifact(page, version), expiresAt);
        return existing;
      }
      await confirmVisualizationBindings(tx, channel, audience, user.uid, page, version, input.bundle.title, prepared);
      const consumed = usage.data()?.bytes ?? 0;
      if (!Number.isSafeInteger(consumed) || consumed < 0 || consumed + Buffer.byteLength(bytes) > budget) fail(429, 'DATA_QUOTA');
      const size = Buffer.byteLength(bytes), expiresAt = clock() + reservationMs;
      const value = { uid: user.uid, destination: target(input), message: page, version, status: 'pending', audience, bytes: size, expiresAt,
        title: input.bundle.title, bindings: prepared, ...(taskId ? { taskId } : {}), intentDigest };
      tx.create(ref, value);
      trackVisualization(tx, team, artifact(page, version));
      tx.create(operation, { intentDigest });
      tx.set(quota, { bytes: consumed + size });
      scheduleVisualizationCleanup(tx, db, team, artifact(page, version), expiresAt);
      return value;
    });
    if (committed(row)) return undefined;
    const blob = visualizationFile(bucket, page, version);
    try {
      await blob.save(bytes, { resumable: false, preconditionOpts: { ifGenerationMatch: 0 },
        metadata: { contentType: 'application/json', cacheControl: 'no-store' } });
    } catch (error) {
      if (Number(error.code) !== 412) throw error;
      const [existing] = await blob.download({ validation: 'crc32c' });
      if (existing.length > maxPackageBytes || hash(existing) !== version) fail(503, 'DATA_UNAVAILABLE');
    }
    return descriptor(row, page, version);
  }

  async function audienceFor(input, user) {
    if (!canPublish(input)) fail(404, 'VISUALIZATION_UNAVAILABLE');
    return db.runTransaction(async tx => {
      const { team, thread } = await threadAccess(tx, db, input, user.uid, true);
      return channelAudience(tx, team, thread);
    });
  }

  async function completeBot(user, input) {
    const command = input.completion;
    const bot = new SharedBotsStore(db, clock);
    const { task, publish, replay } = await bot.inspectVisualizationCompletion(user, command);
    if (task.taskId !== input.artifact.artifactId) fail(403, 'EXECUTION_PERMIT_REQUIRED');
    const location = { teamId: command.teamId, conversationId: task.conversationId, generation: command.generation };
    const contents = input.artifact.parts[0].data;
    const bytes = canonical(contents.bundle), version = hash(bytes);
    const page = 'viz_' + key([target(location), task.taskId]);
    const intent = key(input.artifact);
    const finish = descriptor => bot.execute(user, command, undefined, undefined, { intent, descriptor });
    if (replay || !publish || !canPublish(location)) return finish();
    try {
      const { audience, prepared } = await db.runTransaction(async tx => {
        const { team, thread, ref } = await threadAccess(tx, db, location, user.uid);
        const entries = contents.bindings;
        const snapshots = entries.length ? await tx.getAll(...entries.map(item => ref.collection('messages').doc(item.messageId))) : [];
        const prepared = entries.map((item, index) => {
          const source = snapshots[index]?.data();
          if (!source || source.recalled || source.seq > task.messageSeq) fail(403, 'DATA_ACCESS_REQUIRED');
          const selected = item.sourceAlias
            ? source.visualization?.bindings?.find(binding => binding.alias === item.sourceAlias)
            : { reference: source.resourceRef, grant: source.resourceGrant };
          if (!dataRef.safeParse(selected?.reference).success || !id.safeParse(selected?.grant).success) fail(403, 'DATA_ACCESS_REQUIRED');
          return { alias: item.alias, reference: selected.reference, grant: selected.grant,
            mode: selected.reference.kind === 'dataset' ? 'live' : 'pinned' };
        });
        return { audience: await channelAudience(tx, team, thread), prepared };
      });
      const visualization = await reserveAndUpload({ ...location, ...contents }, user, audience, prepared, page, version, bytes, task.taskId);
      return await finish(visualization);
    } catch (error) {
      if (!(error instanceof LingxiDataError) || !['DATA_ACCESS_REQUIRED', 'DATA_SHARE_CHANGED', 'DATA_QUOTA'].includes(error.code)) throw error;
      return finish();
    }
  }

  async function execute(user, token, raw) {
    const input = request.parse(raw);
    if (input.action === 'completeBot') return completeBot(user, input);
    if (input.action === 'checkBatch') return db.runTransaction(tx => checkBatch(tx, input, user));
    if (input.action === 'audience') {
      const audience = await audienceFor(input, user);
      return { audience, audienceDigest: key([target(input), audience]) };
    }
    if (input.action === 'publish') {
      const bytes = canonical(input.bundle), version = hash(bytes);
      const page = 'viz_' + key([user.uid, target(input), input.operation]);
      const result = { status: 'published', messageId: page, reference: artifact(page, version) };
      const intentDigest = key([input, null]);
      function receipt(row) {
        if (!committed(row)) return false;
        if (row.uid !== user.uid || row.destination.team !== input.teamId || row.destination.conversation !== input.conversationId
            || row.intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
        return true;
      }
      try {
        const audience = await db.runTransaction(async tx => {
          const { team, thread } = await threadAccess(tx, db, input, user.uid);
          const row = (await tx.get(rowRef(db, page, version))).data();
          if (receipt(row)) return null;
          if (!canPublish(input)) fail(404, 'VISUALIZATION_UNAVAILABLE');
          if (!thread.participants.includes(`user:${user.uid}`)) fail(403, 'DATA_ACCESS_REQUIRED');
          return channelAudience(tx, team, thread);
        });
        if (!audience) return result;
        if (input.audienceDigest !== key([target(input), audience])) fail(409, 'DATA_SHARE_CHANGED');
        const prepared = await prepareBindings(input, token, page, version, audience);
        if (!prepared) return { status: 'building' };
        const delivered = await reserveAndUpload(input, user, audience, prepared, page, version, bytes);
        if (!delivered) return result;
        await new TeamMessagesStore(db, clock).execute(user, { action: 'mutate', operation: 'create', teamId: input.teamId,
          generation: input.generation, conversationId: input.conversationId, changeId: page, messageId: page,
          text: delivered.title }, undefined, delivered);
      } catch (error) {
        // Recover a committed delivery even if another attempt changed the audience
        // or recalled the message while this attempt was still sending.
        const published = await db.runTransaction(async tx => {
          await threadAccess(tx, db, input, user.uid);
          return receipt((await tx.get(rowRef(db, page, version))).data());
        });
        if (!published) throw error;
      }
      return result;
    }
    if (input.action === 'state') {
      return db.runTransaction(async tx => {
        await authority(tx, input, user);
        const ref = stateRef(input, user.uid);
        const previous = (await tx.get(ref)).data()?.version ?? 0;
        if (previous !== input.expected) fail(409, 'DATA_CONFLICT');
        const next = { ...input.state, version: previous + 1 };
        tx.set(ref, { version: next.version, json: JSON.stringify(input.state) });
        return next;
      });
    }
    const verified = await db.runTransaction(tx => authority(tx, input, user));
    if (input.action === 'data') {
      if (!core) fail(503, 'DATA_UNAVAILABLE');
      return core(token, { protocol: DATA_PROTOCOL_VERSION, operation: input.operation, command: {
        action: 'visualization', destination: target(input), message: input.message, version: input.version,
        alias: input.alias, request: input.request,
      } });
    }
    const [bytes] = await visualizationFile(bucket, input.message, input.version).download({ validation: 'crc32c' });
    if (bytes.length > maxPackageBytes || hash(bytes) !== input.version) fail(503, 'DATA_UNAVAILABLE');
    const state = await db.runTransaction(async tx => {
      await authority(tx, input, user);
      return viewerState((await tx.get(stateRef(input, user.uid))).data());
    });
    return { bundle: bundle.parse(JSON.parse(bytes)), bindings: verified.bindings, state };
  }

  return { execute, audienceFor, reserveAndUpload, canPublish, sweep: () => sweepVisualizations(db, bucket, clock()) };
}
