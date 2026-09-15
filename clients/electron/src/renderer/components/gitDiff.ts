/** Preserve complete unified hunks, including their file headers, for Git apply. */
export interface ReviewHunk { heading: string; lines: string[]; patch: string; oldStart: number; newStart: number }
export function reviewHunks(patch: string): ReviewHunk[] {
  const lines = patch.split('\n');
  const result: ReviewHunk[] = [];
  let header: string[] = [];
  let current: ReviewHunk | undefined;
  for (const line of lines) {
    if (line.startsWith('diff --git ')) { header = [line]; current = undefined; }
    else if (line.startsWith('@@ ')) {
      const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      current = { heading: line, lines: [], patch: [...header, line].join('\n') + '\n', oldStart: Number(match?.[1] ?? 0), newStart: Number(match?.[2] ?? 0) };
      result.push(current);
    } else if (current && (line.startsWith('+') || line.startsWith('-') || line.startsWith(' ') || line.startsWith('\\'))) {
      current.lines.push(line); current.patch += line + '\n';
    } else if (!current && line) header.push(line);
  }
  return result;
}
export interface ReviewLine { old?: number; next?: number; text: string; kind: 'add' | 'remove' | 'context' | 'note' }
export function reviewLines(hunk: ReviewHunk): ReviewLine[] {
  let old = hunk.oldStart; let next = hunk.newStart;
  return hunk.lines.map((line) => line.startsWith('+') ? { next: next++, text: line.slice(1), kind: 'add' } : line.startsWith('-') ? { old: old++, text: line.slice(1), kind: 'remove' } : line.startsWith('\\') ? { text: line, kind: 'note' } : { old: old++, next: next++, text: line.slice(1), kind: 'context' });
}
export function splitReviewLines(lines: ReviewLine[]): [ReviewLine | undefined, ReviewLine | undefined][] {
  const rows: [ReviewLine | undefined, ReviewLine | undefined][] = [];
  for (let i = 0; i < lines.length;) {
    if (lines[i].kind === 'remove' || lines[i].kind === 'add') {
      const removed: ReviewLine[] = []; const added: ReviewLine[] = [];
      while (i < lines.length && lines[i].kind === 'remove') removed.push(lines[i++]);
      while (i < lines.length && lines[i].kind === 'add') added.push(lines[i++]);
      for (let j = 0; j < Math.max(removed.length, added.length); j++) rows.push([removed[j], added[j]]);
    } else { rows.push([lines[i], lines[i]]); i++; }
  }
  return rows;
}
