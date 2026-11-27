import { useEffect, useRef, useState } from 'react';
import { BOT_REPLY_POLICY_USED_CHARS, BOT_REPLY_KEYWORD_CHARS_MAX, type BotReplyApprovalUi } from '../../shared/bots';
import type { ApprovalOutcome, TaskDelegation, TaskRound, TriageAction, TriageReason } from '../../../../../packages/teams/execution';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { desktopHost } from '../bridge/bridgeConnection';
import { useAccountState } from '../bridge/useAccountState';
import {
  startingQuietHours, withDeclineThreshold, withQuietHours, withReplyKeyword, withRunThreshold,
  type BotDraft, type BotDraftProblems, type BotKeywordAdd, type BotReplyDraft,
} from '../bridge/botForm';
import {
  BotTriageLogController, delegatedBy, desktopTriageSources, readTaskDelegation, triageOutcome, triagePercent,
  type TriageLogSection, type TriageLogState, type TriageOutcome,
} from '../bridge/botTriageLog';
import { Field, problemText, useObservable } from './BotFields';
import { Icon } from './Icon';
import { Toggle } from './settings/primitives';

interface ReplyFieldProps {
  draft: BotDraft;
  problems: BotDraftProblems;
  disabled?: boolean;
  onChange(change: Partial<BotDraft>): void;
}

/**
 * The Auto-reply tab as the page shows it: when the Bot answers on its own, which is saved with the rest of the Bot, and below it the log of
 * what the Bot decided, which is read each time the tab is opened and is the Bot's maintainer's alone.
 */
export function BotAutoReplyTab({ botId, ...props }: ReplyFieldProps & { botId: string }) {
  const { state, readError } = useAccountState();
  const uid = state?.user?.uid;
  const uidRef = useRef(uid);
  uidRef.current = uid;
  const [log] = useState(() => new BotTriageLogController(() => desktopTriageSources(desktopHost(), uidRef.current, botId)));
  const logState = useObservable(log);
  // Whether there is an account is not known until the account has answered: asking before would say that nobody is signed in for a moment.
  const settled = desktopHost()?.account === undefined || state !== null || readError !== '';
  useEffect(() => { if (settled) void log.load(); }, [log, settled, uid]);
  return <>
    <BotAutoReplyField {...props} />
    <BotTriageLog state={logState} onRefresh={() => void log.load()} onMore={(section) => void log.more(section.target)} />
  </>;
}

export function BotAutoReplyField({ draft, problems, disabled = false, onChange }: ReplyFieldProps) {
  const reply = draft.autoReply;
  const replace = (next: BotReplyDraft) => onChange({ autoReply: next });
  return <div className="bots-fields bots-reply">
    <p className="bots-hint">{tr('bots_reply_intro')}</p>
    <Field label={tr('bots_reply_policy_title')} hint={tr('bots_reply_policy_hint', BOT_REPLY_POLICY_USED_CHARS)} problem={problems.replyPolicy ? problemText(problems.replyPolicy) : undefined}>
      {(control) => <textarea {...control} rows={5} value={reply.policy} disabled={disabled} onChange={(event) => replace({ ...reply, policy: event.target.value })} />}
    </Field>
    <BotReplyKeywords reply={reply} problems={problems} disabled={disabled} replace={replace} />
    <BotReplyQuietHours reply={reply} problems={problems} disabled={disabled} replace={replace} />
    <Field label={tr('bots_reply_cooldown_title')} hint={tr('bots_reply_cooldown_hint')} problem={problems.replyCooldown ? problemText(problems.replyCooldown) : undefined}>
      {(control) => <input {...control} className="bots-reply-number" inputMode="numeric" autoComplete="off" value={reply.cooldown} disabled={disabled}
        onChange={(event) => replace({ ...reply, cooldown: event.target.value })} />}
    </Field>
    <BotReplyThresholds reply={reply} problems={problems} disabled={disabled} replace={replace} />
    <BotReplyApproval reply={reply} disabled={disabled} replace={replace} />
    <p className="bots-hint">{tr('bots_reply_mention_note')}</p>
  </div>;
}

export interface PartProps {
  reply: BotReplyDraft;
  problems: BotDraftProblems;
  disabled: boolean;
  replace(next: BotReplyDraft): void;
}

function keywordProblemText(problem: Exclude<BotKeywordAdd, { ok: true }>['problem']): string {
  switch (problem) {
    case 'invalid': return tr('bots_reply_keyword_invalid', BOT_REPLY_KEYWORD_CHARS_MAX);
    case 'duplicate': return tr('bots_reply_keyword_duplicate');
    case 'too_many': return problemText('reply_keywords_too_many');
  }
}

