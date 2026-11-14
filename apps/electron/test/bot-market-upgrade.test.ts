import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import { botFixture } from './bot-helpers.js';
import {
  prepareBotPackage,
  type PreparedBotPackage,
} from '../src/main/bots/package.js';
import { BotService } from '../src/main/bots/service.js';
import { BotMarketInstaller } from '../src/main/bots/marketInstaller.js';
import {
  BotMarketUpgrader,
  accessChange,
  upgradeState,
} from '../src/main/bots/marketUpgrade.js';
import { MarketApiClient } from '../src/main/local-apps/marketClient.js';
import { sha256 } from '../src/main/local-apps/sourcePackage.js';
import type { BotUpgradePlan } from '../src/shared/botMarket.js';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  BotUpgradeReview,
  upgradeReady,
} from '../src/renderer/components/BotUpgradeReview';

(globalThis as { React?: typeof React }).React = React;

/**
 * An author's Bot published as release1, and a market that serves whatever releases the test has published since. The
 * newest release is the last one published.
 */
async function fixture(t: Parameters<typeof botFixture>[0]) {
  const f = botFixture(t, { ids: true });
  const author = f.store.create({
    name: 'Travel helper',
    persona: 'Use only published travel notes.',
  });
  const authorHome = f.store.homeDir(author.bot_id);
  const authorSkill = (name: string, text: string | null) => {
    const dir = join(authorHome, 'skills', name);
    if (text === null) return rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, 'SKILL.md'), text);
  };
  authorSkill('packing', 'Prepare a packing list.');
  authorSkill('budget', 'Keep a trip budget.');
  /** The Bots whose running Team tasks were told to stop. */
  const stopped: string[] = [];
  const service = new BotService({
    store: f.store,
    platform: 'darwin',
    teamRevoking: (botId) => stopped.push(botId),
    runtimes: {
      newSession: async () => {
        throw new Error('unused');
      },
      openSession: async () => {},
      closeProject: async () => {},
    },
    catalogFor: () => ({
      list: async () => ({ sessions: [] }),
      find: async () => undefined,
      workspaceSettings: async () => ({ environment: {}, configHome: '/unused' }),
      invalidate: () => {},
    }),
  });
  const releases = new Map<string, PreparedBotPackage>();
  const state = { account: 'own-account', downloads: 0, corrupt: false };
  const publish = async (
    id: string,
    version: string,
    skills: string[],
    resources: { id: string; required: boolean }[] = [],
    license = 'LicenseRef-Author-Defined',
  ) => {
    releases.set(
      id,
      await prepareBotPackage(
        f.store,
        {
          botId: author.bot_id,
          expectedRevision: f.store.get(author.bot_id).revision,
          version,
          license,
          licenseText: 'The author grants use of these texts.',
          skills,
          resources,
          examples: ['Make a packing list'],
        },
        join(f.root, 'exports', id),
      ),
    );
  };
  const fetcher = (async (input: URL | RequestInfo) => {
    const path = String(input);
    if (path.includes('/versions?'))
      return Response.json({ data: [{ id: [...releases.keys()].at(-1) }] });
    const id = /\/versions\/([^/]+)/.exec(path)![1]!;
    const p = releases.get(id);
    if (!p) return new Response('missing', { status: 404 });
    if (path.endsWith('/download')) {
      state.downloads++;
      return new Response(state.corrupt ? Buffer.from('corrupt') : p.archive);
    }
    return Response.json({
      data: {
        id,
        appId: 'listing1',
        resourceKind: 'bot',
        schema: 3,
        manifest: p.preview.manifest,
        files: p.inventory,
        dependencies: [],
        releaseDigest: p.preview.digest,
        sha256: sha256(p.archive),
        sizeBytes: p.archive.length,
        downloadPath: `/api/v3/bots/listing1/versions/${id}/download`,
      },
    });
  }) as typeof fetch;
  const api = new MarketApiClient(
    async () => 'own-token',
    'https://market.test/api/v3/',
    fetcher,
  );
  await publish('release1', '1.0.0', ['packing', 'budget']);
  const installed = (await new BotMarketInstaller({
    store: f.store,
    createBot: (input, origin, prepare) =>
      service.createFromMarket(input, origin, prepare),
    api,
    accountId: () => state.account,
    assertAvailable: () => {},
    confirm: async () => true,
    changed: () => {},
  }).install('listing1'))!;
  const upgrader = new BotMarketUpgrader({
    store: f.store,
    api,
    accountId: () => state.account,
    assertAvailable: () => {},
    upgradeBot: (botId, revision, release) =>
      service.upgradeFromMarket(botId, revision, release),
  });
  const home = f.store.homeDir(installed.bot_id);
  const role = () => readFileSync(join(home, 'LINGXI.md'), 'utf8');
  const skill = (name: string) => {
    const path = join(home, 'skills', name, 'SKILL.md');
    return existsSync(path) ? readFileSync(path, 'utf8') : null;
  };
  const apply = (
    plan: BotUpgradePlan,
    choices: Record<string, 'mine' | 'theirs'> = {},
    approveAccess = false,
  ) =>
    upgrader.apply({
      botId: plan.botId,
      expectedRevision: plan.expectedRevision,
      releaseId: plan.to.releaseId,
      plan: plan.digest,
      choices,
      approveAccess,
    });
  const states = (plan: BotUpgradePlan) =>
    Object.fromEntries(plan.files.map((file) => [file.key, file.state]));
  return {
    ...f,
    author,
    authorHome,
    authorSkill,
    service,
    stopped,
    publish,
    installed,
    upgrader,
    home,
    role,
    skill,
    apply,
    states,
    state,
  };
}

