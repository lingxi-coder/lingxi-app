import assert from 'node:assert/strict';
import test from 'node:test';

import { BotLeaveGuard } from '../src/renderer/bridge/botLeaveGuard';

/** A guard, and what happened around it: the edits it was told to give up, and the ways out that were taken. */
function guardWithLog() {
  const log: string[] = [];
  const guard = new BotLeaveGuard(() => log.push('discard'));
  const way = (name: string) => () => { log.push(name); };
  return { guard, log, way };
}

test('with no edits to lose a way out is taken at once, and nothing is asked', () => {
  const { guard, log, way } = guardWithLog();
  guard.leave(way('chat'));
  assert.deepEqual(log, ['chat']);
  assert.equal(guard.getSnapshot().asking, false);

  guard.hold(true);
  guard.hold(false);
  guard.leave(way('teams'));
  assert.deepEqual(log, ['chat', 'teams'], 'edits that were saved, or taken back, are nothing to lose');
});

test('with edits to lose a way out waits for the owner: the question is open and nothing has gone', () => {
  const { guard, log, way } = guardWithLog();
  guard.hold(true);
  guard.leave(way('chat'));
  assert.equal(guard.getSnapshot().asking, true);
  assert.deepEqual(log, [], 'neither the edits were given up nor the way out taken');
});

test('answering no leaves the owner where they are with the edits as they were, and the next way out asks again', () => {
  const { guard, log, way } = guardWithLog();
  guard.hold(true);
  guard.leave(way('chat'));
  guard.cancel();
  assert.equal(guard.getSnapshot().asking, false);
  assert.deepEqual(log, []);

  guard.confirm();
  assert.deepEqual(log, [], 'an answer to a question nobody asked takes nobody anywhere');

  guard.leave(way('teams'));
  assert.equal(guard.getSnapshot().asking, true, 'the page still holds them');
  assert.deepEqual(log, []);
});

test('answering yes gives the edits up first and then takes the way out, once', () => {
  const { guard, log, way } = guardWithLog();
  guard.hold(true);
  guard.leave(way('chat'));
  guard.confirm();
  assert.deepEqual(log, ['discard', 'chat']);
  assert.equal(guard.getSnapshot().asking, false);
  guard.confirm();
  assert.deepEqual(log, ['discard', 'chat'], 'a second yes has nothing left to take');
});

test('where the way out goes is what the owner ends up at, not the list the discarded page returns to', () => {
  let view = 'detail:alpha';
  const guard = new BotLeaveGuard(() => { view = 'list'; });
  guard.hold(true);
  guard.leave(() => { view = 'detail:beta'; });
  assert.equal(view, 'detail:alpha', 'still on the Bot being edited while the question is open');
  guard.confirm();
  assert.equal(view, 'detail:beta');
});

test('a newer way out replaces one that is still waiting, and only the newer is taken', () => {
  const { guard, log, way } = guardWithLog();
  guard.hold(true);
  guard.leave(way('home'));
  guard.leave(way('teams'));
  assert.equal(guard.getSnapshot().asking, true);
  guard.confirm();
  assert.deepEqual(log, ['discard', 'teams']);
});

test('a page that goes away, or whose edits are saved, takes its question with it', () => {
  const { guard, log, way } = guardWithLog();
  guard.hold(true);
  guard.leave(way('chat'));
  guard.hold(false);
  assert.equal(guard.getSnapshot().asking, false);
  guard.confirm();
  assert.deepEqual(log, [], 'the way out that was waiting is not taken behind the owner\'s back');
});

test('the page is told when the question opens and when it closes', () => {
  const { guard, way } = guardWithLog();
  const seen: boolean[] = [];
  guard.subscribe(() => seen.push(guard.getSnapshot().asking));
  guard.hold(true);
  guard.leave(way('chat'));
  guard.leave(way('teams'));
  guard.cancel();
  guard.leave(way('chat'));
  guard.confirm();
  assert.deepEqual(seen, [true, false, true, false], 'a newer way out does not reopen a question that is open');
});
