import { z } from 'zod';

export const mentionEntitiesSchema = z.array(z.object({
  userId: z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u),
  offset: z.number().int().min(0).max(8192),
  length: z.number().int().min(2).max(8192)
}).strict()).max(200);

export function validMentionEntities(text, entities = []) {
  let end = 0;
  const boundary = index => index === text.length || !/[\uDC00-\uDFFF]/u.test(text[index] ?? '');
  for (const { offset, length } of entities) {
    const label = text.slice(offset + 1, offset + length);
    if (offset < end || offset + length > text.length || text[offset] !== '@' || label !== label.trim()
      || /[\r\n]/u.test(label) || !boundary(offset + length)) return false;
    end = offset + length;
  }
  return true;
}

/** Clients without ranges can preserve only spans outside every plausible edit. */
export function preserveMentionEntities(before, after, entities) {
  if (!entities?.length) return [];
  const common = Math.min(before.length, after.length);
  let prefix = 0, suffix = 0;
  while (prefix < common && before[prefix] === after[prefix]) prefix++;
  while (suffix < common && before[before.length - 1 - suffix] === after[after.length - 1 - suffix]) suffix++;
  const start = before === after ? before.length : Math.min(prefix, common - suffix);
  const end = before.length - Math.min(suffix, common - prefix);
  const delta = after.length - before.length;
  return entities.flatMap(entity => {
    if (entity.offset + entity.length <= start) return [entity];
    if (entity.offset < end) return [];
    const moved = { ...entity, offset: entity.offset + delta };
    return validMentionEntities(after, [moved]) ? [moved] : [];
  });
}

export function mentionMemberRefs(team, conversation, entities, sender) {
  if (!entities?.length) return [];
  if (!['group', 'channel'].includes(conversation.kind)) return null;
  const ids = [...new Set(entities.map(({ userId }) => userId))];
  const participants = new Set(conversation.participants), members = new Set(conversation.memberUids);
  if (ids.some(id => id === sender || !participants.has(`user:${id}`) || !members.has(id))) return null;
  return ids.map(id => team.collection('members').doc(id));
}

export function mentionFields(entities, bots = []) {
  const mentionRecipientUids = [...new Set(entities.map(({ userId }) => userId))];
  return {
    mentionEntities: entities,
    mentionRecipientUids,
    mentions: [...bots, ...mentionRecipientUids.map(uid => `user:${uid}`)]
  };
}

/** Delivery recipients are frozen on send. Inline ranges can change on edit. */
export function messageMentionRecipients(message) {
  const stored = message.mentionRecipientUids;
  const values = Array.isArray(stored) ? stored : Array.isArray(message.mentions) ? message.mentions
    .filter(value => typeof value === 'string' && !value.startsWith('shared_bot:'))
    .map(value => value.startsWith('user:') ? value.slice(5) : value) : [];
  return [...new Set(values.filter(uid => typeof uid === 'string' && uid.length > 0 && uid.length <= 128 && uid !== message.author?.uid))];
}
