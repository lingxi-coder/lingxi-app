import { test } from 'node:test';
import assert from 'node:assert/strict';
import { scheduledTaskCron, scheduledTaskInput, scheduledTaskFromJob, scheduledTaskAdvice, localScheduledTimezone, type ScheduledTaskDraft } from '../src/renderer/bridge/scheduledTaskDraft';

const draft: ScheduledTaskDraft = { title: 'Weekly summary', instructions: 'Report completed changes and blockers.',
  frequency: 'Weekly', day: 'Monday', time: '09:30', timezone: localScheduledTimezone(), recurring: true, durable: true };

test('weekly, weekdays and daily controls generate five-field local cron', () => {
  assert.equal(scheduledTaskCron(draft), '30 9 * * 1');
  assert.equal(scheduledTaskCron({ ...draft, day: 'Sunday' }), '30 9 * * 0');
  assert.equal(scheduledTaskCron({ ...draft, frequency: 'Daily' }), '30 9 * * *');
  assert.equal(scheduledTaskCron({ ...draft, frequency: 'Weekdays' }), '30 9 * * 1-5');
  assert.throws(() => scheduledTaskCron({ ...draft, time: '24:00' }));
  assert.throws(() => scheduledTaskCron({ ...draft, day: 'Never' }));
  assert.throws(() => scheduledTaskInput({ ...draft, timezone: 'Invalid/Zone' }));
});

test('saved title and full multiline instructions round-trip through existing cron prompt', () => {
  const input = scheduledTaskInput({ ...draft, title: ' Edited title ', instructions: 'First paragraph.\n\nSecond paragraph.' });
  const restored = scheduledTaskFromJob({ id: 'job', ...input });
  assert.equal(restored.title, 'Edited title');
  assert.equal(restored.instructions, 'First paragraph.\n\nSecond paragraph.');
  assert.equal(restored.frequency, 'Weekly');
  assert.deepEqual(scheduledTaskInput(restored), input);
  assert.throws(() => scheduledTaskInput({ ...draft, instructions: '  ' }));
});

test('editing external cron jobs preserves advanced schedules and one-shot behavior', () => {
  const restored = scheduledTaskFromJob({ id: 'external', cron: '*/15 8-18 * * 1-5', prompt: 'Do not discard\nthis body.', recurring: false, durable: true });
  assert.equal(restored.frequency, 'Custom');
  assert.equal(restored.instructions, 'Do not discard\nthis body.');
  assert.equal(scheduledTaskInput(restored).cron, '*/15 8-18 * * 1-5');
  assert.equal(scheduledTaskInput(restored).recurring, false);
  assert.equal(scheduledTaskInput({ ...restored, frequency: 'Daily', time: '10:45' }).cron, '45 10 * * *');
});

test('tasks default to no expiry and custom expiry round-trips through persistence', () => {
  assert.equal(scheduledTaskInput(draft).no_expiry, true);
  const input = scheduledTaskInput({ ...draft, expiresAt: '2099-10-10T10:30' });
  assert.equal(input.expires_at, new Date('2099-10-10T10:30').getTime());
  const restored = scheduledTaskFromJob({ id: 'bounded', ...input });
  assert.equal(restored.expiresAt, '2099-10-10T10:30');
  assert.equal(scheduledTaskInput({ ...restored, expiresAt: undefined }).no_expiry, true);
  assert.throws(() => scheduledTaskInput({ ...draft, expiresAt: '2000-01-01T00:00' }));
});

test('frequency advice is optional and distinguishes high-frequency monitoring from ongoing work', () => {
  const frequent = { ...draft, frequency: 'Custom' as const, cron: '*/5 * * * *' };
  assert.equal(scheduledTaskAdvice(frequent).suggestedDays, 1);
  assert.equal(scheduledTaskAdvice(frequent).suggestSlower, true);
  assert.equal(scheduledTaskInput(frequent).no_expiry, true, 'advice must never impose an expiry');
  assert.equal(scheduledTaskAdvice({ ...frequent, cron: '0 * * * *' }).suggestedDays, 7);
  assert.equal(scheduledTaskAdvice(draft).suggestedDays, undefined);
  assert.match(scheduledTaskAdvice(draft).text, /No expiry/);
  assert.equal(scheduledTaskAdvice({ ...frequent, recurring: false }).suggestedDays, undefined);
});

test('editing unrelated fields preserves sub-minute expiry precision', () => {
  const expires_at = new Date('2099-10-10T10:30:59.789').getTime();
  const restored = scheduledTaskFromJob({ id: 'precise', cron: '0 9 * * *', prompt: '# Task\n\nInstructions', recurring: true, expires_at });
  assert.equal(scheduledTaskInput({ ...restored, title: 'Renamed' }).expires_at, expires_at);
  assert.equal(scheduledTaskInput({ ...restored, expiresAt: '2099-10-11T11:00' }).expires_at, new Date('2099-10-11T11:00').getTime());
});
