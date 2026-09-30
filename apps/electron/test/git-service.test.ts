import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, mkdir, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { GitService } from '../src/main/git';
async function fixture(t: {
    after: (fn: () => Promise<void>) => void;
}, busy?: () => boolean | Promise<boolean>) { const root = await mkdtemp(path.join(tmpdir(), 'lingxi-git-unit-')); const service = new GitService({ isBusy: busy }); const scope = { projectPath: root, sessionId: 'test' }; t.after(async () => { service.dispose(); await rm(root, { recursive: true, force: true }); }); const run = (...args: string[]) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim(); run('init', '-b', 'main'); run('config', 'user.name', 'Test'); run('config', 'user.email', 'test@example.invalid'); run('config', 'commit.gpgsign', 'false'); const token = async () => (await service.request(scope, { kind: 'status' })).status!.token; return { root, service, scope, run, token }; }
test('initial repository, staging and unstaging preserves working data', async (t) => { const f = await fixture(t); await writeFile(path.join(f.root, '初始.txt'), 'hello\n'); assert.equal((await f.service.request(f.scope, { kind: 'status' })).status!.head, ''); await f.service.request(f.scope, { kind: 'stage', token: await f.token() }); assert.equal(f.run('diff', '--cached', '--name-only'), '"\\345\\210\\235\\345\\247\\213.txt"'); await f.service.request(f.scope, { kind: 'unstage', token: await f.token() }); assert.equal(f.run('diff', '--cached', '--name-only'), ''); });
test('agent busy blocks working tree mutations but permits safe staging', async (t) => { const f = await fixture(t, async () => true); await writeFile(path.join(f.root, 'a'), 'one'); await f.service.request(f.scope, { kind: 'stage', token: await f.token() }); await assert.rejects(f.service.request(f.scope, { kind: 'checkout', create: true, branch: 'next', token: await f.token() }), /agent/); });
test('path traversal and symlink parent escapes are rejected', async (t) => { const f = await fixture(t); await mkdir(path.join(f.root, 'nested')); await symlink(tmpdir(), path.join(f.root, 'escape')); for (const p of ['../outside', '.git/config', 'escape/outside'])
    await assert.rejects(f.service.request(f.scope, { kind: 'stage', paths: [p], token: await f.token() }), /Invalid|escapes/); });
