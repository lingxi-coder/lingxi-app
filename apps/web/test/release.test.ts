import assert from 'node:assert/strict';
import test from 'node:test';
import { releaseArtifacts } from '../src/data/mockData';
import { formatReleaseBadge, getReleaseStatusMeta, pickRecommendedArtifact } from '../src/utils/release';

test('localizes release status without inventing a download action', () => {
  assert.equal(getReleaseStatusMeta('coming-soon', 'zh').label, '即将推出');
  assert.match(formatReleaseBadge(releaseArtifacts[0], 'en'), /Beta/);
});

test('recommends the matching platform, then a safe repository artifact', () => {
  assert.equal(pickRecommendedArtifact(releaseArtifacts, 'android').platform, 'Android');
  assert.ok(pickRecommendedArtifact(releaseArtifacts, 'unknown'));
});

test('the prototype has no falsely advertised stable public release', () => {
  assert.equal(releaseArtifacts.some((artifact) => artifact.status === 'stable'), false);
});
