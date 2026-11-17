import { useEffect, useId, useRef, useState, type ChangeEvent, type ClipboardEvent, type DragEvent, type KeyboardEvent, type ReactElement } from 'react';
import { ATTACHMENT_ACCEPT, type FileRefusal } from './attachments.js';
import { DraftFiles } from './MessageFiles';
import { BotMemberDetails, botAvailabilityText } from './BotMemberDetails';
import { messageErrorText } from './errors.js';
import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { botDmHandedOver, byteLength, canSendText, mentionAt, mentionCandidates, MESSAGE_BYTES, sendsOnEnter, snippet, withoutMention } from './compose.js';
import { ANSWER_REFUSED, type TeamConversation, type TeamMessage } from './messages.js';
import { Avatar, Icon, SLOT, useIsoLayoutEffect, withSlot, type RenderAvatar } from './ui';
import type { PickedFile, TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { intlTag, teamsText, useTeamsText } from './i18n.js';

/** Bots one message can ask. The server accepts at most this many targets. */
export const MAX_TARGETS = 8;

/**
 * Why the viewer cannot write in a conversation, or an empty string when they can. A Bot's owner
 * who is not a participant only observes. A Bot dm needs its Bot to be usable: until the Bot list
 * is known (or when it could not be fetched) sending waits rather than guessing.
 */
export function composerLock(input: { uid: string; conversation: Pick<TeamConversation, 'kind' | 'participants' | 'readers'>; bots: readonly SharedBotCard[]; botsLoaded: boolean; botsUnverified: boolean }, locale: string): string {
  const t = teamsText(locale);
  const { conversation } = input;
  if (conversation.participants.length && !conversation.participants.includes(`user:${input.uid}`)) {
    return t('teams_composer_lock_owner_reading');
  }
  if (botDmHandedOver(conversation, input.bots)) {
    return t('tw_composer_handed_over');
  }
  if (conversation.kind === 'bot_dm' && !input.bots.some(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`))) {
    if (input.botsUnverified) return t('teams_composer_lock_bot_unverified');
    if (!input.botsLoaded) return t('teams_composer_lock_bot_checking');
    return t('teams_composer_lock_bot_unavailable');
  }
  return '';
}

export interface ComposerProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  conversation: TeamConversation;
  /** What to call the conversation, for the placeholder. */
  label: string;
  locale: string;
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
 *
 * Files are added with the paperclip, by pasting or by dropping them on the box. Each starts uploading at once for this
 * conversation and shows as a chip; the message can be sent once every file has arrived, and may be files alone.
 */
export function Composer({ workspace, snapshot, conversation, label, locale, renderAvatar, editing, reply, answering, focusToken, onClearEditing, onClearReply, onClearAnswering, onAnswered, onEditLast }: ComposerProps): ReactElement {
  const t = useTeamsText(locale);
  const listId = useId();
  const input = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState(() => workspace.draft(conversation.id));
  const [targets, setTargets] = useState<string[]>([]);
  const [caret, setCaret] = useState(0), [active, setActive] = useState(0), [dismissed, setDismissed] = useState(-1);
  const picker = useRef<HTMLInputElement>(null);
  const [refused, setRefused] = useState<{ name: string; code: FileRefusal }[]>([]);
  // Files belong to a new message: an edit and an answer to a Bot's question carry none.
  const files = editing || answering ? [] : snapshot.draftFiles;
  const ready = files.filter(file => file.state === 'ready').map(file => file.uploadId), filesSettled = ready.length === files.length;
  const saved = useRef<string>();
  const caretTo = useRef<number>();
  const latest = useRef(text);
  latest.current = text;
  const lock = composerLock({ uid: snapshot.uid, conversation, bots: snapshot.bots, botsLoaded: snapshot.botsLoaded, botsUnverified: snapshot.botsUnverified }, locale);
  const asks = !editing && !answering && (conversation.kind === 'group' || conversation.kind === 'channel');
  const mention = asks ? mentionAt(text, caret) : null;
  const candidates = mention && mention.start !== dismissed && targets.length < MAX_TARGETS ? mentionCandidates(snapshot.bots, conversation, targets, mention.query) : [];
  const open = candidates.length > 0;
  const choice = Math.min(active, Math.max(0, candidates.length - 1));
  const askable = asks && snapshot.bots.some(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`));
  const dmBot = conversation.kind === 'bot_dm' ? snapshot.bots.find(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`)) : undefined;
  const contextBots = dmBot ? [dmBot] : snapshot.bots.filter(bot => targets.includes(bot.sharedBotId));
  const requestedBots = dmBot ? [dmBot.sharedBotId] : targets;
  const unavailable = !editing && !answering && (requestedBots.some(id => snapshot.blockedBotIds.includes(id)) || snapshot.bots.some(bot => requestedBots.includes(bot.sharedBotId) && (bot.state === 'paused' || ['paused', 'offline', 'maintenance', 'quota_exhausted'].includes(bot.availability ?? ''))));
  const bytes = byteLength(text);
  const botName = (key: string) => snapshot.bots.find(bot => bot.sharedBotId === key)?.name ?? t('teams_bot_tag');

  // Observe each example intent, including identical text after a local edit or after the DM mounted.
  useEffect(() => {
    const draft = workspace.draft(conversation.id);
    if (!editing && !answering && draft !== latest.current) { setText(draft); caretTo.current = draft.length; input.current?.focus(); }
  }, [workspace, conversation.id, snapshot.draftText, snapshot.draftFillRevision, editing?.id, answering?.waitId]);

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

  const add = (chosen: readonly PickedFile[]) => { if (chosen.length && !editing && !answering && !lock) setRefused(workspace.addFiles(conversation.id, chosen)); };
  const paste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const pasted = [...event.clipboardData.files];
    if (pasted.length && !editing && !answering) { event.preventDefault(); add(pasted); }
  };
  const drop = (event: DragEvent<HTMLFormElement>) => {
    if (!event.dataTransfer.files.length) return;
    event.preventDefault(); add([...event.dataTransfer.files]);
  };
  const sendable = (canSendText(text) || (ready.length > 0 && !text.trim())) && filesSettled && byteLength(text) <= MESSAGE_BYTES;

  const send = async () => {
    if (lock || unavailable || !sendable || snapshot.messageBusy) return;
    const body = text, replied = reply?.message, changing = editing, answered = answering?.waitId, carried = ready;
    const ok = await workspace.run(async () => {
      const model = snapshot.messaging;
      if (!model) return;
      if (answered) { await model.answer(answered, body); return; }
      if (!changing && targets.some(key => !snapshot.bots.some(bot => bot.sharedBotId === key && sharedBotUsable(bot)))) throw new Error('BOT_USE_REQUIRED');
      await model.queue(changing ? 'edit' : 'create', body, changing?.id, replied?.id, changing ? [] : targets, carried);
    });
    if (!ok) {
      // A refused answer ends answer mode; the text stays in the box.
      if (answered && ANSWER_REFUSED.includes(workspace.getSnapshot().actionError)) onClearAnswering();
      return;
    }
    if (answered) { onClearAnswering(); onAnswered(); }
    if (changing) onClearEditing();
    else {
      setTargets([]); setRefused([]);
      workspace.sentFiles(conversation.id, carried);
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

  // After a handover the Bot answers in a new chat, which its new maintainer can read: one step from here.
  const moved = botDmHandedOver(conversation, snapshot.bots) ? snapshot.bots.find(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`)) : undefined;
  if (lock) return <div className="tw-composer tw-composer-locked"><p className="tw-readonly" role="status">{lock}</p>
    {moved ? <button type="button" disabled={snapshot.messageBusy} onClick={() => void workspace.openBotDm(moved)}>{t('tw_composer_new_chat')}</button> : null}</div>;
  return <form className="tw-composer" onSubmit={event => { event.preventDefault(); void send(); }} onDragOver={event => { if (event.dataTransfer.types.includes('Files')) event.preventDefault(); }} onDrop={drop}>
    {contextBots.map(bot => <div key={bot.sharedBotId}>{!dmBot ? <b>{bot.name}</b> : null}<BotMemberDetails bot={bot} locale={locale} busy={snapshot.messageBusy || !!editing || !!answering} onExample={dmBot ? example => workspace.fillExample(conversation.id, example) : undefined} /></div>)}
    {editing ? <div className="tw-context"><Icon name="edit" /><span>{t('teams_editing_message')}</span><button type="button" className="tw-link" onClick={onClearEditing}>{t('common_cancel')}</button></div> : null}
    {reply ? <div className="tw-context"><Icon name="reply" /><span className="tw-context-text">{withSlot(t('teams_replying_to_fmt', SLOT), <b>{reply.author}</b>)}: {snippet(reply.message.recalled ? t('tw_composer_recalled') : reply.message.text)}</span>
      <button type="button" className="tw-icon-btn" aria-label={t('teams_cancel_reply')} onClick={onClearReply}><Icon name="x" /></button></div> : null}
    {answering ? <div className="tw-context"><Icon name="reply" /><span className="tw-context-text">{withSlot(t('tw_composer_answering_fmt', SLOT), <b>{answering.author}</b>)}{answering.question ? <>: {snippet(answering.question.text)}</> : null}</span>
      <button type="button" className="tw-icon-btn" aria-label={t('teams_cancel_reply')} onClick={onClearAnswering}><Icon name="x" /></button></div> : null}
    {targets.length && !answering ? <ul className="tw-targets" aria-label={t('teams_composer_bots_asked')}>
      {targets.map(key => <li key={key} className="tw-target">@{botName(key)}
        <button type="button" className="tw-icon-btn" aria-label={t('teams_composer_remove_bot_fmt', botName(key))} onClick={() => setTargets(old => old.filter(value => value !== key))}><Icon name="x" size={12} /></button></li>)}
    </ul> : null}
    <DraftFiles files={files} locale={locale} onRemove={uploadId => workspace.removeFile(conversation.id, uploadId)} />
    {refused.length ? <ul className="tw-file-refusals" role="alert">
      {refused.map((item, index) => <li key={`${item.name}:${index}`}><b>{item.name}</b>: {messageErrorText(item.code, locale)}</li>)}
      <li><button type="button" className="tw-link" onClick={() => setRefused([])}>{t('tw_dismiss')}</button></li>
    </ul> : null}
    <div className="tw-compose-box">
      {open ? <ul id={listId} role="listbox" className="tw-mentions" aria-label={t('teams_composer_bots_can_ask')}>
        {candidates.map((bot, index) => <li key={bot.sharedBotId} id={`${listId}-${index}`} role="option" aria-selected={index === choice} className="tw-mention"
          onMouseDown={event => { event.preventDefault(); pick(bot); }}>
          <Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={24} render={renderAvatar} />
          <b>{bot.name}</b><small>{botAvailabilityText(bot, locale)}</small>
        </li>)}
      </ul> : null}
      <textarea ref={input} rows={1} value={text} onChange={change} onPaste={paste} onSelect={event => setCaret(event.currentTarget.selectionStart)} onKeyDown={keys}
        aria-label={t('tw_composer_message')} aria-haspopup={asks ? 'listbox' : undefined} aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? `${listId}-${choice}` : undefined} aria-autocomplete={asks ? 'list' : undefined}
        placeholder={editing ? t('teams_composer_edit_hint') : answering ? t('tw_composer_reply_to_fmt', answering.author) : t('teams_composer_hint_fmt', `${conversation.kind === 'channel' ? '#' : ''}${label}`)} />
      <div className="tw-compose-tools">
        {!editing && !answering ? <>
          <input ref={picker} type="file" multiple hidden accept={ATTACHMENT_ACCEPT} aria-hidden="true" tabIndex={-1}
            onChange={event => { add([...(event.target.files ?? [])]); event.target.value = ''; }} />
          <button type="button" className="tw-icon-btn" aria-label={t('teams_file_add')} title={t('tw_composer_attach')} onClick={() => picker.current?.click()}><Icon name="clip" /></button>
        </> : null}
        {asks ? <button type="button" className="tw-icon-btn" aria-label={t('teams_composer_ask_bot')} title={t('teams_composer_ask_bot')} disabled={!askable || targets.length >= MAX_TARGETS} onClick={insertAt}><Icon name="at" /></button> : null}
        {bytes > MESSAGE_BYTES * 0.9 ? <span className="tw-count" data-over={bytes > MESSAGE_BYTES} role="status">{bytes.toLocaleString(intlTag(locale))} / {MESSAGE_BYTES.toLocaleString(intlTag(locale))}</span> : null}
        {editing ? <button type="button" onClick={onClearEditing}>{t('common_cancel')}</button> : null}
        <button type="submit" className="tw-primary tw-send" disabled={snapshot.messageBusy || unavailable || !sendable} aria-label={editing ? t('teams_message_save') : t('teams_message_send')}>
          {editing ? t('common_save') : <Icon name="send" />}
        </button>
      </div>
    </div>
    <p className="tw-hint">
      {unavailable ? `${t('tw_composer_unavailable')} ` : ''}
      {files.length && !filesSettled ? `${files.some(file => file.state === 'failed') ? t('teams_file_note_failed') : t('teams_file_note_uploading')} ` : ''}
      {answering ? t('tw_composer_reply_note')
        : conversation.kind === 'bot_dm' ? t('teams_composer_note_bot_dm')
        : askable && !editing ? t('teams_composer_note_group')
        : t('tw_composer_keys_hint')}
      {snapshot.messagingConnecting ? ` ${t('tw_composer_connecting')}`
        : snapshot.offline ? ` ${t('teams_composer_note_offline')}` : ''}
    </p>
  </form>;
}
