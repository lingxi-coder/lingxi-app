import { useEffect, useState } from 'react';
import { executionCommand, operationUnverified, type ExecutionScope, type OperationResolved, type TaskOperation, type TaskOperations, type TeamArtifact, type TeamTaskView, type TaskUsage, type TriageLogItem } from './execution.js';
import { pendingPayload } from './messages.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { DocumentDialog, type DocumentPeople } from './DocumentDialog';
import { Dialog, Icon, type IconName, type RenderMarkdown } from './ui';
import type { TeamsSnapshot } from './workspace.js';
import { intlTag, teamsText, tk, type TeamsKey, useTeamsText } from './i18n.js';
import { triageProbabilityText, triageReasonText } from './triage-log.js';
import { delegationChain, mayCancel, waitingForBot } from './delegation.js';

/** Only Completed is `ok` and only Failed `danger` (U4); every other state is `muted`. */
type RunTone = 'ok' | 'danger' | 'muted';
/** How one state of a Bot task reads and looks on its card: the key of its words, the key of the sentence under them (none for Completed, whose line is the usage), the glyph and the tone. */
interface RunLook { label: TeamsKey; detail?: TeamsKey; icon: IconName; tone: RunTone; spins?: boolean }

/**
 * The states of a run card (U4), by the state the service reports; the words are the keys iOS and Android use for the same states. A state
 * this table does not know reads as `Unknown`; a state the service adds is one more row here (label, detail, glyph, tone), and nothing else changes.
 */
export const RUN_STATES: Record<string, RunLook> = {
  queued: { label: tk('teams_run_queued'), detail: tk('teams_run_queued_detail'), icon: 'clock', tone: 'muted' },
  running: { label: tk('teams_run_processing'), detail: tk('teams_run_processing_detail'), icon: 'spinner', tone: 'muted', spins: true },
  completed: { label: tk('chat_status_completed'), icon: 'checkCircle', tone: 'ok' },
  failed: { label: tk('chat_status_failed'), detail: tk('teams_run_failed_detail'), icon: 'alert', tone: 'danger' },
  expired: { label: tk('teams_run_expired'), detail: tk('teams_run_expired_detail'), icon: 'clockSlash', tone: 'muted' },
  interrupted: { label: tk('teams_run_interrupted'), detail: tk('teams_run_interrupted_detail'), icon: 'pauseCircle', tone: 'muted' },
  authorization_required: { label: tk('teams_run_authorization_required'), detail: tk('teams_run_authorization_required_detail'), icon: 'lock', tone: 'muted' },
  revoked: { label: tk('teams_run_revoked'), detail: tk('teams_run_revoked_detail'), icon: 'ban', tone: 'muted' },
  waiting: { label: tk('teams_task_waiting'), icon: 'dotsCircle', tone: 'muted' },
  cancelled: { label: tk('teams_task_cancelled'), icon: 'xCircle', tone: 'muted' },
  needs_reconciliation: { label: tk('teams_task_needs_checking'), icon: 'help', tone: 'muted' },
  declined: { label: tk('teams_task_declined'), icon: 'help', tone: 'muted' },
  // The reply is made and its maintainer reads it first; the words are those of an approval a member gives, which says the same.
  awaiting_approval: { label: tk('teams_task_waiting_approval'), detail: tk('teams_run_awaiting_approval_detail'), icon: 'clock', tone: 'muted' },
};
const UNKNOWN_STATE: RunLook = { label: tk('chat_status_unknown'), icon: 'help', tone: 'muted' };
export const runLook = (state: string): RunLook => Object.prototype.hasOwnProperty.call(RUN_STATES, state) ? RUN_STATES[state]! : UNKNOWN_STATE;

/** A positive cost below this many micros reads "<" and the smallest amount (0.0001). */
const SMALLEST_COST_MICROS = 100;

/** An amount with two to four decimals, in the currency of the task; without a currency it says so. Ties round to even, as on iOS and Android. */
function amountText(value: number, currency: string | undefined, locale: string): string {
  const t = teamsText(locale);
  const digits = { minimumFractionDigits: 2, maximumFractionDigits: 4, roundingMode: 'halfEven' } as Intl.NumberFormatOptions;
  if (!currency) return `${new Intl.NumberFormat(intlTag(locale), digits).format(value)} ${t('teams_usage_currency_unknown')}`;
  try { return new Intl.NumberFormat(intlTag(locale), { ...digits, style: 'currency', currency: currency.toUpperCase() }).format(value); }
  catch { return `${new Intl.NumberFormat(intlTag(locale), digits).format(value)} ${currency.toUpperCase()}`; }
}

