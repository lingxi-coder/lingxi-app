import { useCallback, useEffect, useRef, useState, type ReactElement } from 'react';
import { ChatPageHeading } from './ChatTopbar';
import { friendsErrorText } from './errors.js';
import { classifyQuery, NOTE_MAX, relationAction, type LookupResult, type Proof } from './friends.js';
import type { Confirmed, FriendsWorkspace } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { Avatar, Icon, type RenderAvatar } from './ui';
import { teamsText, useTeamsText } from './i18n.js';

export const nowFriends = (name: string, locale: string): string => teamsText(locale)('friends_now_friends_fmt', name);
export const requestSentTo = (name: string, locale: string): string => teamsText(locale)('friends_request_sent_to_fmt', name);

/**
 * The result of a lookup (D6): the person and the one action their relation allows. Adding sends the note (never for a
 * link) and asks `onDone` to take over once the request is sent or the friendship made.
 */
export function LookupCard({ workspace, locale, proof, result, renderAvatar, onDone, onConfirmingChange, stillActive }: {
  workspace: FriendsWorkspace; locale: string; proof: Proof; result: LookupResult; renderAvatar?: RenderAvatar;
  /** The confirmation went through (`done`), or the chat of a friend was opened (nothing). */
  onDone(done?: Exclude<Confirmed, { error: string }>): void;
  onConfirmingChange?(confirming: boolean): void;
  stillActive?(): boolean;
}): ReactElement {
  const t = useTeamsText(locale);
  const [note, setNote] = useState(''), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const action = relationAction(result.relation);
  const confirm = async () => {
    setBusy(true); setError(''); onConfirmingChange?.(true);
    const outcome = await workspace.confirm(proof, result, note, stillActive);
    setBusy(false); onConfirmingChange?.(false);
    if (stillActive && !stillActive()) return;
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

/** The Add friend page (D6): live username search, and exact email or link lookup. */
export function AddFriendView({ workspace, snapshot, locale, renderAvatar }: FriendsViewProps): ReactElement {
  const t = useTeamsText(locale);
  const [text, setText] = useState('');
  const [found, setFound] = useState<{ proof: Proof; result: LookupResult }>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [sent, setSent] = useState('');
  const [matches, setMatches] = useState<LookupResult[]>();
  const [composing, setComposing] = useState(false);
  const [searchedHandle, setSearchedHandle] = useState<string>();
  const cooldownUntil = snapshot.handleSearchRetryUntil ?? 0;
  const [confirming, setConfirming] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const round = useRef(0);
  const request = useRef<AbortController>();
  useEffect(() => {
    return () => { round.current++; request.current?.abort(); };
  }, [workspace]);
  const query = classifyQuery(text);
  const handle = query.kind === 'handle' ? query.handle.toLowerCase() : undefined;
  const edit = (value: string) => {
    setText(value);
    const next = classifyQuery(value);
    if (handle && next.kind === 'handle' && next.handle.toLowerCase() === handle) return;
    round.current++;
    request.current?.abort();
    request.current = undefined;
    setFound(undefined); setMatches(undefined); setSearchedHandle(undefined);
    setBusy(false); setError(''); setSent('');
  };
  const look = useCallback(async (proof: Proof) => {
    const current = ++round.current;
    setBusy(true); setError(''); setFound(undefined); setMatches(undefined); setSent('');
    if (proof.kind === 'handle') {
      const controller = new AbortController();
      request.current?.abort();
      request.current = controller;
      setSearchedHandle(proof.handle.toLowerCase());
      const outcome = await workspace.searchHandles(proof.handle, controller.signal);
      if (request.current === controller) request.current = undefined;
      if (current !== round.current) return;
      if ('error' in outcome) {
        if (outcome.retryAfterMs) setSearchedHandle(undefined);
        else setError(outcome.error);
      } else setMatches(outcome.results);
    } else {
      const outcome = await workspace.lookup(proof);
      if (current !== round.current) return;
      if ('error' in outcome) setError(outcome.error); else setFound({ proof, result: outcome.result });
    }
    setBusy(false);
  }, [workspace]);
  useEffect(() => {
    if (!handle || composing || cooldownUntil || searchedHandle === handle) return;
    const current = round.current;
    const timer = setTimeout(() => {
      if (current === round.current) void look({ kind: 'handle', handle });
    }, 350);
    return () => clearTimeout(timer);
  }, [handle, composing, cooldownUntil, searchedHandle, look]);
  const disableSearch = query.kind === 'invalid' || busy || confirming || (handle !== undefined && ((matches?.length ?? 0) > 0 || cooldownUntil > 0));
  const errorCode = handle && cooldownUntil ? 'RATE_LIMITED' : error;
  const selectionRound = round.current;
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-add">
    <ChatPageHeading id="tw-friends-add" title={t('friends_add')} />
    <section className="tw-friends-discovery" aria-label={t('friends_search_title')}>
      <search>
        <form className="tw-friends-find" onSubmit={event => {
          event.preventDefault();
          if (!composing && !disableSearch) void look(query);
        }}>
          <div className="tw-friend-search-field">
            <Icon name="search" size={17} />
            <input ref={input} type="search" name="query" disabled={confirming} aria-label={t('friends_search_title')} placeholder={t('friends_search_placeholder')}
              autoCapitalize="off" autoCorrect="off" spellCheck={false} value={text} onChange={event => { if (!confirming) edit(event.target.value); }}
              onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)} />
            {text ? <button type="button" className="tw-icon-btn tw-friend-search-clear" disabled={confirming} aria-label={t('friends_search_clear')}
              onClick={() => { edit(''); input.current?.focus(); }}><Icon name="x" /></button> : null}
          </div>
          <button type="submit" className="tw-primary" disabled={disableSearch}>{t('friends_search_action')}</button>
        </form>
      </search>
      {query.kind === 'invalid' ? <p className="tw-hint">{t('friends_search_hint')}</p> : null}
      {errorCode ? <p role="alert" className="tw-error">{friendsErrorText(errorCode, locale)}</p> : null}
      {busy ? <p role="status" className="tw-hint">{t('teams_loading')}</p> : null}
      {!found && matches?.length === 0 ? <p role="status" className="tw-hint">{t('friends_error_not_found')}</p> : null}
      {!found && matches?.length ? <ul className="tw-friend-results" aria-label={t('friends_search_title')}>
        {matches.map(result => {
          const { handle } = result;
          if (!handle) return null;
          return <li key={result.uid}><button type="button" className="tw-lookup-who"
            onClick={() => setFound({ proof: { kind: 'handle', handle, expectedUid: result.uid }, result })}>
            <Avatar identity={{ kind: 'user', id: result.uid, name: result.displayName }} size={40} render={renderAvatar} />
            <span className="tw-person-text"><b>{result.displayName}</b><small>@{handle}</small></span>
            <Icon name="chevronRight" size={16} />
          </button></li>;
        })}
      </ul> : null}
      {found ? <>
        {matches ? <button type="button" disabled={confirming} onClick={() => setFound(undefined)}>{t('friends_search_back')}</button> : null}
        <LookupCard key={found.result.uid} workspace={workspace} locale={locale} proof={found.proof} result={found.result} renderAvatar={renderAvatar}
          stillActive={() => selectionRound === round.current}
          onConfirmingChange={value => { if (selectionRound === round.current) setConfirming(value); }}
          onDone={done => {
            if (selectionRound !== round.current) return;
            edit(''); if (done?.done === 'sent') setSent(requestSentTo(done.name, locale));
          }} />
      </> : null}
      {sent ? <p role="status" className="tw-hint">{sent}</p> : null}
    </section>
  </section>;
}
