import { useState } from 'react';
import { executionCommand, type ExecutionScope, type TeamArtifact, type TeamTaskView, type TaskUsage } from './execution.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog, Icon, type IconName, type RenderMarkdown } from './ui';

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

export function ArtifactButton({ api, scope, artifactId, zh, renderMarkdown }: { api: TeamsApi; scope: ExecutionScope; artifactId: string; zh: boolean; renderMarkdown?: RenderMarkdown }) {
  const [artifact, setArtifact] = useState<TeamArtifact>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const open = async () => {
    if (busy) return; setBusy(true); setError(''); setArtifact(undefined);
    try { setArtifact(await api.messages<TeamArtifact>(executionCommand(scope, 'readArtifact', { artifactId }))); }
    catch (e) { setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); }
    finally { setBusy(false); }
  };
  return <><button type="button" disabled={busy} onClick={() => void open()}>{busy ? (zh ? '正在打开…' : 'Opening…') : (zh ? '打开文档' : 'Open document')}</button>{error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}{artifact ? <Dialog title={artifact.document.title} onClose={() => setArtifact(undefined)}><article className="tw-artifact"><p className="tw-hint">{zh ? `版本 ${artifact.revision}` : `Version ${artifact.revision}`}</p>{renderMarkdown ? <div className="tw-md">{renderMarkdown(artifact.document.body)}</div> : <div style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{artifact.document.body}</div>}<button type="button" onClick={() => setArtifact(undefined)}>{zh ? '关闭' : 'Close'}</button></article></Dialog> : null}</>;
}

/**
 * A Bot task's card (U4): the glyph and the state, one line under it (what is going on or what to do; the usage once it is completed) and
 * at most one action, the document. The glyph is decoration and the state is text, so the state is never colour alone. The Bot's name is the
 * heading of the row the card sits in. A task that is still going has nothing to refresh by hand: the cards watch the service.
 */
export function BotRun({ task, api, scope, zh, renderMarkdown }: { task: TeamTaskView; api: TeamsApi; scope: ExecutionScope; zh: boolean; renderMarkdown?: RenderMarkdown }) {
  const look = runLook(task.state), pick = (pair: [string, string]) => pair[zh ? 1 : 0];
  const detail = task.state === 'completed' ? usageText(task.usage, zh) : look.detail ? pick(look.detail) : '';
  return <section className="team-bot-run" data-state={task.state} data-tone={look.tone} aria-label={zh ? '任务状态' : 'Task status'}>
    <p className="tw-run-state" role="status">
      <span className="tw-run-glyph"><Icon name={look.icon} className={look.spins ? 'tw-run-spin' : undefined} />{look.spins ? <Icon name="dotsCircle" className="tw-run-still" /> : null}</span>
      <span>{pick(look.label)}</span>
    </p>
    {detail ? <p className="tw-run-detail" title={task.state === 'completed' ? tokenSplit(task.usage) || undefined : undefined}>{detail}</p> : null}
    {task.artifactId ? <ArtifactButton api={api} scope={scope} artifactId={task.artifactId} zh={zh} renderMarkdown={renderMarkdown} /> : null}
  </section>;
}
