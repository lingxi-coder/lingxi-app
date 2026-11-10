import { useEffect, useState, type ReactElement } from 'react';
import { LookupCard, nowFriends, requestSentTo } from './AddFriendView';
import { friendsErrorText } from './errors.js';
import type { LookupResult, Proof } from './friends.js';
import type { FriendsWorkspace } from './friends-workspace.js';
import type { TeamMember } from './index.js';
import { Dialog, type RenderAvatar } from './ui';

/** Runs one lookup when the dialog opens. `undefined` while it runs. */
function useLookup(workspace: FriendsWorkspace, proof: Proof): { result: LookupResult } | { error: string } | undefined {
  const [outcome, setOutcome] = useState<{ result: LookupResult } | { error: string }>();
  const key = JSON.stringify(proof);
  useEffect(() => {
    let live = true;
    setOutcome(undefined);
    void workspace.lookup(proof).then(value => { if (live) setOutcome(value); });
    return () => { live = false; };
  // The proof is read once per key.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, key]);
  return outcome;
}

/**
 * D8: a friend link was opened. It is looked up and the person is asked before anything is redeemed; the code is
 * dropped when the dialog closes, whatever came of it.
 */
export function FriendLinkDialog({ workspace, zh, code, onClose }: { workspace: FriendsWorkspace; zh: boolean; code: string; onClose(): void }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const proof: Proof = { kind: 'code', code };
  const found = useLookup(workspace, proof);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const result = found && 'result' in found ? found.result : undefined;
  const confirm = async () => {
    if (!result) return;
    setBusy(true); setError('');
    const outcome = await workspace.confirm(proof, result, '');
    setBusy(false);
    if ('error' in outcome) setError(outcome.error); else onClose();
  };
  const cancel = <button type="button" data-autofocus onClick={onClose}>{t('Cancel', '取消')}</button>;
  return <Dialog title={result && result.relation !== 'friends' ? t(`Add “${result.displayName}” as a friend?`, `将“${result.displayName}”添加为好友？`) : result ? result.displayName : t('Add friend', '添加好友')} onClose={onClose}>
    <div className="tw-form">
      {!found ? <p className="tw-hint" role="status">{t('Loading…', '正在加载…')}</p>
        : 'error' in found ? <>
          <p role="alert" className="tw-error">{found.error === 'NOT_FOUND' ? t('This friend link no longer works. Ask them for a new one.', '此好友链接已失效，请向对方索取新链接。') : friendsErrorText(found.error, zh)}</p>
          <div className="tw-dialog-actions">{cancel}</div>
        </>
        : result!.relation === 'friends' ? <>
          <p>{t('Already friends', '已是好友')}</p>
          <div className="tw-dialog-actions">{cancel}<button type="button" className="tw-primary" onClick={() => { workspace.openChat(result!.uid); onClose(); }}>{t('Open chat', '打开聊天')}</button></div>
        </> : <>
          <p>{t('You opened a friend link. Adding them makes you friends right away.', '你正在打开好友链接。添加后会立即成为好友。')}</p>
          {error ? <p role="alert" className="tw-error">{friendsErrorText(error, zh)}</p> : null}
          <div className="tw-dialog-actions">{cancel}<button type="button" className="tw-primary" disabled={busy} onClick={() => void confirm()}>{t('Add friend', '添加好友')}</button></div>
        </>}
    </div>
  </Dialog>;
}

/** D15: Add friend on a Team member. The result card is the one of the Add friend page; a member who is already a friend says so. */
export function MemberFriendDialog({ workspace, zh, teamId, member, renderAvatar, onClose }: {
  workspace: FriendsWorkspace; zh: boolean; teamId: string; member: Pick<TeamMember, 'uid' | 'displayName'>; renderAvatar?: RenderAvatar; onClose(): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const proof: Proof = { kind: 'team', teamId, uid: member.uid };
  const found = useLookup(workspace, proof);
  const [done, setDone] = useState('');
  return <Dialog title={t('Add friend', '添加好友')} onClose={onClose}>
    <div className="tw-form">
      {done ? <p role="status">{done}</p>
        : !found ? <p className="tw-hint" role="status">{t('Loading…', '正在加载…')}</p>
        : 'error' in found ? <p role="alert" className="tw-error">{friendsErrorText(found.error, zh)}</p>
        : <LookupCard workspace={workspace} zh={zh} proof={proof} result={found.result} renderAvatar={renderAvatar}
          onDone={outcome => outcome ? setDone(outcome.done === 'sent' ? requestSentTo(outcome.name, zh) : nowFriends(outcome.name, zh)) : onClose()} />}
      <div className="tw-dialog-actions"><button type="button" data-autofocus={done ? '' : undefined} onClick={onClose}>{done ? t('Close', '关闭') : t('Cancel', '取消')}</button></div>
    </div>
  </Dialog>;
}