test('a file follows the release only where the owner left it as installed', () => {
  assert.equal(upgradeState('a', 'a', 'a'), 'none');
  assert.equal(upgradeState('a', 'mine', 'a'), 'kept');
  assert.equal(upgradeState('a', undefined, 'a'), 'kept');
  assert.equal(upgradeState('a', 'a', 'b'), 'update');
  assert.equal(upgradeState(undefined, undefined, 'b'), 'add');
  assert.equal(upgradeState('a', 'a', undefined), 'remove');
  // The owner already has what the release brings, or already dropped what it drops.
  assert.equal(upgradeState('a', 'b', 'b'), 'same');
  assert.equal(upgradeState('a', undefined, undefined), 'same');
  // Both changed it, differently: changed and changed, changed and removed, removed and changed, both added.
  assert.equal(upgradeState('a', 'mine', 'b'), 'conflict');
  assert.equal(upgradeState('a', 'mine', undefined), 'conflict');
  assert.equal(upgradeState('a', undefined, 'b'), 'conflict');
  assert.equal(upgradeState(undefined, 'mine', 'b'), 'conflict');
});

test('an upgrade takes what the release changed, keeps what the owner changed, and touches nothing else of the Bot', async (t) => {
  const f = await fixture(t);
  assert.equal(await f.upgrader.check(f.installed.bot_id), null);

  // The owner's own work on the installed Bot: a changed Skill, a Skill of their own, memory, a model, a name.
  writeFileSync(join(f.home, 'skills', 'budget', 'SKILL.md'), 'Budget in euros, my way.');
  mkdirSync(join(f.home, 'skills', 'mine'), { recursive: true });
  writeFileSync(join(f.home, 'skills', 'mine', 'SKILL.md'), 'A Skill I wrote.');
  mkdirSync(join(f.home, 'memory'), { recursive: true });
  writeFileSync(join(f.home, 'memory', 'trip.md'), 'We leave on Friday.');
  const renamed = f.service.update(f.installed.bot_id, f.installed.revision, {
    name: 'Our trips',
    model: 'my-own-model',
  });

  // The author's next release: a new role, a changed Skill, a new Skill; `budget` is as before.
  writeFileSync(join(f.authorHome, 'LINGXI.md'), 'Use published travel notes and say which.');
  f.authorSkill('packing', 'Prepare a packing list by weather.');
  f.authorSkill('visa', 'Check visa rules.');
  await f.publish('release2', '1.1.0', ['packing', 'budget', 'visa']);

  const plan = (await f.upgrader.check(f.installed.bot_id))!;
  assert.deepEqual(plan.from, { releaseId: 'release1', version: '1.0.0' });
  assert.deepEqual(plan.to, { releaseId: 'release2', version: '1.1.0', name: 'Travel helper' });
  assert.deepEqual(f.states(plan), {
    role: 'update',
    'skill:budget': 'kept',
    'skill:packing': 'update',
    'skill:visa': 'add',
  });
  assert.equal(plan.expands, false);
  assert.equal(plan.license, undefined);
  // Looking changes nothing.
  assert.equal(f.role(), 'Use only published travel notes.');
  assert.equal(f.store.get(f.installed.bot_id).marketOrigin!.releaseId, 'release1');

  f.stopped.length = 0;
  const upgraded = await f.apply(plan);
  // Tasks a Team is running with this Bot are not stopped: they run what the Bot was shared as, to their end.
  assert.deepEqual(f.stopped, []);
  assert.equal(upgraded.bot_id, f.installed.bot_id);
  assert.equal(upgraded.revision, renamed.revision + 1);
  assert.equal(upgraded.marketOrigin!.releaseId, 'release2');
  assert.notEqual(upgraded.marketOrigin!.digest, f.installed.marketOrigin!.digest);
  assert.equal(upgraded.marketOrigin!.installIntent, f.installed.marketOrigin!.installIntent);
  assert.equal(f.role(), 'Use published travel notes and say which.');
  assert.equal(f.skill('packing'), 'Prepare a packing list by weather.');
  assert.equal(f.skill('visa'), 'Check visa rules.');
  // The owner's: their changed Skill, their own Skill, memory, model and name.
  assert.equal(f.skill('budget'), 'Budget in euros, my way.');
  assert.equal(f.skill('mine'), 'A Skill I wrote.');
  assert.equal(readFileSync(join(f.home, 'memory', 'trip.md'), 'utf8'), 'We leave on Friday.');
  assert.equal(upgraded.name, 'Our trips');
  assert.equal(upgraded.model, 'my-own-model');
  // The description was still the one the install wrote, so it names the new version.
  assert.match(upgraded.description, /^Installed Bot 1\.1\.0\./);
  assert.equal(
    JSON.parse(readFileSync(join(f.store.botDir(upgraded.bot_id), 'market-package.json'), 'utf8')).version,
    '1.1.0',
  );
  assert.equal(await f.upgrader.check(f.installed.bot_id), null);
  // Still one installed Bot: an upgrade is never another copy.
  assert.equal(f.store.list().filter((b) => b.marketOrigin).length, 1);
});

