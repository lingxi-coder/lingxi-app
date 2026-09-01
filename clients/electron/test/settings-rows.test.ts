import { test } from 'node:test';
import assert from 'node:assert/strict';

import { provenanceLabel } from '../src/renderer/components/settings/rows';

test('every provenance value has a label', () => {
  for (const d of ['device', 'user', 'project', 'local', 'managed'] as const) {
    const label = provenanceLabel(d);
    assert.ok(label && label.length > 0, `${d} must have a badge label`);
  }
});

test('managed is labelled as policy-locked, not as an editable layer', () => {
  assert.notEqual(
    provenanceLabel('managed'), provenanceLabel('user'),
    'a policy-locked value must not read like a user-editable one',
  );
});
