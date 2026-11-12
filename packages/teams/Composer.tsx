import { useEffect, useId, useRef, useState, type ChangeEvent, type KeyboardEvent, type ReactElement } from 'react';
import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { byteLength, canSendText, mentionAt, mentionCandidates, MESSAGE_BYTES, sendsOnEnter, snippet, withoutMention } from './compose.js';
import { ANSWER_REFUSED, type TeamConversation, type TeamMessage } from './messages.js';
import { Avatar, Icon, useIsoLayoutEffect, type RenderAvatar } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

/** Bots one message can ask. The server accepts at most this many targets. */
export const MAX_TARGETS = 8;

/**
 * Why the viewer cannot write in a conversation, or an empty string when they can. A Bot's owner
 * who is not a participant only observes. A Bot dm needs its Bot to be usable: until the Bot list
 * is known (or when it could not be fetched) sending waits rather than guessing.
 */
export function composerLock(input: { uid: string; conversation: Pick<TeamConversation, 'kind' | 'participants'>; bots: readonly SharedBotCard[]; botsLoaded: boolean; botsUnverified: boolean }, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  const { conversation } = input;
  if (conversation.participants.length && !conversation.participants.includes(`user:${input.uid}`)) {
    return t('You can read this conversation because you own its Bot, but only participants can send messages.', '你因拥有其中的 Bot 而可阅读此会话，但只有参与者可以发送消息。');
  }
  if (conversation.kind === 'bot_dm' && !input.bots.some(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`))) {
    if (input.botsUnverified) return t('Bot availability could not be verified. Reconnect to check.', '未能确认 Bot 可用状态，请联网后重试。');
    if (!input.botsLoaded) return t('Checking whether this Bot is available…', '正在确认此 Bot 是否可用…');
    return t('This Bot is not available to you right now.', '此 Bot 目前对你不可用。');
  }
  return '';
}

export interface ComposerProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  conversation: TeamConversation;
  /** What to call the conversation, for the placeholder. */
  label: string;
  zh: boolean;
  renderAvatar?: RenderAvatar;
  /** The message being edited; its text is in the box until the edit is saved or cancelled. */
  editing?: TeamMessage;
  /** The message being answered, and who wrote it. */
  reply?: { message: TeamMessage; author: string };
  /** The Bot question being answered: the next message answers `waitId` instead of asking anything. `question` is the Bot's message, once loaded. */
  answering?: { waitId: string; author: string; question?: TeamMessage };
  /** Increase to move focus into the box (after choosing Reply or Edit). */
  focusToken: number;
  onClearEditing(): void;
  onClearReply(): void;
  onClearAnswering(): void;
  /** The answer was accepted: fetch the task again. */
  onAnswered(): void;
  /** Up arrow in an empty box: edit the viewer's newest message. */
  onEditLast(): void;
}

/**
 * The message box. Enter sends and Shift+Enter breaks the line (not while an input method is
 * composing). In a group or channel, typing `@` offers the Bots that can be asked; only Bots chosen
 * there run, and they show as chips. The unsent text is kept per conversation by the workspace.
 */
export function Composer({ workspace, snapshot, conversation, label, zh, renderAvatar, editing, reply, answering, focusToken, onClearEditing, onClearReply, onClearAnswering, onAnswered, onEditLast }: ComposerProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const listId = useId();
  const input = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState(() => workspace.draft(conversation.id));
  const [targets, setTargets] = useState<string[]>([]);
  const [caret, setCaret] = useState(0), [active, setActive] = useState(0), [dismissed, setDismissed] = useState(-1);
  const saved = useRef<string>();
  const caretTo = useRef<number>();
  const latest = useRef(text);
  latest.current = text;
  const lock = composerLock({ uid: snapshot.uid, conversation, bots: snapshot.bots, botsLoaded: snapshot.botsLoaded, botsUnverified: snapshot.botsUnverified }, zh);
  const asks = !editing && !answering && (conversation.kind === 'group' || conversation.kind === 'channel');
  const mention = asks ? mentionAt(text, caret) : null;
  const candidates = mention && mention.start !== dismissed && targets.length < MAX_TARGETS ? mentionCandidates(snapshot.bots, conversation, targets, mention.query) : [];
  const open = candidates.length > 0;
  const choice = Math.min(active, Math.max(0, candidates.length - 1));
  const askable = asks && snapshot.bots.some(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`));
  const bytes = byteLength(text);
  const botName = (key: string) => snapshot.bots.find(bot => bot.sharedBotId === key)?.name ?? 'Bot';

  // An edit borrows the box: the draft is set aside and comes back when the edit ends.
  useEffect(() => {
    if (editing) { saved.current ??= text; setText(editing.text); setTargets([]); }
    else if (saved.current !== undefined) { setText(saved.current); saved.current = undefined; }
  // Only a change of the message being edited is an edit starting or ending.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing?.id]);
  // Answering a Bot's question asks no Bot: chosen targets are set aside.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { if (answering) setTargets([]); }, [answering?.waitId]);
  useEffect(() => {
    const node = input.current;
    if (!focusToken || !node) return;
    node.focus(); node.setSelectionRange(node.value.length, node.value.length);
    setCaret(node.value.length);
  }, [focusToken]);
  useIsoLayoutEffect(() => {
    const node = input.current;
    if (!node) return;
    node.style.height = 'auto';
    node.style.height = `${Math.min(node.scrollHeight, 200)}px`;
    if (caretTo.current !== undefined) { node.setSelectionRange(caretTo.current, caretTo.current); setCaret(caretTo.current); caretTo.current = undefined; }
  }, [text]);

  const change = (event: ChangeEvent<HTMLTextAreaElement>) => {
    const value = event.target.value;
    setText(value); setCaret(event.target.selectionStart); setActive(0); setDismissed(-1);
    if (!editing) workspace.setDraft(conversation.id, value);
  };
  const pick = (bot: SharedBotCard) => {
    if (!mention) return;
    const next = withoutMention(text, mention.start, caret);
    setTargets(old => [...old, bot.sharedBotId]);
    setText(next); caretTo.current = mention.start;
    workspace.setDraft(conversation.id, next);
    input.current?.focus();
  };
  const insertAt = () => {
    const node = input.current;
    if (!node) return;
    const at = node.selectionStart ?? text.length;
    const prefix = at > 0 && !/\s/u.test(text[at - 1]!) ? ' ' : '';
    const next = `${text.slice(0, at)}${prefix}@${text.slice(node.selectionEnd ?? at)}`;
    setText(next); caretTo.current = at + prefix.length + 1; workspace.setDraft(conversation.id, next);
    node.focus();
  };

  const send = async () => {
    if (lock || !canSendText(text) || snapshot.messageBusy) return;
    const body = text, replied = reply?.message, changing = editing, answered = answering?.waitId;
    const ok = await workspace.run(async () => {
      const model = snapshot.messaging;
      if (!model) return;
      if (answered) { await model.answer(answered, body); return; }
      if (!changing && targets.some(key => !snapshot.bots.some(bot => bot.sharedBotId === key && sharedBotUsable(bot)))) throw new Error('BOT_USE_REQUIRED');
      await model.queue(changing ? 'edit' : 'create', body, changing?.id, replied?.id, changing ? [] : targets);
    });
    if (!ok) {
      // A refused answer ends answer mode; the text stays in the box.
      if (answered && ANSWER_REFUSED.includes(workspace.getSnapshot().actionError)) onClearAnswering();
      return;
    }
    if (answered) { onClearAnswering(); onAnswered(); }
    if (changing) onClearEditing();
    else {
      setTargets([]);
      // Text typed while the message was being queued stays.
      if (latest.current === body) { setText(''); workspace.setDraft(conversation.id, ''); }
    }
    if (replied) onClearReply();
  };
  const keys = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    const composing = event.nativeEvent.isComposing || event.keyCode === 229;
    if (open && !composing) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setActive((choice + (event.key === 'ArrowDown' ? 1 : candidates.length - 1)) % candidates.length); return; }
      if (event.key === 'Enter' || event.key === 'Tab') { event.preventDefault(); pick(candidates[choice]!); return; }
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setDismissed(mention!.start); return; }
    }
    if (sendsOnEnter({ key: event.key, shiftKey: event.shiftKey, isComposing: event.nativeEvent.isComposing, keyCode: event.keyCode })) { event.preventDefault(); void send(); return; }
    if (event.key === 'Escape' && (editing || reply || answering)) { event.preventDefault(); if (editing) onClearEditing(); else if (reply) onClearReply(); else onClearAnswering(); return; }
    if (event.key === 'ArrowUp' && !text && !editing && !composing) { event.preventDefault(); onEditLast(); }
  };

  if (lock) return <div className="tw-composer tw-composer-locked"><p className="tw-readonly" role="status">{lock}</p></div>;
  return <form className="tw-composer" onSubmit={event => { event.preventDefault(); void send(); }}>
    {editing ? <div className="tw-context"><Icon name="edit" /><span>{t('Editing message', '正在编辑消息')}</span><button type="button" className="tw-link" onClick={onClearEditing}>{t('Cancel', '取消')}</button></div> : null}
    {reply ? <div className="tw-context"><Icon name="reply" /><span className="tw-context-text">{t('Replying to', '回复')} <b>{reply.author}</b>: {snippet(reply.message.recalled ? t('Recalled message', '已撤回消息') : reply.message.text)}</span>
      <button type="button" className="tw-icon-btn" aria-label={t('Cancel reply', '取消回复')} onClick={onClearReply}><Icon name="x" /></button></div> : null}
    {answering ? <div className="tw-context"><Icon name="reply" /><span className="tw-context-text">{t('Replying to', '回复')} <b>{answering.author}</b>{t('’s question', ' 的问题')}{answering.question ? <>: {snippet(answering.question.text)}</> : null}</span>
      <button type="button" className="tw-icon-btn" aria-label={t('Cancel reply', '取消回复')} onClick={onClearAnswering}><Icon name="x" /></button></div> : null}
    {targets.length && !answering ? <ul className="tw-targets" aria-label={t('Bots that will be asked', '将请求的 Bot')}>
      {targets.map(key => <li key={key} className="tw-target">@{botName(key)}
        <button type="button" className="tw-icon-btn" aria-label={t(`Do not ask ${botName(key)}`, `不再请求 ${botName(key)}`)} onClick={() => setTargets(old => old.filter(value => value !== key))}><Icon name="x" size={12} /></button></li>)}
    </ul> : null}
    <div className="tw-compose-box">
      {open ? <ul id={listId} role="listbox" className="tw-mentions" aria-label={t('Bots you can ask', '可请求的 Bot')}>
        {candidates.map((bot, index) => <li key={bot.sharedBotId} id={`${listId}-${index}`} role="option" aria-selected={index === choice} className="tw-mention"
          onMouseDown={event => { event.preventDefault(); pick(bot); }}>
          <Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={20} render={renderAvatar} />
          <b>{bot.name}</b><small>{bot.ready ? t('Online', '在线') : t('Host unavailable', '宿主不可用')}</small>
        </li>)}
      </ul> : null}
      <textarea ref={input} rows={1} value={text} onChange={change} onSelect={event => setCaret(event.currentTarget.selectionStart)} onKeyDown={keys}
        aria-label={t('Message', '消息')} aria-haspopup={asks ? 'listbox' : undefined} aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? `${listId}-${choice}` : undefined} aria-autocomplete={asks ? 'list' : undefined}
        placeholder={editing ? t('Edit your message', '修改你的消息') : answering ? t(`Reply to ${answering.author}`, `回复 ${answering.author}`) : t(`Message ${conversation.kind === 'channel' ? '#' : ''}${label}`, `发送给 ${conversation.kind === 'channel' ? '#' : ''}${label}`)} />
      <div className="tw-compose-tools">
        {asks ? <button type="button" className="tw-icon-btn" aria-label={t('Ask a Bot (@)', '请求 Bot (@)')} title={t('Ask a Bot (@)', '请求 Bot (@)')} disabled={!askable || targets.length >= MAX_TARGETS} onClick={insertAt}><Icon name="at" /></button> : null}
        {bytes > MESSAGE_BYTES * 0.9 ? <span className="tw-count" data-over={bytes > MESSAGE_BYTES} role="status">{bytes.toLocaleString(zh ? 'zh-CN' : 'en-US')} / {MESSAGE_BYTES.toLocaleString(zh ? 'zh-CN' : 'en-US')}</span> : null}
        {editing ? <button type="button" onClick={onClearEditing}>{t('Cancel', '取消')}</button> : null}
        <button type="submit" className="tw-primary tw-send" disabled={snapshot.messageBusy || !canSendText(text)} aria-label={editing ? t('Save edit', '保存修改') : t('Send', '发送')}>
          {editing ? t('Save', '保存') : <Icon name="send" />}
        </button>
      </div>
    </div>
    <p className="tw-hint">
      {answering ? t('Your reply goes to the Bot, which then continues its task.', '你的回复会发给 Bot，由它继续完成任务。')
        : conversation.kind === 'bot_dm' ? t('Messages ask this Bot automatically. Its owner can read this conversation.', '发送消息会请求此 Bot。其所有者可阅读此会话。')
        : askable && !editing ? t('Type @ to ask a Bot. Only Bots chosen from the list run.', '输入 @ 请求 Bot。仅从列表中选择的 Bot 会执行。')
        : t('Enter to send, Shift+Enter for a new line.', '按 Enter 发送，Shift+Enter 换行。')}
      {snapshot.messagingConnecting ? ` ${t('Connecting: messages will send after server confirmation.', '正在连接：服务器确认后发送消息。')}`
        : snapshot.offline ? ` ${t('Offline: messages are saved and sent when you reconnect.', '当前离线：消息将保存并在联网后发送。')}` : ''}
    </p>
  </form>;
}