/** The words that make the Bot answer: the ones there are, as chips that can be taken away, and one more to add. */
export function BotReplyKeywords({ reply, problems, disabled, replace }: PartProps) {
  const [text, setText] = useState('');
  const [problem, setProblem] = useState<Exclude<BotKeywordAdd, { ok: true }>['problem']>();
  const { keywords } = reply;
  const add = () => {
    if (text.trim() === '') return;
    const added = withReplyKeyword(keywords, text);
    if (!added.ok) { setProblem(added.problem); return; }
    setProblem(undefined);
    setText('');
    replace({ ...reply, keywords: added.keywords });
  };
  return <fieldset className="bots-fields bots-reply-keywords" disabled={disabled}>
    <legend className="bots-label">{tr('bots_reply_keywords_title')}</legend>
    <p className="bots-hint">{tr('bots_reply_keywords_hint')}</p>
    {keywords.length > 0 ? <ul className="bots-chips">
      {keywords.map((keyword) => <li key={keyword} className="bots-chip">
        <span>{keyword}</span>
        <button type="button" data-variant="quiet" aria-label={tr('bots_reply_keyword_remove', keyword)} onClick={() => replace({ ...reply, keywords: keywords.filter((known) => known !== keyword) })}><Icon name="x" size={12} /></button>
      </li>)}
    </ul> : <p className="bots-empty-line">{tr('bots_reply_keywords_empty')}</p>}
    <div className="bots-reply-add">
      <input value={text} placeholder={tr('bots_reply_keyword_placeholder')} aria-label={tr('bots_reply_keyword_placeholder')} autoComplete="off" spellCheck={false}
        aria-invalid={problem ? true : undefined}
        onChange={(event) => { setText(event.target.value); setProblem(undefined); }}
        // Enter that confirms a word being composed (Chinese, Japanese, Korean) is not the owner adding it.
        onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); add(); } }} />
      <button type="button" disabled={text.trim() === ''} onClick={add}>{tr('bots_reply_keyword_add')}</button>
    </div>
    {problem ? <p className="bots-problem" role="alert">{keywordProblemText(problem)}</p> : null}
    {problems.replyKeywords ? <p className="bots-problem" role="alert">{problemText(problems.replyKeywords)}</p> : null}
  </fieldset>;
}

/** The hours in which the Bot does not answer on its own: off, or between two times of day in the time zone they were set in. */
export function BotReplyQuietHours({ reply, problems, disabled, replace }: PartProps) {
  const quiet = reply.quietHours;
  return <div className="bots-fields bots-reply-quiet">
    <ul className="bots-rows">
      <li className="bots-row">
        <div className="bots-row-main">
          <span className="bots-row-title">{tr('bots_reply_quiet_title')}</span>
          <span className="bots-row-text">{tr('bots_reply_quiet_hint')}</span>
        </div>
        <Toggle value={quiet !== undefined} disabled={disabled} label={tr('bots_reply_quiet_title')}
          onChange={(on) => replace(withQuietHours(reply, on ? startingQuietHours() : undefined))} />
      </li>
    </ul>
    {quiet ? <>
      <div className="bots-reply-times">
        <Field label={tr('bots_reply_quiet_from')}>
          {(control) => <input {...control} type="time" value={quiet.start} disabled={disabled} onChange={(event) => replace(withQuietHours(reply, { ...quiet, start: event.target.value }))} />}
        </Field>
        <Field label={tr('bots_reply_quiet_until')}>
          {(control) => <input {...control} type="time" value={quiet.end} disabled={disabled} onChange={(event) => replace(withQuietHours(reply, { ...quiet, end: event.target.value }))} />}
        </Field>
      </div>
      <p className="bots-hint">{tr('bots_reply_quiet_zone', quiet.timeZone)}</p>
    </> : null}
    {problems.replyQuiet ? <p className="bots-problem" role="alert">{problemText(problems.replyQuiet)}</p> : null}
  </div>;
}

/** How sure the Decision service must be: the value below which the Bot stays silent and the one from which it answers, the first never above the second. */
export function BotReplyThresholds({ reply, problems, disabled, replace }: PartProps) {
  return <fieldset className="bots-fields bots-reply-thresholds" disabled={disabled}>
    <legend className="bots-label">{tr('bots_reply_thresholds_title')}</legend>
    <p className="bots-hint">{tr('bots_reply_thresholds_hint')}</p>
    <ThresholdSlider label={tr('bots_reply_decline_label')} percent={triagePercent(reply.thresholds.decline)} onChange={(percent) => replace(withDeclineThreshold(reply, percent / 100))} />
    <ThresholdSlider label={tr('bots_reply_run_label')} percent={triagePercent(reply.thresholds.run)} onChange={(percent) => replace(withRunThreshold(reply, percent / 100))} />
    {problems.replyThresholds ? <p className="bots-problem" role="alert">{problemText(problems.replyThresholds)}</p> : null}
  </fieldset>;
}

