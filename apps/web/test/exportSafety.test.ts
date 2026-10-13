import assert from 'node:assert/strict';
import { mkdtemp, mkdir, symlink, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { preflightGeneratedPaths, validateGeneratedPath } from '../scripts/export-files';
import { validateDocBaseline, docsBaseline, resolveDocSources } from '../src/data/docs/sources';
import { docPages } from '../src/data/docs';

test('source provenance rejects equal-count substitutions and obsolete pages without SDK checkouts', () => {
  const mutated = structuredClone(docPages);
  mutated[0].sources![0].path = 'different-source.rs';
  assert.throws(() => validateDocBaseline(mutated), /sources.*stale/u);
  assert.deepEqual(resolveDocSources(mutated[0]), []);
  const baseline = structuredClone(docsBaseline);
  baseline.pages.obsolete = [];
  assert.throws(() => validateDocBaseline(docPages, baseline), /page set is stale/u);
});

test('export preflight rejects traversal and symlink redirects before any mutation', async () => {
  for (const bad of ['../../README.md', '/tmp/outside.md', 'zh/../en/page.md', 'zh\\page.md', '']) assert.throws(() => validateGeneratedPath(bad));
  const root = await mkdtemp(path.join(tmpdir(), 'lingxi-export-guard-'));
  try {
    const output = path.join(root, 'technical');
    await mkdir(output);
    await preflightGeneratedPaths(output, ['zh/overview.md', 'assets/diagram.svg'], root);
    const external = path.join(root, 'unowned'); await mkdir(external);
    await writeFile(path.join(external, 'protected.md'), 'preserve');
    await symlink(external, path.join(output, 'zh'));
    await assert.rejects(preflightGeneratedPaths(output, ['zh/protected.md'], root), /symbolic links/u);
    await assert.rejects(preflightGeneratedPaths(output, ['../../README.md'], root), /Invalid generated path/u);
  } finally { await rm(root, { recursive: true, force: true }); }
});
