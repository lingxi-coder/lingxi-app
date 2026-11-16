import { useEffect, useState } from 'react';
import { executionCommand, operationUnverified, type ExecutionScope, type OperationResolved, type TaskOperation, type TaskOperations, type TeamArtifact, type TeamTaskView, type TaskUsage } from './execution.js';
import { pendingPayload } from './messages.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { DocumentDialog, type DocumentPeople } from './DocumentDialog';
import { Dialog, Icon, type IconName, type RenderMarkdown } from './ui';
import type { TeamsSnapshot } from './workspace.js';

/** Only Completed is `ok` and only Failed `danger` (U4); every other state is `muted`. */
type RunTone = 'ok' | 'danger' | 'muted';
/** How one state of a Bot task reads and looks on its card: the words, the sentence under them (none for Completed, whose line is the usage), the glyph and the tone. */
interface RunLook { label: [string, string]; detail?: [string, string]; icon: IconName; tone: RunTone; spins?: boolean }

/**
 * The states of a run card (U4), by the state the service reports. A state this table does not know reads as `Unknown`; a state the
 * service adds is one more row here (label, detail, glyph, tone), and nothing else changes.
 */
export const RUN_STATES: Record<string, RunLook> = {
  queued: { label: ['Queued', '已排队'], detail: ['Waiting for the Bot’s Desktop to pick this up.', '等待 Bot 所在的 Desktop 接收此请求。'], icon: 'clock', tone: 'muted' },
  running: { label: ['Processing', '处理中'], detail: ['The Bot is working on it.', 'Bot 正在处理。'], icon: 'spinner', tone: 'muted', spins: true },
  completed: { label: ['Completed', '已完成'], icon: 'checkCircle', tone: 'ok' },
  failed: { label: ['Failed', '失败'], detail: ['The Bot couldn’t finish this request. Send it again.', 'Bot 未能完成此请求，请重新发送。'], icon: 'alert', tone: 'danger' },
  expired: { label: ['Expired', '已过期'], detail: ['The Bot’s Desktop didn’t pick this up in time. Send it again.', 'Bot 所在的 Desktop 未能及时接收，请重新发送。'], icon: 'clockSlash', tone: 'muted' },
  interrupted: { label: ['Interrupted', '已中断'], detail: ['The Bot’s Desktop stopped before finishing. Send a new request.', 'Bot 所在的 Desktop 在完成前停止了，请发送新的请求。'], icon: 'pauseCircle', tone: 'muted' },
  authorization_required: { label: ['Authorization required', '需要授权'], detail: ['This result can’t be shown yet. Ask the Bot’s maintainer to authorize it.', '暂时无法显示此结果，请联系 Bot 的维护者授权。'], icon: 'lock', tone: 'muted' },
  revoked: { label: ['Access revoked', '授权已撤销'], detail: ['You no longer have access to this Bot.', '你已无法再使用此 Bot。'], icon: 'ban', tone: 'muted' },
  waiting: { label: ['Waiting for a reply', '等待回复'], icon: 'dotsCircle', tone: 'muted' },
  cancelled: { label: ['Cancelled', '已取消'], detail: ['This request was cancelled.', '此请求已取消。'], icon: 'xCircle', tone: 'muted' },
  needs_reconciliation: { label: ['Needs checking', '需要核实'], icon: 'help', tone: 'muted' },
};
const UNKNOWN_STATE: RunLook = { label: ['Unknown', '未知'], icon: 'help', tone: 'muted' };
export const runLook = (state: string): RunLook => Object.prototype.hasOwnProperty.call(RUN_STATES, state) ? RUN_STATES[state]! : UNKNOWN_STATE;

/** A positive cost below this many micros reads "<" and the smallest amount (0.0001). */
const SMALLEST_COST_MICROS = 100;

/** An amount with two to four decimals, in the currency of the task; without a currency it says so. Ties round to even, as on iOS and Android. */
function amountText(value: number, currency: string | undefined, zh: boolean, locale: string): string {
  const digits = { minimumFractionDigits: 2, maximumFractionDigits: 4, roundingMode: 'halfEven' } as Intl.NumberFormatOptions;
  if (!currency) return `${new Intl.NumberFormat(locale, digits).format(value)} ${zh ? '币种未知' : 'currency unknown'}`;
  try { return new Intl.NumberFormat(locale, { ...digits, style: 'currency', currency: currency.toUpperCase() }).format(value); }
  catch { return `${new Intl.NumberFormat(locale, digits).format(value)} ${currency.toUpperCase()}`; }
}

