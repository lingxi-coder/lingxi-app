import { useRef, useState } from 'react';
import { executionCommand, type ExecutionScope, type MemberExecutionCommand } from './execution.js';
import { messageErrorText } from './errors.js';
import type { TeamsApi } from './index.js';
import type { ListApproval, ListApprovalChange, TeamMessage } from './messages.js';
import { teamsText, useTeamsText } from './i18n.js';

/** One proposed change in words. `item` is the item as it read when the Bot asked; an item that was gone by then has no text to show. */
export function approvalChangeText(change: ListApprovalChange, locale: string): string {
  const t = teamsText(locale);
  const item = change.item ? `“${change.item}”` : t('teams_approval_missing_item');
  if (change.op === 'add') return t('teams_approval_add_fmt', `“${change.text ?? ''}”`);
  if (change.op === 'remove') return t('teams_approval_remove_fmt', item);
  if (change.text !== undefined) return t(change.done === undefined ? 'teams_approval_change_fmt' : change.done ? 'teams_approval_change_done_fmt' : 'teams_approval_change_undone_fmt', item, `“${change.text}”`);
  return change.done ? t('teams_approval_done_fmt', item) : t('teams_approval_undone_fmt', item);
}

/** The Bot's request to change its list, under the message that carries it. While the task waits, only the member it works for gets the two buttons;
 * everyone else sees who is deciding. Afterwards the card says how it ended. Nothing is saved to the list before an approval. */
export function ApprovalCard({ api, scope, conversationId, message, approval, state, decider, locale, onChanged }: {
  api: TeamsApi; scope: ExecutionScope; conversationId: string; message: TeamMessage; approval: ListApproval;
  /** `decide`: this viewer may decide now. `waiting`: someone else may. Otherwise how it ended, or `closed` when it ended without a decision. */
  state: 'decide' | 'waiting' | 'approved' | 'declined' | 'closed';
  /** Who decides, or decided. */
  decider: string; locale: string;
  onChanged?(): void;
}) {
  const t = useTeamsText(locale);
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
  const failure = ['WAIT_CLOSED', 'WAIT_REQUIRED', 'WAIT_KIND_MISMATCH'].includes(error) ? t('tw_approval_not_waiting')
    : error === 'WAIT_RECIPIENT_REQUIRED' ? t('tw_approval_requester_only') : error ? messageErrorText(error, locale) : '';
  return <section className="team-approval" aria-label={t('tw_approval_title')}>
    <p><b>{t('teams_approval_title_fmt', approval.title)}</b></p>
    <ul>{approval.changes.map((change, index) => <li key={index}>{approvalChangeText(change, locale)}</li>)}</ul>
    {state === 'decide' ? <>
      <p className="teams-hint">{t('teams_approval_hint')}</p>
      <div className="teams-row-actions">
        <button type="button" disabled={!!sending} onClick={() => void decide('approve')}>{sending === 'approve' ? t('tw_approval_approving') : t('teams_approval_approve')}</button>
        <button type="button" disabled={!!sending} onClick={() => void decide('deny')}>{sending === 'deny' ? t('tw_approval_declining') : t('teams_approval_decline')}</button>
      </div></> : <p className="teams-hint" role="status">{state === 'waiting' ? t('teams_approval_waiting_member_fmt', decider)
        : state === 'approved' ? t('teams_approval_approved_by_fmt', decider)
        : state === 'declined' ? t('teams_approval_declined_by_fmt', decider)
        : t('teams_approval_closed')}</p>}
    {failure ? <p role="alert">{failure}</p> : null}
  </section>;
}
