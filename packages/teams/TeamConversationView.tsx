import { formatTags, messageFormatParts, type MessageFormat } from './message-format.js';
import { mentionTextParts } from './mentions.js';
import {SourceNoticePreview} from '../inbox/SourceNoticePreview';
import type {InboxNoticeTarget} from '../inbox/contracts';
import { durableReadFrontier,observeTranscript,type VisibleReadFrontier } from '../inbox/visibility.js';
import { Fragment, memo, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { ChannelSettingsPage } from './ChannelManagement';
import { canChannelSend } from './channel-settings';
import { ChatTopbarHeader, ChatTopbarTarget, TopbarConnection } from './ChatTopbar';
import { useTeamTasks } from './bot-runs.js';
import type { TeamTaskView } from './execution.js';
import { MemberHome } from './MemberHome';
import { conversationShareLink } from './share.js';
import { ShareLinkButton } from './ShareLinkButton';
import { botAvailabilityText } from './BotMemberDetails';
import { BotDatasetTaskSettings } from './BotDatasetTaskSettings.js';
import { snippet } from './compose.js';
import { Composer, composerLock } from './Composer';
import { heldReplyErrorText, messageErrorText } from './errors.js';
import { ApprovalCard } from './ApprovalCard';
import { ConversationData } from './ConversationData.js';
import { PrivatePreviewRequest } from './PrivatePreviewRequest.js';
import { DataRecordButton } from './DataRecordButton.js';
import { DatasetConversationPicker, SharedDatasetCard } from './DatasetConversation.js';
import { DatasetPageCard, DatasetPagePublisher } from './DatasetPage.js';
import { ReportCard } from './ReportViews.js';
import { parseReportLink } from './reports.js';
import { MessageFiles } from './MessageFiles';
import { PostActions } from './PostActions';
import type { MessageAttachment } from './attachments.js';
import { executionCommand } from './execution.js';
import { createsMessage, pendingPayload, type TeamConversation, type TeamMessage, type VisiblePendingMessage } from './messages.js';
import { ArtifactButton, BotRun } from './BotRunCards';
import { VisualizationComposer } from './VisualizationComposer.js';
import { VisualizationMessage } from './VisualizationMessage.js';
import { AutoReplyDialog, autoReplyBots } from './AutoReplyDialog';
import { BotScopeDialog, scopedBots } from './BotScopeDialog';
import { TriageLogDialog } from './TriageLogDialog';
import { useTriageReasons } from './triage-hooks.js';
import { maintainsBot, triageBots } from './triage-log.js';
import { HeldReplyCard } from './HeldReplyCard';
import { useHeldReplies } from './held-hooks.js';
import { heldReplyBots, placeHeld, type HeldRow } from './held-replies.js';
import { buildStreamRows, formatClock, formatPostTime, placeTasks, visibleStart, WINDOW_SIZE, type StreamRow } from './stream.js';
import { Avatar, ConversationLead, Dialog, Icon, PopupMenu, TeamMark, platformChip, platformNotice, useIsoLayoutEffect, type MenuItem, type RenderAvatar, type RenderMarkdown } from './ui';
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
  sharedPostId?: string;
  onExitPost?(): void;
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  /** Whether the details drawer is open. With `onToggleDetails` the header shows a button for it. */
  detailsOpen?: boolean;
  onToggleDetails?(): void;
  onOpenChannelSettings?(): void;
  onOpenTeamSettings?(opener: HTMLButtonElement): void;
  onOpenTeamSettingsInChat?(opener: HTMLButtonElement): void;
  /** Settings covers this mounted conversation; messages underneath should not count as read. */
  covered?: boolean;
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
  const people = members ? t(members === 1 ? 'teams_members_count_one_fmt' : 'teams_members_count_other_fmt', members) : '';
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
    const openSettings = team && !snapshot.selectedConversationId && !snapshot.creation ? props.onOpenTeamSettings : undefined;
    const chrome = hasTopbar || openSettings ? <ChatTopbarHeader><header className="tw-conv-head">
      <div className="tw-conv-identity tw-team-home-heading">
        {team && openSettings ? <TeamMark id={team.id} name={team.name} avatar={snapshot.detail?.team.id === team.id ? snapshot.detail.profile?.avatar : undefined} /> : null}
        <div className="tw-conv-title"><h2>{team?.name ?? t('teams_title')}</h2></div>
        {openSettings ? <button type="button" className="tw-icon-btn" data-team-settings-home="" data-team-settings-opener=""
          aria-label={t('tw_team_settings')} title={t('tw_team_settings')} onClick={event => openSettings(event.currentTarget)}><Icon name="gear" /></button> : null}
      </div>
      <TopbarConnection snapshot={snapshot} workspace={props.workspace} locale={locale} />
    </header></ChatTopbarHeader> : null;
    if (snapshot.loading) return <>{chrome}<section className="tw-scope tw-conv tw-conv-empty"><p className="tw-empty-line" role="status">{t('teams_loading')}</p></section></>;
    if (team && !snapshot.selectedConversationId && !snapshot.creation) return <>{chrome}<MemberHome key={`${snapshot.uid}:${team.id}:${team.generation}`} workspace={props.workspace} snapshot={snapshot} locale={locale} renderAvatar={props.renderAvatar} renderMarkdown={props.renderMarkdown} /></>;
    return <>{chrome}<section className="tw-scope tw-conv tw-conv-empty">
      <div className="tw-empty"><h2>{title}</h2><p>{body}</p><p className="tw-hint">{platformNotice(locale)}</p></div>
      {snapshot.creation ? <CreationBanner workspace={props.workspace} snapshot={snapshot} locale={locale} /> : null}
    </section></>;
  }
  if(notificationTarget && token!==dismissed)return <SourceNoticePreview api={props.workspace.api} kind="team" sourceId={team.id} generation={team.generation} conversationId={conversation.id} locale={locale} target={notificationTarget} onReturn={()=>setDismissed(token)} />;
  return <ConversationPane key={`${snapshot.uid}:${team.id}:${team.generation}:${conversation.id}:${props.sharedPostId ?? ''}`} {...props} conversation={conversation} model={model} team={team} />;
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

