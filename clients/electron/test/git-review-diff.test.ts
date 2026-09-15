import assert from 'node:assert/strict';
import { test } from 'node:test';
import { reviewHunks, reviewLines, splitReviewLines } from '../src/renderer/components/gitDiff';
test('review preserves per-file complete patches and Unicode, with independent hunk ranges', () => {
 const patch = 'diff --git a/a b/a\nindex 123..456 100644\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-old\n+中文\n tail\n@@ -20 +20 @@\n-x\n+y\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -2 +2 @@\n-a\n+b\n';
 const hunks = reviewHunks(patch); assert.equal(hunks.length, 3); assert.match(hunks[1].patch, /^diff --git a\/a b\/a\n/); assert.ok(!hunks[1].patch.includes('中文')); assert.match(hunks[2].patch, /^diff --git a\/b b\/b\n/);
 const lines = reviewLines(hunks[0]); assert.deepEqual(lines.map(l=>[l.old,l.next,l.text]), [[1,undefined,'old'],[undefined,1,'中文'],[2,2,'tail']]);
 const split = splitReviewLines(lines); assert.equal(split.length, 2); assert.equal(split[0][0]?.kind, 'remove'); assert.equal(split[0][1]?.kind, 'add');
});
test('review handles added-only hunks and no-newline markers without fabricated line numbers', () => {
 const hunks = reviewHunks('diff --git a/new b/new\nnew file mode 100644\n--- /dev/null\n+++ b/new\n@@ -0,0 +1,2 @@\n+one\n+two\n\\ No newline at end of file\n');
 const lines = reviewLines(hunks[0]); assert.equal(lines[2].kind, 'note'); assert.equal(lines[2].next, undefined); assert.equal(splitReviewLines(lines).length, 3);
});