test('a file both changed stays as it is until the owner chooses; declining leaves the Bot on its release', async (t) => {
  const f = await fixture(t);
  writeFileSync(join(f.home, 'LINGXI.md'), 'My own role.');
  writeFileSync(join(f.home, 'skills', 'packing', 'SKILL.md'), 'Pack light, always.');
  rmSync(join(f.home, 'skills', 'budget'), { recursive: true });
  writeFileSync(join(f.authorHome, 'LINGXI.md'), 'The author rewrote the role.');
  f.authorSkill('packing', 'Prepare a packing list by weather.');
  f.authorSkill('budget', 'Keep a trip budget per person.');
  await f.publish('release2', '1.1.0', ['packing', 'budget']);

  const plan = (await f.upgrader.check(f.installed.bot_id))!;
  assert.deepEqual(f.states(plan), {
    role: 'conflict',
    'skill:budget': 'conflict',
    'skill:packing': 'conflict',
  });
  const budget = plan.files.find((file) => file.key === 'skill:budget')!;
  assert.equal(budget.mine, null);
  assert.equal(budget.theirs, 'Keep a trip budget per person.');
  assert.equal(plan.files.find((file) => file.key === 'role')!.mine, 'My own role.');

  // Without a choice for each, nothing is written and the Bot is on release1 as before.
  await assert.rejects(f.apply(plan), /BOT_UPGRADE_CHOICE_REQUIRED/);
  await assert.rejects(f.apply(plan, { role: 'mine', 'skill:packing': 'theirs' }), /BOT_UPGRADE_CHOICE_REQUIRED/);
  assert.equal(f.role(), 'My own role.');
  assert.equal(f.skill('packing'), 'Pack light, always.');
  assert.equal(f.skill('budget'), null);
  const still = f.store.get(f.installed.bot_id);
  assert.equal(still.revision, f.installed.revision);
  assert.equal(still.marketOrigin!.releaseId, 'release1');

  const upgraded = await f.apply(plan, { role: 'mine', 'skill:packing': 'theirs', 'skill:budget': 'mine' });
  assert.equal(upgraded.marketOrigin!.releaseId, 'release2');
  assert.equal(f.role(), 'My own role.');
  assert.equal(f.skill('packing'), 'Prepare a packing list by weather.');
  assert.equal(f.skill('budget'), null);

  // What the owner kept is their change to the new release from now on: the next release that leaves it alone keeps it.
  f.authorSkill('visa', 'Check visa rules.');
  await f.publish('release3', '1.2.0', ['packing', 'budget', 'visa']);
  const later = (await f.upgrader.check(f.installed.bot_id))!;
  assert.deepEqual(f.states(later), { role: 'kept', 'skill:budget': 'kept', 'skill:visa': 'add' });
});

