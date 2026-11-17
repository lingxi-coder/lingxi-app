import { useEffect, useState, type ReactElement } from 'react';
import { friendsErrorText } from './errors.js';
import { classifyQuery, NOTE_MAX, relationAction, type LookupResult, type Proof } from './friends.js';
import type { Confirmed, FriendsWorkspace } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { Avatar, Dialog, type RenderAvatar } from './ui';
import { teamsText, useTeamsText } from './i18n.js';

export const nowFriends = (name: string, locale: string): string => teamsText(locale)('friends_now_friends_fmt', name);
export const requestSentTo = (name: string, locale: string): string => teamsText(locale)('friends_request_sent_to_fmt', name);

/**
 * The result of a lookup (D6): the person and the one action their relation allows. Adding sends the note (never for a
 * link) and asks `onDone` to take over once the request is sent or the friendship made.
 */
export function LookupCard({ workspace, locale, proof, result, renderAvatar, onDone }: {
  workspace: FriendsWorkspace; locale: string; proof: Proof; result: LookupResult; renderAvatar?: RenderAvatar;
  /** The confirmation went through (`done`), or the chat of a friend was opened (nothing). */
  onDone(done?: Exclude<Confirmed, { error: string }>): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const [note, setNote] = useState(''), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const action = relationAction(result.relation);
  const confirm = async () => {
    setBusy(true); setError('');
    const outcome = await workspace.confirm(proof, result, note);
    setBusy(false);
    if ('error' in outcome) setError(outcome.error); else onDone(outcome);
  };
  return <div className="tw-lookup" role="group" aria-label={result.displayName}>
    <div className="tw-lookup-who">
      <Avatar identity={{ kind: 'user', id: result.uid, name: result.displayName }} size={40} render={renderAvatar} />
      <div className="tw-person-text"><b>{result.displayName}</b>{result.handle ? <small>@{result.handle}</small> : null}</div>
    </div>
    {action === 'add' && proof.kind !== 'code' ? <label className="tw-lookup-note">
      <span className="tw-sr">{t('friends_note_placeholder')}</span>
      <textarea rows={2} maxLength={NOTE_MAX} placeholder={t('friends_note_placeholder')} value={note} onChange={event => setNote(event.target.value)} />
    </label> : null}
    <div className="tw-lookup-actions">
      {action === 'friends' ? <>
        <span className="tw-hint">{t('friends_relation_friends')}</span>
        <button type="button" className="tw-primary" onClick={() => { workspace.openChat(result.uid); onDone(); }}>{t('friends_open_chat')}</button>
      </> : action === 'sent' ? <button type="button" disabled>{t('friends_request_sent')}</button>
        : <button type="button" className="tw-primary" disabled={busy} onClick={() => void confirm()}>{action === 'accept' ? t('friends_request_accept') : t('friends_add')}</button>}
    </div>
    {error ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
  </div>;
}

/** Middle-truncated on one line; the whole link stays selectable and copied. */
function LinkText({ link }: { link: string }): ReactElement {
  const cut = Math.max(8, link.length - 12);
  return <span className="tw-link-text" title={link}><span className="tw-link-head">{link.slice(0, cut)}</span><span className="tw-link-tail">{link.slice(cut)}</span></span>;
}

/** D6 step 1: my friend link, with copy, share, reset and turn off. */
function MyLink({ workspace, snapshot, locale }: Pick<FriendsViewProps, 'workspace' | 'snapshot' | 'locale'>): ReactElement {
  const t = useTeamsText(locale);
  const { profile } = snapshot, code = profile?.friendCode;
  const [copied, setCopied] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [ask, setAsk] = useState<'reset' | 'off'>();
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  const run = async (action: () => Promise<string>) => {
    setBusy(true); setError('');
    const failed = await action();
    setBusy(false); setError(failed); setAsk(undefined);
  };
  const share = typeof navigator !== 'undefined' && typeof navigator.share === 'function';
  return <section className="tw-friends-card" aria-labelledby="tw-friends-link">
    <h3 id="tw-friends-link">{t('friends_link_title')}</h3>
    {snapshot.profileLoading && !profile ? <p className="tw-hint" role="status">{t('teams_loading')}</p>
      : !profile ? <div className="tw-form">
        <p role="alert" className="tw-error">{friendsErrorText(snapshot.profileError, locale)}</p>
        <button type="button" onClick={() => void workspace.refreshProfile()}>{t('common_retry')}</button>
      </div>
      : code ? <>
        <LinkText link={code.link} />
        <div className="tw-lookup-actions">
          <button type="button" onClick={() => void navigator.clipboard?.writeText(code.link).then(() => setCopied(true), () => undefined)}>{t('friends_link_copy')}</button>
          {share ? <button type="button" onClick={() => void navigator.share({ text: `${t('friends_link_share_message')} ${code.link}` }).catch(() => undefined)}>{t('friends_link_share')}</button> : null}
          <button type="button" disabled={busy} onClick={() => setAsk('reset')}>{t('friends_link_reset')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => setAsk('off')}>{t('friends_link_off')}</button>
          {copied ? <span className="tw-hint" role="status">{t('friends_link_copied')}</span> : null}
        </div>
      </> : <div className="tw-lookup-actions">
        <span className="tw-hint">{t('friends_link_is_off')}</span>
        <button type="button" disabled={busy} onClick={() => void run(workspace.rotateCode)}>{t('friends_link_create')}</button>
      </div>}
    {error ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
    <p className="tw-hint">{t('friends_link_disclosure')}</p>
    {ask ? <Dialog role="alertdialog" title={ask === 'reset' ? t('friends_link_reset_title') : t('friends_link_off_title')}
      description={ask === 'reset' ? t('friends_link_reset_body') : t('friends_link_off_body')}
      onClose={() => setAsk(undefined)}>
      <div className="tw-form">
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setAsk(undefined)}>{t('common_cancel')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void run(ask === 'reset' ? workspace.rotateCode : workspace.revokeCode)}>{ask === 'reset' ? t('friends_link_reset') : t('friends_link_off')}</button>
        </div>
      </div>
    </Dialog> : null}
  </section>;
}

/** The Add friend page (D6): my link, then one field for a @username, an email or a friend link. */
export function AddFriendView({ workspace, snapshot, locale, renderAvatar }: FriendsViewProps): ReactElement {
  const t = useTeamsText(locale);
  const [text, setText] = useState('');
  const [found, setFound] = useState<{ proof: Proof; result: LookupResult }>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [sent, setSent] = useState('');
  // D2: every open of this page reads the profile again.
  useEffect(() => { void workspace.refreshProfile(); }, [workspace]);
  const query = classifyQuery(text);
  const edit = (value: string) => { setText(value); setFound(undefined); setError(''); setSent(''); };
  const look = async () => {
    if (query.kind === 'invalid' || busy) return;
    setBusy(true); setError(''); setFound(undefined); setSent('');
    const outcome = await workspace.lookup(query);
    setBusy(false);
    if ('error' in outcome) setError(outcome.error); else setFound({ proof: query, result: outcome.result });
  };
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-add">
    <h2 id="tw-friends-add">{t('friends_add')}</h2>
    <MyLink workspace={workspace} snapshot={snapshot} locale={locale} />
    <section className="tw-friends-card" aria-labelledby="tw-friends-find">
      <h3 id="tw-friends-find">{t('friends_search_title')}</h3>
      <form className="tw-friends-find" onSubmit={event => { event.preventDefault(); void look(); }}>
        <input type="text" aria-label={t('friends_search_title')} placeholder={t('friends_search_placeholder')}
          autoCapitalize="off" autoCorrect="off" spellCheck={false} value={text} onChange={event => edit(event.target.value)} />
        <button type="submit" className="tw-primary" disabled={query.kind === 'invalid' || busy}>{t('friends_search_action')}</button>
      </form>
      {text.trim() && query.kind === 'invalid' ? <p className="tw-hint">{t('friends_search_hint')}</p> : null}
      {error ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
      {found ? <LookupCard key={found.result.uid} workspace={workspace} locale={locale} proof={found.proof} result={found.result} renderAvatar={renderAvatar}
        onDone={done => { setFound(undefined); setText(''); if (done?.done === 'sent') setSent(requestSentTo(done.name, locale)); }} /> : null}
      {sent ? <p role="status" className="tw-hint">{sent}</p> : null}
    </section>
  </section>;
}
