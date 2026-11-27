import { MAX_MEMBER_MENTIONS, memberMentionAt, mentionReplacementEnd, mentionSearchKey, replaceMentionRanges, shiftMentionEntities, type MentionEditRange, type MentionEntity } from './mentions.js';
import type { TeamMember } from './index.js';
import { ComposerEditor, ComposerSurface, ComposerToolbar } from '../chat-ui/ComposerSurface';
import { useContext, useEffect, useId, useMemo, useRef, useState, type ClipboardEvent, type DragEvent, type KeyboardEvent, type ReactElement } from 'react';
import { ComposerActions, type FormatAction } from './ComposerActions';
import { messageStyles, maxMessageFormats, type MessageFormat } from './message-format.js';
import { editorRange, pasteMessageEditor, placeEditorCaret, readMessageEditor, replaceEditorFormats, restoreEditorRange, writeMessageEditor, type EditorDraft } from './message-editor.js';
import { ChatTopbarTarget } from './ChatTopbar';
import { ATTACHMENT_ACCEPT, PHOTO_ATTACHMENT_ACCEPT, type FileRefusal } from './attachments.js';
import { DraftFiles } from './MessageFiles';
import { BotMemberDetails, botAvailabilityText } from './BotMemberDetails';
import { messageErrorText } from './errors.js';
import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { botDmHandedOver, byteLength, canSendText, mentionCandidates, MESSAGE_BYTES, sendsOnEnter, snippet, withoutMention } from './compose.js';
import { canChannelSend } from './channel-settings';
import { ANSWER_REFUSED, type TeamConversation, type TeamMessage } from './messages.js';
import { Avatar, Icon, SLOT, useIsoLayoutEffect, withSlot, type RenderAvatar } from './ui';
import type { PickedFile, TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { intlTag, teamsText, useTeamsText } from './i18n.js';

/** Bots one message can ask. The server accepts at most this many targets. */
export const MAX_TARGETS = 8;
const MENTION_ROW_HEIGHT = 40;
const MENTION_VIEWPORT_ROWS = 6;
const MENTION_OVERSCAN = 2;

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
 * composing). In a group or channel, typing `@` offers members and Bots. Members appear
 * inline and Bots chosen to run appear as chips. The unsent text is kept per conversation by the workspace.
 *
 * Files are added through the + menu, by pasting or by dropping them on the box. Each starts uploading at once for this
 * conversation and shows as a chip; the message can be sent once every file has arrived, and may be files alone.
 */
