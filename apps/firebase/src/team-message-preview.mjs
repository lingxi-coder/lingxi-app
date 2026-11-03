/** Conversation-list preview limit in Unicode code points. firestore.default.rules
 * enforces the same bound with `preview.matches('[\\s\\S]{0,120}')`: rules `size()`
 * counts UTF-16 units, so only the RE2 match counts code points like this file. */
export const previewLimit = 120;

/** `lastMessage.preview`: whitespace runs (including newlines) collapse to one
 * space and the text is trimmed, then cut to `previewLimit` code points (trimmed
 * again, so a cut never leaves a trailing space). Counting code points, not UTF-16
 * units, means an astral character is never split in half. A message without text
 * (or a recalled one) previews as ''. */
export function messagePreview(text) {
  if (typeof text !== 'string') return '';
  const flat = text.replace(/\s+/gu, ' ').trim();
  let units = 0, points = 0;
  for (const point of flat) {
    if (points === previewLimit) return flat.slice(0, units).trimEnd();
    units += point.length; points++;
  }
  return flat;
}
