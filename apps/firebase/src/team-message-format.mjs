import { z } from 'zod';

// Same UTF-16 range and style contract as the message composer.
export const messageFormatsSchema = z.array(z.object({
  offset: z.number().int().min(0).max(8192),
  length: z.number().int().min(1).max(8192),
  style: z.number().int().min(1).max(15),
}).strict()).max(256);

export function validMessageFormats(text, formats) {
  let previousEnd = 0;
  for (const { offset, length } of formats ?? []) {
    const end = offset + length;
    if (offset < previousEnd || end > text.length || splitsSurrogate(text, offset) || splitsSurrogate(text, end)) return false;
    previousEnd = end;
  }
  return true;
}

function splitsSurrogate(text, offset) {
  const left = text.charCodeAt(offset - 1), right = text.charCodeAt(offset);
  return left >= 0xd800 && left <= 0xdbff && right >= 0xdc00 && right <= 0xdfff;
}
