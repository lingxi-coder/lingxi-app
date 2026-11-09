import { useState } from 'react';
import { executionCommand, type ExecutionScope, type TeamArtifact, type TeamTaskView, type TaskUsage } from './execution.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog, type RenderMarkdown } from './ui';

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
export function BotRun({ task, api, scope, zh, renderMarkdown }: { task: TeamTaskView; api: TeamsApi; scope: ExecutionScope; zh: boolean; renderMarkdown?: RenderMarkdown }) {
  const labels: Record<TeamTaskView['state'], [string, string]> = {
    queued: ['Queued', '已排队'], running: ['Processing', '处理中'], completed: ['Completed', '已完成'], failed: ['Failed', '失败'], expired: ['Expired', '已过期'], interrupted: ['Interrupted', '已中断'], authorization_required: ['Authorization required', '需要授权'], revoked: ['Access revoked', '授权已撤销'],
  };
  return <section className="team-bot-run" aria-label={zh ? '任务状态' : 'Task status'}><p role="status">{labels[task.state][zh ? 1 : 0]}</p><p className="teams-hint">{usageText(task.usage, zh)}</p>{task.artifactId ? <ArtifactButton api={api} scope={scope} artifactId={task.artifactId} zh={zh} renderMarkdown={renderMarkdown} /> : null}</section>;
}