const APPROVALS: readonly BotReplyApprovalUi[] = ['never', 'uncertain', 'always'];
/** What each choice of asking the maintainer first is called, and what it means. */
export function approvalText(approval: BotReplyApprovalUi): { label: string; hint: string } {
  switch (approval) {
    case 'never': return { label: tr('bots_reply_approval_never'), hint: tr('bots_reply_approval_never_hint') };
    case 'uncertain': return { label: tr('bots_reply_approval_uncertain'), hint: tr('bots_reply_approval_uncertain_hint') };
    case 'always': return { label: tr('bots_reply_approval_always'), hint: tr('bots_reply_approval_always_hint') };
  }
}

/** Whether the owner reads the Bot's reply before it is posted: never, only when the Bot was unsure it should answer, or every time. The reply is approved in the Team's chat. */
export function BotReplyApproval({ reply, disabled, replace }: Pick<PartProps, 'reply' | 'disabled' | 'replace'>) {
  return <div className="bots-fields bots-reply-approval">
    <Field label={tr('bots_reply_approval_title')} hint={approvalText(reply.approval).hint}>
      {(control) => <select {...control} value={reply.approval} disabled={disabled}
        onChange={(event) => { const approval = APPROVALS.find((known) => known === event.target.value); if (approval) replace({ ...reply, approval }); }}>
        {APPROVALS.map((approval) => <option key={approval} value={approval}>{approvalText(approval).label}</option>)}
      </select>}
    </Field>
    <p className="bots-hint">{tr('bots_reply_approval_note')}</p>
  </div>;
}

function ThresholdSlider({ label, percent, onChange }: { label: string; percent: number; onChange(percent: number): void }) {
  return <label className="bots-slider">
    <span>{label}</span>
    <input type="range" min={0} max={100} step={5} value={percent} aria-valuetext={`${percent}%`} onChange={(event) => onChange(Number(event.target.value))} />
    <output>{percent}%</output>
  </label>;
}

export function triageActionText(action: TriageAction): string {
  switch (action) {
    case 'decline': return tr('bots_triage_action_decline');
    case 'run': return tr('bots_triage_action_run');
    case 'run_may_decline': return tr('bots_triage_action_maybe');
  }
}

/** Why, in words; nothing for a reason this version does not know. */
export function triageReasonText(reason: TriageReason | undefined): string {
  switch (reason) {
    case 'quiet_hours': return tr('bots_triage_reason_quiet_hours');
    case 'cooldown': return tr('bots_triage_reason_cooldown');
    case 'keyword': return tr('bots_triage_reason_keyword');
    case 'addressed': return tr('bots_triage_reason_addressed');
    case 'below_threshold': return tr('bots_triage_reason_below_threshold');
    case 'between_thresholds': return tr('bots_triage_reason_between_thresholds');
    case 'above_threshold': return tr('bots_triage_reason_above_threshold');
    case 'decision_unavailable': return tr('bots_triage_reason_decision_unavailable');
    case 'message_unavailable': return tr('bots_triage_reason_message_unavailable');
    default: return '';
  }
}

export function triageOutcomeText(outcome: TriageOutcome): string {
  switch (outcome) {
    case 'replied': return tr('bots_triage_state_replied');
    case 'silent': return tr('bots_triage_state_silent');
    case 'failed': return tr('bots_triage_state_failed');
    case 'waiting': return tr('bots_triage_state_waiting');
    case 'approval': return tr('teams_task_waiting_approval');
  }
}

/** How a held reply was decided, in words: nothing for a message whose reply was never held, one that still waits, or a code this version does not know. */
export function triageApprovalText(approval: ApprovalOutcome | undefined): string {
  switch (approval) {
    case 'approved': return tr('teams_approval_reply_approved');
    case 'rejected': return tr('teams_approval_reply_declined');
    case 'timeout': return tr('teams_triage_approval_timeout');
    case 'superseded': return tr('teams_triage_approval_superseded');
    default: return '';
  }
}

/** What became of the Bot when several could answer the message, in words: nothing for a message that was not such a round or an outcome this version does not know. */
export function triageRoundText(round: TaskRound | undefined): string {
  switch (round?.outcome) {
    case 'won': return tr('teams_triage_round_won');
    case 'lost': return tr('teams_triage_round_lost');
    case 'abstained': return tr('teams_triage_round_abstained');
    case 'promoted': return tr('teams_triage_round_promoted');
    case 'superseded': return tr('teams_triage_round_superseded');
    default: return '';
  }
}

/**
 * Which Bot asked for the task, in words (Bot v3 B8): nothing for a message a person sent. The row is shown whatever the service sent of it, so a delegation this version
 * cannot read says nothing, as on the Teams views.
 */
export function triageDelegatedText(delegation: TaskDelegation | undefined): string {
  const by = delegatedBy(readTaskDelegation(delegation), tr('teams_bot_tag'));
  return by === undefined ? '' : tr('teams_triage_delegated_by_fmt', by);
}

