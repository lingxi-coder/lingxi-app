import { createHash } from 'node:crypto';
import { z } from 'zod';
import { LingxiDataError } from './lingxi-data.mjs';

export const dataId = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
export const generation = z.number().int().min(1).max(Number.MAX_SAFE_INTEGER);
const member = z.object({ uid: dataId, generation }).strict();
const bot = z.object({ sharedBotId: dataId, principalUid: dataId, generation }).strict();
const sourceSpec = z.object({
  teamId: dataId, conversationId: dataId, version: generation, enabled: z.boolean(),
  owner: member, requesters: z.array(member).max(500), bots: z.array(bot).max(32),
}).strict();

export function denyPreview(status = 403, code = 'DATA_ACCESS_REQUIRED') {
  throw new LingxiDataError(status, code);
}

export function sourceBotPrincipal(teamId, source, sharedBotId) {
  return 'lx_data_bot_' + createHash('sha256').update(JSON.stringify([teamId, source, sharedBotId])).digest('hex').slice(0, 40);
}

export async function sourceBinding(tx, db, source) {
  const decoded = sourceSpec.safeParse((await tx.get(db.collection('lingxiDataSourceBindings').doc(source))).data());
  if (!decoded.success || !decoded.data.enabled) denyPreview();
  const binding = decoded.data;
  const principals = binding.bots.map(({ principalUid }) => principalUid);
  if (binding.bots.some(bot => bot.principalUid !== sourceBotPrincipal(binding.teamId, source, bot.sharedBotId))
    || principals.includes(binding.owner.uid) || new Set(principals).size !== principals.length
    || new Set(binding.bots.map(({ sharedBotId }) => sharedBotId)).size !== binding.bots.length) denyPreview();
  const team = db.collection('teams').doc(binding.teamId);
  const [teamDoc, owner] = await tx.getAll(team, team.collection('members').doc(binding.owner.uid));
  if (teamDoc.data()?.state !== 'active' || owner.data()?.generation !== binding.owner.generation) denyPreview();
  return { binding, team };
}

export async function allowedPrivateSource(tx, db, team, source, conversationId, sharedBotId, requester) {
  const { binding } = await sourceBinding(tx, db, source);
  if (binding.teamId !== team.id || binding.conversationId !== conversationId
    || !binding.requesters.some(member => member.uid === requester.uid && member.generation === requester.generation)) denyPreview();
  const bot = binding.bots.find(actor => actor.sharedBotId === sharedBotId);
  if (!bot) denyPreview();
  return { binding, bot };
}