function ConversationPane({ workspace, snapshot, conversation, model, team, detailsOpen, onToggleDetails, onOpenChannelSettings, onOpenTeamSettingsInChat, renderAvatar, renderMarkdown, locale, now, friend, onBack, sharedPostId, onExitPost, covered = false }: Pane): ReactElement {
  const t = useTeamsText(locale);
  const hasTopbar = useContext(ChatTopbarTarget) !== undefined;
  const uid = snapshot.uid, messages = snapshot.messages, tag = intlTag(locale);
  const channel = conversation.kind === 'channel';
  const postsMode = channel && conversation.posting?.mode === 'posts';
  const [currentMinute, setCurrentMinute] = useState(() => Math.floor(Date.now() / 60000));
  useEffect(() => {
    if (now !== undefined || !channel) return;
    const tick = () => setCurrentMinute(Math.floor(Date.now() / 60000));
    const timer = setInterval(tick, 60000);
    document.addEventListener('visibilitychange', tick);
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', tick); };
  }, [now, channel]);
  const minute = now === undefined ? (channel ? currentMinute : Math.floor(Date.now() / 60000)) : Math.floor(Number(now) / 60000);
  const [threadPages, setThreadPages] = useState<Record<string, { root: TeamMessage; replies: TeamMessage[]; next: number | null }>>({});
  const [threadLoading, setThreadLoading] = useState(new Set<string>());
  const threadRequests = useRef(new Set<string>());
  const [threadErrors, setThreadErrors] = useState(new Set<string>());
  const [settings, setSettings] = useState(false);
  const gear = useRef<HTMLButtonElement>(null);
  const returnToGear = useRef(false);
  useIsoLayoutEffect(() => {
    if (!settings && returnToGear.current) { gear.current?.focus(); returnToGear.current = false; }
  }, [settings]);
  const [startId, setStartId] = useState<string>();
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const requestingEarlier = useRef(false);
  const [editing, setEditing] = useState<TeamMessage>(), [reply, setReply] = useState<TeamMessage>(), [recall, setRecall] = useState<TeamMessage>();
  const [takeBack, setTakeBack] = useState<MessageAttachment>();
  const [autoReplyOpen, setAutoReplyOpen] = useState(false), [botScopeOpen, setBotScopeOpen] = useState(false);
  /** The auto-reply log of this Chat, and the Bots the service said this person does not maintain after all. */
  const [triageOpen, setTriageOpen] = useState(false), [triageDenied, setTriageDenied] = useState<string[]>([]);
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
  const pinnedRef = useRef(true), placed = useRef(false), pendingCount = useRef(snapshot.pending.length);
  const anchor = useRef<{ targetId: string; row: HTMLElement; offset: number }>();

  const label = friend ? friend.name : conversationLabel(conversation, snapshot.directory, uid, { bots: snapshot.bots, locale });
  const unread = unreadCount(conversation);
  const start = useMemo(() => visibleStart(messages, { startId, lastReadSeq: conversation.lastReadSeq, uid }), [messages, startId, conversation.lastReadSeq, uid]);
  const visible = useMemo(() => messages.slice(start), [messages, start]);
  const byId = useMemo(() => {
    const indexed = new Map<string, TeamMessage>();
    for (const page of Object.values(threadPages)) {
      indexed.set(page.root.id, page.root);
      for (const reply of page.replies) indexed.set(reply.id, reply);
    }
    for (const message of messages) indexed.set(message.id, message);
    return indexed;
  }, [messages, threadPages]);
  const { outgoing, operations } = useMemo(() => {
    const outgoing = new Map<string, { message: TeamMessage; intent: VisiblePendingMessage; fileCount: number }>();
    const operations: VisiblePendingMessage[] = [];
    for (const intent of snapshot.pending) {
      const payload = pendingPayload(intent);
      if (!createsMessage(intent) || byId.has(payload.messageId) || outgoing.has(payload.messageId)) {
        operations.push(intent);
        continue;
      }
      const replyTo = 'replyTo' in payload ? payload.replyTo : undefined;
      const target = replyTo ? byId.get(replyTo) : undefined;
      const postId = target?.postId ?? (target?.channelPost ? replyTo : undefined);
      const message: TeamMessage = {
        id: payload.messageId, seq: 0, changeId: payload.changeId, changeSeq: 0, author: { kind: 'user', uid },
        kind: 'text', text: payload.text, createdAt: intent.enqueuedAt ?? null,
        replyTo,
        ...(postId ? { postId } : 'channelMode' in payload && payload.channelMode === 'posts' && !replyTo ? { channelPost: true as const } : {}),
        mentionEntities: 'mentionEntities' in payload ? payload.mentionEntities : undefined,
        formats: payload.formats,
        dataRef: 'dataRef' in payload ? payload.dataRef : undefined,
      };
      outgoing.set(message.id, { message, intent, fileCount: 'attachments' in payload ? payload.attachments?.length ?? 0 : 0 });
    }
    return { outgoing, operations };
  }, [snapshot.pending, byId, uid]);
  const rows = useMemo(() => buildStreamRows(outgoing.size ? [...visible, ...Array.from(outgoing.values(), value => value.message)] : visible,
    { lastReadSeq: conversation.lastReadSeq, uid, locale: tag, now: minute * 60_000 }), [visible, outgoing, conversation.lastReadSeq, uid, tag, minute]);
  const loadPost = useCallback(async (postId: string, older = false) => {
    if (threadRequests.current.has(postId) || (!older && threadPages[postId])) return;
    threadRequests.current.add(postId);
    setThreadLoading(new Set(threadRequests.current));
    setThreadErrors(previous => new Set([...previous].filter(id => id !== postId)));
    const before = older ? threadPages[postId]?.next ?? undefined : undefined;
    try {
      const page = await workspace.api.messages<{ root: TeamMessage; replies: TeamMessage[]; next: number | null }>({
        action: 'postHistory', teamId: team.id, generation: team.generation, conversationId: conversation.id, postId, ...(before !== undefined ? { before } : {}),
      });
      setThreadPages(previous => ({ ...previous, [postId]: {
        ...page, replies: [...page.replies, ...(before !== undefined ? previous[postId]?.replies ?? [] : [])],
      } }));
    } catch { setThreadErrors(previous => new Set(previous).add(postId)); }
    finally { threadRequests.current.delete(postId); setThreadLoading(new Set(threadRequests.current)); }
  }, [threadPages, workspace.api, team.id, team.generation, conversation.id]);
  useEffect(() => {
    if (sharedPostId && !snapshot.offline && !threadErrors.has(sharedPostId)) void loadPost(sharedPostId);
  }, [sharedPostId, snapshot.offline, threadErrors, loadPost]);
  const { postReplies, timeline } = useMemo(() => {
    const replies = new Map<string, Extract<(typeof rows)[number], { type: 'message' }>[]>();
    const roots = new Set<string>();
    for (const row of rows) if (row.type === 'message') {
      if (row.message.channelPost) roots.add(row.message.id);
      if (row.message.postId) {
        const thread = replies.get(row.message.postId) ?? [];
        thread.push(row);
        replies.set(row.message.postId, thread);
      }
    }
    for (const [id, page] of Object.entries(threadPages)) {
      const loaded = replies.get(id) ?? [];
      const byId = new Map(page.replies.map(message => [message.id, { type: 'message' as const, key: message.id, message, grouped: false }]));
      for (const row of loaded) byId.set(row.message.id, row);
      replies.set(id, [...byId.values()].sort((a, b) => (a.message.seq || Infinity) - (b.message.seq || Infinity)));
    }
    if (!replies.size) return { postReplies: replies, timeline: rows };
    const shown = new Set<string>();
    const timeline: (StreamRow | { type: 'missingPost'; key: string; postId: string })[] = [];
    let date: Extract<StreamRow, { type: 'date' }> | undefined;
    let unread: Extract<StreamRow, { type: 'unread' }> | undefined;
    let followedReply = false;
    for (const row of rows) {
      if (row.type === 'date') { date = row; continue; }
      if (row.type === 'unread') { unread = row; continue; }
      const id = row.message.postId;
      if (id && (roots.has(id) || shown.has(id))) {
        followedReply = true;
        continue;
      }
      if (date) timeline.push({ ...date, key: date.key + ':' + row.key });
      if (unread) timeline.push(unread);
      if (id) {
        const root = threadPages[id]?.root;
        timeline.push(root ? { type: 'message', key: 'message:' + root.id, message: root, grouped: false } : { type: 'missingPost', key: id, postId: id });
        shown.add(id);
      } else timeline.push(followedReply && row.grouped ? { ...row, grouped: false } : row);
      date = undefined;
      unread = undefined;
      followedReply = false;
    }
    if (unread) timeline.push(unread);
    return { postReplies: replies, timeline };
  }, [rows, threadPages]);
  const focusedPost = sharedPostId ? byId.get(sharedPostId) : undefined;
  const chatDates = useMemo(() => {
    const dates = new Set<string>();
    if (postsMode) {
      let day: string | undefined;
      for (const row of timeline) {
        if (row.type === 'date') day = row.key;
        else if (day && row.type === 'message' && !row.message.channelPost) dates.add(day);
      }
    }
    return dates;
  }, [postsMode, timeline]);
  const feed = sharedPostId
    ? (focusedPost?.channelPost ? [{ type: 'message' as const, key: focusedPost.id, message: focusedPost, grouped: false }] : [])
    : timeline;
  const runs = useTeamTasks(workspace.api, { teamId: team.id, generation: team.generation }, conversation.id, !friend);
  const membersById = useMemo(() => new Map(snapshot.directory.map(member => [member.uid, member])), [snapshot.directory]);
  const botsById = useMemo(() => new Map(snapshot.bots.map(bot => [bot.sharedBotId, bot])), [snapshot.bots]);
  const { tasksByReply, waitingById, withoutReply } = useMemo(() => {
    const tasksByReply = new Map<string, TeamTaskView[]>();
    const waitingById = new Map<string, TeamTaskView>();
    const withoutReply: TeamTaskView[] = [];
    for (const task of runs.tasks) {
      if (task.replyId) {
        const replies = tasksByReply.get(task.replyId);
        if (replies) replies.push(task);
        else tasksByReply.set(task.replyId, [task]);
      } else withoutReply.push(task);
      if (task.state === 'waiting' && task.wait && !waitingById.has(task.wait.waitId)) waitingById.set(task.wait.waitId, task);
    }
    return { tasksByReply, waitingById, withoutReply: withoutReply.reverse() };
  }, [runs.tasks]);
  const decisionsByWait = useMemo(() => {
    const decisions = new Map<string, TeamMessage>();
    for (const message of messages) {
      if (message.waitId && message.decision !== undefined && message.author.kind === 'user' && !decisions.has(message.waitId)) {
        decisions.set(message.waitId, message);
      }
    }
    return decisions;
  }, [messages]);
  const pendingWaits = useMemo(() => {
    const waits = new Set<string>();
    for (const item of snapshot.pending) {
      const payload = pendingPayload(item);
      if ('waitId' in payload) waits.add(payload.waitId);
    }
    return waits;
  }, [snapshot.pending]);

  const lock = composerLock({ uid, conversation, bots: snapshot.bots, botsLoaded: snapshot.botsLoaded, botsUnverified: snapshot.botsUnverified }, locale);
  const participant = !conversation.participants.length || conversation.participants.includes(`user:${uid}`);
  const conversationBot = conversation.kind === 'bot_dm' ?
    conversation.participants.find(id => id.startsWith('shared_bot:'))?.slice(11) : undefined;
  const maintainedBot = conversationBot && snapshot.bots.find(bot => bot.sharedBotId === conversationBot &&
    bot.maintainer.uid === uid && bot.maintainer.generation === team.generation && bot.state === 'published');
  const nameOf = (id: string) => membersById.get(id)?.displayName || (id === uid ? t('teams_you') : t('teams_member'));
  const isViewer = (who: { uid: string; generation: number }) => who.uid === uid && who.generation === team.generation;
  const authorOf = (message: TeamMessage): Identity => {
    if (message.author.kind === 'user') return { kind: 'user', id: message.author.uid, name: nameOf(message.author.uid) };
    const author = message.author;
    if (author.kind === 'task') return { kind: 'task', id: author.taskId, name: author.displayName };
    const card = botsById.get(author.sharedBotId);
    return { kind: 'bot', id: card?.sharedBotId ?? author.sharedBotId, name: card?.name || author.displayName || t('teams_bot_tag') };
  };
  const botIdentity = (sharedBotId: string): Identity => ({ kind: 'bot', id: sharedBotId, name: botsById.get(sharedBotId)?.name || t('teams_bot_tag') });
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
  // Why the Bot of this Bot DM did not reply, for the person who maintains it: read from the auto-reply log of this Chat, and only once there is a task that did not reply.
  const reasonBot = !friend && conversationBot ? snapshot.bots.find(bot => bot.sharedBotId === conversationBot && maintainsBot(bot, uid, team.generation)) : undefined;
  const reasons = useTriageReasons(workspace.api, scope, reasonBot ? { sharedBotId: reasonBot.sharedBotId, conversationId: conversation.id } : undefined,
    reasonBot ? runs.tasks.filter(task => task.state === 'declined' && task.sharedBotId === reasonBot.sharedBotId).map(task => task.taskId) : [], !snapshot.offline);
  // The replies of the Bots this person maintains that wait for them to be let go (Bot v3, B6): read when the Chat opens, whenever a task of it starts or stops waiting for approval, and
  // whenever the service says a Bot or the Chat changed, since a reply nobody asked for is no task of the Chat's list.
  const heldBots = friend ? [] : heldReplyBots(conversation, snapshot.bots, uid, team.generation);
  const held = useHeldReplies(workspace.api, scope, heldBots.length ? { sharedBotIds: heldBots.map(bot => bot.sharedBotId), conversationId: conversation.id } : undefined,
    { online: !snapshot.offline, watch: runs.tasks.filter(task => task.state === 'awaiting_approval').map(task => task.taskId).join(','), onDecided: runs.refresh });
  const placedHeld = useMemo(() => placeHeld(held.view.rows, new Set(runs.tasks.map(task => task.taskId))), [held.view.rows, runs.tasks]);
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
    const reply = message.waitId ? decisionsByWait.get(message.waitId) : undefined;
    return reply ? { state: reply.decision === 'approve' ? 'approved' : 'declined', decider: authorOf(reply).name } : { state: 'closed', decider: '' };
  };
  const unseen = pinned ? 0 : messages.reduce((count, message) => count + Number(message.seq > seen && !isMine(message)), 0);
  // A task gets a reply message only once it is delivered. Until then (queued, running, failed, expired, …)
  // it has no message to sit under, so it is shown as its own Bot row placed by time among the messages.
  const unanswered = useMemo(() => placeTasks(visible, withoutReply, start > 0), [visible, withoutReply, start]);
  const heldCard = (row: HeldRow) => <HeldReplyCard row={row} now={held.view.now} locale={locale} renderMarkdown={renderMarkdown}
    onApprove={text => held.approve(row.reply.taskId, text)} onReject={() => held.reject(row.reply.taskId)} onDismiss={() => held.dismiss(row.reply.taskId)} />;
  // The maintainer reads the reply their Bot made where everyone else is told it waits for approval; the others see that line.
  const botRun = (task: TeamTaskView, replyText?: string) => {
    const row = task.state === 'awaiting_approval' ? placedHeld.byTask.get(task.taskId) : undefined;
    return row ? heldCard(row) : <BotRun task={task} api={workspace.api} scope={scope} locale={locale} renderMarkdown={renderMarkdown} snapshot={snapshot} tasks={runs.tasks} reason={reasons.get(task.taskId)} replyText={replyText}
      onReply={answerable(task) ? startAnswer : undefined} onChanged={runs.refresh} />;
  };
  const taskRow = (task: TeamTaskView) => <TaskRow key={`task:${task.taskId}`} task={task} author={botIdentity(task.sharedBotId)} locale={locale} renderAvatar={renderAvatar}>
    <div className="tw-embed">{botRun(task)}</div>
  </TaskRow>;
  let afterTask = unanswered.head.length > 0;
  const pendingLabel = (intent: VisiblePendingMessage) => {
    if (intent.error) return messageErrorText(intent.error, locale) || t('tw_sidebar_not_sent');
    if (intent.acknowledged) return t('tw_message_sent');
    return intent.attempted || !snapshot.offline ? t('teams_message_confirming') : t('teams_message_pending');
  };
  const pendingActions = (intent: VisiblePendingMessage) => {
    const { changeId, text } = pendingPayload(intent);
    return <>
      {intent.error === 'MENTION_TARGET_REQUIRED' && text
        ? <button type="button" onClick={() => void workspace.attempt(() => navigator.clipboard.writeText(text))}>{t('chat_message_copy')}</button> : null}
      {intent.error === 'HOST_UNAVAILABLE' ? <button type="button" onClick={() => void workspace.attempt(() => model.retryHostUnavailable(changeId))}>{t('common_retry')}</button> : null}
      {!intent.attempted || intent.error ? <button type="button" disabled={(intent.leaseUntil ?? 0) > Date.now()} onClick={() => void workspace.attempt(() => model.discard(changeId))}>{t('teams_message_discard')}</button> : null}
    </>;
  };
  const loadingMessages = !messages.length && conversation.lastSeq > 0;

  // ── Scrolling: open at the first unread message (else the newest), follow the newest while at the bottom ──
  const toBottom = () => { const node = scroller.current; if (node && !sharedPostId) node.scrollTop = node.scrollHeight; };
  const measure = () => {
    const node = scroller.current;
    if (!node) return;
    const near = node.scrollHeight - node.scrollTop - node.clientHeight < 48;
    if (pinnedRef.current !== near) {
      pinnedRef.current = near;
      setPinned(near);
    }
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
    if (!node || !saved || visible[0]?.id !== saved.targetId) return;
    anchor.current = undefined;
    if (node.contains(saved.row)) {
      node.scrollTop += saved.row.getBoundingClientRect().top - node.getBoundingClientRect().top - saved.offset;
      measure();
    }
  }, [visible]);
  useEffect(() => { if (pinned) setSeen(tail?.seq ?? 0); }, [pinned, tail?.seq]);
  const readFrontier=useRef<VisibleReadFrontier>();
  const recheckRead = useRef<() => void>();
  const foreground = useRef(!covered && !settings);
  const [readError,setReadError]=useState(false);
  useIsoLayoutEffect(() => {
    foreground.current = !covered && !settings;
    recheckRead.current?.();
  }, [covered, settings]);
  // Capture the manual flag only on entry. A later flag set on another device survives old exposure callbacks.
  useEffect(()=>{
    const node=scroller.current;
    if(!node || typeof IntersectionObserver==='undefined')return;
    const frontier=durableReadFrontier(uid,'team:'+team.id+':'+conversation.id,team.generation,conversation.lastReadSeq,conversation.manualUnreadRevision??0,
      (through,revision)=>{void model.visibleRead(conversation.id,through,revision,conversation.pendingActionRevision,conversation.failureRevision).then(()=>setReadError(false)).catch(()=>setReadError(true));},()=>setReadError(true));
    readFrontier.current=frontier;
    return observeTranscript(node,frontier, {
      isForeground: () => foreground.current,
      onRecheck: recheck => { recheckRead.current = recheck; return () => { recheckRead.current = undefined; }; },
    });
  },[model,conversation.id]);
  useEffect(()=>readFrontier.current?.advance(conversation.lastReadSeq),[conversation.lastReadSeq]);
  useEffect(() => {
    const node = content.current, box = scroller.current;
    if (!node || !box || typeof ResizeObserver === 'undefined') return;
    // Cards, images, and the composer can change how much of the latest message is visible.
    const observer = new ResizeObserver(() => { if (pinnedRef.current && !sharedPostId) box.scrollTop = box.scrollHeight; });
    observer.observe(node);
    observer.observe(box);
    return () => observer.disconnect();
  }, []);
  const showEarlier = async () => {
    if (requestingEarlier.current) return;
    const preservePosition = (targetId: string) => {
      const node = scroller.current;
      if (!node || !content.current) return;
      const top = node.getBoundingClientRect().top;
      for (const row of content.current.querySelectorAll<HTMLElement>('[data-message-id]')) {
        const bounds = row.getBoundingClientRect();
        if (bounds.bottom <= top) continue;
        anchor.current = { targetId, row, offset: bounds.top - top };
        break;
      }
    };
    if (start > 0) {
      const id = messages[Math.max(0, start - WINDOW_SIZE)].id;
      preservePosition(id);
      setStartId(id);
      return;
    }
    requestingEarlier.current = true;
    setLoadingEarlier(true);
    try {
      await workspace.attempt(async () => {
        const id = await model.older(preservePosition);
        if (id) setStartId(id);
      });
    } finally {
      requestingEarlier.current = false;
      setLoadingEarlier(false);
    }
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
  const hasChannelSettings = channel && !friend;
  const auto = friend || hasChannelSettings ? { bots: [], on: [] } : autoReplyBots(conversation, snapshot);
  const limitable = friend || hasChannelSettings ? [] : scopedBots(conversation, snapshot);
  const logBots = friend ? [] : triageBots(conversation, snapshot.bots, uid, team.generation).filter(bot => !triageDenied.includes(bot.sharedBotId));
  // A friendship's Bot DM (D22) has no link of its own: its pair team is not one the Teams workspace opens.
  // The header offers the conversation's link, which opens it in the app where installed and on the web otherwise; a
  // friendship's Bot DM (reached with Back) has no link of its own.
  const link = !(onBack && conversation.kind === 'bot_dm') ? conversationShareLink(team.id, conversation) : null;

  const settingsLabel = t('tw_channel_settings');
  return <>
    {settings && channel ? <ChannelSettingsPage workspace={workspace} snapshot={snapshot} locale={locale} conversation={conversation}
      onClose={() => { returnToGear.current = true; setSettings(false); }} /> : null}
    <section className="tw-scope tw-conv" aria-label={label} hidden={settings} style={settings ? { display: 'none' } : undefined}>
    {!settings ? <ChatTopbarHeader><header className="tw-conv-head">
      {onBack ? <button type="button" className="tw-icon-btn" aria-label={t('remote_back')} title={t('remote_back')} onClick={onBack}><Icon name="chevronLeft" /></button> : null}
      <div className="tw-conv-identity">
        {friend ? <Avatar identity={{ kind: 'user', id: friend.uid, name: friend.name }} size={hasTopbar ? 32 : 40} render={renderAvatar} />
          : <ConversationLead conversation={conversation} label={label} snapshot={snapshot} locale={locale} size={hasTopbar ? 32 : 40} render={renderAvatar} />}
        <div className="tw-conv-title">
          <h2>{label}</h2>
          {friend ? friend.handle ? <p>@{friend.handle}</p> : null : subtitle ? <p>{subtitle}</p> : null}
        </div>
        {friend?.onOpenProfile || (!friend && onToggleDetails) ? <button type="button" className="tw-conv-identity-button"
          aria-label={t(friend ? 'friends_menu_view_profile' : channel ? 'tw_channel_details' : 'teams_conversation_details')}
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
        {participant && !lock && !snapshot.offline && conversation.kind === 'channel' &&
          <VisualizationComposer api={workspace.api} scope={{ ...scope, conversationId: conversation.id }} locale={locale}
            participantName={uid => snapshot.directory.find(member => member.uid === uid)?.displayName ?? uid} />}
        {!channel && participant && !lock && !snapshot.offline && <DatasetConversationPicker
          api={workspace.api} destination={{ team: team.id, conversation: conversation.id }} generation={team.generation}
          sharedBot={conversationBot}
          locale={locale} participantName={uid => snapshot.directory.find(member => member.uid === uid)?.displayName
            || (uid === snapshot.uid ? t('teams_you') : uid)} />}
        {participant && !snapshot.offline && maintainedBot && <BotDatasetTaskSettings
          key={conversation.id + ':' + maintainedBot.revision} api={workspace.api} locale={locale} name={maintainedBot.name}
          scope={{ team: team.id, conversation: conversation.id, bot: maintainedBot.sharedBotId }} />}
        {!channel && !friend && <ConversationData api={workspace.api} scope={{ kind: 'team', ...scope, conversationId: conversation.id }} locale={locale}
          shareReport={participant && !lock && !snapshot.offline && conversation.kind !== 'bot_dm'
            ? link => workspace.run(() => model.queue('create', link)) : undefined}
          share={participant && !lock && !snapshot.offline && conversation.kind !== 'bot_dm'
            ? (reference, text) => workspace.run(() => model.queue('create', text, undefined, undefined, [], [], reference))
            : undefined}/>}
        {!channel && !friend && participant && !lock && !snapshot.offline && <PrivatePreviewRequest key={conversation.id}
          api={workspace.api} scope={{ kind: 'team', ...scope, conversationId: conversation.id }} locale={locale}
          request={offer => workspace.run(() => model.queue('create', 'Private data preview requested.', undefined,
            undefined, [offer.sharedBotId], [], undefined, offer.source))} />}
        {auto.bots.length ? <button type="button" className="tw-icon-btn" aria-pressed={auto.on.length > 0} aria-label={t('teams_auto_reply_title')} title={t('teams_auto_reply_title')} onClick={() => setAutoReplyOpen(true)}><Icon name="bot" /></button> : null}
        {limitable.length ? <button type="button" className="tw-icon-btn" aria-label={t('teams_bot_scope_title')} title={t('teams_bot_scope_title')} onClick={() => setBotScopeOpen(true)}><Icon name="sliders" /></button> : null}
        {logBots.length ? <button type="button" className="tw-icon-btn" aria-label={t('teams_triage_title')} title={t('teams_triage_title')} onClick={() => setTriageOpen(true)}><Icon name="clock" /></button> : null}
        {hasChannelSettings ? <button ref={gear} type="button" className="tw-icon-btn" aria-label={settingsLabel} title={settingsLabel}
          onClick={() => { onOpenChannelSettings?.(); setSettings(true); }}><Icon name="gear" /></button> : null}
        {onOpenTeamSettingsInChat && !friend ? <button type="button" className="tw-icon-btn" data-team-settings-opener=""
          aria-label={t('tw_team_settings')} title={t('tw_team_settings')} onClick={event => onOpenTeamSettingsInChat(event.currentTarget)}>
          <Icon name={hasChannelSettings ? 'sliders' : 'gear'} />
        </button> : null}
        {onToggleDetails && !friend ? <button type="button" className="tw-icon-btn" aria-pressed={!!detailsOpen} aria-label={t(channel ? 'tw_channel_details' : 'teams_team_details')} title={t(channel ? 'tw_channel_details' : 'teams_team_details')} onClick={onToggleDetails}><Icon name="panel" /></button> : null}
      </div>
    </header></ChatTopbarHeader> : null}
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
    {held.view.status === 'failed' && held.view.error && held.view.error !== runs.error
      ? <div role="alert" className="tw-banner" data-kind="error"><span>{heldReplyErrorText(held.view.error, locale)}</span><button type="button" className="tw-link" onClick={held.reload}>{t('common_retry')}</button></div> : null}

    <div className="tw-stream-wrap tw-chat-thread" data-posts-mode={postsMode || Boolean(sharedPostId) || undefined}>
      <div ref={scroller} className="tw-stream" role="log" aria-label={t('tw_messages')} tabIndex={0} onScroll={measure}>
        {sharedPostId && <div className="tw-post-context">
          <button type="button" className="tw-link" onClick={onExitPost}><Icon name="chevronLeft" />{t('tw_channel_back')}</button>
          {!focusedPost?.channelPost && (threadErrors.has(sharedPostId) ? <p role="alert">{t('tw_channel_load_post_failed')} <button type="button" disabled={snapshot.offline} onClick={() => void loadPost(sharedPostId)}>{t('common_retry')}</button></p>
            : <p role="status">{t(snapshot.offline ? 'teams_offline_title' : 'teams_loading_messages')}</p>)}
        </div>}
        {!sharedPostId && (start > 0 || (messages[0]?.seq ?? 0) > 1 && workspace.api.historyPage) ? <div className="tw-earlier">
          <button type="button" aria-disabled={loadingEarlier} onClick={() => void showEarlier()}>
            <span role="status">{t(loadingEarlier ? 'teams_loading_messages' : 'teams_messages_earlier')}</span>
          </button>
        </div> : null}
        {!sharedPostId && !messages.length && !snapshot.pending.length && !runs.tasks.length ? loadingMessages
          ? <p className="tw-empty-line" role="status">{t('teams_loading_messages')}</p>
          : <div className="tw-empty" role="status"><h3>{t('teams_preview_none')}</h3><p>{t('teams_empty_thread_hint')}</p></div> : null}
        <ol ref={content} className="tw-messages">
          {!sharedPostId && unanswered.head.map(taskRow)}
          {feed.map(function renderRow(row): ReactNode {
            if (row.type === 'missingPost') return <li key={row.key} className="tw-post-context">
              <button type="button" className="tw-link" disabled={threadLoading.has(row.postId) || snapshot.offline} onClick={() => void loadPost(row.postId)}>
                {threadLoading.has(row.postId) ? t('teams_loading_messages') : t('tw_channel_open_post')}
              </button>
              {threadErrors.has(row.postId) ? <p role="alert">{t('tw_channel_load_post_failed')}</p> : null}
            </li>;
            if (row.type === 'date') return !postsMode || chatDates.has(row.key) ? <li key={row.key} className="tw-date"><span>{row.label}</span></li> : null;
            if (row.type === 'unread') return <li key={row.key} className="tw-unread" data-unread-divider=""><span>{t('teams_new_messages_divider')}</span></li>;
            const message = row.message, author = authorOf(message), mine = isMine(message);
            const pending = outgoing.get(message.id);
            const delivery: PendingDelivery | undefined = pending ? {
              label: pendingLabel(pending.intent), state: pending.intent.error ? 'failed' : pending.intent.acknowledged ? 'sent' : 'pending',
              fileCount: pending.fileCount,
              actions: !pending.intent.attempted || pending.intent.error ? pendingActions(pending.intent) : undefined,
            } : undefined;
            const tasks = tasksByReply.get(message.id);
            const replyText = message.author.kind === 'shared_bot' && !message.recalled && message.text ? message.text : undefined;
            // A Bot's question; while its task still waits for this viewer, Reply on it answers instead of quoting.
            const question = message.author.kind === 'shared_bot' && message.waitId !== undefined;
            const approval = question && !message.recalled ? message.approval : undefined;
            const waiting = question ? waitingById.get(message.waitId!) : undefined;
            const answering = answerable(waiting) ? waiting : undefined;
            const thread = postReplies.get(message.id), page = threadPages[message.id];
            const canReply = !pending && !lock && !message.recalled && canChannelSend(conversation, uid, message);
            const placed = unanswered.after.get(message.id), grouped = row.grouped && !afterTask && !question && !message.channelPost && !message.postId;
            afterTask = !!placed;
            return <Fragment key={row.key}><MessageItem message={localized(message)} grouped={grouped} author={author} mine={mine} owner={message.author.kind === 'shared_bot' ? nameOf(botsById.get(message.author.sharedBotId)?.maintainer.uid ?? '') : ''}
              quote={message.postId && message.replyTo === message.postId ? undefined : quoteOf(message)} locale={tag} now={minute * 60000} renderAvatar={renderAvatar} renderMarkdown={renderMarkdown} question={question && !approval} approval={!!approval} pending={delivery}
              read={mine && message === tail && peerRead !== null && peerRead >= message.seq}
              onReply={!canReply || message.channelPost ? undefined : answering ? () => startAnswer(answering) : answer} onEdit={!pending && mine && participant && !message.decision && message.kind !== 'visualization' ? edit : undefined} onRecall={!pending && mine && participant && !message.decision ? setRecall : undefined}>
              {message.readFiles?.length && !message.recalled ? <p className="tw-msg-sources" title={message.readFiles.map(file => file.name).join('\n')}>
                <Icon name="clip" size={12} /> {t('tw_conv_read_files_fmt', message.readFiles.map(file => file.name).join(t('tw_list_separator')))}</p> : null}
              {message.resourceRef && message.resourceRef.kind !== 'pageVersion' && message.resourceGrant && !message.recalled
                ? <SharedDatasetCard api={workspace.api} reference={message.resourceRef} via={message.resourceGrant} locale={locale} /> : null}
              {message.resourceRef?.kind === 'pageVersion' && !message.recalled &&
                <DatasetPageCard api={workspace.api} reference={message.resourceRef} locale={locale} />}
              {!message.recalled && message.visualization && <VisualizationMessage key={message.visualization.reference.version} api={workspace.api}
                scope={{ ...scope, conversationId: conversation.id }} visualization={message.visualization}
                online={!snapshot.offline} locale={locale}
                forwardTargets={participant && !lock ? snapshot.conversations.filter(item => item.kind === 'channel' && item.participants.includes('user:' + snapshot.uid)) : undefined}
                participantName={uid => snapshot.directory.find(member => member.uid === uid)?.displayName ?? uid} />}
              {(mine || (message.author.kind === 'task' && message.resourceRef?.kind === 'reportRelease')) &&
                participant && !lock && !snapshot.offline && conversation.kind !== 'bot_dm' &&
                message.resourceRef && message.resourceRef.kind !== 'pageVersion' &&
                message.resourceGrant && !message.recalled &&
                <DatasetPagePublisher api={workspace.api} destination={{ team: team.id, conversation: conversation.id }}
                  generation={team.generation} dataset={message.resourceRef.dataset} via={message.resourceGrant} locale={locale} />}
              {message.dataRef && !message.recalled ? <DataRecordButton
                api={workspace.api} scope={{ kind: 'team', teamId: team.id, conversationId: conversation.id, generation: team.generation }}
                reference={message.dataRef} locale={locale} allowEdit /> : null}
              {!message.recalled && parseReportLink(message.text) && <ReportCard key={message.text} api={workspace.api}
                scope={{ kind: 'team', ...scope, conversationId: conversation.id }} reference={parseReportLink(message.text)!} locale={locale} />}
              {message.attachments?.length && !message.recalled ? <MessageFiles api={workspace.api} scope={scope} files={message.attachments}
                locale={locale} layout={message.channelPost ? 'gallery' : undefined} onRevoke={mine && participant ? file => setTakeBack(file) : undefined} /> : null}
              {approval ? <div className="tw-embed"><ApprovalCard api={workspace.api} scope={scope} conversationId={conversation.id} message={message} approval={approval} locale={locale} onChanged={runs.refresh} {...approvalOf(message, waiting)} /></div> : null}
              {answering && !pendingWaits.has(message.waitId!)
                ? <div className="tw-embed"><div className="teams-row-actions"><button type="button" aria-label={t('tw_run_reply')} onClick={() => startAnswer(answering)}>{t('teams_message_reply')}</button></div></div> : null}
              {message.artifactId && !tasks?.some(task => task.artifactId === message.artifactId) ? <ArtifactButton key={message.artifactId} api={workspace.api} scope={scope} artifactId={message.artifactId} locale={locale} renderMarkdown={renderMarkdown} people={snapshot} replyText={replyText} /> : null}
              {tasks?.map(task => <div className="tw-embed" key={task.taskId}>{botRun(task, replyText)}</div>)}
              {message.channelPost && !pending ? <PostActions api={workspace.api} scope={scope} conversationId={conversation.id} post={message} locale={tag}
                online={!snapshot.offline && !covered && !settings} participant={participant} knownReplies={thread?.filter(row => !row.message.recalled).length ?? 0}
                threadComplete={page?.next === null}
                openForReply={sharedPostId === message.id || reply?.id === message.id || reply?.postId === message.id}
                needsThread={!page && !threadLoading.has(message.id) && !threadErrors.has(message.id)}
                loadThread={() => void loadPost(message.id)} onReply={canReply ? () => answering ? startAnswer(answering) : answer(message) : undefined}>
                  {page?.next || threadErrors.has(message.id) ? <button type="button" className="tw-link" disabled={threadLoading.has(message.id) || snapshot.offline}
                    onClick={() => void loadPost(message.id, Boolean(page?.next))}>{threadErrors.has(message.id) ? t('common_retry') : t('teams_messages_earlier')}</button> : null}
                  {threadLoading.has(message.id) ? <p role="status">{t('teams_loading_messages')}</p> : null}
                  {thread?.length ? <ol className="tw-messages">{thread.map(renderRow)}</ol> : page && !threadLoading.has(message.id)
                    ? <p>{t('tw_channel_replies_empty')}</p> : null}
              </PostActions> : null}
            </MessageItem>{!sharedPostId && placed?.map(taskRow)}</Fragment>;
          })}
          {!sharedPostId && unanswered.tail.map(taskRow)}
          {!sharedPostId && placedHeld.orphans.map(row => <HeldReplyRow key={`held:${row.reply.taskId}`} row={row} author={botIdentity(row.reply.sharedBotId)} renderAvatar={renderAvatar} locale={locale}>{heldCard(row)}</HeldReplyRow>)}
          {!sharedPostId && operations.map(intent => {
            const payload = pendingPayload(intent);
            const operation = 'operation' in payload ? payload.operation : 'create';
            return <li key={payload.changeId} className="tw-msg tw-pending" data-mine="true" data-error={intent.error ? 'true' : undefined}>
              <div className="tw-msg-main tw-msg-bubble">
                <div className="tw-msg-head"><small role="status">{pendingLabel(intent)}</small></div>
                <p className="tw-msg-body">{operation === 'recall' ? t('tw_conv_recall_message') : `${operation === 'edit' ? `${t('teams_message_edit')}: ` : ''}${payload.text}`}</p>
                {'attachments' in payload && payload.attachments?.length ? <p className="tw-msg-body tw-pending-files"><Icon name="clip" size={12} /> {payload.attachments.length === 1 ? t('tw_files_one') : t('tw_files_other_fmt', payload.attachments.length)}</p> : null}
                {!intent.attempted || intent.error ? <div className="tw-pending-actions">{pendingActions(intent)}</div> : null}
              </div>
            </li>;
          })}
        </ol>
      </div>
      {!sharedPostId && (!pinned || (tail && tail.seq < conversation.lastSeq)) ? <button type="button" className="tw-jump" onClick={() => { if(tail && tail.seq<conversation.lastSeq) model.select(conversation.id); void workspace.markRead(); toBottom(); pinnedRef.current = true; setPinned(true); }}>
        <Icon name="arrowDown" />{unseen > 0 ? t('teams_new_messages_pill_fmt', unseen) : t('tw_conv_latest')}
      </button> : null}
    </div>

    {(!sharedPostId || reply || editing || answerTask) && <Composer workspace={workspace} snapshot={snapshot} conversation={conversation} label={label} locale={locale} renderAvatar={renderAvatar}
      editing={editing} reply={reply ? { message: reply, author: authorOf(reply).name } : undefined} focusToken={focusToken}
      answering={answerTask ? { waitId: answerTask.wait!.waitId, author: botIdentity(answerTask.sharedBotId).name, question: byId.get(answerTask.wait!.messageId) } : undefined}
      onClearAnswering={() => setAnswerWait(undefined)} onAnswered={runs.refresh}
      onClearEditing={() => setEditing(undefined)} onClearReply={() => setReply(undefined)} onEditLast={() => {
        if (!participant) return;
        for (let i = messages.length - 1; i >= 0; i--) {
          const message = messages[i];
          if (isMine(message) && !message.recalled) { edit(message); break; }
        }
      }} />}

    {autoReplyOpen ? <AutoReplyDialog workspace={workspace} snapshot={snapshot} conversation={conversation} scope={scope} locale={locale} onClose={() => setAutoReplyOpen(false)} /> : null}
    {botScopeOpen ? <BotScopeDialog workspace={workspace} snapshot={snapshot} conversation={conversation} scope={scope} locale={locale} onClose={() => setBotScopeOpen(false)} /> : null}
    {triageOpen && logBots.length ? <TriageLogDialog api={workspace.api} scope={scope} bots={logBots} conversation={{ id: conversation.id, title: label }} locale={locale} onClose={() => setTriageOpen(false)}
      onDenied={id => setTriageDenied(old => [...old, id])} /> : null}
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
  </section></>;
}

/** The row of a reply nobody asked for that waits for its Bot's maintainer: the service lists no task for it, so it is a row of its own, like a task's. */
function HeldReplyRow({ row, author, locale, renderAvatar, children }: { row: HeldRow; author: Identity; locale: string; renderAvatar?: RenderAvatar; children: ReactNode }): ReactElement {
  const t = useTeamsText(locale);
  return <li className="tw-msg" data-grouped="false" data-kind="bot" data-task-id={row.reply.taskId} data-task-state="awaiting_approval" data-held="true">
    <div className="tw-msg-lead"><Avatar identity={author} size={32} render={renderAvatar} /></div>
    <div className="tw-msg-main">
      <div className="tw-msg-head"><b className="tw-msg-author">{author.name}</b><span className="tw-tag">{t('teams_bot_tag')}</span></div>
      <div className="tw-embed">{children}</div>
    </div>
  </li>;
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

const MessageMarkdown = memo(function MessageMarkdown({ text, render }: { text: string; render: RenderMarkdown }) {
  return <div className="tw-msg-body tw-md" data-markdown="true">{render(text)}</div>;
});

type PendingDelivery = { label: string; state: 'failed' | 'pending' | 'sent'; fileCount: number; actions?: ReactNode };
const deliveryIcon = { failed: 'circleAlert', pending: 'clock', sent: 'check', read: 'checks' } as const;

function PostMenu({ items, label }: { items: MenuItem[]; label: string }) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  return <div className="tw-post-menu">
    <button type="button" className="tw-icon-btn" ref={trigger} aria-label={label} title={label} aria-haspopup="menu"
      aria-expanded={open} onClick={() => setOpen(!open)}><Icon name="more" size={18} /></button>
    {open && <PopupMenu items={items} label={label} anchor={trigger} align="end" onClose={restore => {
      setOpen(false); if (restore) trigger.current?.focus();
    }} />}
  </div>;
}

function messageInlineText(text: string, formats: MessageFormat[], mentions: TeamMessage['mentionEntities']): ReactNode {
  const runs = messageFormatParts(text, formats);
  let offset = 0, index = 0;
  return mentionTextParts(text, mentions).map(part => {
    const start = offset;
    const end = offset + part.text.length;
    const children = [];
    while (index < runs.length) {
      const run = runs[index]!;
      const runEnd = run.offset + run.text.length;
      const from = Math.max(start, run.offset), to = Math.min(end, runEnd);
      if (from < to) {
        let content: ReactNode = text.slice(from, to);
        for (const [style, Tag] of formatTags) if (run.style & style) content = <Tag>{content}</Tag>;
        children.push(<Fragment key={from}>{content}</Fragment>);
      }
      if (runEnd > end) break;
      index++;
    }
    offset = end;
    return part.mention ? <strong className="tw-member-mention" key={start}>{children}</strong> : <Fragment key={start}>{children}</Fragment>;
  });
}

function MessageItem({ message, grouped, author, mine, owner, quote, question, approval, locale, now, renderAvatar, renderMarkdown, onReply, onEdit, onRecall, pending, read, children }: {
  message: TeamMessage; grouped: boolean; author: Identity; mine: boolean; owner: string; quote?: { author: string; text: string }; question?: boolean; /** A request to approve a change; the card under it says what. */ approval?: boolean; locale: string; renderAvatar?: RenderAvatar; renderMarkdown?: RenderMarkdown;
  onReply?(message: TeamMessage): void; onEdit?(message: TeamMessage): void; onRecall?(message: TeamMessage): void; children?: ReactNode;
  pending?: PendingDelivery; now: number;
  read: boolean;
}): ReactElement {
  const t = useTeamsText(locale);
  const { clock, full } = useMemo(() => ({
    clock: formatClock(message.createdAt, locale),
    full: message.createdAt ? new Date(message.createdAt).toLocaleString(locale) : undefined,
  }), [message.createdAt, locale]);
  const acts = !message.recalled && (onReply || onEdit || onRecall || pending?.actions);
  // Bot replies use Markdown. Human messages carry explicit inline formatting ranges.
  const markdown = message.author.kind === 'shared_bot' && !message.recalled && renderMarkdown !== undefined;
  const hasFiles = !!(message.attachments?.length || pending?.fileCount);
  const filesOnly = !message.text && !message.recalled && hasFiles;
  const body = message.visualization?.title === message.text ? null : markdown ? <MessageMarkdown text={message.text} render={renderMarkdown} /> : filesOnly ? null : <p className="tw-msg-body">
    {message.recalled ? <em>{t('teams_message_recalled')}</em> : message.mentionEntities?.length || message.formats?.length
      ? messageInlineText(message.text, message.formats ?? [], message.mentionEntities) : message.text}
  </p>;
  const postEntry = Boolean(message.channelPost || message.postId);
  const shownTime = postEntry ? formatPostTime(message.createdAt, now, locale) : clock;
  const edited = message.editedAt && !message.recalled;
  const delivery = mine && !message.recalled;
  const label = pending?.label ?? t(read ? 'tw_message_read' : 'tw_message_sent');
  const state = pending?.state ?? (read ? 'read' : 'sent');
  const meta = clock || edited || delivery ? <div className="tw-msg-meta">
    {edited ? <small className="tw-edited">({t('tw_edited_tag')})</small> : null}
    {shownTime ? <time className="tw-msg-time" dateTime={message.createdAt ?? undefined} title={full} aria-label={full}>{shownTime}</time> : null}
    {delivery ? <span className="tw-msg-delivery" data-state={state} role="img" aria-label={state === 'failed' ? t('tw_sidebar_not_sent') : label} title={label}>
      <Icon name={deliveryIcon[state]} size={13} />
    </span> : null}
  </div> : null;
  const bot = author.kind === 'bot';
  const inlineTime = !bot && !!body && !hasFiles && !!meta;
  const actions = acts ? <div className="tw-msg-actions" role="group" aria-label={t('teams_message_actions')}>
    {onReply ? <button type="button" className="tw-icon-btn" aria-label={t('teams_message_reply')} title={t('teams_message_reply')} onClick={() => onReply(message)}><Icon name="reply" /></button> : null}
    {onEdit ? <button type="button" className="tw-icon-btn" aria-label={t('teams_message_edit')} title={t('teams_message_edit')} onClick={() => onEdit(message)}><Icon name="edit" /></button> : null}
    {onRecall ? <button type="button" className="tw-icon-btn tw-danger" aria-label={t('teams_message_recall')} title={t('teams_message_recall')} onClick={() => onRecall(message)}><Icon name="trash" /></button> : null}
    {pending?.actions}
  </div> : null;
  const authorHeader = <div className="tw-msg-head">
    <b className="tw-msg-author">{author.name}</b>
    {bot ? <span className="tw-msg-bot-label">{t('teams_bot_tag')}</span> : null}
    {question ? <span className="tw-tag">{t('services_question')}</span> : null}
    {approval ? <span className="tw-tag">{t('teams_approval_tag')}</span> : null}
    {owner ? <small>{t('teams_bot_owner_fmt', owner)}</small> : null}
    {postEntry && meta ? <><span className="tw-post-dot" aria-hidden="true">·</span>{meta}</> : null}
  </div>;
  const quoteContent = quote ? <blockquote className="tw-quote">{quote.author ? <b>{quote.author}</b> : null} {quote.text}</blockquote> : null;
  const pendingFiles = pending?.fileCount ? <p className="tw-msg-body tw-pending-files"><Icon name="clip" size={12} /> {pending.fileCount === 1 ? t('tw_files_one') : t('tw_files_other_fmt', pending.fileCount)}</p> : null;
  const deliveryError = state === 'failed' && pending ? <small className="tw-msg-delivery-error" role="status">{pending.label}</small> : null;
  const messageAttributes = { 'data-recalled': message.recalled ? 'true' : undefined, 'data-message-id': message.id, 'data-message-seq': pending ? undefined : message.seq };

  if (postEntry) {
    const items: MenuItem[] = [];
    if (!message.recalled && onEdit) items.push({ id: 'edit', label: t('teams_message_edit'), icon: 'edit', onSelect: () => onEdit(message) });
    if (!message.recalled && onRecall) items.push({ id: 'recall', label: t('teams_message_recall'), icon: 'trash', danger: true, onSelect: () => onRecall(message) });
    return <li className={message.channelPost ? 'tw-post' : 'tw-post-comment'} data-channel-post={message.channelPost} {...messageAttributes}>
      <Avatar identity={author} size={message.channelPost ? 40 : 32} render={renderAvatar} />
      <div className="tw-post-main">
        <div className="tw-post-header">{authorHeader}{items.length > 0 && <PostMenu items={items} label={t('teams_message_actions')} />}</div>
        {quoteContent}{body}{pendingFiles}
        {children}
        {onReply && !message.recalled && <button type="button" className="tw-post-write" onClick={() => onReply(message)}><Icon name="reply" size={14} />{t('teams_message_reply')}</button>}
        {deliveryError}{pending?.actions}
      </div>
    </li>;
  }

  return <li className="tw-msg" data-grouped={grouped} data-mine={mine} data-kind={author.kind} {...messageAttributes}>
    {!mine ? <div className="tw-msg-lead">{!grouped ? <Avatar identity={author} size={32} render={renderAvatar} /> : null}</div> : null}
    <div className="tw-msg-main">
      <div className={author.kind === 'user' ? 'tw-msg-bubble' : 'tw-msg-agent'}>
        {!mine && !grouped ? authorHeader : null}
        {quoteContent}
        {inlineTime ? <div className="tw-msg-text-line">{body}{meta}</div> : body}
        {pendingFiles}
        {children}
        {bot ? (meta || actions) && <div className="tw-msg-footer">{meta}{actions}</div> : inlineTime ? null : meta}
      </div>
      {deliveryError}
      {bot ? null : actions}
    </div>
  </li>;
}