/**
 * The usage line of a finished run (U5): "1.2K tokens · $0.0012". The tokens are the total of input and output in the compact number of
 * the language; the cost is in the currency of the task with two to four decimals, "<$0.0001" for a positive cost below that, "cost unknown" when
 * the task carries none and the amount with "currency unknown" when it has no currency. No usage reads "Usage unavailable" and a partial
 * one is marked "Partial usage"; a cost is never invented. iOS (`TeamsBotText.usageLine`) and Android say the same.
 */
export function usageText(usage: TaskUsage | undefined, zh: boolean, locale: string = zh ? 'zh-CN' : 'en-US'): string {
  if (!usage || usage.status === 'missing') return zh ? '用量不可用' : 'Usage unavailable';
  const tokens = `${new Intl.NumberFormat(locale, { notation: 'compact' }).format((usage.inputTokens ?? 0) + (usage.outputTokens ?? 0))} tokens`;
  const micros = usage.costMicros;
  const currency = usage.currency?.trim() || undefined;
  const cost = micros === undefined || !Number.isFinite(micros) ? (zh ? '费用未知' : 'cost unknown')
    : micros > 0 && micros < SMALLEST_COST_MICROS ? `<${amountText(SMALLEST_COST_MICROS / 1_000_000, currency, zh, locale)}`
    : amountText(micros / 1_000_000, currency, zh, locale);
  return [usage.status === 'partial' ? (zh ? '用量不完整' : 'Partial usage') : '', tokens, cost].filter(Boolean).join(' · ');
}

/** "123 / 456 tokens": the split behind the usage line, for its tooltip. Nothing when there is no usage to speak of. */
export function tokenSplit(usage: TaskUsage | undefined): string {
  return !usage || usage.status === 'missing' ? '' : `${usage.inputTokens ?? 0} / ${usage.outputTokens ?? 0} tokens`;
}

export function ArtifactButton({ api, scope, artifactId, zh, renderMarkdown, people }: { api: TeamsApi; scope: ExecutionScope; artifactId: string; zh: boolean; renderMarkdown?: RenderMarkdown; people?: DocumentPeople }) {
  const [artifact, setArtifact] = useState<TeamArtifact>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const open = async () => {
    if (busy) return; setBusy(true); setError(''); setArtifact(undefined);
    try { setArtifact(await api.messages<TeamArtifact>(executionCommand(scope, 'readArtifact', { artifactId }))); }
    catch (e) { setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); }
    finally { setBusy(false); }
  };
  return <><button type="button" disabled={busy} onClick={() => void open()}>{busy ? (zh ? '正在打开…' : 'Opening…') : (zh ? '打开文档' : 'Open document')}</button>{error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}{artifact ? <DocumentDialog api={api} scope={scope} first={artifact} zh={zh} renderMarkdown={renderMarkdown} people={people} onClose={() => setArtifact(undefined)} /> : null}</>;
}
/** How long until `at`, as a short phrase ("in 4 min"); never less than "under a minute". */
export function expiresIn(at: number, now: number, zh: boolean): string {
  const seconds = Math.max(0, Math.round((at - now) / 1000));
  if (seconds < 60) return zh ? '不到 1 分钟后' : 'in under a minute';
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return zh ? `${minutes} 分钟后` : `in ${minutes} min`;
  const hours = Math.ceil(minutes / 60);
  return hours < 48 ? (zh ? `${hours} 小时后` : `in ${hours} h`) : (zh ? `${Math.ceil(hours / 24)} 天后` : `in ${Math.ceil(hours / 24)} days`);
}
/**
 * What a task did outside the Chat. The Bot's maintainer confirms the actions nobody could verify:
 * the task ends only when each of them is known to have happened or not.
 */
