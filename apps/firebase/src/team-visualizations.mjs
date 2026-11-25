import { z } from 'zod';
import { DATA_PROTOCOL_VERSION } from './dataset-resource.mjs';
import { TeamMessagesStore } from './team-messages.mjs';
import { SharedBotsStore } from './shared-bots.mjs';
import { sharedBotCommand } from './shared-bot-commands.mjs';
import { LingxiDataError } from './lingxi-data.mjs';
import {
  bundle, canonical, channelAudience, confirmVisualizationBindings, dataRef, fail, hash, id, key, maxPackageBytes, messageId,
  mode, projection, rowRef, sameAudience, scope, threadAccess, versionId, visualizationChannelEnabled,
} from './team-visualization-contract.mjs';

const locator = { ...scope, message: messageId, version: versionId };
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
  z.object({ action: z.literal('check'), ...locator }).strict(),
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

export function createTeamVisualizations({ db, bucket, core, channels = '', clock = Date.now }) {
  if (!bucket) return undefined;
  const canPublish = input => visualizationChannelEnabled(channels, input.teamId, input.conversationId);
  const file = (page, version) => bucket.file(`team-visualizations/${page}/${version}.json`);
  const stateRef = (input, uid) => rowRef(db, input.message, input.version).collection('viewerState')
    .doc(key([uid, input.generation]));
  const emptyState = { version: 0, modelContent: null, privateContent: null };

  async function authority(tx, input, user) {
    const { ref, generation } = await threadAccess(tx, db, input, user.uid);
    const [source, document] = await tx.getAll(rowRef(db, input.message, input.version), ref.collection('messages').doc(input.message));
    const row = source.data(), message = document.data();
    if (!row || row.status !== 'active' || row.audience[user.uid] !== generation
        || row.destination.team !== input.teamId || row.destination.conversation !== input.conversationId
        || !message || message.recalled || message.visualization?.reference?.version !== input.version) {
      fail(403, 'DATA_ACCESS_REQUIRED');
    }
    return message.visualization;
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
      const { team, thread, ref: channel } = await threadAccess(tx, db, input, user.uid, !taskId);
      if (!sameAudience(audience, await channelAudience(tx, team, thread))) fail(409, 'DATA_SHARE_CHANGED');
      const operation = channel.collection('visualizationOperations').doc(page);
      const quota = team.collection('privateSettings').doc('visualizations');
      const [previous, usage, receipt] = await tx.getAll(ref, quota, operation);
      if (receipt.exists && receipt.data().intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
      if (previous.exists) return previous.data();
      await confirmVisualizationBindings(tx, channel, audience, user.uid, page, version, input.bundle.title, prepared);
      const consumed = usage.data()?.bytes ?? 0;
      if (!Number.isSafeInteger(consumed) || consumed < 0 || consumed + Buffer.byteLength(bytes) > budget) fail(429, 'DATA_QUOTA');
      const value = { uid: user.uid, destination: target(input), message: page, version, status: 'pending', audience,
        title: input.bundle.title, bindings: prepared, ...(taskId ? { taskId } : {}), intentDigest };
      tx.create(ref, value);
      tx.create(operation, { intentDigest });
      tx.set(quota, { bytes: consumed + Buffer.byteLength(bytes) });
      return value;
    });
    if (row.intentDigest !== intentDigest) fail(409, 'MESSAGE_ID_CONFLICT');
    const blob = file(page, version);
    try {
      await blob.save(bytes, { resumable: false, preconditionOpts: { ifGenerationMatch: 0 },
        metadata: { contentType: 'application/json', cacheControl: 'no-store' } });
    } catch (error) {
      if (Number(error.code) !== 412) throw error;
      const [existing] = await blob.download({ validation: 'crc32c' });
      if (existing.length > maxPackageBytes || hash(existing) !== version) fail(503, 'DATA_UNAVAILABLE');
    }
    return { reference: artifact(page, version), title: row.title, bindings: row.bindings };
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
    const { task, publish, replay, expectsVisualization } = await bot.inspectVisualizationCompletion(user, command);
    if (task.taskId !== input.artifact.artifactId) fail(403, 'EXECUTION_PERMIT_REQUIRED');
    const location = { teamId: command.teamId, conversationId: task.conversationId, generation: command.generation };
    const contents = input.artifact.parts[0].data;
    const bytes = canonical(contents.bundle), version = hash(bytes);
    const page = 'viz_' + key([target(location), task.taskId]);
    if (replay) {
      const stored = expectsVisualization && (await rowRef(db, page, version).get()).data();
      const visualization = stored && { reference: artifact(page, version), title: stored.title, bindings: stored.bindings };
      return bot.execute(user, command, undefined, undefined, visualization || undefined);
    }
    if (!publish || !canPublish(location)) {
      return bot.execute(user, command);
    }
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
      return await bot.execute(user, command, undefined, undefined, visualization);
    } catch (error) {
      if (!(error instanceof LingxiDataError) || !['DATA_ACCESS_REQUIRED', 'DATA_SHARE_CHANGED', 'DATA_QUOTA'].includes(error.code)) throw error;
      return bot.execute(user, command);
    }
  }

  async function execute(user, token, raw) {
    const input = request.parse(raw);
    if (input.action === 'completeBot') return completeBot(user, input);
    if (input.action === 'audience' || input.action === 'publish') {
      const audience = await audienceFor(input, user), audienceDigest = key([target(input), audience]);
      if (input.action === 'audience') return { audience, audienceDigest };
      if (input.audienceDigest !== audienceDigest) fail(409, 'DATA_SHARE_CHANGED');
      const bytes = canonical(input.bundle), version = hash(bytes);
      const page = 'viz_' + key([user.uid, target(input), input.operation]);
      const prepared = await prepareBindings(input, token, page, version, audience);
      if (!prepared) return { status: 'building' };
      const delivered = await reserveAndUpload(input, user, audience, prepared, page, version, bytes);
      await new TeamMessagesStore(db, clock).execute(user, { action: 'mutate', operation: 'create', teamId: input.teamId,
        generation: input.generation, conversationId: input.conversationId, changeId: page, messageId: page,
        text: delivered.title }, undefined, delivered);
      return { status: 'published', messageId: page, reference: delivered.reference };
    }
    const verified = await db.runTransaction(tx => authority(tx, input, user));
    if (input.action === 'check') return { active: true };
    if (input.action === 'state') {
      return db.runTransaction(async tx => {
        await authority(tx, input, user);
        const ref = stateRef(input, user.uid);
        const previous = (await tx.get(ref)).data() ?? emptyState;
        if (previous.version !== input.expected) fail(409, 'DATA_CONFLICT');
        const next = { ...input.state, version: previous.version + 1 };
        tx.set(ref, next);
        return next;
      });
    }
    if (input.action === 'data') {
      if (!core) fail(503, 'DATA_UNAVAILABLE');
      return core(token, { protocol: DATA_PROTOCOL_VERSION, operation: input.operation, command: {
        action: 'visualization', destination: target(input), message: input.message, version: input.version,
        alias: input.alias, request: input.request,
      } });
    }
    const [bytes] = await file(input.message, input.version).download({ validation: 'crc32c' });
    if (bytes.length > maxPackageBytes || hash(bytes) !== input.version) fail(503, 'DATA_UNAVAILABLE');
    const state = await db.runTransaction(async tx => {
      await authority(tx, input, user);
      return (await tx.get(stateRef(input, user.uid))).data() ?? emptyState;
    });
    return { bundle: bundle.parse(JSON.parse(bytes)), bindings: verified.bindings, state };
  }

  return { execute, audienceFor, reserveAndUpload, canPublish };
}
