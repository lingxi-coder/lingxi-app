import assert from 'node:assert/strict';
import test from 'node:test';
import { GROUP_WINDOW_MS, buildStreamRows, dayLabel, placeTasks, firstUnreadMessage, formatClock, formatListTime, type StreamOptions, type StreamRow } from '../../../packages/teams/stream';
import type { TeamMessage } from '../../../packages/teams/messages';
import { message } from './support/fakeTeams';

const now = Date.UTC(2026, 10, 2, 12, 0, 0); // Monday 2 November 2026, noon UTC
const options = (more: Partial<StreamOptions> = {}): StreamOptions => ({ lastReadSeq: 99, uid: 'alice', locale: 'en-US', now, timeZone: 'UTC', ...more });
const when = (day: number, hour: number, minute: number, second = 0) => new Date(Date.UTC(2026, 10, day, hour, minute, second)).toISOString();
const msg = (seq: number, createdAt: string | null, more: Partial<TeamMessage> = {}) => message(seq, { createdAt, ...more });
const shape = (rows: StreamRow[]) => rows.map(row => row.type === 'message' ? `${row.message.seq}${row.grouped ? '+' : ''}` : row.type === 'date' ? `[${row.label}]` : '<new>').join(' ');

test('a run of messages from one author within five minutes groups; the first of a run does not', () => {
  const rows = buildStreamRows([msg(1, when(2, 10, 0)), msg(2, when(2, 10, 1)), msg(3, when(2, 10, 3, 30))], options());
  assert.equal(shape(rows), '[Today] 1 2+ 3+');
});

test('the five-minute window is inclusive and measured from the previous message, not the first of the run', () => {
  assert.equal(GROUP_WINDOW_MS, 300_000);
  assert.equal(shape(buildStreamRows([msg(1, when(2, 10, 0)), msg(2, when(2, 10, 5, 0))], options())), '[Today] 1 2+');
  assert.equal(shape(buildStreamRows([msg(1, when(2, 10, 0)), msg(2, when(2, 10, 5, 1))], options())), '[Today] 1 2');
  assert.equal(shape(buildStreamRows([msg(1, when(2, 10, 0)), msg(2, when(2, 10, 4)), msg(3, when(2, 10, 8)), msg(4, when(2, 10, 14))], options())), '[Today] 1 2+ 3+ 4');
});

test('a different author, author kind or Bot breaks the run', () => {
  const bot = (sharedBotId: string, displayName = 'Helper') => ({ author: { kind: 'shared_bot' as const, sharedBotId, displayName } });
  const rows = buildStreamRows([
    msg(1, when(2, 10, 0)), msg(2, when(2, 10, 1), { author: { kind: 'user', uid: 'carol' } }), msg(3, when(2, 10, 2), { author: { kind: 'user', uid: 'carol' } }),
    msg(4, when(2, 10, 3), bot('bot:owner:h:a')), msg(5, when(2, 10, 3), bot('bot:owner:h:a')), msg(6, when(2, 10, 3), bot('assistant-fixture')),
    msg(7, when(2, 10, 3), { author: { kind: 'user', uid: 'owner' } }),
  ], options());
  assert.equal(shape(rows), '[Today] 1 2 3+ 4 5+ 6 7');
});

test('a new day starts a run and gets a date divider, even a minute apart', () => {
  const rows = buildStreamRows([msg(1, when(1, 23, 59)), msg(2, when(2, 0, 0, 30)), msg(3, when(2, 0, 1))], options());
  assert.equal(shape(rows), '[Yesterday] 1 [Today] 2 3+');
  assert.deepEqual(rows.filter(row => row.type === 'date').map(row => row.key), ['date:2026-11-01', 'date:2026-11-02']);
});

test('days are the viewer\'s days: the same instants split differently in another time zone', () => {
  const list = [msg(1, when(2, 15, 58)), msg(2, when(2, 16, 1))]; // 23:58 and 00:01 the next day in Shanghai
  assert.equal(shape(buildStreamRows(list, options())), '[Today] 1 2+');
  const shanghai = buildStreamRows(list, options({ timeZone: 'Asia/Shanghai', locale: 'zh-CN' }));
  assert.deepEqual(shanghai.map(row => row.type), ['date', 'message', 'date', 'message']);
  assert.equal(shanghai[0]!.type === 'date' && shanghai[0]!.label, '今天');
  assert.match(shanghai[2]!.type === 'date' ? shanghai[2]!.label : '', /11月3日/);
  assert.equal(shanghai[3]!.type === 'message' && shanghai[3]!.grouped, false);
});

