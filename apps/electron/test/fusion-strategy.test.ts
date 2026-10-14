import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fusionInvocation, chooseFusionStrategy, pinFusionStrategy } from '../src/shared/fusionStrategy';

const selection = { reference: { scope: 'project' as const, id: 'review' }, revision: 'aabbcc' };

test('only the flag prefix supplies a strategy, never quoted flags in the task prompt', () => {
  assert.equal(fusionInvocation('/fusion explain --strategy user:wrong')?.strategy, undefined);
  assert.deepEqual(fusionInvocation('/fusion --verify "echo --strategy user:wrong" --strategy project:review implement')?.strategy, selection.reference);
  assert.deepEqual(fusionInvocation('/fusion --strategy="project:review" inspect')?.strategy, selection.reference);
  assert.throws(() => fusionInvocation('/fusion --strategy user:first --strategy user:second check'), /more than once/);
});

test('setup, cleanup and publication retries bypass model credential resolution', () => {
  for (const raw of ['/fusion', '/fusion setup', '/fusion clean', '/fusion SETUP', '/fusion ClEaN', '/fusion --retry-publication fu_abc']) assert.equal(fusionInvocation(raw), null);
});

test('credential fence preserves original selection and prompt exactly', () => {
  const raw = '/fusion --fast --strategy="project:review"  compare\n\nthese --strategy words';
  assert.equal(pinFusionStrategy(raw, selection), '/fusion --strategy-revision aabbcc --fast --strategy="project:review"  compare\n\nthese --strategy words');
  assert.equal(pinFusionStrategy('/fusion inspect', selection), '/fusion --strategy-revision aabbcc inspect');
  assert.equal(pinFusionStrategy('/fusion\nAnalyze this\nexactly', selection), '/fusion --strategy-revision aabbcc\nAnalyze this\nexactly');
  assert.equal(pinFusionStrategy('/fusion\uFEFFAnalyze this', selection), '/fusion --strategy-revision aabbcc\uFEFFAnalyze this');
  assert.throws(() => pinFusionStrategy('/fusion --strategy-revision aa inspect', selection), /managed by the application/);
});

test('composer choices use scoped stable identities and retain task text', () => {
  assert.equal(chooseFusionStrategy('/fusion --fast compare\nthese', selection.reference), '/fusion --strategy project:review --fast compare\nthese');
  assert.equal(chooseFusionStrategy('/fusion --strategy user:old task', selection.reference), '/fusion --strategy project:review  task');
});

test('natural-language apostrophes and unmatched prompt quotes are never parsed as flags', () => {
  for (const prompt of ["What's wrong with the API?", "Don't change the behavior", "'partial quote", '"partial quote']) {
    const raw = `/fusion --fast ${prompt}`;
    assert.equal(fusionInvocation(raw)?.strategy, undefined);
    assert.equal(pinFusionStrategy(raw, selection), `/fusion --strategy-revision aabbcc --fast ${prompt}`);
    assert.equal(chooseFusionStrategy(raw, selection.reference), `/fusion --strategy project:review --fast ${prompt}`);
    assert.deepEqual(fusionInvocation(`/fusion --strategy project:review ${prompt}`)?.strategy, selection.reference);
  }
});


test('credential options come only from the leading flags and preserve quoted model lists', () => {
  const invocation = fusionInvocation('/fusion --fast --max-panel=2 --models "openai:gpt, custom:fast" inspect --quality --models unrelated:model');
  assert.equal(invocation?.preset, 'fast');
  assert.equal(invocation?.maxPanel, 2);
  assert.deepEqual(invocation?.models, ['openai:gpt', 'custom:fast']);
  const promptOnly = fusionInvocation('/fusion inspect --fast --max-panel 2 --models unrelated:first,unrelated:second');
  assert.equal(promptOnly?.preset, undefined);
  assert.equal(promptOnly?.maxPanel, undefined);
  assert.equal(promptOnly?.models, undefined);
  assert.deepEqual(fusionInvocation('/fusion --models=fast,boss inspect')?.models, ['fast', 'boss']);
});

test('invalid credential-changing flags fail before preparation', () => {
  for (const raw of ['/fusion --fast --quality inspect', '/fusion --models only-one inspect', '/fusion --models openai:,kimi:model inspect', '/fusion --max-panel 9 inspect', '/fusion --max-panel 2 --max-panel 3 inspect']) {
    assert.throws(() => fusionInvocation(raw));
  }
});
