import { z } from 'zod';
import { DATA_PROTOCOL_VERSION } from './dataset-resource.mjs';
import { TeamMessagesStore } from './team-messages.mjs';
import { SharedBotsStore } from './shared-bots.mjs';
import { sharedBotCommand } from './shared-bot-commands.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import { reservationMs, rowRef, scheduleVisualizationCleanup, sweepVisualizations, trackVisualization, visualizationPart, visualizationQuota } from './team-visualization-storage.mjs';
import {
  canonical, channelAudience, checkVisualizationSize, completeUpload, confirmVisualizationBindings, dataRef, fail, hash, id, key,
  maxPackageMiB, maxPackageParts, messageId, mode, packageBytes, packagePartBytes, partSize, projection, sameAudience, scope, threadAccess,
  versionId, visualizationChannelEnabled, visualizationLimit, visualizationPackage,
} from './team-visualization-contract.mjs';

const cardLocator = { message: messageId, version: versionId };
const locator = { ...scope, ...cardLocator };
const alias = { alias: id };
const stage = z.enum(['prepare', 'commit']);
const index = z.number().int().min(0).max(maxPackageParts - 1);
const bindings = z.array(z.discriminatedUnion('kind', [
  z.object({ ...alias, kind: z.literal('reuse'), reference: dataRef, grant: id, mode }).strict(),
  z.object({ ...alias, kind: z.literal('share'), dataset: id, via: id.optional(),
    mode, projection: projection.default({}), reference: dataRef.optional() }).strict(),
])).max(4).refine(items => new Set(items.map(item => item.alias)).size === items.length);
const a2aArtifact = z.object({ artifactId: id, name: z.string(),
  parts: z.tuple([z.object({ mediaType: z.literal('application/json'), data: z.object({
    version: z.literal(2), package: visualizationPackage, bindings: z.array(z.object({ alias: id, messageId: id, sourceAlias: id.optional() }).strict())
      .max(4).refine(items => new Set(items.map(item => item.alias)).size === items.length),
  }).strict() }).strict()]),
}).strict().refine(value => value.name === value.parts[0].data.package.title);
const request = z.discriminatedUnion('action', [
  z.object({ action: z.literal('audience'), ...scope }).strict(),
  z.object({ action: z.literal('settings'), ...scope }).strict(),
  z.object({ action: z.literal('setLimit'), ...scope, limitMiB: z.number().int().min(1).max(maxPackageMiB),
    expectedRevision: z.number().int().nonnegative() }).strict(),
  z.object({ action: z.literal('publish'), ...scope, stage, operation: id, audienceDigest: id,
    package: visualizationPackage, bindings: bindings.default([]) }).strict(),
  z.object({ action: z.literal('completeBot'), stage, completion: sharedBotCommand, artifact: a2aArtifact }).strict(),
  z.object({ action: z.literal('putPart'), ...locator, index,
    data: z.string().min(4).max(Math.ceil(packagePartBytes / 3) * 4).regex(/^[A-Za-z0-9+/]+={0,2}$/u) }).strict(),
  z.object({ action: z.literal('readPart'), ...locator, index }).strict(),
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
const uploadPlan = (location, message, version, row) => ({
  status: 'uploading', teamId: location.teamId, conversationId: location.conversationId, generation: location.generation,
  message, version, missing: row.package.parts.flatMap((_, index) => row.uploaded.includes(index) ? [] : [index]),
});

export function createTeamVisualizations({ db, bucket, core, channels = '', clock = Date.now }) {
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
        && message.visualization.reference.version === input.version) return { row, descriptor: message.visualization };
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
        audienceDigest: key([target(input), audience]), textDigest: hash(input.package.title, 'base64url'),
        visualization: { message: page, version, binding: spec.alias },
      } });
      if (prepared.messageId !== page || !dataRef.safeParse(prepared.reference).success || !id.safeParse(prepared.grant).success) {
        fail(503, 'DATA_UNAVAILABLE');
      }
      results.push({ alias: spec.alias, reference: prepared.reference, grant: prepared.grant, mode: spec.mode });
    }
    return results;
  }

  async function reserve(input, user, audience, prepared, page, version, intentDigest, taskId) {
    const ref = rowRef(db, page, version);
    return db.runTransaction(async tx => {
      const { team, thread, ref: channel } = await threadAccess(tx, db, input, user.uid);
      const operation = channel.collection('visualizationOperations').doc(page);
      const quota = visualizationQuota(team);
      const [previous, usage, receipt] = await tx.getAll(ref, quota, operation);
      if (receipt.exists && receipt.data().intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
      const existing = previous.data();
      if (existing && existing.intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
      if (committed(existing)) return existing;
      const size = packageBytes(input.package);
      checkVisualizationSize(thread, size);
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
      await confirmVisualizationBindings(tx, channel, audience, user.uid, page, version, input.package.title, prepared);
      const consumed = usage.data()?.bytes ?? 0;
      const expiresAt = clock() + reservationMs;
      if (!Number.isSafeInteger(consumed) || consumed < 0 || consumed + size > budget) fail(429, 'DATA_QUOTA');
      const value = { uid: user.uid, destination: target(input), message: page, version, status: 'pending', audience, bytes: size, expiresAt,
        title: input.package.title, package: input.package, uploaded: [], bindings: prepared, ...(taskId ? { taskId } : {}), intentDigest };
      tx.create(ref, value);
      trackVisualization(tx, team, artifact(page, version));
      tx.create(operation, { intentDigest });
      tx.set(quota, { bytes: consumed + size });
      scheduleVisualizationCleanup(tx, db, team, artifact(page, version), expiresAt);
      return value;
    });
  }

  async function putPart(input, user) {
    const ref = rowRef(db, input.message, input.version);
    const row = await db.runTransaction(async tx => {
      const { team, thread } = await threadAccess(tx, db, input, user.uid);
      const row = (await tx.get(ref)).data();
      if (!row || row.uid !== user.uid || row.destination.team !== input.teamId || row.destination.conversation !== input.conversationId) {
        fail(403, 'DATA_ACCESS_REQUIRED');
      }
      if (row.status === 'active' && row.uploaded?.includes(input.index)) return row;
      if (row.status !== 'pending' || !(row.expiresAt > clock())) fail(409, 'DATA_SHARE_CHANGED');
      checkVisualizationSize(thread, row.bytes);
      if (!row.package.parts[input.index]) fail(400, 'INVALID_DATA_REQUEST');
      if (row.expiresAt - clock() < reservationMs / 2) {
        const expiresAt = clock() + reservationMs;
        tx.update(ref, { expiresAt });
        scheduleVisualizationCleanup(tx, db, team, artifact(input.message, input.version), expiresAt);
      }
      return row;
    });
    const bytes = Buffer.from(input.data, 'base64');
    const expected = row.package.parts[input.index];
    if (bytes.length !== partSize(row.package, input.index) || hash(bytes) !== expected) fail(400, 'INVALID_DATA_REQUEST');
    if (row.uploaded.includes(input.index)) return { ok: true };
    const blob = visualizationPart(bucket, input.message, input.version, input.index);
    try {
      await blob.save(bytes, { resumable: false, preconditionOpts: { ifGenerationMatch: 0 },
        metadata: { contentType: 'application/octet-stream', cacheControl: 'no-store' } });
    } catch (error) {
      if (Number(error.code) !== 412) throw error;
      const [existing] = await blob.download({ validation: 'crc32c' });
      if (existing.length !== bytes.length || hash(existing) !== expected) fail(503, 'DATA_UNAVAILABLE');
    }
    const kept = await db.runTransaction(async tx => {
      const current = (await tx.get(ref)).data();
      if (current?.status === 'active') return current.uploaded.includes(input.index);
      if (current?.status !== 'pending' || !(current.expiresAt > clock())) return false;
      if (!current.uploaded.includes(input.index)) tx.update(ref, { uploaded: [...current.uploaded, input.index] });
      return true;
    });
    if (!kept) {
      await blob.delete({ ignoreNotFound: true });
      fail(409, 'DATA_SHARE_CHANGED');
    }
    return { ok: true };
  }

  async function audienceFor(input, user) {
    if (!canPublish(input)) fail(404, 'VISUALIZATION_UNAVAILABLE');
    return db.runTransaction(async tx => {
      const { team, thread } = await threadAccess(tx, db, input, user.uid, true);
      const audience = await channelAudience(tx, team, thread);
      return { audience, audienceDigest: key([target(input), audience]) };
    });
  }

  function settings(input, user) {
    return db.runTransaction(async tx => {
      const { thread, ref, member } = await threadAccess(tx, db, input, user.uid);
      const current = visualizationLimit(thread);
      const canEdit = ['owner', 'admin'].includes(member.role);
      if (input.action === 'settings') return { ...current, canEdit };
      if (!canEdit) fail(403, 'MANAGER_REQUIRED');
      if (current.revision !== input.expectedRevision) {
        if (current.limitMiB === input.limitMiB) return { ...current, canEdit };
        fail(409, 'VISUALIZATION_LIMIT_CONFLICT');
      }
      if (current.limitMiB === input.limitMiB) return { ...current, canEdit };
      const next = { limitMiB: input.limitMiB, revision: current.revision + 1 };
      tx.update(ref, { visualizationLimit: next });
      return { ...next, canEdit };
    });
  }

  async function completeBot(user, input) {
    const command = input.completion;
    const bot = new SharedBotsStore(db, clock);
    const { task, publish, replay } = await bot.inspectVisualizationCompletion(user, command);
    if (task.taskId !== input.artifact.artifactId) fail(403, 'EXECUTION_PERMIT_REQUIRED');
    const location = { teamId: command.teamId, conversationId: task.conversationId, generation: command.generation };
    const contents = input.artifact.parts[0].data;
    const version = hash(canonical(contents.package));
    const page = 'viz_' + key([target(location), task.taskId]);
    const intent = key(input.artifact);
    const finish = async descriptor => ({ status: 'completed', result: await bot.execute(user, command, undefined, undefined, { intent, descriptor }) });
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
      const row = await reserve({ ...location, ...contents }, user, audience, prepared, page, version, intent, task.taskId);
      if (committed(row)) return finish();
      if (input.stage === 'prepare') return uploadPlan(location, page, version, row);
      if (!completeUpload(row)) fail(409, 'VISUALIZATION_UPLOAD_INCOMPLETE');
      return await finish(descriptor(row, page, version));
    } catch (error) {
      if (!(error instanceof LingxiDataError) || !['DATA_ACCESS_REQUIRED', 'DATA_SHARE_CHANGED', 'DATA_QUOTA', 'VISUALIZATION_TOO_LARGE'].includes(error.code)) throw error;
      return finish();
    }
  }

  async function execute(user, token, raw) {
    const input = request.parse(raw);
    if (input.action === 'settings' || input.action === 'setLimit') return settings(input, user);
    if (!bucket) fail(404, 'VISUALIZATION_UNAVAILABLE');
    if (input.action === 'completeBot') return completeBot(user, input);
    if (input.action === 'putPart') return putPart(input, user);
    if (input.action === 'checkBatch') return db.runTransaction(tx => checkBatch(tx, input, user));
    if (input.action === 'audience') return audienceFor(input, user);
    if (input.action === 'publish') {
      const version = hash(canonical(input.package));
      const page = 'viz_' + key([user.uid, target(input), input.operation]);
      const result = { status: 'published', messageId: page, reference: artifact(page, version) };
      const { stage, ...intent } = input;
      const intentDigest = key(intent);
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
          checkVisualizationSize(thread, packageBytes(input.package));
          return channelAudience(tx, team, thread);
        });
        if (!audience) return result;
        if (input.audienceDigest !== key([target(input), audience])) fail(409, 'DATA_SHARE_CHANGED');
        const prepared = await prepareBindings(input, token, page, version, audience);
        if (!prepared) return { status: 'building' };
        const row = await reserve(input, user, audience, prepared, page, version, intentDigest);
        if (committed(row)) return result;
        if (stage === 'prepare') return uploadPlan(input, page, version, row);
        if (!completeUpload(row)) fail(409, 'VISUALIZATION_UPLOAD_INCOMPLETE');
        const delivered = descriptor(row, page, version);
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
    if (input.action === 'open') return db.runTransaction(async tx => {
      const { row, descriptor } = await authority(tx, input, user);
      if (!completeUpload(row)) fail(403, 'DATA_ACCESS_REQUIRED');
      const state = viewerState((await tx.get(stateRef(input, user.uid))).data());
      return { package: row.package, bindings: descriptor.bindings, state };
    });
    const { row } = await db.runTransaction(tx => authority(tx, input, user));
    if (input.action === 'data') {
      if (!core) fail(503, 'DATA_UNAVAILABLE');
      return core(token, { protocol: DATA_PROTOCOL_VERSION, operation: input.operation, command: {
        action: 'visualization', destination: target(input), message: input.message, version: input.version,
        alias: input.alias, request: input.request,
      } });
    }
    const expected = row.package?.parts[input.index];
    if (!expected) fail(400, 'INVALID_DATA_REQUEST');
    const [bytes] = await visualizationPart(bucket, input.message, input.version, input.index).download({ validation: 'crc32c' });
    if (bytes.length !== partSize(row.package, input.index) || hash(bytes) !== expected) fail(503, 'DATA_UNAVAILABLE');
    await db.runTransaction(tx => authority(tx, input, user));
    return { data: bytes.toString('base64') };
  }

  return { execute, sweep: async () => { if (bucket) await sweepVisualizations(db, bucket, clock()); } };
}