const VERDICT_ICON: Record<TriageAction, string> = { decline: 'pauseCircle', run: 'playCircle', run_may_decline: 'circleAlert' };
/** The colour of the tag that says where the message ended up; the ones that are none of these keep the plain tag. */
const OUTCOME_STATUS: Partial<Record<TriageOutcome, string>> = { replied: 'completed', failed: 'failed', waiting: 'working', approval: 'input-required' };

/** What the Bot decided about the messages nobody asked it about, for each Team it is published to. */
export function BotTriageLog({ state, onRefresh, onMore }: { state: TriageLogState; onRefresh(): void; onMore(section: TriageLogSection): void }) {
  const busy = state.phase === 'idle' || state.phase === 'loading';
  return <section className="bots-fields bots-triage" aria-label={tr('bots_triage_title')}>
    <header className="bots-triage-head">
      <h2>{tr('bots_triage_title')}</h2>
      <button type="button" data-variant="quiet" disabled={busy || state.phase === 'unavailable'} onClick={onRefresh}>{tr('bots_triage_refresh')}</button>
    </header>
    <p className="bots-hint">{tr('bots_triage_intro')}</p>
    {busy ? <p role="status" className="bots-muted">{tr('bots_triage_loading')}</p> : null}
    {state.phase === 'unavailable' ? <p role="status" className="bots-muted">{tr('bots_triage_signed_out')}</p> : null}
    {state.phase === 'failed' ? <p role="alert" className="bots-problem">{tr('bots_triage_failed', state.error ?? '')} <button type="button" onClick={onRefresh}>{tr('common_retry')}</button></p> : null}
    {state.phase === 'ready' && state.sections.length === 0 ? <p className="bots-empty-line">{tr('bots_triage_not_published')}</p> : null}
    {state.sections.map((section) => <BotTriageSection key={`${section.target.teamId}:${section.target.sharedBotId}`} section={section} onRetry={onRefresh} onMore={() => onMore(section)} />)}
  </section>;
}

function BotTriageSection({ section, onRetry, onMore }: { section: TriageLogSection; onRetry(): void; onMore(): void }) {
  const { locale } = useI18n();
  const { items, status } = section;
  return <div className="bots-triage-section">
    <h3 className="bots-triage-team">{section.target.teamName}</h3>
    {status === 'loading' ? <p role="status" className="bots-muted">{tr('bots_triage_loading')}</p> : null}
    {status === 'ready' && items.length === 0 ? <p className="bots-empty-line">{tr('bots_triage_empty')}</p> : null}
    {items.length > 0 ? <ul className="bots-rows" aria-label={section.target.teamName}>
      {items.map((item) => {
        const outcome = triageOutcome(item.state);
        const why = triageReasonText(item.triage.reason), decided = triageApprovalText(item.approval), round = triageRoundText(item.round), delegated = triageDelegatedText(item.delegation);
        return <li key={item.taskId} className="bots-row bots-triage-row">
          <span className="bots-triage-icon" data-action={item.triage.action} aria-hidden="true"><Icon name={VERDICT_ICON[item.triage.action]} size={16} /></span>
          <div className="bots-row-main">
            <span className="bots-row-title">
              {triageActionText(item.triage.action)}
              {why ? <span className="bots-triage-why">{why}</span> : null}
              {item.triage.probability !== undefined ? <span className="bots-tag">{tr('bots_triage_probability', triagePercent(item.triage.probability))}</span> : null}
            </span>
            <span className="bots-row-text">{item.conversationTitle || tr('bots_triage_chat_untitled')} · <time dateTime={new Date(item.triage.at).toISOString()}>{formatLogTime(item.triage.at, locale)}</time>{decided ? ` · ${decided}` : ''}{round ? ` · ${round}` : ''}{delegated ? ` · ${delegated}` : ''}</span>
          </div>
          <span className="bots-tag" data-status={OUTCOME_STATUS[outcome]}>{triageOutcomeText(outcome)}</span>
        </li>;
      })}
    </ul> : null}
    {status === 'failed' ? <p role="alert" className="bots-problem">{tr('bots_triage_failed', section.error ?? '')}{items.length === 0 ? <button type="button" onClick={onRetry}>{tr('common_retry')}</button> : null}</p> : null}
    {section.nextCursor !== undefined ? <div><button type="button" disabled={status === 'more'} aria-busy={status === 'more' || undefined} onClick={onMore}>{status === 'more' ? tr('bots_triage_loading') : tr('bots_triage_more')}</button></div> : null}
  </div>;
}

/** A time in epoch milliseconds, as the owner reads dates: the day and the hour, in their language and their time zone. */
export function formatLogTime(at: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, { dateStyle: 'short', timeStyle: 'short' }).format(at);
}
