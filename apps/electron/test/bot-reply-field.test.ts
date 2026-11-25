// Bot v3 B5a: the Auto-reply tab of a Bot: the settings in the form, what saving them sends, and the log of what the Bot decided.
import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import { BOT_REPLY_POLICY_USED_CHARS, validTimeZone, type BotAutoReplyUi, type BotUi, type BotsApi } from '../src/shared/bots';
import { defaultAutoReply, parseAutoReply } from '../src/main/bots/store';
import {
  botChanges, botCreateRequest, botDraftProblems, computerTimeZone, cooldownSeconds, draftFromBot, emptyBotDraft, emptyReplyDraft, hasBotDraftProblems,
  saveBot, sameReply, startingQuietHours, withDeclineThreshold, withQuietHours, withReplyKeyword, withRunThreshold, type BotDraft, type BotReplyDraft,
} from '../src/renderer/bridge/botForm';
import { BotEditorController, botEditorDirty } from '../src/renderer/bridge/botEditor';
import {
  BotTriageLogController, TRIAGE_LOG_PAGE, desktopTriageSources, triageOutcome, triagePercent, type TriageLogSources, type TriageLogState,
} from '../src/renderer/bridge/botTriageLog';
import { Field, problemText } from '../src/renderer/components/BotFields';
import {
  BotAutoReplyField, BotAutoReplyTab, BotReplyQuietHours, BotReplyThresholds, BotTriageLog, formatLogTime, triageActionText, triageOutcomeText, triageReasonText,
} from '../src/renderer/components/BotAutoReplyField';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { TaskState, TriageLogItem, TriageLogPage, TriageLogRequest, TriageTarget } from '../../../packages/teams/execution';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;