/**
 * The usage line of a finished run (U5): "1.2K tokens · $0.0012". The tokens are the total of input and output in the compact number of
 * the language; the cost is in the currency of the task with two to four decimals, "<$0.0001" for a positive cost below that, "cost unknown" when
 * the task carries none and the amount with "currency unknown" when it has no currency. No usage reads "Usage unavailable" and a partial
 * one is marked "Partial usage"; a cost is never invented. iOS (`TeamsBotText.usageLine`) and Android say the same.
 */
export function usageText(usage: TaskUsage | undefined, locale: string): string {
  const t = teamsText(locale);
  if (!usage || usage.status === 'missing') return t('teams_usage_missing');
  const tokens = t('teams_usage_tokens_total_fmt', new Intl.NumberFormat(intlTag(locale), { notation: 'compact' }).format((usage.inputTokens ?? 0) + (usage.outputTokens ?? 0)));
  const micros = usage.costMicros;
  const currency = usage.currency?.trim() || undefined;
  const cost = micros === undefined || !Number.isFinite(micros) ? (t('teams_usage_cost_unknown'))
    : micros > 0 && micros < SMALLEST_COST_MICROS ? `<${amountText(SMALLEST_COST_MICROS / 1_000_000, currency, locale)}`
    : amountText(micros / 1_000_000, currency, locale);
  return [usage.status === 'partial' ? (t('teams_usage_partial')) : '', tokens, cost].filter(Boolean).join(' · ');
}

/** "123 / 456 tokens": the split behind the usage line, for its tooltip. Nothing when there is no usage to speak of. */
export function tokenSplit(usage: TaskUsage | undefined, locale: string): string {
  return !usage || usage.status === 'missing' ? '' : teamsText(locale)('teams_usage_tokens_fmt', usage.inputTokens ?? 0, usage.outputTokens ?? 0);
}

export function ArtifactButton({ api, scope, artifactId, locale, renderMarkdown, people }: { api: TeamsApi; scope: ExecutionScope; artifactId: string; locale: string; renderMarkdown?: RenderMarkdown; people?: DocumentPeople }) {
  const t = useTeamsText(locale);
  const [artifact, setArtifact] = useState<TeamArtifact>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const open = async () => {
    if (busy) return; setBusy(true); setError(''); setArtifact(undefined);
    try { setArtifact(await api.messages<TeamArtifact>(executionCommand(scope, 'readArtifact', { artifactId }))); }
    catch (e) { setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); }
    finally { setBusy(false); }
  };
  return <><button type="button" disabled={busy} onClick={() => void open()}>{busy ? (t('tw_opening')) : (t('teams_artifact_open'))}</button>{error ? <p role="alert">{messageErrorText(error, locale)}</p> : null}{artifact ? <DocumentDialog api={api} scope={scope} first={artifact} locale={locale} renderMarkdown={renderMarkdown} people={people} onClose={() => setArtifact(undefined)} /> : null}</>;
}
/** How long until `at`, as a short phrase ("in 4 min"); never less than "under a minute". */
export function expiresIn(at: number, now: number, locale: string): string {
  const t = teamsText(locale);
  const seconds = Math.max(0, Math.round((at - now) / 1000));
  if (seconds < 60) return t('tw_in_under_minute');
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return t('tw_in_minutes_fmt', minutes);
  const hours = Math.ceil(minutes / 60);
  return hours < 48 ? (t('tw_in_hours_fmt', hours)) : (t('tw_in_days_fmt', Math.ceil(hours / 24)));
}
/**
 * What a task did outside the Chat. The Bot's maintainer confirms the actions nobody could verify:
 * the task ends only when each of them is known to have happened or not.
 */
