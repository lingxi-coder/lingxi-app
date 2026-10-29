import { useEffect, useReducer, useRef, useState } from 'react';
import type { TeamDetail, TeamSummary, TeamsApi } from './index';
import { TeamMessaging, type TeamMessage } from './messages';
import { indexedDbMessageStorage } from './message-storage';

export function MessagesPanel({ api, uid, team, members, zh, onClose }: {
  api: TeamsApi; uid: string; team: TeamSummary; members: TeamDetail['members']; zh: boolean; onClose(): void;
}) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const submitting = useRef(false);
  const [, render] = useReducer(value => value + 1, 0);
  const [model, setModel] = useState<TeamMessaging>();
  const [text, setText] = useState(''), [title, setTitle] = useState('');
  const [kind, setKind] = useState<'dm' | 'group' | 'channel'>('dm');
  const [parties, setParties] = useState<string[]>([]);
  const [editing, setEditing] = useState<TeamMessage>(), [reply, setReply] = useState<TeamMessage>();
  const [recall, setRecall] = useState<TeamMessage>();
  const [error, setError] = useState(''), [busy, setBusy] = useState(false), [shown, setShown] = useState(100);
  useEffect(() => {
    const current = new TeamMessaging(api, indexedDbMessageStorage(), uid, { teamId: team.id, generation: team.generation });
    const stop = current.onChanged(render); setModel(current);
    void current.start().catch(() => setError(t('Local message storage is unavailable. Retry after freeing space.', '本地消息存储不可用，请检查空间后重试。')));
    const online = () => void current.refresh(); window.addEventListener('online', online);
    return () => { stop(); current.close(); window.removeEventListener('online', online); };
  // The controller owns one account and one membership generation.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, uid, team.id, team.generation]);
  const errorText = (code: string) => ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)
    ? t('Access changed. Return to members and refresh before continuing.', '访问权限已变化，请返回成员页并刷新。')
    : /STORAGE|Quota|disk/iu.test(code) ? t('Local message storage is unavailable. Check free space and try again.', '本地消息存储不可用，请检查空间后重试。')
    : ['MESSAGE_RECALLED', 'MESSAGE_ID_CONFLICT', 'MESSAGE_AUTHOR_REQUIRED', 'PARTICIPANT_REQUIRED', 'CONVERSATION_REJECTED', 'INVALID_INPUT'].includes(code)
      ? t('This pending change was rejected. Discard it and try again.', '此待发操作未被接受，请丢弃后重试。')
      : t('Could not sync messages. Check your connection and try again.', '消息同步未完成，请检查网络后重试。');
  const selected = model?.selected ?? '';
  useEffect(() => { setEditing(undefined); setReply(undefined); setRecall(undefined); setText(''); setShown(100); }, [selected]);
  const attempt = async (action: () => Promise<unknown>) => {
    if (submitting.current) return; submitting.current = true;
    setBusy(true); setError('');
    try { await action(); } catch (error) { setError(error instanceof Error ? error.message : t('Try again.', '请重试。')); }
    finally { submitting.current = false; setBusy(false); }
  };
  const conversation = model?.state.conversations.find(value => value.id === selected);
  const messages = model?.state.threads[selected]?.messages ?? [];
  const pending = model?.state.outbox.filter(value => value.conversationId === selected) ?? [];
  const name = (id: string) => members.find(member => member.uid === id)?.displayName ?? (id === uid ? t('You', '你') : t('Member', '成员'));
  const submit = () => attempt(async () => {
    if (!model) return;
    await model.queue(editing ? 'edit' : 'create', text, editing?.id, reply?.id);
    setText(value => value === text ? '' : value); setEditing(value => value?.id === editing?.id ? undefined : value); setReply(value => value?.id === reply?.id ? undefined : value);
  });
  return <section className="team-messaging" aria-label={t('Team conversations', '团队会话')}>
    <header className="teams-team-header"><div><h2>{team.name}</h2><p>{t('Conversations · platform readable', '会话 · 平台可读')}</p></div><button onClick={onClose}>{t('Back to members', '返回成员')}</button></header>
    <p role="status" className="teams-hint">{model?.offline ? t('Saved on this device. Reconnect to confirm access and send pending messages.', '当前显示本机缓存。联网后确认权限并发送待发消息。') : t('Connected. Message history is saved on this device.', '已连接。消息历史保存在本机。')}</p>
    {error || model?.error ? <p role="alert" className="teams-error">{errorText(error || model?.error || "")}</p> : null}
    {model?.state.creation ? <p role="status">{t("Creating conversation…", "正在创建会话…")} {model.state.creation.title} {model.state.creation.error ? errorText(model.state.creation.error) : ""} <button disabled={Boolean(model.state.creation.attempted && !model.state.creation.error) || (model.state.creation.leaseUntil ?? 0) > Date.now()} onClick={() => void attempt(() => model.discardCreation())}>{t("Discard", "丢弃")}</button></p> : null}
    <div className="team-message-layout"><aside>
      <button disabled={busy} onClick={() => void model?.refresh()}>{t('Sync messages', '同步消息')}</button>
      <nav aria-label={t('Conversations', '会话')}>{model?.state.conversations.map(value => <button key={value.id} className="team-thread" aria-current={value.id === selected ? 'page' : undefined} onClick={() => model.select(value.id)}><b>{value.title}</b><span>{({ dm: t('Direct', '私聊'), group: t('Group', '群聊'), channel: t('Channel', '频道'), bot_dm: 'Bot' })[value.kind]} · {Math.max(0, value.lastSeq - value.lastReadSeq)} {t('unread', '未读')}</span></button>)}</nav>
      <details><summary>{t('New conversation', '新建会话')}</summary><form onSubmit={event => { event.preventDefault(); void attempt(async () => { await model?.create(kind, title.trim(), [uid, ...parties]); setTitle(''); setParties([]); }); }}>
        <label>{t('Title', '标题')}<input value={title} onChange={event => setTitle(event.target.value)} maxLength={120} required /></label>
        <label>{t('Type', '类型')}<select value={kind} onChange={event => { setKind(event.target.value as typeof kind); setParties([]); }}><option value="dm">{t('Direct message', '私聊')}</option><option value="group">{t('Group', '群聊')}</option><option value="channel">{t('Channel', '频道')}</option></select></label>
        <fieldset><legend>{t('Participants', '参与成员')}</legend>{members.filter(member => member.uid !== uid).map(member => <label key={member.uid} className="team-party"><input type={kind === 'dm' ? 'radio' : 'checkbox'} name="participants" checked={parties.includes(member.uid)} onChange={event => setParties(kind === 'dm' ? [member.uid] : event.target.checked ? [...parties, member.uid] : parties.filter(id => id !== member.uid))} />{member.displayName}</label>)}</fieldset>
        <button disabled={busy || !title.trim() || parties.length === 0 || Boolean(model?.offline)}>{t('Create conversation', '创建会话')}</button>
      </form></details>
    </aside><div className="team-message-detail">
      {conversation ? <><header className="teams-team-header"><h3>{conversation.title}</h3><button disabled={busy || model?.offline} onClick={() => void attempt(() => model!.markRead())}>{t('Mark read', '标为已读')}</button></header>
        {messages.length > shown ? <button onClick={() => setShown(value => value + 100)}>{t('Show earlier messages', '显示更早消息')}</button> : null}
        <ol className="team-messages" aria-label={t('Messages', '消息')}>{messages.slice(-shown).map(message => <li key={message.id} data-message-id={message.id} data-message-seq={message.seq}>
          <header><b>{name(message.author.uid)}</b><time>{message.createdAt ? new Date(message.createdAt).toLocaleString() : ''}</time>{message.editedAt ? <small>{t('Edited', '已编辑')}</small> : null}</header>
          {message.replyTo ? <blockquote>{messages.find(value => value.id === message.replyTo)?.recalled ? t('Recalled message', '已撤回消息') : messages.find(value => value.id === message.replyTo)?.text ?? t('Reply to an earlier message', '回复早前的消息')}</blockquote> : null}
          <p>{message.recalled ? <em>{t('Message recalled', '消息已撤回')}</em> : message.text}</p>
          {!message.recalled ? <div className="teams-row-actions"><button onClick={() => { setReply(message); setEditing(undefined); }}>{t('Reply', '回复')}</button>{message.author.uid === uid && message.author.kind === 'user' ? <><button onClick={() => { setEditing(message); setReply(undefined); setText(message.text); }}>{t('Edit', '编辑')}</button><button className="teams-danger" onClick={() => setRecall(message)}>{t('Recall', '撤回')}</button></> : null}</div> : null}
        </li>)}</ol>
        {pending.map(intent => <div key={intent.changeId} className="team-pending"><small>{(intent.error ? errorText(intent.error) : '') || (intent.attempted ? t('Waiting for server confirmation. Retries keep the same message identity.', '等待服务器确认，重试会沿用同一条消息。') : t('Pending · sends when connected', '待发送 · 联网后发送'))}</small><p>{intent.operation === 'recall' ? t('Recall message', '撤回消息') : intent.text}</p><button disabled={Boolean(intent.attempted && !intent.error) || (intent.leaseUntil ?? 0) > Date.now()} onClick={() => void attempt(() => model!.discard(intent.changeId))}>{t('Discard', '丢弃')}</button></div>)}
        <form className="team-composer" onSubmit={event => { event.preventDefault(); void submit(); }}>
          {editing || reply ? <div>{editing ? t('Editing message', '正在编辑消息') : t('Replying', '正在回复')}<button type="button" onClick={() => { setEditing(undefined); setReply(undefined); setText(''); }}>{t('Cancel', '取消')}</button></div> : null}
          <label>{t('Message', '消息')}<textarea disabled={busy} value={text} onChange={event => setText(event.target.value)} rows={3} maxLength={8192} /></label>
          <button disabled={busy || !text.trim() || new TextEncoder().encode(text).length > 8192}>{editing ? t('Save edit', '保存修改') : t('Send', '发送')}</button>
        </form>
      </> : <p>{t('Choose a conversation or start one with team members.', '选择会话，或与团队成员新建会话。')}</p>}
    </div></div>
    {recall ? <div className="teams-modal"><section role="alertdialog" aria-modal="true" aria-label={t('Recall message?', '撤回消息？')}><h2>{t('Recall this message for everyone?', '为所有人撤回此消息？')}</h2><div><button autoFocus onClick={() => setRecall(undefined)}>{t('Cancel', '取消')}</button><button disabled={busy} className="teams-danger" onClick={() => void attempt(async () => { await model?.queue('recall', '', recall.id); setRecall(undefined); })}>{t('Recall', '撤回')}</button></div></section></div> : null}
  </section>;
}
