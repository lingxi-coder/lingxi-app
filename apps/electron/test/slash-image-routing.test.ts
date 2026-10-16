import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

test('desktop slash submission forwards attached images and clears only after admission', () => {
  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  assert.doesNotMatch(source, /if\s*\(isSlashCommand\s*&&\s*imageAttachments\.length\)/);
  assert.match(source, /const images: ImageRefDto\[\] = imageAttachments\.map/);
  assert.match(source, /bridge\.runSlashCommand\(slashCommand, images, restoreOnTerminalError\)/);
  const dispatch = source.indexOf('const accepted = await bridge.runSlashCommand(slashCommand, images, restoreOnTerminalError);');
  assert.ok(dispatch >= 0, 'the composer must wait for host slash admission');
  const failedAdmission = source.indexOf('if (!accepted)', dispatch);
  const successBranch = source.indexOf('if (ownsVisibleDraft())', failedAdmission);
  const clear = source.indexOf('clearComposer(submittingSessionId, submitted.imageIds)', successBranch);
  assert.ok(failedAdmission > dispatch && successBranch > failedAdmission && clear > successBranch,
    'a failed or stale submission must not clear the current image draft');
  assert.match(source, /桌面本地命令不支持图片附件/,
    'only a desktop-local command without an image consumer should remain blocked');
});
