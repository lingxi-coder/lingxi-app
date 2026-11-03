import assert from 'node:assert/strict';
import test from 'node:test';

import { modeForSettledChat, pageForMode, reconcileSidebarMode, restoredSidebarMode, SidebarModeSync, type AppPage } from '../src/renderer/bridge/sidebarMode';

test('Home goes to the chat view and Teams to its page, from wherever the window is', () => {
  const pages: AppPage[] = ['chat', 'scheduled', 'apps', 'changes', 'bots', 'teams'];
  for (const page of pages) {
    for (const botChat of [true, false]) {
      assert.equal(pageForMode('home', { page, botChat }), 'chat', `${page}/${botChat}`);
      assert.equal(pageForMode('teams', { page, botChat }), 'teams', `${page}/${botChat}`);
    }
  }
});

test('Bots goes to the Bots page, except that a Bot\'s chat on screen stays on screen', () => {
  assert.equal(pageForMode('bots', { page: 'chat', botChat: true }), 'chat');
  assert.equal(pageForMode('bots', { page: 'chat', botChat: false }), 'bots', 'a project chat is not a Bot\'s');
  for (const page of ['scheduled', 'apps', 'changes', 'bots', 'teams'] as const) {
    assert.equal(pageForMode('bots', { page, botChat: true }), 'bots', `a Bot's chat that is not shown does not hold the page: ${page}`);
  }
});

test('the Teams and Bots pages are their modes; the pages Apps, Scheduled and changes leave the mode as it is', () => {
  assert.equal(reconcileSidebarMode('home', 'teams', true), 'teams');
  assert.equal(reconcileSidebarMode('bots', 'teams', true), 'teams');
  assert.equal(reconcileSidebarMode('home', 'bots', true), 'bots');
  assert.equal(reconcileSidebarMode('teams', 'bots', true), 'bots');
  for (const page of ['chat', 'scheduled', 'apps', 'changes'] as const) {
    for (const mode of ['home', 'bots', 'teams'] as const) {
      assert.equal(reconcileSidebarMode(mode, page, true), mode, `${mode} stays on ${page}`);
    }
  }
});

test('where Bots cannot be used they are no mode and no page', () => {
  assert.equal(reconcileSidebarMode('bots', 'chat', false), 'home');
  assert.equal(reconcileSidebarMode('bots', 'apps', false), 'home');
  assert.equal(reconcileSidebarMode('home', 'bots', false), 'home');
  assert.equal(reconcileSidebarMode('teams', 'chat', false), 'teams', 'Teams does not depend on them');
  assert.equal(reconcileSidebarMode('teams', 'teams', false), 'teams');
});

test('the window opens in the mode it was left in, except that the Bots, where they cannot be used, are no mode to return to', () => {
  assert.equal(restoredSidebarMode('bots', true), 'bots');
  assert.equal(restoredSidebarMode('bots', false), 'home');
  assert.equal(restoredSidebarMode('teams', true), 'teams');
  assert.equal(restoredSidebarMode('teams', false), 'teams', 'Teams does not depend on the Bots');
  assert.equal(restoredSidebarMode('home', true), 'home');
  assert.equal(restoredSidebarMode(undefined, true), 'home', 'a settings file from before the modes');
});

test('the saved mode is looked at once, and a window that is still where it opened is taken to the Bots or the Teams', () => {
  const sync = new SidebarModeSync();
  assert.equal(sync.open({ saved: 'bots', page: 'chat', mode: 'home', botsAvailable: true }), 'bots');
  assert.equal(sync.open({ saved: 'bots', page: 'chat', mode: 'home', botsAvailable: true }), undefined, 'once');
  assert.equal(sync.toSave('bots'), undefined, 'the file already holds it');
  assert.equal(sync.toSave('home'), 'home');
  assert.equal(sync.toSave('home'), undefined, 'and a change is written once');
  assert.equal(sync.toSave('teams'), 'teams');

  const teams = new SidebarModeSync();
  assert.equal(teams.open({ saved: 'teams', page: 'chat', mode: 'home', botsAvailable: false }), 'teams', 'even where the Bots cannot run');
  assert.equal(teams.open({ saved: 'teams', page: 'chat', mode: 'home', botsAvailable: false }), undefined, 'once');
  assert.equal(teams.toSave('teams'), undefined, 'the file already holds it');
  assert.equal(teams.toSave('home'), 'home', 'and leaving it is written');
});

test('a window the owner or a link has moved is not taken anywhere, and its saved mode is kept until a mode is picked', () => {
  const moved = new SidebarModeSync();
  assert.equal(moved.open({ saved: 'bots', page: 'apps', mode: 'home', botsAvailable: true }), undefined);
  assert.equal(moved.toSave('home'), undefined, 'staying on Home does not overwrite the Bots');
  assert.equal(moved.toSave('teams'), 'teams');

  const unavailable = new SidebarModeSync();
  assert.equal(unavailable.open({ saved: 'bots', page: 'chat', mode: 'home', botsAvailable: false }), undefined);
  assert.equal(unavailable.toSave('home'), undefined);

  const teams = new SidebarModeSync();
  assert.equal(teams.open({ saved: 'teams', page: 'apps', mode: 'home', botsAvailable: true }), undefined);
  assert.equal(teams.toSave('home'), undefined, 'a window that opened elsewhere leaves the Teams the file holds');

  const onBots = new SidebarModeSync();
  assert.equal(onBots.open({ saved: 'teams', page: 'chat', mode: 'bots', botsAvailable: true }), undefined, 'a Bot\'s chat that a link opened is not left for the Teams');
  assert.equal(onBots.toSave('bots'), undefined, 'and it is not written over the file before the owner picks a mode');
});

test('nothing is written before the saved mode was looked at, and a pick before then is the one saved', () => {
  const early = new SidebarModeSync();
  assert.equal(early.toSave('bots'), undefined, 'a window that has not read the file cannot overwrite it');
  early.picked(undefined);
  assert.equal(early.toSave('bots'), 'bots');
  assert.equal(early.open({ saved: 'bots', page: 'chat', mode: 'bots', botsAvailable: true }), undefined, 'the pick is not undone by the saved mode that arrives after');

  const fresh = new SidebarModeSync();
  assert.equal(fresh.open({ saved: undefined, page: 'chat', mode: 'home', botsAvailable: true }), undefined);
  assert.equal(fresh.toSave('home'), undefined, 'a file with no mode is Home');
  assert.equal(fresh.toSave('bots'), 'bots');
});

test('the sidebar follows the chat on screen between Home and Bots, and only in the chat view', () => {
  assert.equal(modeForSettledChat('home', 'chat', true), 'bots', 'a Bot chat opened from a link or a notification lists its Bot');
  assert.equal(modeForSettledChat('bots', 'chat', false), 'home', 'a project chat opened from the picker lists its project');
  assert.equal(modeForSettledChat('bots', 'chat', true), 'bots');
  assert.equal(modeForSettledChat('home', 'chat', false), 'home');
  assert.equal(modeForSettledChat('teams', 'chat', true), 'teams', 'Teams stays where the owner put it');
  assert.equal(modeForSettledChat('teams', 'chat', false), 'teams');
  assert.equal(modeForSettledChat('home', 'apps', true), 'home', 'another page on screen moves nothing');
});
