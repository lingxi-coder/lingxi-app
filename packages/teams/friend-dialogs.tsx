import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactElement } from 'react';
import { LookupCard, nowFriends, requestSentTo } from './AddFriendView';
import { friendsErrorText, offerErrorText } from './errors.js';
import { friendCollator, invitePicker, type LookupResult, type Proof } from './friends.js';
import type { FriendsWorkspace } from './friends-workspace.js';
import type { TeamMember, TeamSummary } from './index.js';
import { Avatar, Dialog, type RenderAvatar } from './ui';
import { intlTag, useTeamsText } from './i18n.js';

function useDialogActivity(workspace: FriendsWorkspace, key: string) {
  const instance = useMemo(() => ({ active: false }), [workspace, key]);
  useLayoutEffect(() => {
    instance.active = true;
    return () => { instance.active = false; };
  }, [instance]);
  return instance;
}

/** Runs one lookup when the dialog opens. `undefined` while it runs. */
function useLookup(workspace: FriendsWorkspace, proof: Proof): { result: LookupResult } | { error: string } | undefined {
  const [outcome, setOutcome] = useState<{
    workspace: FriendsWorkspace; key: string; value: { result: LookupResult } | { error: string }
  }>();
  const key = JSON.stringify(proof);
  useEffect(() => {
    let live = true;
    setOutcome(undefined);
    void workspace.lookup(proof).then(value => { if (live) setOutcome({ workspace, key, value }); });
    return () => { live = false; };
  // The proof is read once per key.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, key]);
  return outcome?.workspace === workspace && outcome.key === key ? outcome.value : undefined;
}

/**
 * D8: a friend link was opened. It is looked up and the person is asked before anything is redeemed; the code is
 * dropped when the dialog closes, whatever came of it.
 */
export function FriendLinkDialog({ workspace, locale, code, onClose }: { workspace: FriendsWorkspace; locale: string; code: string; onClose(): void }): ReactElement {
  const t = useTeamsText(locale);
  const proof: Proof = { kind: 'code', code };
  const activity = useDialogActivity(workspace, code);
  const close = () => { activity.active = false; onClose(); };
  const found = useLookup(workspace, proof);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const result = found && 'result' in found ? found.result : undefined;
  const confirm = async () => {
    if (!result) return;
    setBusy(true); setError('');
    const outcome = await workspace.confirm(proof, result, '', () => activity.active);
    if (!activity.active) return;
    setBusy(false);
    if ('error' in outcome) setError(outcome.error); else close();
  };
  const cancel = <button type="button" data-autofocus onClick={close}>{t('common_cancel')}</button>;
  return <Dialog title={result && result.relation !== 'friends' ? t('friends_confirm_title_fmt', result.displayName) : result ? result.displayName : t('friends_add')} onClose={close}>
    <div className="tw-form">
      {!found ? <p className="tw-hint" role="status">{t('teams_loading')}</p>
        : 'error' in found ? <>
          <p role="alert" className="tw-error">{found.error === 'NOT_FOUND' ? t('friends_link_invalid') : friendsErrorText(found.error, locale)}</p>
          <div className="tw-dialog-actions">{cancel}</div>
        </>
        : result!.relation === 'friends' ? <>
          <p>{t('friends_relation_friends')}</p>
          <div className="tw-dialog-actions">{cancel}<button type="button" className="tw-primary" onClick={() => { workspace.openChat(result!.uid); close(); }}>{t('friends_open_chat')}</button></div>
        </> : <>
          <p>{t('friends_confirm_link_body')}</p>
          {error ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
          <div className="tw-dialog-actions">{cancel}<button type="button" className="tw-primary" disabled={busy} onClick={() => void confirm()}>{t('friends_add')}</button></div>
        </>}
    </div>
  </Dialog>;
}

/** D15: Add friend on a Team member. The result card is the one of the Add friend page; a member who is already a friend says so. */
export function MemberFriendDialog({ workspace, locale, teamId, member, renderAvatar, onClose }: {
  workspace: FriendsWorkspace; locale: string; teamId: string; member: Pick<TeamMember, 'uid' | 'displayName'>; renderAvatar?: RenderAvatar; onClose(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const proof: Proof = { kind: 'team', teamId, uid: member.uid };
  const key = JSON.stringify(proof);
  const activity = useDialogActivity(workspace, key);
  const close = () => { activity.active = false; onClose(); };
  const found = useLookup(workspace, proof);
  const [done, setDone] = useState('');
  return <Dialog title={t('friends_add')} onClose={close}>
    <div className="tw-form">
      {done ? <p role="status">{done}</p>
        : !found ? <p className="tw-hint" role="status">{t('teams_loading')}</p>
        : 'error' in found ? <p role="alert" className="tw-error">{friendsErrorText(found.error, locale)}</p>
        : <LookupCard key={key} workspace={workspace} locale={locale} proof={proof} result={found.result} renderAvatar={renderAvatar} stillActive={() => activity.active}
          onDone={outcome => outcome ? setDone(outcome.done === 'sent' ? requestSentTo(outcome.name, locale) : nowFriends(outcome.name, locale)) : close()} />}
      <div className="tw-dialog-actions"><button type="button" data-autofocus={done ? '' : undefined} onClick={close}>{done ? t('common_close') : t('common_cancel')}</button></div>
    </div>
  </Dialog>;
}

/**
 * D10: a friend's profile, from the chat's menu or from the friend's row: the picture, the username and since when the two are friends.
 * The friend's profile is read again while it is open, so a name or username that changed shows up in this session.
 */
export function FriendProfileDialog({ workspace, uid, name, handle, since, locale, renderAvatar, onClose }: {
  workspace: FriendsWorkspace; uid: string; name: string; handle?: string; since?: number; locale: string; renderAvatar?: RenderAvatar; onClose(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  useEffect(() => { void workspace.refreshFriend(uid); }, [workspace, uid]);
  const date = since ? new Intl.DateTimeFormat(intlTag(locale), { dateStyle: 'medium' }).format(since) : '';
  return <Dialog title={name} onClose={onClose}>
    <div className="tw-form">
      <div className="tw-profile">
        <Avatar identity={{ kind: 'user', id: uid, name }} size={56} render={renderAvatar} />
        <div className="tw-person-text">
          {handle ? <p>@{handle}</p> : null}
          {date ? <p className="tw-hint">{t('friends_since_fmt', date)}</p> : null}
        </div>
      </div>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={onClose}>{t('common_close')}</button></div>
    </div>
  </Dialog>;
}

/**
 * D10: Remove friend and Block ask once. The title names the person, the body says what follows, Cancel is always there and the
 * confirming button is the danger one. A failure is shown under the body and the dialog stays; success closes it.
 */
export function FriendLeaveDialog({ workspace, locale, uid, teamId, name, action, onClose }: {
  workspace: FriendsWorkspace; locale: string; uid: string; teamId?: string; name: string; action: 'remove' | 'block'; onClose(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [version] = useState(teamId);
  const leave = async () => {
    setBusy(true); setError('');
    const code = await workspace.leave(uid, action, version);
    setBusy(false); setError(code);
    if (!code) onClose();
  };
  const remove = action === 'remove';
  return <Dialog role="alertdialog" title={remove ? t('friends_remove_title_fmt', name) : t('friends_block_title_fmt', name)}
    description={remove
      ? t('friends_remove_body')
      : t('friends_block_body')}
    onClose={onClose}>
    <div className="tw-form">
      {error ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" data-autofocus onClick={onClose}>{t('common_cancel')}</button>
        <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void leave()}>{remove ? t('friends_menu_remove') : t('friends_menu_block')}</button>
      </div>
    </div>
  </Dialog>;
}

/**
 * D26: invite friends to a team. The list is the friends who are not in the team, in the Friends order, narrowed by the filter; Invite
 * sends an offer for that row and the row then says `Invited` until the dialog closes. Closing reports the names invited, for the
 * one-shot {@link InviteNotice}: nothing is stored, because the sender cannot read offers.
 */
export function InviteFriendsDialog({ workspace, team, members, locale, renderAvatar, refreshMembers, onClose }: {
  workspace: FriendsWorkspace; team: Pick<TeamSummary, 'id' | 'name'>; members: readonly Pick<TeamMember, 'uid'>[]; locale: string; renderAvatar?: RenderAvatar;
  /** A friend turned out to be a member already (`ALREADY_MEMBER`): the team's member list is stale. */
  refreshMembers(): void;
  onClose(invited: string[]): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const snapshot = useSyncExternalStore(workspace.subscribe, workspace.getSnapshot);
  const tag = intlTag(locale);
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
  return <Dialog title={t('friends_invite_title_fmt', team.name)} onClose={close}>
    <div className="tw-form">
      <input type="search" data-autofocus className="tw-friends-filter" aria-label={t('friends_filter_placeholder')} placeholder={t('friends_filter_placeholder')}
        value={query} onChange={event => setQuery(event.target.value)} />
      {snapshot.loading ? <p className="tw-hint" role="status">{t('friends_loading')}</p>
        : snapshot.loadError ? <div className="tw-empty-actions">
          <p role="alert" className="tw-error">{t('friends_load_error')}</p>
          <button type="button" onClick={workspace.retry}>{t('common_retry')}</button>
        </div>
        : !list.length ? <p className="tw-hint" role="status">{query.trim() ? t('friends_filter_empty') : t('friends_invite_empty')}</p>
        : <ul className="tw-people tw-invite-picker" aria-label={t('friends_title')}>
          {list.map(card => {
            const done = sent.some(value => value.uid === card.uid), running = busy.includes(card.uid);
            return <li key={card.uid} aria-busy={running}>
              <Avatar identity={{ kind: 'user', id: card.uid, name: card.displayName }} size={40} render={renderAvatar} />
              <div className="tw-person-text">
                <b>{card.displayName}</b>
                {card.handle ? <small>@{card.handle}</small> : null}
                {failed?.uid === card.uid ? <small role="alert" className="tw-error">{offerErrorText('offerTeam', failed.code, locale)}</small> : null}
              </div>
              <div className="tw-person-actions">
                <button type="button" disabled={running || done} onClick={() => void invite(card.uid, card.displayName)}>{done ? t('friends_invite_sent') : t('friends_invite_send')}</button>
              </div>
            </li>;
          })}
        </ul>}
      {failed && !list.some(card => card.uid === failed.uid) ? <p role="alert" className="tw-error">{offerErrorText('offerTeam', failed.code, locale)}</p> : null}
      <p className="tw-hint">{t('friends_invite_disclosure')}</p>
      <div className="tw-dialog-actions"><button type="button" onClick={close}>{t('common_close')}</button></div>
    </div>
  </Dialog>;
}

/**
 * D26: what the person invited, once, after the picker closed. It goes away by itself, like the other notices of the Friends views.
 * The host mounts it outside any workspace view, so it is a scope of its own (the banner reads the scope's colors).
 */
export function InviteNotice({ names, locale, onDone }: { names: string[]; locale: string; onDone(): void }): ReactElement {
  const t = useTeamsText(locale);
  const done = useRef(onDone);
  done.current = onDone;
  useEffect(() => {
    const timer = setTimeout(() => done.current(), 6000);
    return () => clearTimeout(timer);
  }, [names]);
  const who = names.join(t('tw_list_separator'));
  return <p className="tw-scope tw-banner" role="status">{t('friends_invite_sent_to_fmt', who)}</p>;
}
