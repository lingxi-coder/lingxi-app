// Offsets are UTF-16, like the editor selection and member mentions.
export const messageStyles = { bold: 1, italic: 2, underline: 4, strikeThrough: 8 } as const;
export const formatTags = [
  [messageStyles.bold, 'strong'], [messageStyles.italic, 'em'],
  [messageStyles.underline, 'u'], [messageStyles.strikeThrough, 's'],
] as const;

export const maxMessageFormats = 256;
export type MessageFormat = { offset: number; length: number; style: number };

function splitsSurrogate(text: string, offset: number): boolean {
  const left = text.charCodeAt(offset - 1), right = text.charCodeAt(offset);
  return left >= 0xd800 && left <= 0xdbff && right >= 0xdc00 && right <= 0xdfff;
}

export function validMessageFormats(text: unknown, formats: unknown): formats is MessageFormat[] {
  if (typeof text !== 'string' || !Array.isArray(formats) || formats.length > maxMessageFormats) return false;
  let previousEnd = 0;
  for (const run of formats) {
    if (!run || typeof run !== 'object' || Object.keys(run).length !== 3) return false;
    const { offset, length, style } = run;
    if (!Number.isSafeInteger(offset) || offset < previousEnd || !Number.isSafeInteger(length) || length < 1
      || offset + length > text.length || !Number.isInteger(style) || style < 1 || style > 15
      || splitsSurrogate(text, offset) || splitsSurrogate(text, offset + length)) return false;
    previousEnd = offset + length;
  }
  return true;
}

export function messageFormatParts(text: string, formats: readonly MessageFormat[]) {
  const parts: { offset: number; text: string; style: number }[] = [];
  let offset = 0;
  for (const run of formats) {
    if (offset < run.offset) parts.push({ offset, text: text.slice(offset, run.offset), style: 0 });
    const end = run.offset + run.length;
    parts.push({ offset: run.offset, text: text.slice(run.offset, end), style: run.style });
    offset = end;
  }
  if (offset < text.length) parts.push({ offset, text: text.slice(offset), style: 0 });
  return parts;
}