export function OperationsDialog({ api, scope, task, locale, canResolve, people, onClose, onChanged }: {
  api: TeamsApi; scope: ExecutionScope; task: TeamTaskView; locale: string; canResolve: boolean; people: Pick<TeamsSnapshot, 'directory'>; onClose(): void; onChanged?(): void;
}) {
  const [rows, setRows] = useState<TaskOperation[]>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const t = useTeamsText(locale);
  const load = async () => setRows((await api.messages<TaskOperations>(executionCommand(scope, 'listOperations', { taskId: task.taskId }))).operations);
  const attempt = async (work: () => Promise<void>) => {
    if (busy) return; setBusy(true); setError('');
    try { await work(); } catch (e) { setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); } finally { setBusy(false); }
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- the list belongs to this task; it is reloaded after each decision.
  useEffect(() => { void attempt(load); }, [task.taskId]);
  const resolve = (operation: TaskOperation, resolution: 'succeeded' | 'failed') => attempt(async () => {
    try { await api.messages<OperationResolved>(executionCommand(scope, 'resolveOperation', { taskId: task.taskId, operationId: operation.operationId, resolution })); }
    finally { onChanged?.(); }
    await load();
  });
  const stateText = (operation: TaskOperation) => operationUnverified(operation)
    ? (operation.state === 'unknown' ? t('teams_ops_state_unknown') : t('teams_ops_state_unreported'))
    : operation.state === 'succeeded' ? t('teams_ops_state_happened') : operation.state === 'failed' ? t('teams_ops_state_not_happened') : t('teams_ops_state_repeatable');
  const when = (ms: number) => new Intl.DateTimeFormat(intlTag(locale), { dateStyle: 'medium', timeStyle: 'short' }).format(ms);
  const open = rows?.filter(operationUnverified).length ?? 0;
  return <Dialog title={t('teams_ops_title')} onClose={onClose}>
    {rows && open ? <p>{canResolve
      ? t('teams_ops_intro_maintainer')
      : t('teams_ops_intro_requester')}</p> : null}
    {rows && !rows.length ? <p>{t('teams_ops_empty')}</p> : null}
    {rows?.length ? <ol className="tw-doc-history">{rows.map(operation => <li key={operation.operationId}>
      <span><b>{operation.tool.name}</b> · {stateText(operation)}</span>
      <span className="tw-hint">{when(operation.dispatchedAt)}{operation.resolvedBy ? ` · ${t('teams_ops_confirmed_by_fmt', people.directory.find(member => member.uid === operation.resolvedBy!.uid)?.displayName || t('teams_member'))}` : ''}</span>
      {canResolve && operationUnverified(operation) ? <div className="teams-row-actions">
        <button type="button" disabled={busy} aria-label={t('teams_ops_happened_label_fmt', operation.tool.name)} onClick={() => void resolve(operation, 'succeeded')}>{t('teams_ops_happened')}</button>
        <button type="button" disabled={busy} aria-label={t('teams_ops_not_happened_label_fmt', operation.tool.name)} onClick={() => void resolve(operation, 'failed')}>{t('teams_ops_not_happened')}</button>
      </div> : null}
    </li>)}</ol> : null}
    {!rows && !error ? <p role="status">{t('teams_loading')}</p> : null}
    {error ? <p role="alert">{messageErrorText(error, locale)}</p> : null}
    <div className="tw-dialog-actions"><button type="button" onClick={onClose}>{t('common_close')}</button></div>
  </Dialog>;
}
const CANCELLABLE: TeamTaskView['state'][] = ['queued', 'running', 'waiting', 'awaiting_approval'];
type RunViewer = Pick<TeamsSnapshot, 'uid' | 'bots' | 'directory' | 'pending'>;
export function BotRun({ task, api, scope, locale, renderMarkdown, snapshot, tasks, reason, onReply, onChanged }: {
  task: TeamTaskView; api: TeamsApi; scope: ExecutionScope; locale: string; renderMarkdown?: RenderMarkdown; snapshot: RunViewer;
  /** The other tasks of the Chat: a task that waits for another Bot's reply finds that Bot's name in the task it asked when the Team's list has no card of it. */
  tasks?: readonly TeamTaskView[];
  /** What the Bot decided about the message and why, from its auto-reply log. Given only where this person maintains the Bot; it says why a Bot that did not reply stayed silent. */
  reason?: TriageLogItem;
  /** Start answering the Bot's question; absent where the composer is not at hand. */
  onReply?(task: TeamTaskView): void;
  /** A command changed the task: fetch it again. */
  onChanged?(): void;
}) {
  const [cancelling, setCancelling] = useState(false), [error, setError] = useState(''), [reviewing, setReviewing] = useState(false);
  const t = useTeamsText(locale);
  const nameOf = (uid: string) => snapshot.directory.find(member => member.uid === uid)?.displayName || t('teams_member');
  const isMember = (who: { uid: string; generation: number }) => who.uid === snapshot.uid && who.generation === scope.generation;
  const bot = snapshot.bots.find(card => card.sharedBotId === task.sharedBotId);
  const maintainer = bot?.maintainer;
  const maintains = !!maintainer && isMember(maintainer);
  // The person who asked and the Bot's maintainer; for a task another Bot asked for, also the maintainer of the Bot that asked (the service's `canCancel`).
  const canCancel = CANCELLABLE.includes(task.state) && mayCancel(task, { uid: snapshot.uid, generation: scope.generation }, snapshot.bots);
  const unchecked = task.state === 'needs_reconciliation';
  const stopping = task.state === 'running' && task.cancelRequestedAt !== undefined;
  const wait = task.state === 'waiting' ? task.wait : undefined;
  // An approval is decided on the request itself, never by a typed reply.
  const canReply = !!wait && wait.kind === 'human_input' && !!onReply && isMember(wait.recipient)
    && !snapshot.pending.some(item => { const payload = pendingPayload(item); return 'waitId' in payload && payload.waitId === wait.waitId; });
  // A Bot that chose not to answer: one quiet line, nothing to act on. A run that used something still says so: a cost is never hidden (U5).
  if (task.state === 'declined') {
    // Why, for the maintainer only: the reason the Bot's host gave and, when the Decision service judged, how likely it thought a reply was wanted.
    const why = maintains && reason ? [triageReasonText(t, reason.triage.reason), reason.triage.probability === undefined ? '' : triageProbabilityText(t, reason.triage.probability)] : [];
    return <p className="teams-hint team-bot-run-declined" role="status" title={tokenSplit(task.usage, locale) || undefined}>
      {[t('teams_task_declined'), ...why, task.usage ? usageText(task.usage, locale) : ''].filter(Boolean).join(' · ')}</p>;
  }
  const cancel = async () => {
    if (cancelling) return; setCancelling(true); setError('');
    try { await api.messages(executionCommand(scope, 'cancelTask', { taskId: task.taskId })); }
    catch (e) { const code = e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'; if (code !== 'TASK_FINISHED') setError(code); }
    finally { setCancelling(false); onChanged?.(); }
  };
  const look = runLook(task.state);
  const label = stopping ? t('teams_task_cancelling') : wait?.kind === 'tool_approval' ? t('teams_task_waiting_approval') : t(look.label);
  // One line under the state (U4): who it waits for, what to check, or what the state means.
  const expires = wait ? expiresIn(wait.expiresAt, Date.now(), locale) : '';
  // A wait for another Bot's reply (Bot v3 B8) is not for the person who asked, whom the service names as its recipient: it says which Bot it waits for.
  const detail = wait ? (wait.kind === 'delegation' ? t('teams_task_waiting_bot_fmt', waitingForBot(task, snapshot.bots, tasks) ?? t('teams_bot_tag'))
      : isMember(wait.recipient) ? t('teams_task_waiting_you_fmt', expires) : t('teams_task_waiting_member_fmt', nameOf(wait.recipient.uid), expires))
    : unchecked ? (maintains
      ? t('teams_task_needs_checking_maintainer')
      : t('teams_task_needs_checking_hint'))
    // Sent while the Bot's Desktop was off (rule 6): it answers when the Desktop is back, within a day.
    : task.state === 'queued' && task.hostWait ? t('teams_bot_offline_wait_fmt', bot?.name ?? t('teams_bot_tag'))
    : look.detail ? t(look.detail) : '';
  // The usage (U5): always once completed; for any other state only when the run did use something, so a cost is never hidden.
  const usage = task.state === 'completed' || task.usage ? usageText(task.usage, locale) : '';
  // A task another Bot asked for says whose work it is part of: "Planner → Travel", from the Bot the person asked to this one.
  const chain = delegationChain(task, snapshot.bots, t('teams_bot_tag'));
  return <section className="team-bot-run" data-state={task.state} data-tone={look.tone} aria-label={t('tw_run_status_label')}>
    <p className="tw-run-state" role="status">
      <span className="tw-run-glyph"><Icon name={look.icon} className={look.spins && !stopping ? 'tw-run-spin' : undefined} />{look.spins && !stopping ? <Icon name="dotsCircle" className="tw-run-still" /> : null}</span>
      <span>{label}</span>
    </p>
    {chain ? <p className="tw-run-detail tw-run-chain">{t('teams_task_delegation_chain_fmt', chain)}</p> : null}
    {detail ? <p className="tw-run-detail">{detail}</p> : null}
    {usage ? <p className="tw-run-detail" title={tokenSplit(task.usage, locale) || undefined}>{usage}</p> : null}
    {canReply || (canCancel && !stopping) ? <div className="teams-row-actions">
      {canReply ? <button type="button" aria-label={t('tw_run_reply')} onClick={() => onReply!(task)}>{t('teams_message_reply')}</button> : null}
      {canCancel && !stopping ? <button type="button" disabled={cancelling} aria-label={t('teams_task_cancel')} onClick={() => void cancel()}>{t('common_cancel')}</button> : null}
    </div> : null}
    {unchecked && (maintains || isMember(task.requester)) ? <div className="teams-row-actions"><button type="button" onClick={() => setReviewing(true)}>{maintains ? t('teams_ops_review') : t('teams_ops_view')}</button></div> : null}
    {reviewing ? <OperationsDialog api={api} scope={scope} task={task} locale={locale} canResolve={maintains} people={snapshot} onClose={() => setReviewing(false)} onChanged={onChanged} /> : null}
    {error ? <p role="alert">{messageErrorText(error, locale)}</p> : null}
    {task.artifactId ? <ArtifactButton api={api} scope={scope} artifactId={task.artifactId} locale={locale} renderMarkdown={renderMarkdown} people={snapshot} /> : null}
  </section>;
}