test('a release that reads more is not taken without the owner agreeing; one that reads less needs no agreement', async (t) => {
  const f = await fixture(t);
  writeFileSync(join(f.authorHome, 'LINGXI.md'), 'Use the travel notes resource.');
  await f.publish('release2', '2.0.0', ['packing', 'budget'], [{ id: 'travel-notes', required: true }], 'MIT');
  const plan = (await f.upgrader.check(f.installed.bot_id))!;
  assert.equal(plan.expands, true);
  assert.deepEqual(plan.tools, { added: ['shared_resource.search'], removed: [] });
  assert.deepEqual(plan.resources, { added: ['travel-notes'], removed: [], nowRequired: [] });
  assert.deepEqual(plan.license, { from: 'LicenseRef-Author-Defined', to: 'MIT' });

  await assert.rejects(f.apply(plan), /BOT_UPGRADE_APPROVAL_REQUIRED/);
  assert.equal(f.role(), 'Use only published travel notes.');
  assert.equal(f.store.get(f.installed.bot_id).marketOrigin!.releaseId, 'release1');
  assert.deepEqual(
    JSON.parse(readFileSync(join(f.store.botDir(f.installed.bot_id), 'market-package.json'), 'utf8')).resources,
    [],
  );

  const upgraded = await f.apply(plan, {}, true);
  assert.equal(upgraded.marketOrigin!.releaseId, 'release2');
  assert.equal(f.role(), 'Use the travel notes resource.');

  // The next release gives the resource up, and one it keeps becomes required: neither widens what the Bot reads.
  await f.publish('release3', '2.1.0', ['packing', 'budget'], [], 'MIT');
  const narrower = (await f.upgrader.check(f.installed.bot_id))!;
  assert.equal(narrower.expands, false);
  assert.deepEqual(narrower.tools, { added: [], removed: ['shared_resource.search'] });
  assert.deepEqual(narrower.resources, { added: [], removed: ['travel-notes'], nowRequired: [] });
  await f.apply(narrower);
  const optional = { resources: [{ id: 'notes', binding: 'installer', required: false }], capabilities: [{ tool: 'shared_resource.search', operations: ['read'] }] };
  const required = { ...optional, resources: [{ id: 'notes', binding: 'installer', required: true }] };
  assert.deepEqual(accessChange(optional as never, required as never), {
    tools: { added: [], removed: [] },
    resources: { added: [], removed: [], nowRequired: ['notes'] },
    expands: false,
  });
});

test('a plan that no longer holds is refused, and so is another account, a tampered download or a Bot that was not installed', async (t) => {
  const f = await fixture(t);
  writeFileSync(join(f.authorHome, 'LINGXI.md'), 'A new role.');
  await f.publish('release2', '1.1.0', ['packing', 'budget']);
  const plan = (await f.upgrader.check(f.installed.bot_id))!;

  // The owner edited a file after looking: what they agreed to is not what would happen now.
  writeFileSync(join(f.home, 'LINGXI.md'), 'Edited after the review.');
  await assert.rejects(f.apply(plan), /BOT_UPGRADE_CHANGED/);
  assert.equal(f.role(), 'Edited after the review.');
  writeFileSync(join(f.home, 'LINGXI.md'), 'Use only published travel notes.');

  // The Bot's record changed after looking.
  const edited = f.service.update(f.installed.bot_id, f.installed.revision, { name: 'Renamed' });
  await assert.rejects(f.apply(plan), /BOT_UPGRADE_CHANGED/);
  await assert.rejects(f.apply({ ...plan, expectedRevision: edited.revision }), /BOT_UPGRADE_CHANGED/);

  // A download that is not the release's bytes changes nothing.
  f.state.corrupt = true;
  await assert.rejects(f.upgrader.check(f.installed.bot_id), /checksum mismatch/);
  f.state.corrupt = false;

  // Another account cannot move the Bot, and neither can nobody.
  f.state.account = 'other-account';
  await assert.rejects(f.upgrader.check(f.installed.bot_id), /account that installed this Bot/);
  f.state.account = '';
  await assert.rejects(f.upgrader.check(f.installed.bot_id), /Sign in/);
  f.state.account = 'own-account';
  await assert.rejects(f.upgrader.check(f.author.bot_id), /not installed from the market/);

  assert.equal(f.role(), 'Use only published travel notes.');
  assert.equal(f.store.get(f.installed.bot_id).marketOrigin!.releaseId, 'release1');
  // A fresh look is carried out.
  const fresh = (await f.upgrader.check(f.installed.bot_id))!;
  assert.equal(fresh.expectedRevision, edited.revision);
  const upgraded = await f.apply(fresh);
  assert.equal(f.role(), 'A new role.');
  // The owner renamed the Bot, so the name is theirs.
  assert.equal(upgraded.name, 'Renamed');
});