const NO_REPLY: BotAutoReplyUi = { policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' };
const SET_REPLY: BotAutoReplyUi = {
  policy: 'Answer questions about billing.', keywords: ['invoice', '账单'], quietHours: { start: '22:00', end: '07:00', timeZone: 'Asia/Tokyo' },
  cooldownSec: 90, thresholds: { decline: 0.3, run: 0.75 }, approval: 'uncertain',
};

function bot(autoReply: BotAutoReplyUi = NO_REPLY): BotUi {
  return {
    botId: 'b1', name: 'Scribe', description: '', createdAt: 1_000, updatedAt: 1_000, revision: 3, workspaces: [],
    sandbox: { enabled: true, allowedDomains: [] }, scope: { localApps: [], datasets: [] }, autoReply,
  };
}

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

const draftOf = (reply: Partial<BotReplyDraft>, saved: BotAutoReplyUi = NO_REPLY): BotDraft => {
  const draft = draftFromBot(bot(saved), '');
  return { ...draft, autoReply: { ...draft.autoReply, ...reply } };
};

// ---- The draft and what saving it sends ----

test('a new Bot starts with what the main process gives a Bot, and is made with no auto-reply setting of its own unless one is given', () => {
  assert.deepEqual(emptyBotDraft().autoReply, { policy: '', keywords: [], cooldown: '0', thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' });
  assert.deepEqual(emptyBotDraft().autoReply.thresholds, defaultAutoReply().thresholds, 'the one number the two share');
  assert.ok(!('autoReply' in botCreateRequest({ ...emptyBotDraft(), name: 'New' })));
  const request = botCreateRequest({ ...emptyBotDraft(), name: 'New', autoReply: { ...emptyReplyDraft(), policy: '  Answer billing questions.  ', cooldown: '30' } });
  assert.deepEqual(request.autoReply, { policy: 'Answer billing questions.', keywords: [], cooldownSec: 30, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' });
});

test('a saved Bot fills the draft with its own copy, and a draft that was not changed is no change', () => {
  const saved = bot(SET_REPLY);
  const draft = draftFromBot(saved, '');
  assert.deepEqual(draft.autoReply, {
    policy: 'Answer questions about billing.', keywords: ['invoice', '账单'], quietHours: { start: '22:00', end: '07:00', timeZone: 'Asia/Tokyo' },
    cooldown: '90', thresholds: { decline: 0.3, run: 0.75 }, approval: 'uncertain',
  });
  draft.autoReply.keywords.push('refund');
  draft.autoReply.quietHours!.start = '23:00';
  draft.autoReply.thresholds.run = 0.9;
  assert.deepEqual(saved.autoReply, SET_REPLY, 'the draft is its own copy');
  assert.deepEqual(botChanges(saved, '', draftFromBot(saved, '')).patch, {});
  assert.deepEqual(botChanges(bot(), '', draftFromBot(bot(), '')).patch, {});
});

test('a change to any part sends the whole setting, with the approval as it is saved; a new Bot is not given more than it was', () => {
  const saved = bot(SET_REPLY);
  const sent = (reply: Partial<BotReplyDraft>) => botChanges(saved, '', draftOf(reply, SET_REPLY)).patch;
  assert.deepEqual(sent({ cooldown: '120' }), { autoReply: { ...SET_REPLY, cooldownSec: 120 } });
  assert.deepEqual(sent({ keywords: ['invoice'] }), { autoReply: { ...SET_REPLY, keywords: ['invoice'] } });
  assert.deepEqual(sent({ thresholds: { decline: 0.1, run: 0.9 } }), { autoReply: { ...SET_REPLY, thresholds: { decline: 0.1, run: 0.9 } } });
  assert.deepEqual(sent({ quietHours: { start: '23:00', end: '07:00', timeZone: 'Asia/Tokyo' } }), { autoReply: { ...SET_REPLY, quietHours: { start: '23:00', end: '07:00', timeZone: 'Asia/Tokyo' } } });
  const off = botChanges(saved, '', { ...draftOf({}, SET_REPLY), autoReply: withQuietHours(draftOf({}, SET_REPLY).autoReply, undefined) }).patch.autoReply!;
  assert.ok(!('quietHours' in off), 'quiet hours turned off are left out, and replace the ones saved');
  assert.equal(off.approval, 'uncertain', 'what is not edited here is carried');
  const on = botChanges(bot(), '', draftOf({ quietHours: startingQuietHours('UTC') })).patch.autoReply!;
  assert.deepEqual(on.quietHours, { start: '22:00', end: '07:00', timeZone: 'UTC' });
});

test('blanks around the policy, and a cooldown written 030 for 30, are no change; a policy is sent without its blanks', () => {
  const saved = bot({ ...SET_REPLY, cooldownSec: 30 });
  assert.deepEqual(botChanges(saved, '', draftOf({ policy: '  Answer questions about billing.\n', cooldown: ' 030 ' }, { ...SET_REPLY, cooldownSec: 30 })).patch, {});
  assert.equal(sameReply(draftFromBot(saved, '').autoReply, draftOf({ cooldown: '31' }, { ...SET_REPLY, cooldownSec: 30 }).autoReply), false);
  assert.equal(botChanges(bot(), '', draftOf({ policy: '   ' })).patch.autoReply, undefined, 'blanks only: still no policy');
  assert.equal(botChanges(bot(), '', draftOf({ policy: '\n Reply to refunds.  ' })).patch.autoReply!.policy, 'Reply to refunds.');
});

test('what the editor sends is what the main process accepts, down to the saved record', () => {
  const drafts: Array<Partial<BotReplyDraft>> = [
    { policy: 'Only billing.' }, { keywords: ['a', 'B', '账单', 'two words'] }, { cooldown: '0' }, { cooldown: '86400' }, { thresholds: { decline: 0, run: 0 } },
    { thresholds: { decline: 1, run: 1 } }, { thresholds: { decline: 0.05, run: 0.95 } }, { quietHours: { start: '00:00', end: '23:59', timeZone: 'Europe/Berlin' } },
    { quietHours: { start: '09:30', end: '09:31', timeZone: computerTimeZone() } },
  ];
  for (const change of drafts) {
    const draft = draftOf({ policy: 'Only billing.', ...change });
    assert.equal(hasBotDraftProblems(botDraftProblems({ ...draft, name: 'Scribe' })), false, JSON.stringify(change));
    const patch = botChanges(bot(), '', draft).patch;
    assert.ok(patch.autoReply, JSON.stringify(change));
    assert.deepEqual(parseAutoReply(JSON.parse(JSON.stringify(patch.autoReply))), patch.autoReply, JSON.stringify(change));
  }
});

test('what the form calls a problem, the main process would refuse', () => {
  const refused = (value: Record<string, unknown>) => assert.throws(() => parseAutoReply({ ...NO_REPLY, ...value }), JSON.stringify(value));
  const problems = (reply: Partial<BotReplyDraft>) => botDraftProblems({ ...draftOf(reply), name: 'Scribe' });
  assert.deepEqual(problems({ policy: 'x'.repeat(4000) }), {});
  assert.deepEqual(problems({ policy: `${'x'.repeat(4000)}   ` }), {}, 'blanks are not counted: they are not sent');
  assert.deepEqual(problems({ policy: 'x'.repeat(4001) }), { replyPolicy: 'reply_policy_too_long' });
  refused({ policy: 'x'.repeat(4001) });
  assert.deepEqual(problems({ keywords: Array.from({ length: 32 }, (_, index) => `word${index}`) }), {});
  assert.deepEqual(problems({ keywords: Array.from({ length: 33 }, (_, index) => `word${index}`) }), { replyKeywords: 'reply_keywords_too_many' });
  refused({ keywords: Array.from({ length: 33 }, (_, index) => `word${index}`) });
  for (const [start, end, timeZone] of [['22:00', '22:00', 'UTC'], ['', '07:00', 'UTC'], ['22:00', '7:00', 'UTC'], ['24:00', '07:00', 'UTC'], ['22:00', '07:00', 'Not/AZone'], ['22:00', '07:00', '']]) {
    assert.deepEqual(problems({ quietHours: { start: start!, end: end!, timeZone: timeZone! } }), { replyQuiet: 'reply_quiet_invalid' }, `${start}-${end} ${timeZone}`);
    refused({ quietHours: { start, end, timeZone } });
  }
  assert.deepEqual(problems({ quietHours: { start: '07:00', end: '22:00', timeZone: 'America/New_York' } }), {});
  for (const text of ['', ' ', 'abc', '-1', '1.5', '1e3', '86401', '1000000', '0x10', '３０']) {
    assert.deepEqual(problems({ cooldown: text }), { replyCooldown: 'reply_cooldown_invalid' }, JSON.stringify(text));
  }
  refused({ cooldownSec: 86401 });
  refused({ cooldownSec: 1.5 });
  for (const text of ['0', '30', ' 30 ', '086400'.slice(1), '00']) assert.deepEqual(problems({ cooldown: text }), {}, JSON.stringify(text));
  assert.deepEqual(problems({ thresholds: { decline: 0.8, run: 0.2 } }), { replyThresholds: 'reply_thresholds_invalid' });
  assert.deepEqual(problems({ thresholds: { decline: Number.NaN, run: 0.5 } }), { replyThresholds: 'reply_thresholds_invalid' });
  assert.deepEqual(problems({ thresholds: { decline: -0.1, run: 0.5 } }), { replyThresholds: 'reply_thresholds_invalid' });
  assert.deepEqual(problems({ thresholds: { decline: 0.5, run: 1.1 } }), { replyThresholds: 'reply_thresholds_invalid' });
  refused({ thresholds: { decline: 0.8, run: 0.2 } });
  assert.deepEqual(problems({ thresholds: { decline: 0.5, run: 0.5 } }), {}, 'equal is allowed: nothing is left in between');
  assert.deepEqual(problems({}), {});
});

test('the cooldown is a whole number of seconds, from none to a day', () => {
  assert.equal(cooldownSeconds('0'), 0);
  assert.equal(cooldownSeconds(' 45 '), 45);
  assert.equal(cooldownSeconds('86400'), 86400);
  for (const text of ['', '-1', '1.5', '1e3', '86401', 'x', '1 2']) assert.equal(cooldownSeconds(text), undefined, JSON.stringify(text));
});

test('keywords are added the way the main process keeps them: one trimmed line, no two that differ only in case, and at most 32', () => {
  assert.deepEqual(withReplyKeyword([], '  Invoice  '), { ok: true, keywords: ['Invoice'] });
  assert.deepEqual(withReplyKeyword(['Invoice'], '账单'), { ok: true, keywords: ['Invoice', '账单'] }, 'in the order they were added');
  assert.deepEqual(withReplyKeyword(['Invoice'], 'INVOICE'), { ok: false, problem: 'duplicate' });
  assert.deepEqual(withReplyKeyword([], '   '), { ok: false, problem: 'invalid' });
  assert.deepEqual(withReplyKeyword([], 'two\nlines'), { ok: false, problem: 'invalid' });
  assert.deepEqual(withReplyKeyword([], 'x'.repeat(65)), { ok: false, problem: 'invalid' });
  assert.equal(withReplyKeyword([], 'x'.repeat(64)).ok, true);
  const full = Array.from({ length: 32 }, (_, index) => `word${index}`);
  assert.deepEqual(withReplyKeyword(full, 'more'), { ok: false, problem: 'too_many' });
  assert.equal(withReplyKeyword(full.slice(1), 'more').ok, true);
  // What was added is what the main process takes.
  const added = withReplyKeyword(['a'], ' b ');
  assert.ok(added.ok);
  assert.doesNotThrow(() => parseAutoReply({ ...NO_REPLY, keywords: added.keywords }));
});

test('the thresholds move on a hundredths scale and never cross: the one to stay silent below is not above the one to answer from', () => {
  const reply = emptyReplyDraft();
  assert.deepEqual(withDeclineThreshold(reply, 0.4).thresholds, { decline: 0.4, run: 0.75 });
  assert.deepEqual(withDeclineThreshold(reply, 0.9).thresholds, { decline: 0.75, run: 0.75 }, 'stopped at the other');
  assert.deepEqual(withDeclineThreshold(reply, -2).thresholds, { decline: 0, run: 0.75 });
  assert.deepEqual(withRunThreshold(reply, 0.2).thresholds, { decline: 0.3, run: 0.3 }, 'stopped at the other');
  assert.deepEqual(withRunThreshold(reply, 5).thresholds, { decline: 0.3, run: 1 });
  assert.deepEqual(withRunThreshold(reply, 0.8500000000000001).thresholds, { decline: 0.3, run: 0.85 }, 'no float dust');
  assert.deepEqual(withDeclineThreshold(withRunThreshold(reply, 0.35), 0.35).thresholds, { decline: 0.35, run: 0.35 });
  assert.equal(reply.thresholds.run, 0.75, 'the settings it was given are not changed');
});

test('quiet hours start as the night in the zone of this computer, and turn off by leaving them out', () => {
  assert.ok(validTimeZone(computerTimeZone()));
  assert.deepEqual(startingQuietHours('Asia/Tokyo'), { start: '22:00', end: '07:00', timeZone: 'Asia/Tokyo' });
  assert.equal(startingQuietHours().timeZone, computerTimeZone());
  const on = withQuietHours(emptyReplyDraft(), startingQuietHours('UTC'));
  assert.deepEqual(on.quietHours, { start: '22:00', end: '07:00', timeZone: 'UTC' });
  assert.ok(!('quietHours' in withQuietHours(on, undefined)));
  assert.ok(!('quietHours' in emptyReplyDraft()));
});

test('saving sends the whole setting with the revision it was read at; a setting with a problem sends nothing', async () => {
  const updates: Array<[string, number, unknown]> = [];
  const api = {
    async update(botId: string, revision: number, patch: unknown) { updates.push([botId, revision, patch]); return { ...bot(SET_REPLY), revision: revision + 1 }; },
    persona: { async set() { /* unused */ } },
  } as unknown as Pick<BotsApi, 'update' | 'persona'>;
  const saved = bot(SET_REPLY);
  const outcome = await saveBot(api, saved, '', draftOf({ cooldown: '45' }, SET_REPLY));
  assert.equal(outcome.ok, true);
  assert.deepEqual(updates, [['b1', 3, { autoReply: { ...SET_REPLY, cooldownSec: 45 } }]]);
  updates.length = 0;
  const refused = await saveBot(api, saved, '', draftOf({ cooldown: 'soon' }, SET_REPLY));
  assert.deepEqual(refused, { ok: false, conflict: false, message: '' });
  assert.deepEqual(updates, []);
});

test('the editor is unsaved while the setting differs from the Bot, cannot be saved with a problem, and is the Bot again once saved', async () => {
  const state = { bot: bot(SET_REPLY), updates: [] as Array<[number, Record<string, unknown>]> };
  const api = {
    async get() { return state.bot; },
    async update(_botId: string, revision: number, patch: Record<string, unknown>) {
      state.updates.push([revision, patch]);
      state.bot = { ...state.bot, ...patch, revision: state.bot.revision + 1 } as BotUi;
      return state.bot;
    },
    persona: { async get() { return ''; }, async set() { /* unused */ } },
  } as unknown as Pick<BotsApi, 'get' | 'update' | 'persona'>;
  const editor = new BotEditorController(api, 'b1', noop);
  await editor.load();
  assert.equal(botEditorDirty(editor.getSnapshot()), false);
  const reply = editor.getSnapshot().draft.autoReply;
  editor.setDraft({ autoReply: { ...reply, cooldown: 'soon' } });
  assert.equal(botEditorDirty(editor.getSnapshot()), true);
  await editor.save();
  assert.deepEqual(state.updates, [], 'a cooldown that is no number is not sent');
  editor.setDraft({ autoReply: withQuietHours({ ...reply, cooldown: '10' }, undefined) });
  await editor.save();
  assert.equal(state.updates.length, 1);
  assert.equal(state.updates[0]![0], 3);
  assert.deepEqual(state.updates[0]![1], { autoReply: { policy: SET_REPLY.policy, keywords: SET_REPLY.keywords, cooldownSec: 10, thresholds: SET_REPLY.thresholds, approval: 'uncertain' } });
  assert.equal(botEditorDirty(editor.getSnapshot()), false);
  assert.equal(editor.getSnapshot().draft.autoReply.cooldown, '10');
  assert.ok(!('quietHours' in editor.getSnapshot().draft.autoReply));
});

// ---- The settings as the tab shows them ----

test('the tab says what each setting is for, and shows the ones the Bot has', () => {
  const html = render(React.createElement(BotAutoReplyField, { draft: draftFromBot(bot(SET_REPLY), ''), problems: {}, onChange: noop }));
  assert.match(html, /In chats where a member turns it on, this Bot answers on its own when it judges a reply is wanted\./);
  assert.match(html, /<label class="bots-label"[^>]*>When should this Bot reply on its own\?<\/label>/);
  assert.match(html, /<textarea[^>]*>Answer questions about billing\.<\/textarea>/);
  assert.match(html, new RegExp(`Only the first ${BOT_REPLY_POLICY_USED_CHARS} characters are used\\.`));
  assert.match(html, /<legend class="bots-label">Keywords<\/legend>/);
  assert.match(html, /A message that contains one of these words is answered without asking the Decision service\./);
  assert.match(html, /<li class="bots-chip"><span>invoice<\/span><button[^>]*aria-label="Remove keyword invoice"/);
  assert.match(html, /<li class="bots-chip"><span>账单<\/span>/);
  assert.match(html, /role="switch"[^>]*aria-checked="true"[^>]*aria-label="Quiet hours"/);
  assert.match(html, /type="time"[^>]*value="22:00"/);
  assert.match(html, /type="time"[^>]*value="07:00"/);
  assert.match(html, /Times are in the Asia\/Tokyo time zone, as set when quiet hours were turned on\./);
  assert.match(html, /Cooldown \(seconds\)/);
  assert.match(html, /class="bots-reply-number"[^>]*value="90"/);
  assert.match(html, /How sure the Decision service must be/);
  assert.match(html, /<span>Stay silent below<\/span><input type="range" min="0" max="100" step="5" aria-valuetext="30%" value="30"\/><output>30%<\/output>/);
  assert.match(html, /<span>Answer from<\/span><input type="range" min="0" max="100" step="5" aria-valuetext="75%" value="75"\/><output>75%<\/output>/);
  assert.match(html, /Below the first value the Bot stays silent; from the second it answers; in between it answers only if it still finds a reply is wanted\./);
  assert.match(html, /A Bot that is @mentioned, replied to or called by name always answers, whatever the quiet hours and cooldown say\./);
  assert.ok(!/assistant/i.test(html), 'a Bot is never called an assistant');
});

test('a Bot with the defaults shows an empty policy, no keywords, the quiet hours off with no time to pick, and no cooldown', () => {
  const html = render(React.createElement(BotAutoReplyField, { draft: draftFromBot(bot(), ''), problems: {}, onChange: noop }));
  assert.match(html, /<textarea[^>]*><\/textarea>/);
  assert.match(html, /No keywords\. The Decision service judges every message\./);
  assert.ok(!html.includes('bots-chip'));
  assert.match(html, /role="switch"[^>]*aria-checked="false"[^>]*aria-label="Quiet hours"/);
  assert.ok(!html.includes('type="time"'));
  assert.ok(!html.includes('time zone'));
  assert.match(html, /class="bots-reply-number"[^>]*value="0"/);
  assert.match(html, /aria-valuetext="30%" value="30"/);
  assert.match(html, /aria-valuetext="75%" value="75"/);
});

test('problems are said next to the part they are in, and saving them is not allowed', () => {
  const draft = draftOf({ policy: 'x'.repeat(4001), keywords: Array.from({ length: 33 }, (_, index) => `w${index}`), quietHours: { start: '10:00', end: '10:00', timeZone: 'UTC' }, cooldown: 'soon', thresholds: { decline: 0.9, run: 0.1 } });
  const problems = botDraftProblems({ ...draft, name: 'Scribe' });
  assert.deepEqual(Object.keys(problems).sort(), ['replyCooldown', 'replyKeywords', 'replyPolicy', 'replyQuiet', 'replyThresholds']);
  const html = render(React.createElement(BotAutoReplyField, { draft, problems, onChange: noop }));
  for (const text of [
    'The description can be at most 4000 characters.', 'At most 32 keywords.', 'Choose a start time and an end time that differ.',
    'Enter a whole number of seconds from 0 to 86400.', 'The value to stay silent below cannot be higher than the value to answer from.',
  ]) assert.ok(html.includes(`role="alert">${text}</p>`) || html.includes(`role="alert" class="bots-problem">${text}</p>`) || html.includes(`${text}</p>`), text);
  assert.equal([...html.matchAll(/role="alert"/g)].length, 5);
  assert.ok(html.includes('aria-invalid="true"'), 'the field with the problem is marked');
  assert.equal(problemText('reply_cooldown_invalid'), 'Enter a whole number of seconds from 0 to 86400.');
});

test('the parts are locked while the Bot is being saved', () => {
  const html = render(React.createElement(BotAutoReplyField, { draft: draftFromBot(bot(SET_REPLY), ''), problems: {}, disabled: true, onChange: noop }));
  assert.match(html, /<textarea[^>]*disabled=""/);
  assert.match(html, /<fieldset class="bots-fields bots-reply-keywords" disabled="">/);
  assert.match(html, /role="switch"[^>]*disabled=""/);
  assert.match(html, /type="time"[^>]*disabled=""/);
  assert.match(html, /class="bots-reply-number"[^>]*disabled=""/);
  assert.match(html, /<fieldset class="bots-fields bots-reply-thresholds" disabled="">/);
});

/** The elements of a tree that is one component deep (a field with no hooks of its own), found without a DOM. */
function find(node: React.ReactNode, wanted: (element: React.ReactElement<Record<string, any>>) => boolean, found: Array<React.ReactElement<Record<string, any>>> = []) {
  if (!React.isValidElement(node)) return found;
  const element = node as React.ReactElement<Record<string, any>>;
  if (wanted(element)) found.push(element);
  React.Children.forEach(element.props.children as React.ReactNode, (child) => { find(child, wanted, found); });
  return found;
}

test('editing the setting changes the draft by the part that was touched, and nothing else', () => {
  const draft = draftFromBot(bot(SET_REPLY), '');
  const changes: Array<Partial<BotDraft>> = [];
  const tree = BotAutoReplyField({ draft, problems: {}, onChange: (change) => changes.push(change) }) as React.ReactElement;
  const input = (label: string) => {
    const field = find(tree, (element) => element.type === Field && element.props.label === label)[0]!;
    return (field.props.children as (control: { id: string }) => React.ReactElement<Record<string, any>>)({ id: 'x' }).props;
  };
  input('When should this Bot reply on its own?').onChange({ target: { value: 'Only refunds.' } });
  input('Cooldown (seconds)').onChange({ target: { value: '15' } });
  assert.deepEqual(changes.map((change) => Object.keys(change)), [['autoReply'], ['autoReply']]);
  assert.deepEqual(changes[0]!.autoReply, { ...draft.autoReply, policy: 'Only refunds.' });
  assert.deepEqual(changes[1]!.autoReply, { ...draft.autoReply, cooldown: '15' });
});

test('quiet hours are turned on in the zone of this computer, their times and zone are what is shown, and turning them off drops them', () => {
  const replaced: BotReplyDraft[] = [];
  const parts = (reply: BotReplyDraft, problems = {}) => BotReplyQuietHours({ reply, problems, disabled: false, replace: (next) => replaced.push(next) }) as React.ReactElement;
  const toggle = (tree: React.ReactElement) => find(tree, (element) => typeof element.type === 'function' && element.props.label === 'Quiet hours' && 'onChange' in element.props)[0]!.props;
  toggle(parts(emptyReplyDraft())).onChange(true);
  assert.deepEqual(replaced[0]!.quietHours, { start: '22:00', end: '07:00', timeZone: computerTimeZone() });
  const on = draftFromBot(bot(SET_REPLY), '').autoReply;
  toggle(parts(on)).onChange(false);
  assert.ok(!('quietHours' in replaced[1]!));
  assert.equal(replaced[1]!.cooldown, '90', 'the rest is kept');
  const times = find(parts(on), (element) => element.type === Field && typeof element.props.label === 'string' && ['From', 'Until'].includes(element.props.label))
    .map((field) => (field.props.children as (control: { id: string }) => React.ReactElement<Record<string, any>>)({ id: 'x' }).props);
  assert.deepEqual(times.map((input) => [input.type, input.value]), [['time', '22:00'], ['time', '07:00']]);
  times[0]!.onChange({ target: { value: '23:15' } });
  times[1]!.onChange({ target: { value: '' } });
  assert.deepEqual(replaced[2]!.quietHours, { start: '23:15', end: '07:00', timeZone: 'Asia/Tokyo' });
  assert.deepEqual(replaced[3]!.quietHours, { start: '22:00', end: '', timeZone: 'Asia/Tokyo' }, 'a cleared time is kept as it is, and is a problem');
  assert.deepEqual(botDraftProblems({ ...draftOf({ quietHours: replaced[3]!.quietHours! }), name: 'Scribe' }), { replyQuiet: 'reply_quiet_invalid' });
});

test('each slider moves its own threshold in steps of five percent, and stops at the other', () => {
  const replaced: BotReplyDraft[] = [];
  const tree = BotReplyThresholds({ reply: emptyReplyDraft(), problems: {}, disabled: false, replace: (next) => replaced.push(next) }) as React.ReactElement;
  // The two sliders are components of their own (without hooks): one level further down is the input of each.
  const sliders = find(tree, (element) => typeof element.type === 'function' && 'percent' in element.props)
    .flatMap((slider) => find((slider.type as (props: unknown) => React.ReactElement)(slider.props), (element) => element.type === 'input' && element.props.type === 'range'))
    .map((element) => element.props);
  assert.equal(sliders.length, 2);
  assert.deepEqual(sliders.map((slider) => [slider.min, slider.max, slider.step, slider.value]), [[0, 100, 5, 30], [0, 100, 5, 75]]);
  sliders[0]!.onChange({ target: { value: '55' } });
  sliders[0]!.onChange({ target: { value: '95' } });
  sliders[1]!.onChange({ target: { value: '20' } });
  sliders[1]!.onChange({ target: { value: '100' } });
  assert.deepEqual(replaced.map((reply) => reply.thresholds), [{ decline: 0.55, run: 0.75 }, { decline: 0.75, run: 0.75 }, { decline: 0.3, run: 0.3 }, { decline: 0.3, run: 1 }]);
  assert.equal(triagePercent(0.55), 55);
});

// ---- The log ----

function item(taskId: string, change: Partial<TriageLogItem> = {}): TriageLogItem {
  return {
    taskId, conversationId: 'chat_trip', messageId: `message_${taskId}`, createdAt: 1_700_000_000_000, state: 'declined', conversationTitle: 'Trip',
    triage: { action: 'decline', source: 'decision', reason: 'below_threshold', probability: 0.12, at: 1_700_000_001_000 }, ...change,
  };
}

const TRIP: TriageTarget = { teamId: 'team_trip', teamName: 'Trip crew', sharedBotId: 'sharedbot_a' };
const WORK: TriageTarget = { teamId: 'team_work', teamName: 'Work', sharedBotId: 'sharedbot_b' };

function sources(pages: Record<string, TriageLogPage[] | Error>, targets: TriageTarget[] = [TRIP, WORK]) {
  const requests: TriageLogRequest[] = [];
  const where: TriageLogSources = {
    async targets() { return targets; },
    async page(request) {
      requests.push(request);
      const queue = pages[request.teamId];
      if (queue instanceof Error) throw queue;
      const page = queue?.shift();
      if (!page) throw new Error('no page left');
      return page;
    },
  };
  return { where, requests };
}

test('the log reads the Teams the Bot is published to, and the first page of each, newest first as the service gives them', async () => {
  const { where, requests } = sources({ team_trip: [{ items: [item('t3'), item('t2')], nextCursor: 't2' }], team_work: [{ items: [] }] });
  const log = new BotTriageLogController(() => where);
  assert.equal(log.getSnapshot().phase, 'idle');
  const loading = log.load();
  assert.equal(log.getSnapshot().phase, 'loading');
  await loading;
  const state = log.getSnapshot();
  assert.equal(state.phase, 'ready');
  assert.deepEqual(state.sections.map((section) => [section.target.teamName, section.items.map((row) => row.taskId), section.nextCursor, section.status]), [['Trip crew', ['t3', 't2'], 't2', 'ready'], ['Work', [], undefined, 'ready']]);
  assert.deepEqual(requests.map((request) => [request.teamId, request.sharedBotId, request.limit, request.cursor]), [['team_trip', 'sharedbot_a', TRIAGE_LOG_PAGE, undefined], ['team_work', 'sharedbot_b', TRIAGE_LOG_PAGE, undefined]]);
  assert.ok(TRIAGE_LOG_PAGE <= 50, 'the service gives at most 50 rows');
});

test('a page is read after the last one with its cursor, and added to the rows without any that are there already', async () => {
  const { where, requests } = sources({
    team_trip: [{ items: [item('t3'), item('t2')], nextCursor: 't2' }, { items: [item('t2'), item('t1')] }],
    team_work: [{ items: [item('w1')] }],
  });
  const log = new BotTriageLogController(() => where);
  await log.load();
  const reading = log.more(TRIP);
  assert.equal(log.getSnapshot().sections[0]!.status, 'more');
  await Promise.all([reading, log.more(TRIP)]);
  assert.deepEqual(requests.filter((request) => request.teamId === 'team_trip').map((request) => request.cursor), [undefined, 't2'], 'one read at a time');
  const [trip, work] = log.getSnapshot().sections;
  assert.deepEqual(trip!.items.map((row) => row.taskId), ['t3', 't2', 't1']);
  assert.equal(trip!.nextCursor, undefined);
  assert.equal(trip!.status, 'ready');
  assert.deepEqual(work!.items.map((row) => row.taskId), ['w1'], 'the other Team is as it was');
  await log.more(TRIP);
  assert.equal(requests.length, 3, 'nothing is asked after the end of the log');
});

test('a Team whose log cannot be read says so and keeps the rows it has; the others are not touched; a failed page is tried again', async () => {
  const { where, requests } = sources({
    team_trip: [{ items: [item('t3')], nextCursor: 't3' }, { items: [item('t2')] }],
    team_work: new Error("Error invoking remote method 'lingxi:teams': MAINTAINER_REQUIRED"),
  });
  const original = where.page;
  let failNext = false;
  where.page = async (request) => { if (failNext && request.cursor !== undefined) { failNext = false; throw new Error('Error invoking remote method \'lingxi:teams\': SERVICE_UNAVAILABLE'); } return original(request); };
  const log = new BotTriageLogController(() => where);
  await log.load();
  const [trip, work] = log.getSnapshot().sections;
  assert.deepEqual([trip!.status, trip!.items.length], ['ready', 1]);
  assert.deepEqual([work!.status, work!.error, work!.items.length], ['failed', 'MAINTAINER_REQUIRED', 0]);
  failNext = true;
  await log.more(TRIP);
  const failed = log.getSnapshot().sections[0]!;
  assert.deepEqual([failed.status, failed.error, failed.items.map((row) => row.taskId), failed.nextCursor], ['failed', 'SERVICE_UNAVAILABLE', ['t3'], 't3']);
  await log.more(TRIP);
  assert.deepEqual(log.getSnapshot().sections[0]!.items.map((row) => row.taskId), ['t3', 't2']);
  assert.equal(log.getSnapshot().sections[0]!.status, 'ready');
  assert.equal(requests.length, 3, 'two first pages and the page after, which the failed attempt did not reach');
});

test('a log that cannot be asked for says why: no account, or the Teams could not be read; reading again starts over', async () => {
  let current: TriageLogSources | undefined;
  const log = new BotTriageLogController(() => current);
  await log.load();
  assert.deepEqual([log.getSnapshot().phase, log.getSnapshot().sections.length], ['unavailable', 0]);
  current = { async targets() { throw new Error('Error invoking remote method \'lingxi:teams\': INVALID_SESSION'); }, async page() { return { items: [] }; } };
  await log.load();
  assert.deepEqual([log.getSnapshot().phase, log.getSnapshot().error], ['failed', 'INVALID_SESSION']);
  current = sources({ team_trip: [{ items: [item('t1')] }] }, [TRIP]).where;
  await log.load();
  assert.deepEqual([log.getSnapshot().phase, log.getSnapshot().error, log.getSnapshot().sections.map((section) => section.items.length)], ['ready', undefined, [1]]);
  current = sources({}, []).where;
  await log.load();
  assert.deepEqual([log.getSnapshot().phase, log.getSnapshot().sections.length], ['ready', 0], 'published nowhere: nothing to read');
});

test('a read that a newer one overtook changes nothing', async () => {
  const gates: Array<() => void> = [];
  const slow: TriageLogSources = {
    async targets() { return [TRIP]; },
    async page(request) { await new Promise<void>((resolve) => gates.push(resolve)); return { items: [item(request.cursor === undefined ? `first${gates.length}` : 'later')] }; },
  };
  const log = new BotTriageLogController(() => slow);
  const first = log.load();
  await new Promise((resolve) => setImmediate(resolve));
  const second = log.load();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(gates.length, 2);
  gates[1]!();
  await second;
  gates[0]!();
  await first;
  assert.deepEqual(log.getSnapshot().sections[0]!.items.map((row) => row.taskId), ['first2'], 'the newer read stands');
});

test('the bridge gives the log only to an account that is signed in, as its own uid, for the Bot that is open', async () => {
  const calls: unknown[][] = [];
  const teams = {
    async triageTargets(...args: unknown[]) { calls.push(['targets', ...args]); return [TRIP]; },
    async triageLog(...args: unknown[]) { calls.push(['log', ...args]); return { items: [] }; },
  };
  assert.equal(desktopTriageSources(undefined, 'uid', 'b1'), undefined);
  assert.equal(desktopTriageSources({ teams: teams as never }, undefined, 'b1'), undefined, 'no account');
  assert.equal(desktopTriageSources({ teams: {} as never }, 'uid', 'b1'), undefined, 'a window without the Teams bridge');
  const where = desktopTriageSources({ teams: teams as never }, 'uid_1', 'bot_1')!;
  assert.deepEqual(await where.targets(), [TRIP]);
  await where.page({ teamId: 'team_trip', sharedBotId: 'sharedbot_a', limit: 20, cursor: 'c' });
  assert.deepEqual(calls, [['targets', 'uid_1', 'bot_1'], ['log', 'uid_1', { teamId: 'team_trip', sharedBotId: 'sharedbot_a', limit: 20, cursor: 'c' }]]);
});

test('where a judged message ended up is one of four words, for every state the service has', () => {
  const states: Record<TaskState, string> = {
    completed: 'replied', declined: 'silent', cancelled: 'silent', queued: 'waiting', running: 'waiting', waiting: 'waiting',
    failed: 'failed', expired: 'failed', interrupted: 'failed', authorization_required: 'failed', revoked: 'failed', needs_reconciliation: 'failed',
  };
  for (const [state, outcome] of Object.entries(states)) assert.equal(triageOutcome(state as TaskState), outcome, state);
  assert.deepEqual(['replied', 'silent', 'failed', 'waiting'].map((outcome) => triageOutcomeText(outcome as never)), ['Replied', 'Stayed silent', 'Failed', 'In progress']);
});

test('every code the service keeps has words, and a code this version does not know has none rather than the code', () => {
  assert.deepEqual((['decline', 'run', 'run_may_decline'] as const).map(triageActionText), ['No reply', 'Reply', 'Left to the Bot']);
  const reasons = ['quiet_hours', 'cooldown', 'keyword', 'addressed', 'below_threshold', 'between_thresholds', 'above_threshold', 'decision_unavailable', 'message_unavailable'] as const;
  const words = reasons.map(triageReasonText);
  assert.ok(words.every((text) => text.length > 0 && !text.includes('_') && !text.startsWith('bots_')), words.join(' | '));
  assert.equal(new Set(words).size, reasons.length, 'each says something else');
  assert.equal(triageReasonText(undefined), '');
  assert.equal(triageReasonText('from_the_future' as never), '');
});

function logState(state: Partial<TriageLogState>): TriageLogState {
  return { phase: 'ready', error: undefined, sections: [], ...state };
}

test('the log tells why there is nothing to show: not read yet, nobody signed in, the Teams could not be read, or the Bot is published nowhere', () => {
  const view = (state: Partial<TriageLogState>) => render(React.createElement(BotTriageLog, { state: logState(state), onRefresh: noop, onMore: noop }));
  assert.match(view({ phase: 'idle' }), /Reading the log…/);
  assert.match(view({ phase: 'loading' }), /Reading the log…/);
  assert.match(view({ phase: 'unavailable' }), /Sign in to read the auto-reply log\./);
  assert.match(view({ phase: 'failed', error: 'INVALID_SESSION' }), /role="alert" class="bots-problem">The log could not be read: INVALID_SESSION <button type="button">Retry<\/button>/);
  const none = view({});
  assert.match(none, /This Bot is not published to a Team yet, so there is no log\./);
  assert.match(none, /<h2>Auto-reply log<\/h2>/);
  assert.match(none, /Only the decision and the reason are kept, never what the model said\./);
  assert.match(view({ phase: 'loading' }), /<button type="button" data-variant="quiet" disabled="">Refresh<\/button>/);
  assert.match(none, /<button type="button" data-variant="quiet">Refresh<\/button>/);
});

test('each row says what was decided and why, how sure the Decision service was, in which chat and when, and where the message ended up', () => {
  const rows = [
    item('t1'),
    item('t2', { conversationTitle: undefined, state: 'completed', triage: { action: 'run', source: 'rule', reason: 'keyword', at: 1_700_000_100_000 } }),
    item('t3', { state: 'completed', triage: { action: 'run_may_decline', source: 'decision', reason: 'between_thresholds', probability: 0.5, at: 1_700_000_200_000 } }),
    item('t4', { state: 'failed', triage: { action: 'run', source: 'decision', reason: 'above_threshold', probability: 0.874, at: 1_700_000_300_000 } }),
    item('t5', { state: 'running', triage: { action: 'decline', source: 'quiet', reason: 'quiet_hours', at: 1_700_000_400_000 } }),
    item('t6', { triage: { action: 'decline', source: 'fallback', at: 1_700_000_500_000 } }),
  ];
  const html = render(React.createElement(BotTriageLog, { state: logState({ sections: [{ target: TRIP, items: rows, nextCursor: undefined, status: 'ready', error: undefined }] }), onRefresh: noop, onMore: noop }));
  assert.match(html, /<h3 class="bots-triage-team">Trip crew<\/h3>/);
  const cells = html.split('<li class="bots-row bots-triage-row">').slice(1);
  assert.equal(cells.length, 6);
  const at = (ms: number) => formatLogTime(ms, 'en');
  assert.match(cells[0]!, /data-action="decline"[^>]*><svg/);
  assert.match(cells[0]!, /No reply<span class="bots-triage-why">A reply seemed unlikely to be wanted<\/span><span class="bots-tag">12% likely<\/span>/);
  assert.match(cells[0]!, new RegExp(`Trip · <time dateTime="2023-11-14T22:13:21.000Z">${at(1_700_000_001_000).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}</time>`));
  assert.match(cells[0]!, /<span class="bots-tag">Stayed silent<\/span>/);
  assert.match(cells[1]!, /data-action="run"/);
  assert.match(cells[1]!, /Reply<span class="bots-triage-why">A keyword matched<\/span><\/span>/);
  assert.ok(!cells[1]!.includes('likely'), 'a rule gives no probability');
  assert.match(cells[1]!, /<span class="bots-row-text">Chat · /, 'a chat with no title is a chat');
  assert.match(cells[1]!, /<span class="bots-tag" data-status="completed">Replied<\/span>/);
  assert.match(cells[2]!, /data-action="run_may_decline"/);
  assert.match(cells[2]!, /Left to the Bot<span class="bots-triage-why">Not sure a reply was wanted<\/span><span class="bots-tag">50% likely<\/span>/);
  assert.match(cells[3]!, /87% likely/);
  assert.match(cells[3]!, /<span class="bots-tag" data-status="failed">Failed<\/span>/);
  assert.match(cells[4]!, /In quiet hours/);
  assert.match(cells[4]!, /<span class="bots-tag" data-status="working">In progress<\/span>/);
  assert.match(cells[5]!, /No reply<\/span>/);
  assert.ok(!cells[5]!.includes('bots-triage-why'), 'a verdict with no reason says none');
  assert.ok(!html.includes('Load more'), 'there is no more');
  assert.ok(!/assistant/i.test(html));
});

test('a Team has its own state: reading, empty, more to read, reading more, and a failure that can be tried again', () => {
  const section = (change: Record<string, unknown>) => ({ target: TRIP, items: [], nextCursor: undefined, status: 'ready' as const, error: undefined, ...change });
  const view = (...sections: ReturnType<typeof section>[]) => render(React.createElement(BotTriageLog, { state: logState({ sections: sections as never }), onRefresh: noop, onMore: noop }));
  assert.match(view(section({ status: 'loading' })), /Reading the log…/);
  assert.match(view(section({})), /Nothing yet\. A line appears here for every message the Bot judges\./);
  const more = view(section({ items: [item('t1')], nextCursor: 't1' }));
  assert.match(more, /<button type="button">Load more<\/button>/);
  const reading = view(section({ items: [item('t1')], nextCursor: 't1', status: 'more' }));
  assert.match(reading, /<button type="button" disabled="" aria-busy="true">Reading the log…<\/button>/);
  const failedMore = view(section({ items: [item('t1')], nextCursor: 't1', status: 'failed', error: 'SERVICE_UNAVAILABLE' }));
  assert.match(failedMore, /role="alert" class="bots-problem">The log could not be read: SERVICE_UNAVAILABLE<\/p>/);
  assert.ok(!failedMore.includes('Retry'), 'the button of the page is the retry');
  assert.match(failedMore, /<button type="button">Load more<\/button>/);
  const failedFirst = view(section({ status: 'failed', error: 'MAINTAINER_REQUIRED' }));
  assert.match(failedFirst, /The log could not be read: MAINTAINER_REQUIRED<button type="button">Retry<\/button>/);
  const two = view(section({ items: [item('t1')] }), section({ target: WORK, items: [item('w1', { conversationTitle: 'Lunch' })] }));
  assert.ok(two.indexOf('Trip crew') < two.indexOf('Work') && two.includes('Lunch'));
});

test('the page keeps the settings and the log together under the one tab, and reads the log without an account bridge as unavailable-or-loading, never as empty', () => {
  const html = render(React.createElement(BotAutoReplyTab, { botId: 'b1', draft: draftFromBot(bot(), ''), problems: {}, onChange: noop }));
  assert.ok(html.indexOf('When should this Bot reply on its own?') < html.indexOf('Auto-reply log'), 'the settings come first');
  assert.match(html, /Reading the log…/);
  assert.ok(!html.includes('is not published to a Team yet'), 'nothing was read, so nothing is said of it');
});

test('the log shows a time as the day and the hour, in the language and the time zone of the owner', () => {
  const at = Date.UTC(2023, 10, 14, 22, 13, 21);
  assert.equal(formatLogTime(at, 'en'), new Intl.DateTimeFormat('en', { dateStyle: 'short', timeStyle: 'short' }).format(at));
  assert.equal(formatLogTime(at, 'zh-Hant'), new Intl.DateTimeFormat('zh-Hant', { dateStyle: 'short', timeStyle: 'short' }).format(at));
});
