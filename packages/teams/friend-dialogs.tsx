import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactElement } from 'react';
import { LookupCard, nowFriends, requestSentTo } from './AddFriendView';
import { friendsErrorText, offerErrorText } from './errors.js';
import { friendCollator, invitePicker, type LookupResult, type Proof } from './friends.js';
import type { FriendsWorkspace } from './friends-workspace.js';
import type { TeamMember, TeamSummary } from './index.js';
import { Avatar, Dialog, type RenderAvatar } from './ui';

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

/**
 * D26: invite friends to a team. The list is the friends who are not in the team, in the Friends order, narrowed by the filter; Invite
 * sends an offer for that row and the row then says `Invited` until the dialog closes. Closing reports the names invited, for the
 * one-shot {@link InviteNotice}: nothing is stored, because the sender cannot read offers.
 */
export function InviteFriendsDialog({ workspace, zh, team, members, locale, renderAvatar, refreshMembers, onClose }: {
  workspace: FriendsWorkspace; zh: boolean; team: Pick<TeamSummary, 'id' | 'name'>; members: readonly Pick<TeamMember, 'uid'>[]; locale?: string; renderAvatar?: RenderAvatar;
  /** A friend turned out to be a member already (`ALREADY_MEMBER`): the team's member list is stale. */
  refreshMembers(): void;
  onClose(invited: string[]): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const snapshot = useSyncExternalStore(workspace.subscribe, workspace.getSnapshot);
  const tag = locale ?? (zh ? 'zh-CN' : 'en-US');
  const collator = useMemo(() => friendCollator(tag), [tag]);
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState<string[]>([]);
  const [sent, setSent] = useState<{ uid: string; name: string }[]>([]);
  // People the server says are members already: out of the list even before the team's member list catches up.
  const [joined, setJoined] = useState<string[]>([]);
  const [failed, setFailed] = useState<{ uid: string; code: string }>();
  const list = useMemo(() => invitePicker(snapshot.cards, [...members.map(member => member.uid), ...joined], query, collator), [snapshot.cards, members, joined, query, collator]);
  const invite = async (uid: string, name: string) => {
    setBusy(old => [...old, uid]); setFailed(undefined);
    const code = await workspace.offerTeam(team.id, uid);
    setBusy(old => old.filter(value => value !== uid));
    if (!code) setSent(old => [...old, { uid, name }]);
    else {
      setFailed({ uid, code });
      if (code === 'ALREADY_MEMBER') { setJoined(old => [...old, uid]); refreshMembers(); }
    }
  };
  const close = () => onClose(sent.map(value => value.name));
  return <Dialog title={t(`Invite friends to “${team.name}”`, `邀请好友加入“${team.name}”`)} onClose={close}>
    <div className="tw-form">
      <input type="search" data-autofocus className="tw-friends-filter" aria-label={t('Search friends', '搜索好友')} placeholder={t('Search friends', '搜索好友')}
        value={query} onChange={event => setQuery(event.target.value)} />
      {snapshot.loading ? <p className="tw-hint" role="status">{t('Loading friends…', '正在加载好友…')}</p>
        : snapshot.loadError ? <div className="tw-empty-actions">
          <p role="alert" className="tw-error">{t("Couldn't load your friends. Check your connection and try again.", '无法加载好友，请检查网络后重试。')}</p>
          <button type="button" onClick={workspace.retry}>{t('Retry', '重试')}</button>
        </div>
        : !list.length ? <p className="tw-hint" role="status">{query.trim() ? t('No matching friends', '没有匹配的好友') : t('No friends to invite. Friends who are already in this team are not listed.', '没有可邀请的好友。已在此团队中的好友不会显示。')}</p>
        : <ul className="tw-people tw-invite-picker" aria-label={t('Friends', '好友')}>
          {list.map(card => {
            const done = sent.some(value => value.uid === card.uid), running = busy.includes(card.uid);
            return <li key={card.uid} aria-busy={running}>
              <Avatar identity={{ kind: 'user', id: card.uid, name: card.displayName }} size={32} render={renderAvatar} />
              <div className="tw-person-text">
                <b>{card.displayName}</b>
                {card.handle ? <small>@{card.handle}</small> : null}
                {failed?.uid === card.uid ? <small role="alert" className="tw-error">{offerErrorText('offerTeam', failed.code, zh)}</small> : null}
              </div>
              <div className="tw-person-actions">
                <button type="button" disabled={running || done} onClick={() => void invite(card.uid, card.displayName)}>{done ? t('Invited', '已邀请') : t('Invite', '邀请')}</button>
              </div>
            </li>;
          })}
        </ul>}
      {failed && !list.some(card => card.uid === failed.uid) ? <p role="alert" className="tw-error">{offerErrorText('offerTeam', failed.code, zh)}</p> : null}
      <p className="tw-hint">{t('Your friend gets an invitation they can accept or decline. They see the team’s name and your name, and nothing else is shared until they join. Invitations expire after 7 days.', '好友会收到一份可接受或拒绝的邀请，只会看到团队名称和你的名称，加入之前不会共享其他任何内容。邀请 7 天后过期。')}</p>
      <div className="tw-dialog-actions"><button type="button" onClick={close}>{t('Close', '关闭')}</button></div>
    </div>
  </Dialog>;
}

/** D26: what the person invited, once, after the picker closed. It goes away by itself, like the other notices of the Friends views. */
export function InviteNotice({ names, zh, onDone }: { names: string[]; zh: boolean; onDone(): void }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const done = useRef(onDone);
  done.current = onDone;
  useEffect(() => {
    const timer = setTimeout(() => done.current(), 6000);
    return () => clearTimeout(timer);
  }, [names]);
  const who = names.join(zh ? '、' : ', ');
  return <p className="tw-banner" role="status">{t(`Invitation sent to ${who}`, `已向 ${who} 发送邀请`)}</p>;
}