test('the unread divider goes before the first unread message from someone else, after any date divider, and breaks the run', () => {
  const list = [msg(1, when(2, 9, 0)), msg(2, when(2, 10, 0)), msg(3, when(2, 10, 1)), msg(4, when(2, 10, 2))];
  assert.equal(shape(buildStreamRows(list, options({ lastReadSeq: 2 }))), '[Today] 1 2 <new> 3 4+');
  assert.equal(shape(buildStreamRows(list, options({ lastReadSeq: 0 }))), '[Today] <new> 1 2 3+ 4+');
  assert.equal(shape(buildStreamRows(list, options({ lastReadSeq: 4 }))), '[Today] 1 2 3+ 4+', 'everything read: no divider');
  assert.equal(shape(buildStreamRows(list, options({ lastReadSeq: 3 }))), '[Today] 1 2 3+ <new> 4');
  const across = buildStreamRows([msg(1, when(1, 23, 0)), msg(2, when(2, 8, 0))], options({ lastReadSeq: 1 }));
  assert.equal(shape(across), '[Yesterday] 1 [Today] <new> 2');
});

test('the viewer\'s own unread messages never anchor the divider; later messages from others do', () => {
  const mine = (seq: number, createdAt: string) => msg(seq, createdAt, { author: { kind: 'user', uid: 'alice' } });
  const onlyMine = [msg(1, when(2, 10, 0)), mine(2, when(2, 10, 1)), mine(3, when(2, 10, 2))];
  assert.equal(shape(buildStreamRows(onlyMine, options({ lastReadSeq: 1 }))), '[Today] 1 2 3+');
  assert.equal(buildStreamRows(onlyMine, options({ lastReadSeq: 1 })).some(row => row.type === 'unread'), false);
  const mixed = [msg(1, when(2, 10, 0)), mine(2, when(2, 10, 1)), msg(3, when(2, 10, 2)), msg(4, when(2, 10, 3))];
  assert.equal(shape(buildStreamRows(mixed, options({ lastReadSeq: 1 }))), '[Today] 1 2 <new> 3 4+');
  assert.equal(firstUnreadMessage(mixed, 1, 'alice')?.id, 'message-3');
  assert.equal(firstUnreadMessage(mixed, 1, 'bob')?.id, 'message-2', 'someone else\'s view');
  assert.equal(firstUnreadMessage(mixed, 4, 'alice'), undefined);
  const bot = msg(5, when(2, 10, 4), { author: { kind: 'shared_bot', displayName: 'Helper', sharedBotId: 'assistant-alice' } });
  assert.equal(firstUnreadMessage([bot], 4, 'alice')?.id, bot.id, 'a Bot of the viewer is still someone else');
});

test('a recalled message does not begin a run for the next one, but can continue one', () => {
  const rows = buildStreamRows([msg(1, when(2, 10, 0)), msg(2, when(2, 10, 1), { recalled: true, text: '' }), msg(3, when(2, 10, 2))], options());
  assert.equal(shape(rows), '[Today] 1 2+ 3');
});

test('messages without a time never start a day and are never grouped', () => {
  const rows = buildStreamRows([msg(1, null), msg(2, null), msg(3, when(2, 10, 0)), msg(4, null), msg(5, when(2, 10, 1))], options());
  assert.equal(shape(rows), '1 2 [Today] 3 4 5');
  assert.equal(shape(buildStreamRows([msg(1, 'not a date'), msg(2, when(2, 10, 0))], options())), '1 [Today] 2');
});

test('an empty list has no rows and keys are unique and stable', () => {
  assert.deepEqual(buildStreamRows([], options()), []);
  const list = [msg(1, when(1, 10, 0)), msg(2, when(2, 10, 0)), msg(3, when(2, 10, 1))];
  const first = buildStreamRows(list, options({ lastReadSeq: 1 })).map(row => row.key);
  assert.equal(new Set(first).size, first.length);
  assert.deepEqual(buildStreamRows(list, options({ lastReadSeq: 1 })).map(row => row.key), first);
  assert.ok(first.includes('message:message-2'));
});

test('buildStreamRows reuses the very message objects it was given', () => {
  const list = [msg(1, when(2, 10, 0))];
  const row = buildStreamRows(list, options()).find(value => value.type === 'message');
  assert.equal(row?.type === 'message' && row.message, list[0]);
});

