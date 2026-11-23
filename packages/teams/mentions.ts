export interface MentionEntity {
  userId: string;
  offset: number;
  length: number;
}

export const MAX_MEMBER_MENTIONS = 200;

export function isMentionEntity(value: unknown): value is MentionEntity {
  if (!value || typeof value !== 'object') return false;
  const { userId, offset, length } = value as Partial<MentionEntity>;
  return typeof userId === 'string' && userId.length > 0 && userId.length <= 128
    && typeof offset === 'number' && Number.isInteger(offset) && offset >= 0 && offset <= 8192
    && typeof length === 'number' && Number.isInteger(length) && length >= 2 && length <= 8192;
}

export function memberMentionAt(text: string, caret: number, entities: readonly MentionEntity[]) {
  const before = text.slice(0, caret);
  const match = /(^|[\s(（])@([^\r\n@]{0,120})(?![\s\S])/u.exec(before);
  if (!match) return null;
  const query = match[2]!;
  const start = before.length - query.length - 1;
  if (entities.some(entity => start >= entity.offset && start < entity.offset + entity.length)) return null;
  return { start, query };
}

export const mentionSearchKey = (text: string) => text.trim().replace(/\s+/gu, ' ').toLowerCase();

export interface MentionEditRange { start: number; end: number }

export function replaceMentionRanges(entities: readonly MentionEntity[], start: number, end: number, insertedLength: number): MentionEntity[] {
  const delta = insertedLength - (end - start);
  return entities.flatMap(entity => {
    if (entity.offset + entity.length <= start) return [entity];
    if (entity.offset >= end) return [{ ...entity, offset: entity.offset + delta }];
    return [];
  });
}

/** Prefer a verified editor range. An ambiguous text diff can only preserve spans outside every plausible edit. */
export function shiftMentionEntities(before: string, after: string, entities: readonly MentionEntity[],
  selection?: MentionEditRange, caretAfter?: number): MentionEntity[] {
  if (!entities.length) return [];
  const delta = after.length - before.length;
  const fits = ({ start, end }: MentionEditRange) => {
    const inserted = end - start + delta;
    return start >= 0 && end >= start && end <= before.length && inserted >= 0
      && before.slice(0, start) === after.slice(0, start) && before.slice(end) === after.slice(start + inserted);
  };
  const caretRange = caretAfter === undefined || delta === 0 ? undefined : delta > 0
    ? { start: caretAfter - delta, end: caretAfter - delta }
    : { start: caretAfter, end: caretAfter - delta };
  const expectedCaret = selection && selection.end + delta;
  const exact = selection && (caretAfter === undefined || expectedCaret === caretAfter) && fits(selection) ? selection
    : caretRange && fits(caretRange) ? caretRange : undefined;
  if (exact) return replaceMentionRanges(entities, exact.start, exact.end, exact.end - exact.start + delta);
  if (before === after) return [...entities];

  const common = Math.min(before.length, after.length);
  let prefix = 0, suffix = 0;
  while (prefix < common && before[prefix] === after[prefix]) prefix++;
  while (suffix < common && before[before.length - 1 - suffix] === after[after.length - 1 - suffix]) suffix++;
  const firstStart = Math.min(prefix, common - suffix);
  const lastEnd = before.length - Math.min(suffix, common - prefix);
  return replaceMentionRanges(entities, firstStart, lastEnd, lastEnd - firstStart + delta);
}

/** Consume text to the right only when it continues the selected candidate at a word boundary. */
export function mentionReplacementEnd(text: string, start: number, caret: number, name: string): number {
  const typed = mentionSearchKey(text.slice(start + 1, caret)), target = mentionSearchKey(name);
  const startsNamePart = (value: string) => (' ' + target).includes(' ' + value);
  if (!typed || typed === target || !startsNamePart(typed)) return caret;
  let end = caret, matched = caret;
  for (const char of text.slice(caret, start + 1 + Math.max(120, name.length))) {
    if (/[\r\n@]/u.test(char)) break;
    end += char.length;
    if (/\s/u.test(char)) continue;
    const extended = mentionSearchKey(text.slice(start + 1, end));
    if (!startsNamePart(extended)) break;
    if (end === text.length || /^[\s\p{P}]/u.test(text.slice(end))) matched = end;
    if (extended === target) break;
  }
  return matched;
}

export function parseMentionEntities(text: string, values: unknown): MentionEntity[] {
  if (!Array.isArray(values) || values.length > MAX_MEMBER_MENTIONS) return [];
  let end = 0;
  for (const entity of values) {
    if (!isMentionEntity(entity) || entity.offset < end || text[entity.offset] !== '@') return [];
    end = entity.offset + entity.length;
    if (end > text.length || /[\uDC00-\uDFFF]/u.test(text[end] ?? '')) return [];
  }
  return values;
}

export function mentionTextParts(text: string, entities: unknown) {
  let end = 0;
  const parts: { text: string; mention?: MentionEntity }[] = [];
  for (const mention of parseMentionEntities(text, entities)) {
    if (end < mention.offset) parts.push({ text: text.slice(end, mention.offset) });
    end = mention.offset + mention.length;
    parts.push({ text: text.slice(mention.offset, end), mention });
  }
  if (end < text.length) parts.push({ text: text.slice(end) });
  return parts;
}
