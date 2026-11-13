import { useRef, useState } from 'react';
import { executionCommand, type ExecutionScope, type MemberExecutionCommand } from './execution.js';
import { messageErrorText } from './errors.js';
import type { TeamsApi } from './index.js';
import type { ListApproval, ListApprovalChange, TeamMessage } from './messages.js';

/** One proposed change in words. `item` is the item as it read when the Bot asked; an item that was gone by then has no text to show. */
export function approvalChangeText(change: ListApprovalChange, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  const item = change.item ? `“${change.item}”` : t('an item that is no longer on the list', '一个已不在清单中的条目');
  if (change.op === 'add') return t(`Add “${change.text ?? ''}”`, `添加“${change.text ?? ''}”`);
  if (change.op === 'remove') return t(`Remove ${item}`, `移除${item}`);
  const done = change.done === undefined ? '' : change.done ? t('mark it as done', '标记为已完成') : t('mark it as not done', '标记为未完成');
  if (change.text !== undefined) return t(`Change ${item} to “${change.text}”${done ? ` and ${done}` : ''}`, `将${item}改为“${change.text}”${done ? `并${done}` : ''}`);
  return change.done ? t(`Mark ${item} as done`, `将${item}标记为已完成`) : t(`Mark ${item} as not done`, `将${item}标记为未完成`);
}

/** The Bot's request to change its list, under the message that carries it. While the task waits, only the member it works for gets the two buttons;
 * everyone else sees who is deciding. Afterwards the card says how it ended. Nothing is saved to the list before an approval. */
export function ApprovalCard({ api, scope, conversationId, message, approval, state, decider, zh, onChanged }: {
  api: TeamsApi; scope: ExecutionScope; conversationId: string; message: TeamMessage; approval: ListApproval;
  /** `decide`: this viewer may decide now. `waiting`: someone else may. Otherwise how it ended, or `closed` when it ended without a decision. */
  state: 'decide' | 'waiting' | 'approved' | 'declined' | 'closed';
  /** Who decides, or decided. */
  decider: string; zh: boolean;
  onChanged?(): void;
}) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [sending, setSending] = useState<'approve' | 'deny'>(), [error, setError] = useState('');
  // One command per decision: a retry after a lost answer repeats the same ids, so the reply is never posted twice.
  const commands = useRef(new Map<string, Extract<MemberExecutionCommand, { action: 'decideWait' }>>());
  const decide = async (decision: 'approve' | 'deny') => {
    if (sending || !message.waitId) return;
    let command = commands.current.get(decision);
    if (!command) { command = executionCommand(scope, 'decideWait', { conversationId, waitId: message.waitId, messageId: crypto.randomUUID(), changeId: crypto.randomUUID(), decision }); commands.current.set(decision, command); }
    setSending(decision); setError('');
    try { await api.messages(command); }
    catch (e) { setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); }
    finally { setSending(undefined); onChanged?.(); }
  };
  const failure = ['WAIT_CLOSED', 'WAIT_REQUIRED', 'WAIT_KIND_MISMATCH'].includes(error) ? t('This request is no longer waiting for a decision.', '此请求已不再等待决定。')
    : error === 'WAIT_RECIPIENT_REQUIRED' ? t('Only the person who asked the Bot can decide this.', '只有向 Bot 提出请求的人才能决定。') : error ? messageErrorText(error, zh) : '';
  return <section className="team-approval" aria-label={t('Proposed list change', '清单修改请求')}>
    <p><b>{t(`Change the list “${approval.title}”`, `修改清单“${approval.title}”`)}</b></p>
    <ul>{approval.changes.map((change, index) => <li key={index}>{approvalChangeText(change, zh)}</li>)}</ul>
    {state === 'decide' ? <>
      <p className="teams-hint">{t('Nothing is changed unless you approve. If the list has changed since the Bot asked, the change is not saved.', '只有你批准后才会修改。如果清单在 Bot 提出请求后已有变动，此修改不会保存。')}</p>
      <div className="teams-row-actions">
        <button type="button" disabled={!!sending} onClick={() => void decide('approve')}>{sending === 'approve' ? t('Approving…', '正在批准…') : t('Approve', '批准')}</button>
        <button type="button" disabled={!!sending} onClick={() => void decide('deny')}>{sending === 'deny' ? t('Declining…', '正在拒绝…') : t('Decline', '拒绝')}</button>
      </div></> : <p className="teams-hint" role="status">{state === 'waiting' ? t(`Waiting for ${decider} to approve or decline.`, `等待 ${decider} 批准或拒绝。`)
        : state === 'approved' ? t(`Approved by ${decider}.`, `${decider} 已批准。`)
        : state === 'declined' ? t(`Declined by ${decider}. Nothing was changed.`, `${decider} 已拒绝，未作修改。`)
        : t('This request ended without a decision. Nothing was changed.', '此请求未经决定即已结束，未作修改。')}</p>}
    {failure ? <p role="alert">{failure}</p> : null}
  </section>;
}
