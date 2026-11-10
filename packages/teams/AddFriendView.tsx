import { useEffect, useState, type ReactElement } from 'react';
import { friendsErrorText } from './errors.js';
import { classifyQuery, NOTE_MAX, relationAction, type LookupResult, type Proof } from './friends.js';
import type { Confirmed, FriendsWorkspace } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { Avatar, Dialog, type RenderAvatar } from './ui';

export const nowFriends = (name: string, zh: boolean): string => zh ? `你和 ${name} 已成为好友` : `You and ${name} are now friends`;
export const requestSentTo = (name: string, zh: boolean): string => zh ? `已向 ${name} 发送好友请求` : `Friend request sent to ${name}`;

/**
 * The result of a lookup (D6): the person and the one action their relation allows. Adding sends the note (never for a
 * link) and asks `onDone` to take over once the request is sent or the friendship made.
 */
export function LookupCard({ workspace, zh, proof, result, renderAvatar, onDone }: {
  workspace: FriendsWorkspace; zh: boolean; proof: Proof; result: LookupResult; renderAvatar?: RenderAvatar;
  /** The confirmation went through (`done`), or the chat of a friend was opened (nothing). */
  onDone(done?: Exclude<Confirmed, { error: string }>): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
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
      <span className="tw-sr">{t('Add a note (optional)', '添加留言（可选）')}</span>
      <textarea rows={2} maxLength={NOTE_MAX} placeholder={t('Add a note (optional)', '添加留言（可选）')} value={note} onChange={event => setNote(event.target.value)} />
    </label> : null}
    <div className="tw-lookup-actions">
      {action === 'friends' ? <>
        <span className="tw-hint">{t('Already friends', '已是好友')}</span>
        <button type="button" className="tw-primary" onClick={() => { workspace.openChat(result.uid); onDone(); }}>{t('Open chat', '打开聊天')}</button>
      </> : action === 'sent' ? <button type="button" disabled>{t('Request sent', '请求已发送')}</button>
        : <button type="button" className="tw-primary" disabled={busy} onClick={() => void confirm()}>{action === 'accept' ? t('Accept', '接受') : t('Add friend', '添加好友')}</button>}
    </div>
    {error ? <p role="alert" className="tw-error">{friendsErrorText(error, zh)}</p> : null}
  </div>;
}

/** Middle-truncated on one line; the whole link stays selectable and copied. */
function LinkText({ link }: { link: string }): ReactElement {
  const cut = Math.max(8, link.length - 12);
  return <span className="tw-link-text" title={link}><span className="tw-link-head">{link.slice(0, cut)}</span><span className="tw-link-tail">{link.slice(cut)}</span></span>;
}

/** D6 step 1: my friend link, with copy, share, reset and turn off. */
function MyLink({ workspace, snapshot, zh }: Pick<FriendsViewProps, 'workspace' | 'snapshot' | 'zh'>): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
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
    <h3 id="tw-friends-link">{t('My friend link', '我的好友链接')}</h3>
    {snapshot.profileLoading && !profile ? <p className="tw-hint" role="status">{t('Loading…', '正在加载…')}</p>
      : !profile ? <div className="tw-form">
        <p role="alert" className="tw-error">{friendsErrorText(snapshot.profileError, zh)}</p>
        <button type="button" onClick={() => void workspace.refreshProfile()}>{t('Retry', '重试')}</button>
      </div>
      : code ? <>
        <LinkText link={code.link} />
        <div className="tw-lookup-actions">
          <button type="button" onClick={() => void navigator.clipboard?.writeText(code.link).then(() => setCopied(true), () => undefined)}>{t('Copy link', '复制链接')}</button>
          {share ? <button type="button" onClick={() => void navigator.share({ text: `${t('Add me as a friend on LingXi:', '在灵犀上添加我为好友：')} ${code.link}` }).catch(() => undefined)}>{t('Share link', '分享链接')}</button> : null}
          <button type="button" disabled={busy} onClick={() => setAsk('reset')}>{t('Reset link', '重置链接')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => setAsk('off')}>{t('Turn off link', '关闭链接')}</button>
          {copied ? <span className="tw-hint" role="status">{t('Link copied', '链接已复制')}</span> : null}
        </div>
      </> : <div className="tw-lookup-actions">
        <span className="tw-hint">{t('Your friend link is off.', '你的好友链接已关闭。')}</span>
        <button type="button" disabled={busy} onClick={() => void run(workspace.rotateCode)}>{t('Create link', '创建链接')}</button>
      </div>}
    {error ? <p role="alert" className="tw-error">{friendsErrorText(error, zh)}</p> : null}
    <p className="tw-hint">{t('Anyone who opens this link and confirms becomes your friend right away, without your approval. You can reset or turn it off at any time.', '任何人打开此链接并确认后，会立即成为你的好友，无需你批准。你可以随时重置或关闭链接。')}</p>
    {ask ? <Dialog role="alertdialog" title={ask === 'reset' ? t('Reset link', '重置链接') : t('Turn off link', '关闭链接')} onClose={() => setAsk(undefined)}>
      <div className="tw-form">
        <p>{ask === 'reset' ? t('Reset your friend link? The old link stops working immediately.', '重置好友链接？旧链接会立即失效。')
          : t('Turn off your friend link? No one can add you with it until you create a new one.', '关闭好友链接？在你创建新链接之前，任何人都无法通过它添加你。')}</p>
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setAsk(undefined)}>{t('Cancel', '取消')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void run(ask === 'reset' ? workspace.rotateCode : workspace.revokeCode)}>{ask === 'reset' ? t('Reset link', '重置链接') : t('Turn off link', '关闭链接')}</button>
        </div>
      </div>
    </Dialog> : null}
  </section>;
}

/** The Add friend page (D6): my link, then one field for a @username, an email or a friend link. */
export function AddFriendView({ workspace, snapshot, zh, renderAvatar }: FriendsViewProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
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
    <h2 id="tw-friends-add">{t('Add friend', '添加好友')}</h2>
    <MyLink workspace={workspace} snapshot={snapshot} zh={zh} />
    <section className="tw-friends-card" aria-labelledby="tw-friends-find">
      <h3 id="tw-friends-find">{t('Find someone', '查找好友')}</h3>
      <form className="tw-friends-find" onSubmit={event => { event.preventDefault(); void look(); }}>
        <input type="text" aria-label={t('Find someone', '查找好友')} placeholder={t('@username, email or friend link', '@用户名、邮箱或好友链接')}
          autoCapitalize="off" autoCorrect="off" spellCheck={false} value={text} onChange={event => edit(event.target.value)} />
        <button type="submit" className="tw-primary" disabled={query.kind === 'invalid' || busy}>{t('Look up', '查找')}</button>
      </form>
      {text.trim() && query.kind === 'invalid' ? <p className="tw-hint">{t('Enter an exact @username, an email address or a friend link.', '请输入完整的 @用户名、邮箱地址或好友链接。')}</p> : null}
      {error ? <p role="alert" className="tw-error">{friendsErrorText(error, zh)}</p> : null}
      {found ? <LookupCard key={found.result.uid} workspace={workspace} zh={zh} proof={found.proof} result={found.result} renderAvatar={renderAvatar}
        onDone={done => { setFound(undefined); setText(''); if (done?.done === 'sent') setSent(requestSentTo(done.name, zh)); }} /> : null}
      {sent ? <p role="status" className="tw-hint">{sent}</p> : null}
    </section>
  </section>;
}
