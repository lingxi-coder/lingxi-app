import assert from 'node:assert/strict';
import test from 'node:test';
import { crashCategory, isIsolatedCrashProbe } from '../src/main/crashReportingPolicy.js';
// @ts-expect-error Node packaging script has no TypeScript declarations.
import { validateCrashlyticsConfig } from '../scripts/desktop-crashlytics.mjs';

test('normal exits and intentional kills are not reported as crashes', () => {
  for (const reason of ['clean-exit', 'killed', 'launch-failed', 'integrity-failure']) {
    assert.equal(crashCategory('renderer', reason), null);
    assert.equal(crashCategory('GPU', reason), null);
  }
  assert.equal(crashCategory('renderer', 'oom'), 'renderer-oom');
  assert.equal(crashCategory('renderer', 'crashed'), 'renderer-crashed');
  assert.equal(crashCategory('GPU', 'abnormal-exit'), 'gpu-crashed');
});

test('Desktop native reporting rejects Web Firebase IDs, other projects and mismatched bundle IDs', () => {
  const valid = { PROJECT_ID: 'lingxi-5e4b2', BUNDLE_ID: 'com.lingxi.code.development', GOOGLE_APP_ID: '1:55470483519:ios:ee06a2bd809d9f2d57708a' };
  assert.equal(validateCrashlyticsConfig(valid, valid.BUNDLE_ID), valid);
  assert.throws(() => validateCrashlyticsConfig({ ...valid, GOOGLE_APP_ID: '1:55470483519:web:ec5536aff825451a57708a' }, valid.BUNDLE_ID));
  assert.throws(() => validateCrashlyticsConfig({ ...valid, PROJECT_ID: 'another-project' }, valid.BUNDLE_ID));
  assert.throws(() => validateCrashlyticsConfig(valid, 'com.lingxi.code'));
});

test('intentional crash cannot target an ordinary or partially isolated user home', () => {
  const root = '/private/tmp/lingxi-crashlytics-acceptance-test';
  assert.equal(isIsolatedCrashProbe(root, '/private/tmp', `${root}/userData`, `${root}/home`, `${root}/home`), true);
  assert.equal(isIsolatedCrashProbe(root, '/private/tmp', '/Users/user/Library/Application Support/lingxi-code-desktop', `${root}/home`, `${root}/home`), false);
  assert.equal(isIsolatedCrashProbe(root, '/private/tmp', `${root}/userData`, '/Users/user', '/Users/user'), false);
  assert.equal(isIsolatedCrashProbe('/Users/user', '/private/tmp', '/Users/user/userData', '/Users/user/home', '/Users/user/home'), false);
});
