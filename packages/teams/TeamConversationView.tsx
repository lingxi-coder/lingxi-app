import {SourceNoticePreview} from '../inbox/SourceNoticePreview';
import type {InboxNoticeTarget} from '../inbox/contracts';
import { durableReadFrontier,observeTranscript,type VisibleReadFrontier } from '../inbox/visibility.js';
import { Fragment, useContext, useEffect, useMemo, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { ChatTopbarHeader, ChatTopbarTarget, TopbarConnection } from './ChatTopbar';
import { useTeamTasks } from './bot-runs.js';
import type { TeamTaskView } from './execution.js';
import { MemberHome } from './MemberHome';
import { conversationShareLink } from './share.js';
import { ShareLinkButton } from './ShareLinkButton';
import { botAvailabilityText } from './BotMemberDetails';
import { snippet } from './compose.js';
import { Composer, composerLock } from './Composer';
import { messageErrorText } from './errors.js';
import { ApprovalCard } from './ApprovalCard';
import { MessageFiles } from './MessageFiles';
import type { MessageAttachment } from './attachments.js';
import { executionCommand } from './execution.js';
import { pendingPayload, type TeamConversation, type TeamMessage } from './messages.js';
import { ArtifactButton, BotRun } from './BotRunCards';
import { AutoReplyDialog, autoReplyBots } from './AutoReplyDialog';
import { buildStreamRows, formatClock, placeTasks, visibleStart, WINDOW_SIZE } from './stream.js';
import { Avatar, ConversationLead, Dialog, Icon, platformChip, platformNotice, useIsoLayoutEffect, type RenderAvatar, type RenderMarkdown } from './ui';
import { conversationLabel, unreadCount, type Identity, type TeamsSnapshot, type TeamsWorkspace } from './workspace.js';
import { intlTag, teamsText, useTeamsText } from './i18n.js';

/**
 * A friend chat (D9) uses a pair team and the friend's profile in its header. A friendship's Bot DM (D22) uses the regular Teams header.
 */
export interface FriendHeader {
  uid: string; name: string; handle?: string;
  /** The right of the header: the friend menu. */
  actions?: ReactNode;
  onOpenProfile?(): void;
}

export interface TeamConversationViewProps {
  notificationTarget?:InboxNoticeTarget;
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  /** Whether the details drawer is open. With `onToggleDetails` the header shows a button for it. */
  detailsOpen?: boolean;
  onToggleDetails?(): void;
  renderAvatar?: RenderAvatar;
  /** Renders a shared Bot's messages and documents as Markdown. People's messages stay plain text. */
  renderMarkdown?: RenderMarkdown;
  /** The app's language, as a code or a BCP 47 tag (`en`, `zh-Hans`, `zh-TW`, `ja`, `ko`): it picks the language of the text and the locale of times and day labels. */
  locale: string;
  /** The current time for "Today" and "Yesterday". Defaults to now. */
  now?: number | Date;
  friend?: FriendHeader;
  /** A Back button at the start of the header: a friendship's Bot DM returns to the friend chat (D22). */
  onBack?(): void;
}

/** The second line of the header: who is in the conversation, or whether its Bot is reachable. */
export function conversationSubtitle(conversation: TeamConversation, snapshot: Pick<TeamsSnapshot, 'bots' | 'directory' | 'uid'>, locale: string): string {
  const t = teamsText(locale);
  if (conversation.kind === 'bot_dm') {
    const card = snapshot.bots.find(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`));
    if (!card) return '';
    const owner = snapshot.directory.find(member => member.uid === card.maintainer.uid)?.displayName || (card.maintainer.uid === snapshot.uid ? t('teams_you') : t('teams_member'));
    const presence = botAvailabilityText(card, locale);
    return `${presence} · ${t('teams_bot_owner_fmt', owner)}`;
  }
  if (conversation.kind === 'dm') return t('teams_label_direct');
  // Participants mix people (`user:`) and shared Bots (`shared_bot:`); only people are members.
  const members = conversation.participants.filter(id => id.startsWith('user:')).length;
  const bots = conversation.participants.filter(id => id.startsWith('shared_bot:')).length;
  const people = members ? t('teams_members_count_fmt', members) : '';
  const botText = bots ? t(bots === 1 ? 'teams_bots_count_one_fmt' : 'teams_bots_count_other_fmt', bots) : '';
  return [people, botText].filter(Boolean).join(' · ');
}

/** What the pane says while no conversation is open, and why. */
function emptyState(snapshot: TeamsSnapshot, locale: string): { title: string; body: string } {
  const t = teamsText(locale);
  if (!snapshot.teams.length) return { title: t('tw_conv_no_team_title'), body: t('tw_conv_no_team_hint') };
  if (!snapshot.team) return { title: t('teams_choose_team'), body: t('tw_conv_pick_team') };
  if (!snapshot.conversations.length) return { title: t('teams_no_conversations'), body: t('tw_conv_start_hint') };
  return { title: t('teams_choose_conversation'), body: t('tw_conv_pick_conversation') };
}

/**
 * The middle column: header, message stream, composer. It shows the workspace's open conversation
 * and is rebuilt (drafts aside) whenever another one is opened.
 */
export function TeamConversationView(props: TeamConversationViewProps): ReactElement {
  const { snapshot, locale,notificationTarget } = props;
  const [dismissed,setDismissed]=useState('');
  const token=notificationTarget?JSON.stringify([notificationTarget.key,notificationTarget.generation,notificationTarget.through,notificationTarget.messageId,notificationTarget.taskId]):'';
  const t = useTeamsText(locale);
  const conversation = snapshot.conversation, model = snapshot.messaging, team = snapshot.team;
  const hasTopbar = useContext(ChatTopbarTarget) !== undefined;
  if (!conversation || !model || !team) {
    const { title, body } = emptyState(snapshot, locale);
    const chrome = hasTopbar ? <ChatTopbarHeader><header className="tw-conv-head">
      <div className="tw-conv-identity"><div className="tw-conv-title"><h2>{team?.name ?? t('teams_title')}</h2></div></div>
      <TopbarConnection snapshot={snapshot} workspace={props.workspace} locale={locale} />
    </header></ChatTopbarHeader> : null;
    if (snapshot.loading) return <>{chrome}<section className="tw-scope tw-conv tw-conv-empty"><p className="tw-empty-line" role="status">{t('teams_loading')}</p></section></>;
    if (team && !snapshot.selectedConversationId && !snapshot.creation) return <>{chrome}<MemberHome key={`${snapshot.uid}:${team.id}:${team.generation}`} workspace={props.workspace} snapshot={snapshot} locale={locale} renderMarkdown={props.renderMarkdown} /></>;
    return <>{chrome}<section className="tw-scope tw-conv tw-conv-empty">
      <div className="tw-empty"><h2>{title}</h2><p>{body}</p><p className="tw-hint">{platformNotice(locale)}</p></div>
      {snapshot.creation ? <CreationBanner workspace={props.workspace} snapshot={snapshot} locale={locale} /> : null}
    </section></>;
  }
  if(notificationTarget && token!==dismissed)return <SourceNoticePreview api={props.workspace.api} kind="team" sourceId={team.id} generation={team.generation} conversationId={conversation.id} locale={locale} target={notificationTarget} onReturn={()=>setDismissed(token)} />;
  return <ConversationPane key={`${snapshot.uid}:${team.id}:${team.generation}:${conversation.id}`} {...props} conversation={conversation} model={model} team={team} />;
}

export function CreationBanner({ workspace, snapshot, locale }: { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; locale: string }): ReactElement | null {
  const creation = snapshot.creation, model = snapshot.messaging;
  if (!creation || !model) return null;
  const t = useTeamsText(locale);
  return <div className="tw-banner" role="status" data-kind="creation">
    <span>{t('tw_conv_creating')} <b>{pendingPayload(creation).title}</b> {creation.error ? messageErrorText(creation.error, locale) : ''}</span>
    <button type="button" className="tw-link" disabled={Boolean(creation.attempted && !creation.error) || (creation.leaseUntil ?? 0) > Date.now()}
      onClick={() => void workspace.attempt(() => model.discardCreation())}>{t('teams_message_discard')}</button>
  </div>;
}

type Pane = TeamConversationViewProps & { conversation: TeamConversation; model: NonNullable<TeamsSnapshot['messaging']>; team: NonNullable<TeamsSnapshot['team']> };

function ConversationPane({ workspace, snapshot, conversation, model, team, detailsOpen, onToggleDetails, renderAvatar, renderMarkdown, locale, now, friend, onBack }: Pane): ReactElement {
  const t = useTeamsText(locale);
  const hasTopbar = useContext(ChatTopbarTarget) !== undefined;
  const uid = snapshot.uid, messages = snapshot.messages, tag = intlTag(locale);
  const minute = Math.floor((now === undefined ? Date.now() : typeof now === 'number' ? now : now.getTime()) / 60_000);
  const [startId, setStartId] = useState<string>();
  const [editing, setEditing] = useState<TeamMessage>(), [reply, setReply] = useState<TeamMessage>(), [recall, setRecall] = useState<TeamMessage>();
  const [takeBack, setTakeBack] = useState<MessageAttachment>();
  const [autoReplyOpen, setAutoReplyOpen] = useState(false);
  const [answerWait, setAnswerWait] = useState<string>();
  const [focusToken, setFocusToken] = useState(0);
  const [peerRead,setPeerRead]=useState<number|null>(null);
  useEffect(()=>{
    if(conversation.kind!=='dm' || snapshot.offline || !workspace.api.peerReceipt) {setPeerRead(null);return;}
    let live=true,run=0;
    const load=()=>{
      const version=++run;
      if(document.visibilityState!=='visible'){setPeerRead(null);return;}
      void workspace.api.peerReceipt!({teamId:team.id,generation:team.generation},conversation.id)
        .then(value=>{if(live && version===run)setPeerRead(value.readThrough);}).catch(()=>{if(live && version===run)setPeerRead(null);});
    };
    load();const timer=setInterval(load,30000);document.addEventListener('visibilitychange',load);
    return ()=>{live=false;clearInterval(timer);document.removeEventListener('visibilitychange',load);};
  },[workspace,conversation.id,conversation.kind,snapshot.offline,team.id,team.generation]);
  const [pinned, setPinned] = useState(true);
  const [seen, setSeen] = useState(() => messages[messages.length - 1]?.seq ?? 0);
  const scroller = useRef<HTMLDivElement>(null), content = useRef<HTMLOListElement>(null);
  const pinnedRef = useRef(true), placed = useRef(false), anchor = useRef<{ height: number; top: number }>(), pendingCount = useRef(snapshot.pending.length);

  const label = friend ? friend.name : conversationLabel(conversation, snapshot.directory, uid, { bots: snapshot.bots, locale });
  const unread = unreadCount(conversation);
  const start = visibleStart(messages, { startId, lastReadSeq: conversation.lastReadSeq, uid });
  const visible = useMemo(() => messages.slice(start), [messages, start]);
  const rows = useMemo(() => buildStreamRows(visible, { lastReadSeq: conversation.lastReadSeq, uid, locale: tag, now: minute * 60_000 }), [visible, conversation.lastReadSeq, uid, tag, minute]);
  const byId = useMemo(() => new Map(messages.map(message => [message.id, message])), [messages]);
  const runs = useTeamTasks(workspace.api, { teamId: team.id, generation: team.generation }, conversation.id, !friend);

  const lock = composerLock({ uid, conversation, bots: snapshot.bots, botsLoaded: snapshot.botsLoaded, botsUnverified: snapshot.botsUnverified }, locale);
  const participant = !conversation.participants.length || conversation.participants.includes(`user:${uid}`);
  const nameOf = (id: string) => snapshot.directory.find(member => member.uid === id)?.displayName || (id === uid ? t('teams_you') : t('teams_member'));
  const isViewer = (who: { uid: string; generation: number }) => who.uid === uid && who.generation === team.generation;
  const authorOf = (message: TeamMessage): Identity => {
    if (message.author.kind === 'user') return { kind: 'user', id: message.author.uid, name: nameOf(message.author.uid) };
    const author = message.author;
    const card = snapshot.bots.find(bot => bot.sharedBotId === author.sharedBotId);
    return { kind: 'bot', id: card?.sharedBotId ?? message.author.sharedBotId, name: card?.name || message.author.displayName || t('teams_bot_tag') };
  };
  const botIdentity = (sharedBotId: string): Identity => ({ kind: 'bot', id: sharedBotId, name: snapshot.bots.find(bot => bot.sharedBotId === sharedBotId)?.name || t('teams_bot_tag') });
  const isMine = (message: TeamMessage) => message.author.kind === 'user' && message.author.uid === uid;
  /** What a message says in one line: its text, or the names of its files when it is files alone. */
  const said = (message: TeamMessage) => message.text || (message.attachments ?? []).map(file => file.name).join(', ');
  const quoteOf = (message: TeamMessage) => {
    if (!message.replyTo) return undefined;
    const target = byId.get(message.replyTo);
    if (!target) return { author: '', text: t('tw_conv_reply_earlier') };
    return { author: authorOf(target).name, text: target.recalled ? t('tw_composer_recalled') : snippet(said(target), 140) };
  };
  const scope = { teamId: team.id, generation: team.generation };
  // Answer mode ends by itself once the task is not waiting for this viewer on that question any more.
  const answerTask = answerWait ? runs.tasks.find(task => task.state === 'waiting' && task.wait?.waitId === answerWait && task.wait.kind === 'human_input' && isViewer(task.wait.recipient)) : undefined;
  useEffect(() => { if (answerWait && !answerTask) setAnswerWait(undefined); }, [answerWait, answerTask]);
  // Only a question is answered in words; a proposed change is approved or declined on its own card.
  const answerable = (task: TeamTaskView | undefined) => !lock && !!task?.wait && task.wait.kind === 'human_input' && isViewer(task.wait.recipient);
  /** The service writes an approval request and its reply in English; a viewer in another language reads them in theirs, in the words the phones use. */
  const localized = (message: TeamMessage): TeamMessage => t.locale === 'en' || message.recalled ? message
    : message.approval && message.author.kind === 'shared_bot' ? { ...message, text: t('teams_approval_request_fmt', message.approval.title) }
    : message.decision && message.waitId ? { ...message, text: t(message.decision === 'approve' ? 'teams_approval_reply_approved' : 'teams_approval_reply_declined') } : message;
  /** Where a Bot's request to change its list stands for this viewer. The reply to it carries the decision. */
  const approvalOf = (message: TeamMessage, waiting: TeamTaskView | undefined): { state: 'decide' | 'waiting' | 'approved' | 'declined' | 'closed'; decider: string } => {
    if (waiting?.wait) return { state: !lock && isViewer(waiting.wait.recipient) ? 'decide' : 'waiting', decider: nameOf(waiting.wait.recipient.uid) };
    const reply = messages.find(candidate => candidate.waitId === message.waitId && candidate.decision !== undefined && candidate.author.kind === 'user');
    return reply ? { state: reply.decision === 'approve' ? 'approved' : 'declined', decider: authorOf(reply).name } : { state: 'closed', decider: '' };
  };
  const answerPending = (waitId: string) => snapshot.pending.some(item => { const payload = pendingPayload(item); return 'waitId' in payload && payload.waitId === waitId; });
  const lastOwn = [...messages].reverse().find(message => isMine(message) && !message.recalled);
  const unseen = pinned ? 0 : messages.filter(message => message.seq > seen && !isMine(message)).length;
  // A task gets a reply message only once it is delivered. Until then (queued, running, failed, expired, …)
  // it has no message to sit under, so it is shown as its own Bot row placed by time among the messages.
  const unanswered = useMemo(() => placeTasks(visible, runs.tasks.filter(task => !task.replyId).reverse(), start > 0), [visible, runs.tasks, start]);
  const botRun = (task: TeamTaskView) => <BotRun task={task} api={workspace.api} scope={scope} locale={locale} renderMarkdown={renderMarkdown} snapshot={snapshot}
    onReply={answerable(task) ? startAnswer : undefined} onChanged={runs.refresh} />;
  const taskRow = (task: TeamTaskView) => <TaskRow key={`task:${task.taskId}`} task={task} author={botIdentity(task.sharedBotId)} locale={locale} renderAvatar={renderAvatar}>
    <div className="tw-embed">{botRun(task)}</div>
  </TaskRow>;
  let afterTask = unanswered.head.length > 0;
  const loadingMessages = !messages.length && conversation.lastSeq > 0;

  // ── Scrolling: open at the first unread message (else the newest), follow the newest while at the bottom ──
  const toBottom = () => { const node = scroller.current; if (node) node.scrollTop = node.scrollHeight; };
  const measure = () => {
    const node = scroller.current;
    if (!node) return;
    const near = node.scrollHeight - node.scrollTop - node.clientHeight < 48;
    pinnedRef.current = near; setPinned(near);
  };
  useIsoLayoutEffect(() => {
    const node = scroller.current;
    if (!node || placed.current || !rows.length) return;
    placed.current = true;
    const divider = node.querySelector<HTMLElement>('[data-unread-divider]');
    if (divider) { if (typeof CSS === 'undefined' || !CSS.supports('scroll-initial-target','nearest')) node.scrollTop = Math.max(0, divider.offsetTop - 56); } else toBottom();
    measure();
  // Placed once, when the first rows arrive.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows.length]);
  const tail = messages[messages.length - 1];
  useIsoLayoutEffect(() => {
    const grew = snapshot.pending.length > pendingCount.current;
    pendingCount.current = snapshot.pending.length;
    if (placed.current && (pinnedRef.current || grew || (tail && isMine(tail) && tail.seq > seen))) toBottom();
  // A new newest message or a new pending one.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tail?.id, snapshot.pending.length]);
  useIsoLayoutEffect(() => {
    const node = scroller.current, saved = anchor.current;
    if (!node || !saved) return;
    anchor.current = undefined;
    node.scrollTop = saved.top + (node.scrollHeight - saved.height);
  }, [start,startId]);
  useEffect(() => { if (pinned) setSeen(tail?.seq ?? 0); }, [pinned, tail?.seq]);
  const readFrontier=useRef<VisibleReadFrontier>();
  const [readError,setReadError]=useState(false);
  // Capture the manual flag only on entry. A later flag set on another device survives old exposure callbacks.
  useEffect(()=>{
    const node=scroller.current;
    if(!node || typeof IntersectionObserver==='undefined')return;
    const frontier=durableReadFrontier(uid,'team:'+team.id+':'+conversation.id,team.generation,conversation.lastReadSeq,conversation.manualUnreadRevision??0,
      (through,revision)=>{void model.visibleRead(conversation.id,through,revision,conversation.pendingActionRevision,conversation.failureRevision).then(()=>setReadError(false)).catch(()=>setReadError(true));},()=>setReadError(true));
    readFrontier.current=frontier;
    return observeTranscript(node,frontier);
  },[model,conversation.id]);
  useEffect(()=>readFrontier.current?.advance(conversation.lastReadSeq),[conversation.lastReadSeq]);
  useEffect(() => {
    const node = content.current, box = scroller.current;
    if (!node || !box || typeof ResizeObserver === 'undefined') return;
    // Bot cards and images change height after they are shown.
    const observer = new ResizeObserver(() => { if (pinnedRef.current) box.scrollTop = box.scrollHeight; });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const showEarlier = async () => {
    const node = scroller.current;
    if (node) anchor.current = { height: node.scrollHeight, top: node.scrollTop };
    if(start>0) setStartId(messages[Math.max(0,start-WINDOW_SIZE)]?.id);
    else await workspace.attempt(async()=>{const id=await model.older();if(id)setStartId(id);});
  };

  const answer = (message: TeamMessage) => { setReply(message); setEditing(undefined); setAnswerWait(undefined); setFocusToken(token => token + 1); };
  const edit = (message: TeamMessage) => { setEditing(message); setReply(undefined); setAnswerWait(undefined); setFocusToken(token => token + 1); };
  function startAnswer(task: TeamTaskView): void { setAnswerWait(task.wait?.waitId); setReply(undefined); setEditing(undefined); setFocusToken(token => token + 1); }
  const confirmRecall = async () => {
    const target = recall;
    if (!target) return;
    if (await workspace.run(() => model.queue('recall', '', target.id))) setRecall(undefined);
  };
  // Taking a file back is a change to the message, like an edit: one change id, kept for a repeat.
  const confirmTakeBack = async () => {
    const target = takeBack;
    if (!target) return;
    if (await workspace.run(() => workspace.api.messages(executionCommand(scope, 'revokeAttachment', { attachmentId: target.attachmentId, changeId: crypto.randomUUID() })))) setTakeBack(undefined);
  };
  const subtitle = conversationSubtitle(conversation, snapshot, locale);
  const auto = friend ? { bots: [], on: [] } : autoReplyBots(conversation, snapshot);
  // A friendship's Bot DM (D22) has no link of its own: its pair team is not one the Teams workspace opens.
  // The header offers the conversation's link, which opens it in the app where installed and on the web otherwise; a
  // friendship's Bot DM (reached with Back) has no link of its own.
  const link = !(onBack && conversation.kind === 'bot_dm') ? conversationShareLink(team.id, conversation) : null;

  return <section className="tw-scope tw-conv" aria-label={label}>
    <ChatTopbarHeader><header className="tw-conv-head">
      {onBack ? <button type="button" className="tw-icon-btn" aria-label={t('remote_back')} title={t('remote_back')} onClick={onBack}><Icon name="chevronLeft" /></button> : null}
      <div className="tw-conv-identity">
        {friend ? <Avatar identity={{ kind: 'user', id: friend.uid, name: friend.name }} size={hasTopbar ? 32 : 40} render={renderAvatar} />
          : <ConversationLead conversation={conversation} label={label} snapshot={snapshot} locale={locale} size={hasTopbar ? 32 : 40} render={renderAvatar} />}
        <div className="tw-conv-title">
          <h2>{label}</h2>
          {friend ? friend.handle ? <p>@{friend.handle}</p> : null : subtitle ? <p>{subtitle}</p> : null}
        </div>
        {friend?.onOpenProfile || (!friend && onToggleDetails) ? <button type="button" className="tw-conv-identity-button"
          aria-label={t(friend ? 'friends_menu_view_profile' : 'teams_conversation_details')}
          aria-expanded={friend ? undefined : !!detailsOpen} aria-haspopup={friend ? 'dialog' : undefined}
          onClick={friend ? friend.onOpenProfile : onToggleDetails} /> : null}
      </div>
      <div className="tw-head-actions">
        {hasTopbar ? <TopbarConnection snapshot={snapshot} workspace={workspace} locale={locale} friend={!!friend} /> : null}
        {friend ? null : <span className="tw-platform" title={platformNotice(locale)}><Icon name="lock" size={13} /><span>{platformChip(locale)}</span></span>}
        {unread > 0 && !friend ? <button type="button" className="tw-head-action" aria-label={t('tw_mark_as_read')} title={t('tw_mark_as_read')}
          disabled={snapshot.messageBusy || snapshot.offline} onClick={() => void workspace.markRead()}><Icon name="check" /><span>{t('tw_mark_as_read')}</span></button> : null}
        {link ? <ShareLinkButton link={link} title={label} locale={locale} note={conversation.kind === 'bot_dm' ? t('teams_share_bot_note') : undefined} /> : null}
        {friend?.actions}
        {auto.bots.length ? <button type="button" className="tw-icon-btn" aria-pressed={auto.on.length > 0} aria-label={t('teams_auto_reply_title')} title={t('teams_auto_reply_title')} onClick={() => setAutoReplyOpen(true)}><Icon name="bot" /></button> : null}
        {onToggleDetails && !friend ? <button type="button" className="tw-icon-btn" aria-pressed={!!detailsOpen} aria-label={t('teams_team_details')} title={t('teams_team_details')} onClick={onToggleDetails}><Icon name="panel" /></button> : null}
      </div>
    </header></ChatTopbarHeader>
    {readError?<p role="status">{locale.startsWith('zh')?'已读状态暂时无法同步':'Read status could not sync'}</p>:null}
    {!hasTopbar && (snapshot.messagingError
      ? <div className="tw-banner" role="alert" data-kind="error"><span>{messageErrorText(snapshot.messagingError, locale)}</span><button type="button" className="tw-link" onClick={() => workspace.reconnect()}>{t('teams_reconnect')}</button></div>
      : snapshot.messagingConnecting
        ? <div className="tw-banner" role="status" data-kind="connecting">{t('tw_conv_connecting')}</div>
      : snapshot.offline
        ? <div className="tw-banner" role="status" data-kind="offline"><span>{t('teams_messages_offline')}</span><button type="button" className="tw-link" onClick={() => workspace.reconnect()}>{t('teams_reconnect')}</button></div>
        : null)}
    {snapshot.actionError && snapshot.actionError !== snapshot.messagingError ? <p className="tw-banner" role="alert" data-kind="error">{messageErrorText(snapshot.actionError, locale)}</p> : null}
    <CreationBanner workspace={workspace} snapshot={snapshot} locale={locale} />
    {runs.error ? <p role="alert" className="tw-banner">{messageErrorText(runs.error, locale)}</p> : null}

    <div className="tw-stream-wrap tw-chat-thread">
      <div ref={scroller} className="tw-stream" role="log" aria-label={t('tw_messages')} tabIndex={0} onScroll={measure}>
        {start > 0 || (messages[0]?.seq ?? 0) > 1 && workspace.api.historyPage ? <div className="tw-earlier"><button type="button" onClick={()=>void showEarlier()}>{t('teams_messages_earlier')}</button></div> : null}
        {!messages.length && !snapshot.pending.length && !runs.tasks.length ? loadingMessages
          ? <p className="tw-empty-line" role="status">{t('teams_loading_messages')}</p>
          : <div className="tw-empty" role="status"><h3>{t('teams_preview_none')}</h3><p>{t('teams_empty_thread_hint')}</p></div> : null}
        <ol ref={content} className="tw-messages">
          {unanswered.head.map(taskRow)}
          {rows.map(row => {
            if (row.type === 'date') return <li key={row.key} className="tw-date"><span>{row.label}</span></li>;
            if (row.type === 'unread') return <li key={row.key} className="tw-unread" data-unread-divider=""><span>{t('teams_new_messages_divider')}</span></li>;
            const message = row.message, author = authorOf(message), mine = isMine(message);
            const tasks = runs.tasks.filter(task => task.replyId === message.id);
            // A Bot's question; while its task still waits for this viewer, Reply on it answers instead of quoting.
            const question = message.author.kind === 'shared_bot' && message.waitId !== undefined;
            const approval = question && !message.recalled ? message.approval : undefined;
            const waiting = question ? runs.tasks.find(task => task.state === 'waiting' && task.wait?.waitId === message.waitId) : undefined;
            const answering = answerable(waiting) ? waiting : undefined;
            const placed = unanswered.after.get(message.id), grouped = row.grouped && !afterTask && !question;
            afterTask = !!placed;
            return <Fragment key={row.key}><MessageItem message={localized(message)} grouped={grouped} author={author} mine={mine} owner={message.author.kind === 'shared_bot' ? nameOf(snapshot.bots.find(bot => bot.sharedBotId === (message.author as Extract<TeamMessage['author'], { kind: 'shared_bot' }>).sharedBotId)?.maintainer.uid ?? '') : ''}
              quote={quoteOf(message)} locale={tag} renderAvatar={renderAvatar} renderMarkdown={renderMarkdown} question={question && !approval} approval={!!approval}
              onReply={lock ? undefined : answering ? () => startAnswer(answering) : answer} onEdit={mine && participant && !message.decision ? edit : undefined} onRecall={mine && participant && !message.decision ? setRecall : undefined}>
              {mine && peerRead!==null && peerRead>=message.seq && message===tail?<small>{locale.startsWith('zh')?'已读':'Read'}</small>:null}
              {message.readFiles?.length && !message.recalled ? <p className="tw-msg-sources" title={message.readFiles.map(file => file.name).join('\n')}>
                <Icon name="clip" size={12} /> {t('tw_conv_read_files_fmt', message.readFiles.map(file => file.name).join(t('tw_list_separator')))}</p> : null}
              {message.attachments?.length && !message.recalled ? <MessageFiles api={workspace.api} scope={scope} files={message.attachments} locale={locale} onRevoke={mine && participant ? file => setTakeBack(file) : undefined} /> : null}
              {approval ? <div className="tw-embed"><ApprovalCard api={workspace.api} scope={scope} conversationId={conversation.id} message={message} approval={approval} locale={locale} onChanged={runs.refresh} {...approvalOf(message, waiting)} /></div> : null}
              {answering && !answerPending(message.waitId!)
                ? <div className="tw-embed"><div className="teams-row-actions"><button type="button" aria-label={t('tw_run_reply')} onClick={() => startAnswer(answering)}>{t('teams_message_reply')}</button></div></div> : null}
              {message.artifactId && !tasks.some(task => task.artifactId === message.artifactId) ? <ArtifactButton key={message.artifactId} api={workspace.api} scope={scope} artifactId={message.artifactId} locale={locale} renderMarkdown={renderMarkdown} people={snapshot} /> : null}
              {tasks.map(task => <div className="tw-embed" key={task.taskId}>{botRun(task)}</div>)}
            </MessageItem>{placed?.map(taskRow)}</Fragment>;
          })}
          {unanswered.tail.map(taskRow)}
          {snapshot.pending.map(intent => {
            const payload = pendingPayload(intent);
            const operation = 'operation' in payload ? payload.operation : 'create';
            const status = (intent.error ? messageErrorText(intent.error, locale) : '') || (intent.attempted ? t('teams_message_confirming') : t('teams_message_pending'));
            return <li key={payload.changeId} className="tw-msg tw-pending" data-mine="true" data-error={intent.error ? 'true' : undefined}>
              <div className="tw-msg-main tw-msg-bubble">
                <div className="tw-msg-head"><b className="tw-msg-author">{t('teams_you')}</b><small role="status">{status}</small></div>
                <p className="tw-msg-body">{operation === 'recall' ? t('tw_conv_recall_message') : `${operation === 'edit' ? `${t('teams_message_edit')}: ` : ''}${payload.text}`}</p>
                {'attachments' in payload && payload.attachments?.length ? <p className="tw-msg-body tw-pending-files"><Icon name="clip" size={12} /> {payload.attachments.length === 1 ? t('tw_files_one') : t('tw_files_other_fmt', payload.attachments.length)}</p> : null}
                <div className="tw-pending-actions">
                  {intent.error === 'HOST_UNAVAILABLE' ? <button type="button" onClick={() => void workspace.attempt(() => model.retryHostUnavailable(payload.changeId))}>{t('common_retry')}</button> : null}
                  <button type="button" disabled={Boolean(intent.attempted && !intent.error) || (intent.leaseUntil ?? 0) > Date.now()} onClick={() => void workspace.attempt(() => model.discard(payload.changeId))}>{t('teams_message_discard')}</button>
                </div>
              </div>
            </li>;
          })}
        </ol>
      </div>
      {!pinned || (tail && tail.seq < conversation.lastSeq) ? <button type="button" className="tw-jump" onClick={() => { if(tail && tail.seq<conversation.lastSeq) model.select(conversation.id); void workspace.markRead(); toBottom(); pinnedRef.current = true; setPinned(true); }}>
        <Icon name="arrowDown" />{unseen > 0 ? t('teams_new_messages_pill_fmt', unseen) : t('tw_conv_latest')}
      </button> : null}
    </div>

    <Composer workspace={workspace} snapshot={snapshot} conversation={conversation} label={label} locale={locale} renderAvatar={renderAvatar}
      editing={editing} reply={reply ? { message: reply, author: authorOf(reply).name } : undefined} focusToken={focusToken}
      answering={answerTask ? { waitId: answerTask.wait!.waitId, author: botIdentity(answerTask.sharedBotId).name, question: byId.get(answerTask.wait!.messageId) } : undefined}
      onClearAnswering={() => setAnswerWait(undefined)} onAnswered={runs.refresh}
      onClearEditing={() => setEditing(undefined)} onClearReply={() => setReply(undefined)} onEditLast={() => { if (lastOwn && participant) edit(lastOwn); }} />

    {autoReplyOpen ? <AutoReplyDialog workspace={workspace} snapshot={snapshot} conversation={conversation} scope={scope} locale={locale} onClose={() => setAutoReplyOpen(false)} /> : null}
    {takeBack ? <Dialog role="alertdialog" title={t('teams_file_take_back_confirm_fmt', takeBack.name)} onClose={() => setTakeBack(undefined)}
      description={t('teams_file_take_back_detail')}>
      <div className="tw-form">
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setTakeBack(undefined)}>{t('common_cancel')}</button>
          <button type="button" className="tw-danger-btn" disabled={snapshot.messageBusy} onClick={() => void confirmTakeBack()}>{t('tw_handover_take_back')}</button>
        </div>
      </div>
    </Dialog> : null}
    {recall ? <Dialog role="alertdialog" title={t('teams_recall_confirm')} onClose={() => setRecall(undefined)}
      description={`${t('teams_recall_confirm_detail')}${recall.attachments?.some(file => !file.revoked) ? ` ${t('tw_conv_recall_files')}` : ''}`}>
      <div className="tw-form">
        <blockquote className="tw-quote">{snippet(said(recall), 140)}</blockquote>
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setRecall(undefined)}>{t('common_cancel')}</button>
          <button type="button" className="tw-danger-btn" disabled={snapshot.messageBusy} onClick={() => void confirmRecall()}>{t('teams_message_recall')}</button>
        </div>
      </div>
    </Dialog> : null}
  </section>;
}

/** The row of a Bot task that has no reply message yet: the Bot's avatar and name, then its status card. */
function TaskRow({ task, author, locale, renderAvatar, children }: { task: TeamTaskView; author: Identity; locale: string; renderAvatar?: RenderAvatar; children: ReactNode }): ReactElement {
  const t = useTeamsText(locale);
  return <li className="tw-msg" data-grouped="false" data-kind="bot" data-task-id={task.taskId} data-task-state={task.state}>
    <div className="tw-msg-lead"><Avatar identity={author} size={32} render={renderAvatar} /></div>
    <div className="tw-msg-main">
      <div className="tw-msg-head"><b className="tw-msg-author">{author.name}</b><span className="tw-tag">{t('teams_bot_tag')}</span></div>
      {children}
    </div>
  </li>;
}

function MessageItem({ message, grouped, author, mine, owner, quote, question, approval, locale, renderAvatar, renderMarkdown, onReply, onEdit, onRecall, children }: {
  message: TeamMessage; grouped: boolean; author: Identity; mine: boolean; owner: string; quote?: { author: string; text: string }; question?: boolean; /** A request to approve a change; the card under it says what. */ approval?: boolean; locale: string; renderAvatar?: RenderAvatar; renderMarkdown?: RenderMarkdown;
  onReply?(message: TeamMessage): void; onEdit?(message: TeamMessage): void; onRecall?(message: TeamMessage): void; children?: ReactNode;
}): ReactElement {
  const t = useTeamsText(locale);
  const clock = formatClock(message.createdAt, locale);
  const full = message.createdAt ? new Date(message.createdAt).toLocaleString(locale) : undefined;
  const acts = !message.recalled && (onReply || onEdit || onRecall);
  // Only a shared Bot writes Markdown; what a person types is shown exactly as typed.
  const markdown = message.author.kind === 'shared_bot' && !message.recalled && renderMarkdown !== undefined;
  const filesOnly = !message.text && !message.recalled && !!message.attachments?.length;
  const body = markdown ? <div className="tw-msg-body tw-md" data-markdown="true">
    {renderMarkdown(message.text)}
  </div> : filesOnly ? null : <p className="tw-msg-body">
    {message.recalled ? <em>{t('teams_message_recalled')}</em> : message.text}
  </p>;
  const edited = message.editedAt && !message.recalled;
  const meta = clock || edited ? <div className="tw-msg-meta">
    {edited ? <small className="tw-edited">({t('tw_edited_tag')})</small> : null}
    {clock ? <time className="tw-msg-time" dateTime={message.createdAt ?? undefined} title={full}>{clock}</time> : null}
  </div> : null;
  const inlineTime = author.kind === 'user' && !!body && !message.attachments?.length && !!meta;
  return <li className="tw-msg" data-grouped={grouped} data-mine={mine} data-kind={author.kind} data-recalled={message.recalled ? 'true' : undefined} data-message-id={message.id} data-message-seq={message.seq}>
    {!mine ? <div className="tw-msg-lead">{!grouped ? <Avatar identity={author} size={32} render={renderAvatar} /> : null}</div> : null}
    <div className="tw-msg-main">
      <div className={author.kind === 'user' ? 'tw-msg-bubble' : 'tw-msg-agent'}>
        {!grouped ? <div className="tw-msg-head">
          <b className="tw-msg-author">{mine ? t('teams_you') : author.name}</b>
          {author.kind === 'bot' ? <span className="tw-tag">{t('teams_bot_tag')}</span> : null}
          {question ? <span className="tw-tag">{t('services_question')}</span> : null}
          {approval ? <span className="tw-tag">{t('teams_approval_tag')}</span> : null}
          {owner ? <small>{t('teams_bot_owner_fmt', owner)}</small> : null}
        </div> : null}
        {quote ? <blockquote className="tw-quote">{quote.author ? <b>{quote.author}</b> : null} {quote.text}</blockquote> : null}
        {inlineTime ? <div className="tw-msg-text-line">{body}{meta}</div> : body}
        {children}
        {inlineTime ? null : meta}
      </div>
      {acts ? <div className="tw-msg-actions" role="group" aria-label={t('teams_message_actions')}>
        {onReply ? <button type="button" className="tw-icon-btn" aria-label={t('teams_message_reply')} title={t('teams_message_reply')} onClick={() => onReply(message)}><Icon name="reply" /></button> : null}
        {onEdit ? <button type="button" className="tw-icon-btn" aria-label={t('teams_message_edit')} title={t('teams_message_edit')} onClick={() => onEdit(message)}><Icon name="edit" /></button> : null}
        {onRecall ? <button type="button" className="tw-icon-btn tw-danger" aria-label={t('teams_message_recall')} title={t('teams_message_recall')} onClick={() => onRecall(message)}><Icon name="trash" /></button> : null}
      </div> : null}
    </div>
  </li>;
}