test('a Skill the release drops goes only where the owner left it as installed; an interrupted upgrade finishes on the next try', async (t) => {
  const f = await fixture(t);
  writeFileSync(join(f.home, 'skills', 'budget', 'notes.md'), 'My notes next to the Skill.');
  writeFileSync(join(f.authorHome, 'LINGXI.md'), 'A new role.');
  await f.publish('release2', '1.1.0', []);
  const plan = (await f.upgrader.check(f.installed.bot_id))!;
  assert.deepEqual(f.states(plan), { role: 'update', 'skill:budget': 'remove', 'skill:packing': 'remove' });

  // The files were written and the record was not (the app quit in between).
  const interrupted = new BotMarketUpgrader({
    store: f.store,
    api: (f.upgrader as unknown as { options: { api: MarketApiClient } }).options.api,
    accountId: () => 'own-account',
    assertAvailable: () => {},
    upgradeBot: () => {
      throw new Error('quit');
    },
  });
  await assert.rejects(
    interrupted.apply({ botId: plan.botId, expectedRevision: plan.expectedRevision, releaseId: 'release2', plan: plan.digest, choices: {}, approveAccess: false }),
    /quit/,
  );
  assert.equal(f.role(), 'A new role.');
  assert.equal(f.store.get(f.installed.bot_id).marketOrigin!.releaseId, 'release1');

  const again = (await f.upgrader.check(f.installed.bot_id))!;
  assert.deepEqual(again.files, []);
  const upgraded = await f.apply(again);
  assert.equal(upgraded.marketOrigin!.releaseId, 'release2');
  assert.equal(f.skill('packing'), null);
  assert.equal(existsSync(join(f.home, 'skills', 'packing')), false);
  // The owner's file beside the dropped Skill is still there.
  assert.equal(f.skill('budget'), null);
  assert.equal(readFileSync(join(f.home, 'skills', 'budget', 'notes.md'), 'utf8'), 'My notes next to the Skill.');
});

test('the review says what happens to each file, and cannot be carried out before every choice and agreement is given', () => {
  const plan: BotUpgradePlan = {
    kind: 'upgrade',
    botId: 'bot',
    expectedRevision: 3,
    from: { releaseId: 'release1', version: '1.0.0' },
    to: { releaseId: 'release2', version: '2.0.0', name: 'Travel helper' },
    digest: 'a'.repeat(64),
    files: [
      { key: 'role', kind: 'role', name: 'Travel helper', state: 'conflict', mine: 'My role <b>', theirs: 'Their role' },
      { key: 'skill:budget', kind: 'skill', name: 'budget', state: 'kept' },
      { key: 'skill:visa', kind: 'skill', name: 'visa', state: 'add' },
      { key: 'skill:old', kind: 'skill', name: 'old', state: 'conflict', mine: 'Mine', theirs: null },
    ],
    license: { from: 'MIT', to: 'Apache-2.0' },
    tools: { added: ['shared_resource.search'], removed: [] },
    resources: { added: ['travel-notes'], removed: [], nowRequired: [] },
    expands: true,
  };
  const html = renderToStaticMarkup(
    React.createElement(BotUpgradeReview, { plan, busy: false, onUpgrade: () => {}, onDecline: () => {} }),
  );
  assert.match(html, /Version 2\.0\.0 is available \(this Bot is on 1\.0\.0\)/);
  assert.match(html, /Role: you and the new release both changed it/);
  assert.match(html, /My role &lt;b&gt;/);
  assert.match(html, /Skill “budget”: you changed it and the release did not\. Yours stays/);
  assert.match(html, /Skill “visa”: added by the new release/);
  assert.match(html, /\(not in this version\)/);
  assert.match(html, /License changes from MIT to Apache-2\.0/);
  assert.match(html, /shared_resource\.search, travel-notes/);
  assert.match(html, /Stay on 1\.0\.0/);
  // Nothing is chosen yet: the update button is off.
  assert.match(html, /<button type="button" disabled="">Update to 2\.0\.0<\/button>/);

  assert.equal(upgradeReady(plan, {}, true), false);
  assert.equal(upgradeReady(plan, { role: 'mine' }, true), false);
  assert.equal(upgradeReady(plan, { role: 'mine', 'skill:old': 'theirs' }, false), false);
  assert.equal(upgradeReady(plan, { role: 'mine', 'skill:old': 'theirs' }, true), true);
  assert.equal(upgradeReady({ ...plan, expands: false, files: [] }, {}, false), true);
});
