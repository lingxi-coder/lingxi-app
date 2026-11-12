import { useState } from 'react';
import { executionCommand, type ExecutionScope, type TeamArtifact, type TeamTaskView, type TaskUsage } from './execution.js';
import { pendingPayload } from './messages.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog, type RenderMarkdown } from './ui';
import type { TeamsSnapshot } from './workspace.js';

export function usageText(usage: TaskUsage | undefined, zh: boolean): string {
  if (!usage || usage.status === 'missing') return zh ? '用量缺失 · 费用未知' : 'Missing usage · cost unknown';
  const prefix = usage.status === 'partial' ? (zh ? '用量不完整' : 'Partial usage') : (zh ? '完整用量' : 'Complete usage');
  const cost = usage.costMicros === undefined ? (zh ? '费用未知' : 'cost unknown') : `${(usage.costMicros / 1_000_000).toFixed(6)} ${usage.currency ?? (zh ? '币种未知' : 'currency unknown')}`.trim();
  return `${prefix} · ${usage.inputTokens} / ${usage.outputTokens} tokens · ${cost}`;
}
export function ArtifactButton({ api, scope, artifactId, zh, renderMarkdown }: { api: TeamsApi; scope: ExecutionScope; artifactId: string; zh: boolean; renderMarkdown?: RenderMarkdown }) {
  const [artifact, setArtifact] = useState<TeamArtifact>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const open = async () => {
    if (busy) return; setBusy(true); setError(''); setArtifact(undefined);
    try { setArtifact(await api.messages<TeamArtifact>(executionCommand(scope, 'readArtifact', { artifactId }))); }
    catch (e) { setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); }
    finally { setBusy(false); }
  };
  return <><button type="button" disabled={busy} onClick={() => void open()}>{busy ? (zh ? '正在打开…' : 'Opening…') : (zh ? '重新打开文档' : 'Reopen document')}</button>{error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}{artifact ? <Dialog title={artifact.document.title} onClose={() => setArtifact(undefined)}><article className="tw-artifact"><p className="tw-hint">{zh ? '版本' : 'Revision'} {artifact.revision}</p>{renderMarkdown ? <div className="tw-md">{renderMarkdown(artifact.document.body)}</div> : <div style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{artifact.document.body}</div>}<button type="button" onClick={() => setArtifact(undefined)}>{zh ? '关闭' : 'Close'}</button></article></Dialog> : null}</>;
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
const CANCELLABLE: TeamTaskView['state'][] = ['queued', 'running', 'waiting'];
type RunViewer = Pick<TeamsSnapshot, 'uid' | 'bots' | 'directory' | 'pending'>;
export function BotRun({ task, api, scope, zh, renderMarkdown, snapshot, onReply, onChanged }: {
  task: TeamTaskView; api: TeamsApi; scope: ExecutionScope; zh: boolean; renderMarkdown?: RenderMarkdown; snapshot: RunViewer;
  /** Start answering the Bot's question; absent where the composer is not at hand. */
  onReply?(task: TeamTaskView): void;
  /** A command changed the task: fetch it again. */
  onChanged?(): void;
}) {
  const [cancelling, setCancelling] = useState(false), [error, setError] = useState('');
  const labels: Record<TeamTaskView['state'], [string, string]> = {
    queued: ['Queued', '已排队'], running: ['Processing', '处理中'], completed: ['Completed', '已完成'], failed: ['Failed', '失败'], expired: ['Expired', '已过期'], interrupted: ['Interrupted', '已中断'], authorization_required: ['Authorization required', '需要授权'], revoked: ['Access revoked', '授权已撤销'], waiting: ['Waiting for a reply', '等待回复'], cancelled: ['Cancelled', '已取消'],
  };
  const t = (en: string, cn: string) => zh ? cn : en;
  const nameOf = (uid: string) => snapshot.directory.find(member => member.uid === uid)?.displayName || t('Member', '成员');
  const isMember = (who: { uid: string; generation: number }) => who.uid === snapshot.uid && who.generation === scope.generation;
  const maintainer = snapshot.bots.find(bot => bot.sharedBotId === task.sharedBotId)?.maintainer;
  const canCancel = CANCELLABLE.includes(task.state) && (isMember(task.requester) || (!!maintainer && isMember(maintainer)));
  const stopping = task.state === 'running' && task.cancelRequestedAt !== undefined;
  const wait = task.state === 'waiting' ? task.wait : undefined;
  const canReply = !!wait && !!onReply && isMember(wait.recipient)
    && !snapshot.pending.some(item => { const payload = pendingPayload(item); return 'waitId' in payload && payload.waitId === wait.waitId; });
  const cancel = async () => {
    if (cancelling) return; setCancelling(true); setError('');
    try { await api.messages(executionCommand(scope, 'cancelTask', { taskId: task.taskId })); }
    catch (e) { const code = e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'; if (code !== 'TASK_FINISHED') setError(code); }
    finally { setCancelling(false); onChanged?.(); }
  };
  return <section className="team-bot-run" aria-label={t('Task status', '任务状态')}>
    <p role="status">{stopping ? t('Cancelling…', '正在取消…') : labels[task.state][zh ? 1 : 0]}</p>
    {wait ? <p className="teams-hint">{isMember(wait.recipient) ? t('Waiting for you', '等待你回复') : t(`Waiting for ${nameOf(wait.recipient.uid)}`, `等待 ${nameOf(wait.recipient.uid)} 回复`)} · {zh ? `${expiresIn(wait.expiresAt, Date.now(), zh)}过期` : `expires ${expiresIn(wait.expiresAt, Date.now(), zh)}`}</p> : null}
    {task.state === 'cancelled' && !task.usage ? null : <p className="teams-hint">{usageText(task.usage, zh)}</p>}
    {canReply || (canCancel && !stopping) ? <div className="teams-row-actions">
      {canReply ? <button type="button" aria-label={t('Reply to the Bot’s question', '回复 Bot 的问题')} onClick={() => onReply!(task)}>{t('Reply', '回复')}</button> : null}
      {canCancel && !stopping ? <button type="button" disabled={cancelling} aria-label={t('Cancel task', '取消任务')} onClick={() => void cancel()}>{t('Cancel', '取消')}</button> : null}
    </div> : null}
    {error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}
    {task.artifactId ? <ArtifactButton api={api} scope={scope} artifactId={task.artifactId} zh={zh} renderMarkdown={renderMarkdown} /> : null}
  </section>;
}