export function OperationsDialog({ api, scope, task, zh, canResolve, people, onClose, onChanged }: {
  api: TeamsApi; scope: ExecutionScope; task: TeamTaskView; zh: boolean; canResolve: boolean; people: Pick<TeamsSnapshot, 'directory'>; onClose(): void; onChanged?(): void;
}) {
  const [rows, setRows] = useState<TaskOperation[]>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const t = (en: string, cn: string) => zh ? cn : en;
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
    ? (operation.state === 'unknown' ? t('Result unknown', '结果未知') : t('No result reported', '未报告结果'))
    : operation.state === 'succeeded' ? t('Happened', '已执行') : operation.state === 'failed' ? t('Did not happen', '未执行') : t('Safe to repeat', '可安全重试');
  const when = (ms: number) => new Intl.DateTimeFormat(zh ? 'zh-CN' : 'en', { dateStyle: 'medium', timeStyle: 'short' }).format(ms);
  const open = rows?.filter(operationUnverified).length ?? 0;
  return <Dialog title={t('Actions outside the chat', '会话之外的操作')} onClose={onClose}>
    {rows && open ? <p>{canResolve
      ? t('Check each action where it took place, then say whether it happened. The task ends when all of them are confirmed.', '请到操作实际发生的地方逐项核实，然后说明它是否已执行。全部确认后任务才会结束。')
      : t('The Bot’s maintainer needs to confirm these actions before the task can end.', '需要 Bot 维护者确认这些操作后，任务才能结束。')}</p> : null}
    {rows && !rows.length ? <p>{t('This task recorded no outside actions.', '此任务没有记录会话之外的操作。')}</p> : null}
    {rows?.length ? <ol className="tw-doc-history">{rows.map(operation => <li key={operation.operationId}>
      <span><b>{operation.tool.name}</b> · {stateText(operation)}</span>
      <span className="tw-hint">{when(operation.dispatchedAt)}{operation.resolvedBy ? ` · ${t('confirmed by', '确认人')} ${people.directory.find(member => member.uid === operation.resolvedBy!.uid)?.displayName || t('Member', '成员')}` : ''}</span>
      {canResolve && operationUnverified(operation) ? <div className="teams-row-actions">
        <button type="button" disabled={busy} aria-label={t(`${operation.tool.name}: it happened`, `${operation.tool.name}：已执行`)} onClick={() => void resolve(operation, 'succeeded')}>{t('It happened', '已执行')}</button>
        <button type="button" disabled={busy} aria-label={t(`${operation.tool.name}: it did not happen`, `${operation.tool.name}：未执行`)} onClick={() => void resolve(operation, 'failed')}>{t('It did not happen', '未执行')}</button>
      </div> : null}
    </li>)}</ol> : null}
    {!rows && !error ? <p role="status">{t('Loading…', '正在加载…')}</p> : null}
    {error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}
    <div className="tw-dialog-actions"><button type="button" onClick={onClose}>{t('Close', '关闭')}</button></div>
  </Dialog>;
}
const CANCELLABLE: TeamTaskView['state'][] = ['queued', 'running', 'waiting'];
type RunViewer = Pick<TeamsSnapshot, 'uid' | 'bots' | 'directory' | 'pending'>;
export function BotRun({ task, api, scope, zh, renderMarkdown, snapshot, onReply, onChanged }: {
  task: TeamTaskView; api: TeamsApi; scope: ExecutionScope; zh: boolean; renderMarkdown?: RenderMarkdown; snapshot: RunViewer;
  /** Start answering the Bot's question; absent where the composer is not at hand. */
  onReply?(task: TeamTaskView): void;
  /** A command changed the task: fetch it again. */
  onChanged?(): void;
}) {
  const [cancelling, setCancelling] = useState(false), [error, setError] = useState(''), [reviewing, setReviewing] = useState(false);
  const t = (en: string, cn: string) => zh ? cn : en;
  const nameOf = (uid: string) => snapshot.directory.find(member => member.uid === uid)?.displayName || t('Member', '成员');
  const isMember = (who: { uid: string; generation: number }) => who.uid === snapshot.uid && who.generation === scope.generation;
  const maintainer = snapshot.bots.find(bot => bot.sharedBotId === task.sharedBotId)?.maintainer;
  const maintains = !!maintainer && isMember(maintainer);
  const canCancel = CANCELLABLE.includes(task.state) && (isMember(task.requester) || maintains);
  const unchecked = task.state === 'needs_reconciliation';
  const stopping = task.state === 'running' && task.cancelRequestedAt !== undefined;
  const wait = task.state === 'waiting' ? task.wait : undefined;
  // An approval is decided on the request itself, never by a typed reply.
  const canReply = !!wait && wait.kind === 'human_input' && !!onReply && isMember(wait.recipient)
    && !snapshot.pending.some(item => { const payload = pendingPayload(item); return 'waitId' in payload && payload.waitId === wait.waitId; });
  const cancel = async () => {
    if (cancelling) return; setCancelling(true); setError('');
    try { await api.messages(executionCommand(scope, 'cancelTask', { taskId: task.taskId })); }
    catch (e) { const code = e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'; if (code !== 'TASK_FINISHED') setError(code); }
    finally { setCancelling(false); onChanged?.(); }
  };
  const look = runLook(task.state);
  const label = stopping ? t('Cancelling…', '正在取消…') : wait?.kind === 'tool_approval' ? t('Waiting for approval', '等待批准') : look.label[zh ? 1 : 0];
  // One line under the state (U4): who it waits for, what to check, or what the state means.
  const detail = wait ? `${isMember(wait.recipient) ? t('Waiting for you', '等待你回复') : t(`Waiting for ${nameOf(wait.recipient.uid)}`, `等待 ${nameOf(wait.recipient.uid)} 回复`)} · ${zh ? `${expiresIn(wait.expiresAt, Date.now(), zh)}过期` : `expires ${expiresIn(wait.expiresAt, Date.now(), zh)}`}`
    : unchecked ? (maintains
      ? t('An action outside the chat may or may not have gone through. Check it and confirm the result.', '会话之外的某个操作可能已执行，也可能没有。请核实并确认结果。')
      : t('An action outside the chat may or may not have gone through. The Bot’s maintainer needs to confirm it.', '会话之外的某个操作可能已执行，也可能没有，需要 Bot 维护者确认。'))
    : look.detail?.[zh ? 1 : 0] ?? '';
  // The usage (U5): always once completed; for any other state only when the run did use something, so a cost is never hidden.
  const usage = task.state === 'completed' || task.usage ? usageText(task.usage, zh) : '';
  return <section className="team-bot-run" data-state={task.state} data-tone={look.tone} aria-label={t('Task status', '任务状态')}>
    <p className="tw-run-state" role="status">
      <span className="tw-run-glyph"><Icon name={look.icon} className={look.spins && !stopping ? 'tw-run-spin' : undefined} />{look.spins && !stopping ? <Icon name="dotsCircle" className="tw-run-still" /> : null}</span>
      <span>{label}</span>
    </p>
    {detail ? <p className="tw-run-detail">{detail}</p> : null}
    {usage ? <p className="tw-run-detail" title={tokenSplit(task.usage) || undefined}>{usage}</p> : null}
    {canReply || (canCancel && !stopping) ? <div className="teams-row-actions">
      {canReply ? <button type="button" aria-label={t('Reply to the Bot’s question', '回复 Bot 的问题')} onClick={() => onReply!(task)}>{t('Reply', '回复')}</button> : null}
      {canCancel && !stopping ? <button type="button" disabled={cancelling} aria-label={t('Cancel task', '取消任务')} onClick={() => void cancel()}>{t('Cancel', '取消')}</button> : null}
    </div> : null}
    {unchecked && (maintains || isMember(task.requester)) ? <div className="teams-row-actions"><button type="button" onClick={() => setReviewing(true)}>{maintains ? t('Review actions', '核实操作') : t('View actions', '查看操作')}</button></div> : null}
    {reviewing ? <OperationsDialog api={api} scope={scope} task={task} zh={zh} canResolve={maintains} people={snapshot} onClose={() => setReviewing(false)} onChanged={onChanged} /> : null}
    {error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}
    {task.artifactId ? <ArtifactButton api={api} scope={scope} artifactId={task.artifactId} zh={zh} renderMarkdown={renderMarkdown} people={snapshot} /> : null}
  </section>;
}