export function Composer({ workspace, snapshot, conversation, label, locale, renderAvatar, editing, reply, answering, focusToken, onClearEditing, onClearReply, onClearAnswering, onAnswered, onEditLast }: ComposerProps): ReactElement {
  const t = useTeamsText(locale);
  const listId = useId();
  const hasTopbar = useContext(ChatTopbarTarget) !== undefined;
  const input = useRef<HTMLDivElement>(null);
  const [activeStyles, setActiveStyles] = useState(0);
  const actionSelection = useRef<{ range: Range; styles: number }>();
  const domDraft = useRef<EditorDraft>();
  const list = useRef<HTMLUListElement>(null);
  const beforeInput = useRef<{ text: string; range: MentionEditRange }>();
  const [firstRow, setFirstRow] = useState(0);
  const [text, setText] = useState(() => workspace.draft(conversation.id));
  const [entities, setEntities] = useState(() => workspace.draftMentions(conversation.id));
  const [formats, setFormats] = useState(() => workspace.draftFormats(conversation.id));
  const [targets, setTargets] = useState<string[]>([]);
  const [caret, setCaret] = useState(0), [active, setActive] = useState(0), [dismissed, setDismissed] = useState(-1);
  const picker = useRef<HTMLInputElement>(null), photoPicker = useRef<HTMLInputElement>(null);
  const [refused, setRefused] = useState<{ name: string; code: FileRefusal }[]>([]);
  // Files belong to a new message: an edit and an answer to a Bot's question carry none.
  const files = editing || answering ? [] : snapshot.draftFiles;
  const ready = files.filter(file => file.state === 'ready').map(file => file.uploadId), filesSettled = ready.length === files.length;
  const saved = useRef<{ text: string; entities: MentionEntity[]; formats: MessageFormat[] }>();
  const caretTo = useRef<number>();
  const latest = useRef({ text, entities, formats });
  const postingDenied = !editing && !answering && !canChannelSend(conversation, snapshot.uid, reply?.message);
  const lock = composerLock({ uid: snapshot.uid, conversation, bots: snapshot.bots, botsLoaded: snapshot.botsLoaded, botsUnverified: snapshot.botsUnverified }, locale)
    || (postingDenied ? t('tw_channel_posting_denied') : '');
  const newPost = conversation.kind === 'channel' && conversation.posting?.mode === 'posts' && !reply && !editing && !answering;
  const postLabel = t('tw_channel_new_post');
  const canMention = !answering && (conversation.kind === 'group' || conversation.kind === 'channel');
  const asks = canMention && !editing;
  const mention = canMention ? memberMentionAt(text, caret, entities) : null;
  const participants = useMemo(() => new Set(conversation.participants), [conversation.participants]);
  const roster = useMemo(() => {
    const names = new Map<string, number>();
    const members: { member: TeamMember; key: string }[] = [];
    if (canMention) for (const member of snapshot.directory) {
      if (member.uid === snapshot.uid || !participants.has(`user:${member.uid}`) || /[\r\n]/u.test(member.displayName)) continue;
      const key = mentionSearchKey(member.displayName);
      if (!key) continue;
      members.push({ member, key });
      names.set(key, (names.get(key) ?? 0) + 1);
    }
    return { members, names };
  }, [canMention, participants, snapshot.directory, snapshot.uid]);
  const query = mention && mention.start !== dismissed ? mentionSearchKey(mention.query) : undefined;
  const candidates: ({ member: TeamMember; key: string } | { bot: SharedBotCard })[] = query === undefined ? [] : [
    ...(entities.length < MAX_MEMBER_MENTIONS ? roster.members.filter(member => member.key.includes(query)) : []),
    ...(asks && targets.length < MAX_TARGETS ? mentionCandidates(snapshot.bots, conversation, targets, query).map(bot => ({ bot })) : []),
  ];
  const open = candidates.length > 0;
  const choice = Math.min(active, Math.max(0, candidates.length - 1));
  const first = Math.max(0, Math.min(firstRow, candidates.length - MENTION_VIEWPORT_ROWS) - MENTION_OVERSCAN);
  const end = Math.min(candidates.length, first + MENTION_VIEWPORT_ROWS + MENTION_OVERSCAN * 2);
  const rows = Array.from({ length: end - first }, (_, offset) => first + offset);
  // The active descendant stays in the DOM while the user scrolls with the pointer.
  if (open && choice < first) rows.unshift(choice);
  if (open && choice >= end) rows.push(choice);
  const askable = asks && snapshot.bots.some(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`));
  const dmBot = conversation.kind === 'bot_dm' ? snapshot.bots.find(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`)) : undefined;
  const contextBots = dmBot ? [dmBot] : snapshot.bots.filter(bot => targets.includes(bot.sharedBotId));
  const requestedBots = dmBot ? [dmBot.sharedBotId] : targets;
  // A group or channel takes a message for a Bot whose Desktop is off, and its task waits for the Desktop (rule 6); a Bot DM says no at once.
  const hostWaiting = (id: string) => conversation.kind !== 'bot_dm' && snapshot.bots.some(bot => bot.sharedBotId === id && bot.state !== 'paused' && bot.availability === 'offline');
  const unavailable = !editing && !answering && requestedBots.some(id => !hostWaiting(id) && (snapshot.blockedBotIds.includes(id)
    || snapshot.bots.some(bot => bot.sharedBotId === id && (bot.state === 'paused' || ['paused', 'offline', 'maintenance', 'quota_exhausted'].includes(bot.availability ?? '')))));
  const bytes = byteLength(text);
  const botName = (key: string) => snapshot.bots.find(bot => bot.sharedBotId === key)?.name ?? t('teams_bot_tag');

  // Observe each example intent, including identical text after a local edit or after the DM mounted.
  useEffect(() => {
    const draft = workspace.draft(conversation.id);
    if (!editing && !answering && draft !== latest.current.text) {
      const draftEntities = workspace.draftMentions(conversation.id), draftFormats = workspace.draftFormats(conversation.id);
      latest.current = { text: draft, entities: draftEntities, formats: draftFormats };
      setText(draft); setEntities(draftEntities); setFormats(draftFormats); caretTo.current = draft.length; input.current?.focus();
    }
  }, [workspace, conversation.id, snapshot.draftText, snapshot.draftFillRevision, editing?.id, answering?.waitId]);

  // An edit borrows the box: the draft is set aside and comes back when the edit ends.
  useEffect(() => {
    if (editing) {
      saved.current ??= latest.current;
      latest.current = { text: editing.text, entities: editing.mentionEntities ?? [], formats: editing.formats ?? [] };
      setText(editing.text); setEntities(latest.current.entities); setFormats(latest.current.formats); setTargets([]);
    } else if (saved.current) {
      latest.current = saved.current;
      setText(saved.current.text); setEntities(saved.current.entities); setFormats(saved.current.formats);
      saved.current = undefined;
    }
  // Only a change of the message being edited is an edit starting or ending.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing?.id]);
  // Answering a Bot's question asks no Bot: chosen targets are set aside.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { if (answering) setTargets([]); }, [answering?.waitId]);
  useEffect(() => {
    const node = input.current;
    if (!focusToken || !node) return;
    node.focus();
    caretTo.current = latest.current.text.length;
    // Reply can request focus without changing the draft.
    if (domDraft.current?.text === latest.current.text && domDraft.current.formats === latest.current.formats) {
      const range = node.ownerDocument.createRange();
      range.selectNodeContents(node);
      range.collapse(false);
      restoreEditorRange(node, range);
      setCaret(caretTo.current);
      caretTo.current = undefined;
    }
  }, [focusToken]);
  useIsoLayoutEffect(() => {
    const node = input.current;
    if (!node) { domDraft.current = undefined; actionSelection.current = undefined; return; }
    const next = { text, formats };
    if (domDraft.current?.text !== text || domDraft.current.formats !== formats) {
      writeMessageEditor(node, next);
      actionSelection.current = undefined;
      domDraft.current = next;
    }
    if (caretTo.current !== undefined) {
      placeEditorCaret(node, caretTo.current);
      setCaret(caretTo.current);
      caretTo.current = undefined;
    }
  }, [lock, text, formats]);

  useIsoLayoutEffect(() => {
    const node = list.current;
    if (!node) return;
    const top = choice * MENTION_ROW_HEIGHT;
    const bottom = top + MENTION_ROW_HEIGHT;
    if (top < node.scrollTop) node.scrollTop = top;
    else if (bottom > node.scrollTop + node.clientHeight) node.scrollTop = bottom - node.clientHeight;
    setFirstRow(Math.floor(node.scrollTop / MENTION_ROW_HEIGHT));
  }, [open, choice, query]);

  const update = (value: string, nextEntities: MentionEntity[], nextFormats: MessageFormat[]) => {
    latest.current = { text: value, entities: nextEntities, formats: nextFormats };
    setText(value); setEntities(nextEntities); setFormats(nextFormats);
    if (!editing) workspace.setDraft(conversation.id, value, nextEntities, nextFormats);
  };

  useIsoLayoutEffect(() => {
    const node = input.current;
    if (!node) return;
    const capture = (event: Event) => {
      const target = (event as InputEvent).getTargetRanges?.()[0];
      const { start, end } = readMessageEditor(node, target);
      beforeInput.current = { text: latest.current.text, range: { start, end } };
    };
    const selection = () => {
      if (node.ownerDocument.activeElement !== node || !editorRange(node)) return;
      setCaret(readMessageEditor(node).start);
    };
    node.addEventListener('beforeinput', capture);
    node.ownerDocument.addEventListener('selectionchange', selection);
    return () => {
      node.removeEventListener('beforeinput', capture);
      node.ownerDocument.removeEventListener('selectionchange', selection);
      beforeInput.current = undefined;
    };
  }, [lock]);

  const change = (event?: InputEvent) => {
    const node = input.current;
    if (!node) return;
    const next = readMessageEditor(node);
    if (event) actionSelection.current = undefined;
    const current = latest.current;
    const previous = beforeInput.current;
    beforeInput.current = undefined;
    const range = previous?.text === current.text ? previous.range : undefined;
    const sameText = next.text === current.text;
    const replacement = sameText && range && range.start < range.end && event?.inputType?.startsWith('insert')
      && next.start === range.end && next.end === range.end;
    const nextEntities = replacement ? replaceMentionRanges(current.entities, range.start, range.end, range.end - range.start)
      : sameText ? current.entities : shiftMentionEntities(current.text, next.text, current.entities, range, next.start);
    const sameFormats = next.formats.length === current.formats.length && next.formats.every((run, index) => {
      const old = current.formats[index];
      return old?.offset === run.offset && old.length === run.length && old.style === run.style;
    });
    const nextFormats = sameFormats ? current.formats : next.formats;
    domDraft.current = { text: next.text, formats: nextFormats };
    if (!sameText || nextEntities !== current.entities || !sameFormats) update(next.text, nextEntities, nextFormats);
    setCaret(next.start);
    if (!sameText || replacement) {
      setActive(0);
      setDismissed(replacement ? memberMentionAt(next.text, next.start, nextEntities)?.start ?? -1 : -1);
    }
  };

  const saveActionSelection = (node: HTMLElement) => {
    const range = editorRange(node);
    if (!range) return;
    const styles = (Object.keys(messageStyles) as FormatAction[]).reduce((mask, action) =>
      mask | (node.ownerDocument.queryCommandState(action) ? messageStyles[action] : 0), 0);
    actionSelection.current = { range: range.cloneRange(), styles };
  };
  const openActions = () => {
    const node = input.current;
    if (node && node.ownerDocument.activeElement === node) saveActionSelection(node);
    setActiveStyles(actionSelection.current?.styles ?? 0);
    if (mention) setDismissed(mention.start);
  };
  const applyFormat = (action: FormatAction) => {
    const node = input.current;
    if (!node) return;
    node.focus();
    const range = actionSelection.current?.range;
    actionSelection.current = undefined;
    if (range && node.contains(range.commonAncestorContainer)) restoreEditorRange(node, range);
    else if (!editorRange(node)) placeEditorCaret(node, text.length);
    // The browser applies the format to its selection and includes it in undo/redo.
    node.ownerDocument.execCommand(action);
    change();
  };

  const insertMention = () => {
    const node = input.current;
    if (!node) return;
    const current = latest.current;
    const range = actionSelection.current?.range;
    const { start, end } = range && node.contains(range.commonAncestorContainer)
      ? readMessageEditor(node, range) : { start: current.text.length, end: current.text.length };
    const before = current.text.slice(0, start);
    const token = !before || /\s$/u.test(before) ? '@' : ' @';
    actionSelection.current = undefined;
    caretTo.current = start + token.length;
    setDismissed(-1);
    setActive(0);
    node.focus();
    update(before + token + current.text.slice(end),
      replaceMentionRanges(current.entities, start, end, token.length),
      replaceEditorFormats(current.formats, start, end, token.length));
  };

  const pick = (candidate: typeof candidates[number]) => {
    if (!mention) return;
    if ('bot' in candidate) {
      const end = mentionReplacementEnd(text, mention.start, caret, candidate.bot.name);
      const next = withoutMention(text, mention.start, end);
      const removedEnd = mention.start + text.length - next.length;
      update(next, replaceMentionRanges(entities, mention.start, removedEnd, 0), replaceEditorFormats(formats, mention.start, removedEnd, 0));
      setTargets(old => [...old, candidate.bot.sharedBotId]);
      caretTo.current = mention.start;
    } else {
      const label = `@${candidate.member.displayName.trim()}`;
      const end = mentionReplacementEnd(text, mention.start, caret, candidate.member.displayName);
      const remainder = text.slice(end);
      const separator = /^[\s\p{P}]/u.test(remainder) ? '' : ' ';
      const next = text.slice(0, mention.start) + label + separator + remainder;
      const nextEntities = replaceMentionRanges(entities, mention.start, end, label.length + separator.length);
      nextEntities.push({ userId: candidate.member.uid, offset: mention.start, length: label.length });
      update(next, nextEntities.sort((a, b) => a.offset - b.offset), replaceEditorFormats(formats, mention.start, end, label.length + separator.length));
      caretTo.current = mention.start + label.length + (separator || remainder.startsWith(' ') ? 1 : 0);
    }
    setActive(0);
    input.current?.focus();
  };
  const add = (chosen: readonly PickedFile[]) => { if (chosen.length && !editing && !answering && !lock) setRefused(workspace.addFiles(conversation.id, chosen)); };
  const paste = (event: ClipboardEvent<HTMLDivElement>) => {
    const pasted = [...event.clipboardData.files];
    event.preventDefault();
    if (pasted.length && !editing && !answering) { add(pasted); return; }
    pasteMessageEditor(event.currentTarget, event.clipboardData);
  };
  const drop = (event: DragEvent<HTMLFormElement>) => {
    if (!event.dataTransfer.files.length) return;
    event.preventDefault(); add([...event.dataTransfer.files]);
  };
  const sendable = (canSendText(text) || (ready.length > 0 && !text.trim())) && filesSettled && bytes <= MESSAGE_BYTES && formats.length <= maxMessageFormats;

  const send = async () => {
    if (lock || unavailable || !sendable || snapshot.messageBusy) return;
    const body = text, replied = reply?.message, changing = editing, answered = answering?.waitId, carried = ready;
    const ok = await workspace.run(async () => {
      const model = snapshot.messaging;
      if (!model) return;
      if (answered) { await model.answer(answered, body, formats.length ? formats : undefined); return; }
      if (!changing && targets.some(key => !snapshot.bots.some(bot => bot.sharedBotId === key && sharedBotUsable(bot)))) throw new Error('BOT_USE_REQUIRED');
      const mentions = entities.filter(entity => participants.has(`user:${entity.userId}`));
      await model.queue(changing ? 'edit' : 'create', body, changing?.id, replied?.id, changing ? [] : targets, carried,
        undefined, undefined, mentions.length || changing?.mentionEntities?.length ? mentions : undefined,
        formats.length || changing?.formats?.length ? formats : undefined);
    });
    if (!ok) {
      // A refused answer ends answer mode; the text stays in the box.
      if (answered && ANSWER_REFUSED.includes(workspace.getSnapshot().actionError)) onClearAnswering();
      return;
    }
    if (answered) { onClearAnswering(); onAnswered(); }
    if (changing) onClearEditing();
    else {
      setTargets(current => current === targets ? [] : current); setRefused([]);
      workspace.sentFiles(conversation.id, carried);
      // Changes made while the message was being queued stay.
      if (latest.current.text === body && latest.current.entities === entities && latest.current.formats === formats) update('', [], []);
    }
    if (replied) onClearReply();
  };
  const keys = (event: KeyboardEvent<HTMLDivElement>) => {
    const composing = event.nativeEvent.isComposing || event.keyCode === 229;
    if (open && !composing) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setActive((choice + (event.key === 'ArrowDown' ? 1 : candidates.length - 1)) % candidates.length); return; }
      if ((event.key === 'Enter' || event.key === 'Tab') && !event.shiftKey) { event.preventDefault(); pick(candidates[choice]!); return; }
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setDismissed(mention!.start); return; }
    }
    if (event.key === 'Tab' && !composing) saveActionSelection(event.currentTarget);
    if (sendsOnEnter({ key: event.key, shiftKey: event.shiftKey, isComposing: event.nativeEvent.isComposing, keyCode: event.keyCode })) { event.preventDefault(); void send(); return; }
    if (event.key === 'Escape' && (editing || reply || answering)) { event.preventDefault(); if (editing) onClearEditing(); else if (reply) onClearReply(); else onClearAnswering(); return; }
    if (event.key === 'ArrowUp' && !text && !editing && !composing) { event.preventDefault(); onEditLast(); }
  };

  // After a handover the Bot answers in a new chat, which its new maintainer can read: one step from here.
  const moved = botDmHandedOver(conversation, snapshot.bots) ? snapshot.bots.find(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`)) : undefined;
  if (lock) return <div className="tw-composer tw-composer-locked"><p className="tw-readonly" role="status">{lock}</p>
    {reply && postingDenied ? <button type="button" onClick={onClearReply}>{t('common_cancel')}</button> : null}
    {moved ? <button type="button" disabled={snapshot.messageBusy} onClick={() => void workspace.openBotDm(moved)}>{t('tw_composer_new_chat')}</button> : null}</div>;
  return <form className="tw-composer" onSubmit={event => { event.preventDefault(); void send(); }} onDragOver={event => { if (event.dataTransfer.types.includes('Files')) event.preventDefault(); }} onDrop={drop}>
    {contextBots.map(bot => <div key={bot.sharedBotId}>{!dmBot ? <b>{bot.name}</b> : null}<BotMemberDetails bot={bot} bots={snapshot.bots} locale={locale} conversation={conversation} busy={snapshot.messageBusy || !!editing || !!answering} onExample={dmBot ? example => workspace.fillExample(conversation.id, example) : undefined} /></div>)}
    <ComposerSurface className="tw-compose-box">
      {editing || reply || answering || targets.length > 0 || files.length > 0 || refused.length > 0 ? <div className="tw-compose-context">
        {editing ? <div className="tw-context"><Icon name="edit" /><span>{t('teams_editing_message')}</span></div> : null}
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
      </div> : null}
      {open ? <ul ref={list} id={listId} role="listbox" className="tw-mentions" aria-label={t('teams_composer_mention_options')}
        onScroll={event => setFirstRow(Math.floor(event.currentTarget.scrollTop / MENTION_ROW_HEIGHT))}>
        <li role="presentation" aria-hidden="true" style={{ height: candidates.length * MENTION_ROW_HEIGHT }} />
        {rows.map(index => {
          const candidate = candidates[index]!;
          const identity = 'member' in candidate
            ? { kind: 'user' as const, id: candidate.member.uid, name: candidate.member.displayName }
            : { kind: 'bot' as const, id: candidate.bot.sharedBotId, name: candidate.bot.name };
          const detail = 'bot' in candidate ? botAvailabilityText(candidate.bot, locale)
            : (roster.names.get(candidate.key) ?? 0) > 1 ? identity.id : undefined;
          return <li key={identity.kind + ':' + identity.id} id={`${listId}-${index}`} role="option" aria-selected={index === choice}
            aria-posinset={index + 1} aria-setsize={candidates.length}
            style={{ top: index * MENTION_ROW_HEIGHT, height: MENTION_ROW_HEIGHT }} className="tw-mention" onMouseDown={event => event.preventDefault()} onClick={() => pick(candidate)}>
            <Avatar identity={identity} size={24} render={renderAvatar} />
            <b>{identity.name}</b>{detail ? <small>{detail}</small> : null}
          </li>;
        })}
      </ul> : null}
      <ComposerEditor>
        <div ref={input} className="composer-editor-control tw-message-editor" contentEditable suppressContentEditableWarning
          role="textbox" aria-multiline="true" data-empty={!text}
          onInput={event => change(event.nativeEvent as InputEvent)} onPaste={paste} onKeyDown={keys}
          onDrop={event => { if (!event.dataTransfer.types.includes('Files')) event.preventDefault(); }}
          aria-label={t('tw_composer_message')} aria-haspopup={canMention ? 'listbox' : undefined} aria-controls={open ? listId : undefined}
          aria-activedescendant={open ? `${listId}-${choice}` : undefined} aria-autocomplete={canMention ? 'list' : undefined}
          aria-placeholder={editing ? t('teams_composer_edit_hint') : answering ? t('tw_composer_reply_to_fmt', answering.author) : newPost ? postLabel : t('teams_composer_hint_fmt', `${conversation.kind === 'channel' ? '#' : ''}${label}`)} />
      </ComposerEditor>
      <ComposerToolbar>
        <div className="tw-compose-leading">
          {!editing && !answering ? <>
            {[{ ref: picker, accept: ATTACHMENT_ACCEPT }, { ref: photoPicker, accept: PHOTO_ATTACHMENT_ACCEPT }].map(({ ref, accept }) =>
              <input key={accept} ref={ref} type="file" multiple hidden accept={accept} aria-hidden="true" tabIndex={-1}
                onChange={event => { add([...(event.currentTarget.files ?? [])]); event.currentTarget.value = ''; }} />)}
          </> : null}
          <ComposerActions locale={locale} onOpen={openActions}
            onAttach={editing || answering ? undefined : kind => (kind === 'photos' ? photoPicker : picker).current?.click()}
            onMention={canMention && ((roster.members.length > 0 && entities.length < MAX_MEMBER_MENTIONS)
              || (askable && targets.length < MAX_TARGETS)) ? insertMention : undefined}
            onFormat={applyFormat} activeStyles={activeStyles} />
        </div>
        {bytes > MESSAGE_BYTES * 0.9 ? <span className="tw-count" data-over={bytes > MESSAGE_BYTES} role="status">{bytes.toLocaleString(intlTag(locale))} / {MESSAGE_BYTES.toLocaleString(intlTag(locale))}</span> : null}
        {editing ? <button type="button" onClick={onClearEditing}>{t('common_cancel')}</button> : null}
        <button type="submit" className="tw-primary tw-send" data-post={newPost || undefined} disabled={snapshot.messageBusy || unavailable || !sendable} aria-label={editing ? t('teams_message_save') : newPost ? postLabel : t('teams_message_send')}>
          {editing ? t('common_save') : newPost ? postLabel : <Icon name="arrowUp" />}
        </button>
      </ComposerToolbar>
    </ComposerSurface>
    <p className="tw-hint">
      {formats.length > maxMessageFormats ? `${t('tw_format_limit')} ` : ''}
      {unavailable ? `${t('tw_composer_unavailable')} ` : ''}
      {!editing && !answering ? requestedBots.filter(hostWaiting).map(id => `${t('teams_bot_offline_wait_fmt', botName(id))} `).join('') : ''}
      {editing && conversation.kind !== 'dm' ? `${t('teams_edit_no_notify_hint')} ` : ''}
      {files.length && !filesSettled ? `${files.some(file => file.state === 'failed') ? t('teams_file_note_failed') : t('teams_file_note_uploading')} ` : ''}
      {answering ? t('tw_composer_reply_note')
        : conversation.kind === 'bot_dm' ? t('teams_composer_note_bot_dm')
        : askable && !editing ? t('teams_composer_note_group')
        : t('tw_composer_keys_hint')}
      {hasTopbar ? null : snapshot.messagingConnecting ? ` ${t('tw_composer_connecting')}`
        : snapshot.offline ? ` ${t('teams_composer_note_offline')}` : ''}
    </p>
  </form>;
}