test('hook rejection preserves index and reports error', async (t) => { const f = await fixture(t); await writeFile(path.join(f.root, 'a'), 'one'); await f.service.request(f.scope, { kind: 'stage', token: await f.token() }); await writeFile(path.join(f.root, '.git/hooks/pre-commit'), '#!/bin/sh\necho deliberate-rejection >&2\nexit 1\n', { mode: 0o755 }); await assert.rejects(f.service.request(f.scope, { kind: 'commit', message: 'test', token: await f.token() }), /deliberate-rejection/); assert.equal(f.run('diff', '--cached', '--name-only'), 'a'); });
test('watch emits invalidations and close releases watchers', async (t) => {
  const f = await fixture(t);
  const stop = await f.service.watch(f.scope);
  const event = new Promise<void>(resolve => { const off = f.service.onChanged(e => { assert.ok(e.root.endsWith(path.basename(f.root))); off(); resolve(); }); });
  let settled = false;
  event.then(() => { settled = true; });
  // Write REPEATEDLY, not once. `fs.watch(dir, { recursive: true })` is FSEvents
  // on macOS and returns before its stream delivers, so a single write placed
  // immediately after it can be dropped outright — and a dropped event never
  // arrives however long you wait. That is what the old single-write version was
  // hitting: both observed failures burned the FULL budget (15107ms, 15148ms)
  // rather than landing early, which is not the shape of a merely busy machine.
  // The interval must stay ABOVE the service's 180ms debounce; poking faster
  // than that keeps clearing the timer and guarantees no event at all.
  const poke = (async () => {
    for (let n = 0; !settled && n < 30; n++) {
      await writeFile(path.join(f.root, 'watch.txt'), `test ${n}`);
      await new Promise(resolve => setTimeout(resolve, 400));
    }
  })();
  try {
    await Promise.race([event, new Promise((_, reject) => setTimeout(() => reject(new Error('watch timeout')), 15_000))]);
  } finally {
    settled = true;
    await poke;
    stop();
  }
});
test('same-commit branch switch invalidates snapshot', async t => {
 const f=await fixture(t);await writeFile(path.join(f.root,'a'),'one');f.run('add','.');f.run('commit','-m','initial');f.run('branch','next');const old=await f.token();f.run('switch','next');await assert.rejects(f.service.request(f.scope,{kind:'commit',message:'wrong branch',token:old}),/changed/);
});
test('untracked complete hunk staging and stash preview preserve content', async t => {
 const f=await fixture(t);await writeFile(path.join(f.root,'base'),'one');f.run('add','.');f.run('commit','-m','initial');await writeFile(path.join(f.root,'new.txt'),'new\n');const preview=(await f.service.request(f.scope,{kind:'diff',mode:'working',path:'new.txt'})).diff!;await f.service.request(f.scope,{kind:'stage',patch:preview.patch,token:preview.token});assert.equal(f.run('show',':new.txt'),'new');f.run('reset');await f.service.request(f.scope,{kind:'stashCreate',message:'with new',includeUntracked:true,token:await f.token()});const stash=(await f.service.request(f.scope,{kind:'stashList'})).stashes![0];const diff=(await f.service.request(f.scope,{kind:'diff',mode:'stash',target:stash.id})).diff!;assert.ok(diff.patch.includes('+new'));assert.ok(diff.files.some(file=>file.path==='new.txt'));
});
test('dispose cancels queued writes and rejects new requests and watches', async t => {
 let block=false;let release!:()=>void;let entered!:()=>void;
 const held=new Promise<void>(resolve=>{release=resolve;});const ready=new Promise<void>(resolve=>{entered=resolve;});
 const f=await fixture(t,async()=>{if(block){entered();await held;}return false;});
 await writeFile(path.join(f.root,'queued.txt'),'must remain untracked');const token=await f.token();block=true;
 const first=f.service.request(f.scope,{kind:'stage',paths:['queued.txt'],token});
 const firstRejected=assert.rejects(first,/closed/);
 await ready;
 const second=f.service.request(f.scope,{kind:'stage',paths:['queued.txt'],token});const secondRejected=assert.rejects(second,/closed/);
 f.service.dispose();release();await Promise.all([firstRejected,secondRejected]);
 assert.equal(f.run('diff','--cached','--name-only'),'');await assert.rejects(f.service.request(f.scope,{kind:'status'}),/closed/);await assert.rejects(f.service.watch(f.scope),/closed/);
});
test('unstaging a renamed row or all rows restores both index paths', async t => {
 const f=await fixture(t);await writeFile(path.join(f.root,'old.txt'),'same\n');f.run('add','.');f.run('commit','-m','initial');
 for(const all of [false,true]){
  f.run('mv','old.txt','new.txt');const status=(await f.service.request(f.scope,{kind:'status'})).status!;assert.equal(status.files[0].oldPath,'old.txt');
  await f.service.request(f.scope,{kind:'unstage',...(all?{}:{paths:['new.txt']}),token:status.token});assert.equal(f.run('diff','--cached','--name-only'),'');
  f.run('restore','old.txt');await rm(path.join(f.root,'new.txt'));
 }
});
test('external replacement merge is not owned by the application', async t => {
 const f=await fixture(t);await writeFile(path.join(f.root,'a'),'base\n');f.run('add','.');f.run('commit','-m','base');
 for(const branch of ['first','second']) {f.run('switch','-c',branch,'main');await writeFile(path.join(f.root,'a'),branch+'\n');f.run('commit','-am',branch);}
 f.run('switch','main');await writeFile(path.join(f.root,'a'),'main\n');f.run('commit','-am','main');
 await assert.rejects(f.service.request(f.scope,{kind:'merge',branch:'first',token:await f.token()}));f.run('merge','--abort');assert.throws(()=>f.run('merge','--no-edit','second'));
 const status=(await f.service.request(f.scope,{kind:'status'})).status!;assert.equal(status.merging,true);assert.equal(status.mergeOwned,false);await assert.rejects(f.service.request(f.scope,{kind:'mergeAbort',token:status.token}),/not started/);
});