test('day labels: Today, Yesterday, then the weekday and date, with the year only when it differs', () => {
  const label = (ms: number, locale = 'en-US', timeZone = 'UTC') => dayLabel(ms, now, locale, timeZone);
  assert.equal(label(Date.UTC(2026, 10, 2, 0, 0, 1)), 'Today');
  assert.equal(label(Date.UTC(2026, 10, 1, 23, 59, 59)), 'Yesterday');
  assert.match(label(Date.UTC(2026, 9, 30, 12)), /^Friday, October 30$/);
  assert.match(label(Date.UTC(2025, 11, 25, 12)), /December 25, 2025/);
  assert.equal(label(Date.UTC(2026, 10, 2, 12), 'zh-CN'), '今天');
  assert.equal(label(Date.UTC(2026, 10, 1, 12), 'zh-CN'), '昨天');
  assert.match(label(Date.UTC(2026, 9, 30, 12), 'zh-CN'), /10月30日/);
  assert.equal(dayLabel(Date.UTC(2026, 2, 1, 12), Date.UTC(2026, 2, 2, 12), 'en', 'UTC'), 'Yesterday', 'month boundaries');
  assert.equal(dayLabel(Date.UTC(2025, 11, 31, 12), Date.UTC(2026, 0, 1, 12), 'en', 'UTC'), 'Yesterday', 'year boundaries');
});

test('clock and list times', () => {
  assert.match(formatClock(when(2, 9, 5), 'en-US', 'UTC'), /^9:05\s?AM$/);
  assert.equal(formatClock(when(2, 21, 5), 'zh-CN', 'UTC'), '21:05');
  assert.equal(formatClock(null, 'en-US'), ''); assert.equal(formatClock('garbage', 'en-US'), ''); assert.equal(formatClock(undefined, 'en-US'), '');
  const list = (value: string | null | undefined, locale = 'en-US') => formatListTime(value, now, locale, 'UTC');
  assert.match(list(when(2, 9, 5)), /9:05/);
  assert.equal(list(when(1, 22, 0)), 'Yesterday'); assert.equal(list(when(1, 22, 0), 'zh-CN'), '昨天');
  assert.equal(list('2026-10-30T10:00:00.000Z'), 'Fri', 'within the last week: the weekday');
  assert.equal(list('2026-10-27T10:00:00.000Z'), 'Tue', 'six days ago is still unambiguous');
  assert.equal(list('2026-10-30T10:00:00.000Z', 'zh-CN'), '周五');
  assert.equal(list('2026-10-26T12:00:00.000Z'), 'Oct 26', 'seven days ago would repeat today\'s weekday: the date');
  assert.equal(list('2026-09-03T12:00:00.000Z'), 'Sep 3', 'older: the date');
  assert.equal(list('2025-10-27T12:00:00.000Z'), 'Oct 27, 2025', 'another year');
  assert.equal(list(null), ''); assert.equal(list(undefined), ''); assert.equal(list('nope'), '');
});

test('placeTasks puts a task after the last message not newer than it, and keeps active tasks reachable above the window', () => {
  const list = [msg(1, when(2, 10, 0)), msg(2, null), msg(3, when(2, 10, 1)), msg(4, when(6, 9, 0))];
  const task = (id: string, state: 'expired' | 'running', createdAt: number) => ({ id, state, createdAt });
  const at = (day: number, hour: number, minute: number) => Date.UTC(2026, 10, day, hour, minute);
  const ids = (tasks: { id: string }[] | undefined) => tasks?.map(t => t.id);
  const placed = placeTasks(list, [task('a', 'expired', at(2, 10, 1)), task('b', 'expired', at(2, 10, 1)), task('c', 'expired', at(2, 10, 30)), task('d', 'running', at(7, 0, 0))], false);
  assert.deepEqual(ids(placed.after.get('message-3')), ['a', 'b', 'c'], 'same time as a message: after it, and before the next newer one; untimed messages are skipped');
  assert.deepEqual(ids(placed.after.get('message-4')), ['d']);
  assert.deepEqual([placed.head, placed.tail, placed.after.get('message-1')], [[], [], undefined]);
  const older = [task('old', 'expired', at(1, 0, 0)), task('live', 'running', at(1, 0, 0))];
  const top = placeTasks(list, older, false);
  assert.deepEqual([ids(top.head), top.tail], [['old', 'live'], []], 'nothing hidden above: older tasks go first');
  const windowed = placeTasks(list, older, true);
  assert.deepEqual([windowed.head, ids(windowed.tail)], [[], ['live']], 'earlier messages hidden: a finished task waits for them, an active one is pinned at the end');
});
