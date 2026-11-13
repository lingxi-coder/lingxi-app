import { Fragment, useEffect, useMemo, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { useTeamTasks } from './bot-runs.js';
import type { TeamTaskView } from './execution.js';
import { MemberHome } from './MemberHome';
import { snippet } from './compose.js';
import { Composer, composerLock } from './Composer';
import { messageErrorText } from './errors.js';
import { ApprovalCard } from './ApprovalCard';
import { MessageFiles } from './MessageFiles';
import type { MessageAttachment } from './attachments.js';
import { executionCommand } from './execution.js';
import { pendingPayload, type TeamConversation, type TeamMessage } from './messages.js';
import { ArtifactButton, BotRun } from './BotRunCards';
import { buildStreamRows, formatClock, placeTasks, visibleStart, WINDOW_SIZE } from './stream.js';
import { Avatar, ConversationLead, Dialog, Icon, platformChip, platformNotice, useDocumentShown, useIsoLayoutEffect, type RenderAvatar, type RenderMarkdown } from './ui';
import { conversationLabel, unreadCount, type Identity, type TeamsSnapshot, type TeamsWorkspace } from './workspace.js';

/**
 * A friend chat (D9) is this view on a pair team: the header is the friend, there is no details button or platform chip, there are no
 * Bot runs, and the chat marks itself read while its newest message is in view. A Bot DM of a friendship (D22) is the plain Teams view.
 */
export interface FriendHeader {
  uid: string; name: string; handle?: string;
  /** The right of the header: the friend menu. */
  actions?: ReactNode;
}

export interface TeamConversationViewProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  zh: boolean;
  /** Whether the details drawer is open. With `onToggleDetails` the header shows a button for it. */
  detailsOpen?: boolean;
  onToggleDetails?(): void;
  renderAvatar?: RenderAvatar;
  /** Renders a shared Bot's messages and documents as Markdown. People's messages stay plain text. */
  renderMarkdown?: RenderMarkdown;
  /** BCP 47 tag for times and day labels; defaults to `zh-CN` or `en-US` from `zh`. */
  locale?: string;
  /** The current time for "Today" and "Yesterday". Defaults to now. */
  now?: number | Date;
  friend?: FriendHeader;
  /** A Back button at the start of the header: a friendship's Bot DM returns to the friend chat (D22). */
  onBack?(): void;
}

/** The second line of the header: who is in the conversation, or whether its Bot is reachable. */
export function conversationSubtitle(conversation: TeamConversation, snapshot: Pick<TeamsSnapshot, 'bots' | 'directory' | 'uid'>, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  if (conversation.kind === 'bot_dm') {
    const card = snapshot.bots.find(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`));
    if (!card) return '';
    const owner = snapshot.directory.find(member => member.uid === card.maintainer.uid)?.displayName || (card.maintainer.uid === snapshot.uid ? t('You', '你') : t('Member', '成员'));
    const presence = card.state !== 'published' ? t('Disabled', '已停用') : card.ready ? t('Online', '在线') : t('Host unavailable', '宿主不可用');
    return `${presence} · ${t('Owner', '所有者')}: ${owner}`;
  }
  if (conversation.kind === 'dm') return t('Direct message', '私信');
  // Participants mix people (`user:`) and shared Bots (`shared_bot:`); only people are members.
  const members = conversation.participants.filter(id => id.startsWith('user:')).length;
  const bots = conversation.participants.filter(id => id.startsWith('shared_bot:')).length;
  const people = members ? t(`${members} members`, `${members} 位成员`) : '';
  const botText = bots ? t(`${bots} ${bots === 1 ? 'Bot' : 'Bots'}`, `${bots} 个 Bot`) : '';
  return [people, botText].filter(Boolean).join(' · ');
}

/** What the pane says while no conversation is open, and why. */
function emptyState(snapshot: TeamsSnapshot, zh: boolean): { title: string; body: string } {
  const t = (en: string, cn: string) => zh ? cn : en;
  if (!snapshot.teams.length) return { title: t('Create or join a team', '创建或加入团队'), body: t('Use the sidebar to create a team, or join one with an invite link.', '在侧边栏创建团队，或通过邀请链接加入。') };
  if (!snapshot.team) return { title: t('Choose a team', '选择团队'), body: t('Pick a team from the switcher in the sidebar.', '在侧边栏的团队切换器中选择团队。') };
  if (!snapshot.conversations.length) return { title: t('No conversations yet', '还没有会话'), body: t('Create a channel, or start a message with a teammate or a Bot from the sidebar.', '在侧边栏新建频道，或与成员、Bot 开始对话。') };
  return { title: t('Choose a conversation', '选择会话'), body: t('Pick a channel or a message from the sidebar.', '在侧边栏选择频道或私信。') };
}

/**
 * The middle column: header, message stream, composer. It shows the workspace's open conversation
 * and is rebuilt (drafts aside) whenever another one is opened.
 */
export function TeamConversationView(props: TeamConversationViewProps): ReactElement {
  const { snapshot, zh } = props;
  const conversation = snapshot.conversation, model = snapshot.messaging, team = snapshot.team;
  if (!conversation || !model || !team) {
    if (snapshot.loading) return <section className="tw-scope tw-conv tw-conv-empty"><p className="tw-empty-line" role="status">{zh ? '正在加载…' : 'Loading…'}</p></section>;
    if (team && !snapshot.selectedConversationId && !snapshot.creation) return <MemberHome key={`${snapshot.uid}:${team.id}:${team.generation}`} workspace={props.workspace} snapshot={snapshot} zh={zh} renderMarkdown={props.renderMarkdown} />;
    const { title, body } = emptyState(snapshot, zh);
    return <section className="tw-scope tw-conv tw-conv-empty">
      <div className="tw-empty"><h2>{title}</h2><p>{body}</p><p className="tw-hint">{platformNotice(zh)}</p></div>
      {snapshot.creation ? <CreationBanner workspace={props.workspace} snapshot={snapshot} zh={zh} /> : null}
    </section>;
  }
  return <ConversationPane key={`${snapshot.uid}:${team.id}:${team.generation}:${conversation.id}`} {...props} conversation={conversation} model={model} team={team} />;
}

export function CreationBanner({ workspace, snapshot, zh }: { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; zh: boolean }): ReactElement | null {
  const creation = snapshot.creation, model = snapshot.messaging;
  if (!creation || !model) return null;
  const t = (en: string, cn: string) => zh ? cn : en;
  return <div className="tw-banner" role="status" data-kind="creation">
    <span>{t('Creating conversation…', '正在创建会话…')} <b>{pendingPayload(creation).title}</b> {creation.error ? messageErrorText(creation.error, zh) : ''}</span>
    <button type="button" className="tw-link" disabled={Boolean(creation.attempted && !creation.error) || (creation.leaseUntil ?? 0) > Date.now()}
      onClick={() => void workspace.attempt(() => model.discardCreation())}>{t('Discard', '丢弃')}</button>
  </div>;
}

type Pane = TeamConversationViewProps & { conversation: TeamConversation; model: NonNullable<TeamsSnapshot['messaging']>; team: NonNullable<TeamsSnapshot['team']> };

function ConversationPane({ workspace, snapshot, zh, conversation, model, team, detailsOpen, onToggleDetails, renderAvatar, renderMarkdown, locale, now, friend, onBack }: Pane): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const uid = snapshot.uid, messages = snapshot.messages, tag = locale ?? (zh ? 'zh-CN' : 'en-US');
  const minute = Math.floor((now === undefined ? Date.now() : typeof now === 'number' ? now : now.getTime()) / 60_000);
  const [startId, setStartId] = useState<string>();
  const [editing, setEditing] = useState<TeamMessage>(), [reply, setReply] = useState<TeamMessage>(), [recall, setRecall] = useState<TeamMessage>();
  const [takeBack, setTakeBack] = useState<MessageAttachment>();
  const [answerWait, setAnswerWait] = useState<string>();
  const [focusToken, setFocusToken] = useState(0);
  const [pinned, setPinned] = useState(true);
  const [seen, setSeen] = useState(() => messages[messages.length - 1]?.seq ?? 0);
  const scroller = useRef<HTMLDivElement>(null), content = useRef<HTMLOListElement>(null);
  const pinnedRef = useRef(true), placed = useRef(false), anchor = useRef<{ height: number; top: number }>(), pendingCount = useRef(snapshot.pending.length);

  const label = friend ? friend.name : conversationLabel(conversation, snapshot.directory, uid, { bots: snapshot.bots, zh });
  const unread = unreadCount(conversation);
  const start = visibleStart(messages, { startId, lastReadSeq: conversation.lastReadSeq, uid });
  const visible = useMemo(() => messages.slice(start), [messages, start]);
  const rows = useMemo(() => buildStreamRows(visible, { lastReadSeq: conversation.lastReadSeq, uid, locale: tag, now: minute * 60_000 }), [visible, conversation.lastReadSeq, uid, tag, minute]);
  const byId = useMemo(() => new Map(messages.map(message => [message.id, message])), [messages]);
  const runs = useTeamTasks(workspace.api, { teamId: team.id, generation: team.generation }, conversation.id, !friend);

  const lock = composerLock({ uid, conversation, bots: snapshot.bots, botsLoaded: snapshot.botsLoaded, botsUnverified: snapshot.botsUnverified }, zh);
  const participant = !conversation.participants.length || conversation.participants.includes(`user:${uid}`);
  const nameOf = (id: string) => snapshot.directory.find(member => member.uid === id)?.displayName || (id === uid ? t('You', '你') : t('Member', '成员'));
  const isViewer = (who: { uid: string; generation: number }) => who.uid === uid && who.generation === team.generation;
  const authorOf = (message: TeamMessage): Identity => {
    if (message.author.kind === 'user') return { kind: 'user', id: message.author.uid, name: nameOf(message.author.uid) };
    const author = message.author;
    const card = snapshot.bots.find(bot => bot.sharedBotId === author.sharedBotId);
    return { kind: 'bot', id: card?.sharedBotId ?? message.author.sharedBotId, name: card?.name || message.author.displayName || 'Bot' };
  };
  const botIdentity = (sharedBotId: string): Identity => ({ kind: 'bot', id: sharedBotId, name: snapshot.bots.find(bot => bot.sharedBotId === sharedBotId)?.name || 'Bot' });
  const isMine = (message: TeamMessage) => message.author.kind === 'user' && message.author.uid === uid;
  /** What a message says in one line: its text, or the names of its files when it is files alone. */
  const said = (message: TeamMessage) => message.text || (message.attachments ?? []).map(file => file.name).join(', ');
  const quoteOf = (message: TeamMessage) => {
    if (!message.replyTo) return undefined;
    const target = byId.get(message.replyTo);
    if (!target) return { author: '', text: t('Reply to an earlier message', '回复早前的消息') };
    return { author: authorOf(target).name, text: target.recalled ? t('Recalled message', '已撤回消息') : snippet(said(target), 140) };
  };
  const scope = { teamId: team.id, generation: team.generation };
  // Answer mode ends by itself once the task is not waiting for this viewer on that question any more.
  const answerTask = answerWait ? runs.tasks.find(task => task.state === 'waiting' && task.wait?.waitId === answerWait && task.wait.kind === 'human_input' && isViewer(task.wait.recipient)) : undefined;
  useEffect(() => { if (answerWait && !answerTask) setAnswerWait(undefined); }, [answerWait, answerTask]);
  // Only a question is answered in words; a proposed change is approved or declined on its own card.
  const answerable = (task: TeamTaskView | undefined) => !lock && !!task?.wait && task.wait.kind === 'human_input' && isViewer(task.wait.recipient);
  /** The service writes an approval request and its reply in English; a Chinese viewer reads them in Chinese. */
  const localized = (message: TeamMessage): TeamMessage => !zh || message.recalled ? message
    : message.approval && message.author.kind === 'shared_bot' ? { ...message, text: `请求修改清单“${message.approval.title}”（${message.approval.changes.length} 项修改）。` }
    : message.decision && message.waitId ? { ...message, text: message.decision === 'approve' ? '已批准' : '已拒绝' } : message;
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
  const botRun = (task: TeamTaskView) => <BotRun task={task} api={workspace.api} scope={scope} zh={zh} renderMarkdown={renderMarkdown} snapshot={snapshot}
    onReply={answerable(task) ? startAnswer : undefined} onChanged={runs.refresh} />;
  const taskRow = (task: TeamTaskView) => <TaskRow key={`task:${task.taskId}`} task={task} author={botIdentity(task.sharedBotId)} renderAvatar={renderAvatar}>
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
    if (divider) node.scrollTop = Math.max(0, divider.offsetTop - 56); else toBottom();
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
  }, [start]);
  useEffect(() => { if (pinned) setSeen(tail?.seq ?? 0); }, [pinned, tail?.seq]);
  // A friend chat reads itself while it is shown and its newest message is in view (D13); a lost mark is retried by the next change.
  const shown = useDocumentShown();
  useEffect(() => {
    if (friend && shown && pinned && unread > 0 && !snapshot.offline) void workspace.markRead();
  // The mark depends on what is in view, not on the workspace object or the busy flag.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [friend, shown, pinned, unread, tail?.seq, snapshot.offline]);
  useEffect(() => {
    const node = content.current, box = scroller.current;
    if (!node || !box || typeof ResizeObserver === 'undefined') return;
    // Bot cards and images change height after they are shown.
    const observer = new ResizeObserver(() => { if (pinnedRef.current) box.scrollTop = box.scrollHeight; });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const showEarlier = () => {
    const node = scroller.current;
    if (node) anchor.current = { height: node.scrollHeight, top: node.scrollTop };
    setStartId(messages[Math.max(0, start - WINDOW_SIZE)]?.id);
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
  const subtitle = conversationSubtitle(conversation, snapshot, zh);

  return <section className="tw-scope tw-conv" aria-label={label}>
    <header className="tw-conv-head">
      {onBack ? <button type="button" className="tw-icon-btn" aria-label={t('Back', '返回')} title={t('Back', '返回')} onClick={onBack}><Icon name="chevronLeft" /></button> : null}
      {friend ? <Avatar identity={{ kind: 'user', id: friend.uid, name: friend.name }} size={32} render={renderAvatar} />
        : <ConversationLead conversation={conversation} label={label} snapshot={snapshot} zh={zh} size={32} render={renderAvatar} />}
      <div className="tw-conv-title">
        <h2>{label}</h2>
        {friend ? friend.handle ? <p>@{friend.handle}</p> : null : subtitle ? <p>{subtitle}</p> : null}
      </div>
      {friend ? null : <span className="tw-platform" title={platformNotice(zh)}>{platformChip(zh)}</span>}
      {/* A friend chat reads itself (D13), so it has no manual mark, as on Android. */}
      {unread > 0 && !friend ? <button type="button" disabled={snapshot.messageBusy || snapshot.offline} onClick={() => void workspace.markRead()}>{t('Mark as read', '标为已读')}</button> : null}
      {friend?.actions}
      {onToggleDetails && !friend ? <button type="button" className="tw-icon-btn" aria-pressed={!!detailsOpen} aria-label={t('Team details', '团队详情')} title={t('Team details', '团队详情')} onClick={onToggleDetails}><Icon name="panel" /></button> : null}
    </header>
    {snapshot.messagingError
      ? <div className="tw-banner" role="alert" data-kind="error"><span>{messageErrorText(snapshot.messagingError, zh)}</span><button type="button" className="tw-link" onClick={() => workspace.reconnect()}>{t('Reconnect', '重新连接')}</button></div>
      : snapshot.messagingConnecting
        ? <div className="tw-banner" role="status" data-kind="connecting">{t('Connecting… Waiting for the server to confirm access.', '正在连接…等待服务器确认访问权限。')}</div>
      : snapshot.offline
        ? <div className="tw-banner" role="status" data-kind="offline"><span>{t('Saved on this device. Reconnect to confirm access and send pending messages.', '当前显示本机缓存。联网后确认权限并发送待发消息。')}</span><button type="button" className="tw-link" onClick={() => workspace.reconnect()}>{t('Reconnect', '重新连接')}</button></div>
        : null}
    {snapshot.actionError && snapshot.actionError !== snapshot.messagingError ? <p className="tw-banner" role="alert" data-kind="error">{messageErrorText(snapshot.actionError, zh)}</p> : null}
    <CreationBanner workspace={workspace} snapshot={snapshot} zh={zh} />
    {runs.error ? <p role="alert" className="tw-banner">{messageErrorText(runs.error, zh)}</p> : null}

    <div className="tw-stream-wrap">
      <div ref={scroller} className="tw-stream" role="log" aria-label={t('Messages', '消息')} tabIndex={0} onScroll={measure}>
        {start > 0 ? <div className="tw-earlier"><button type="button" onClick={showEarlier}>{t('Show earlier messages', '显示更早的消息')} ({start})</button></div> : null}
        {!messages.length && !snapshot.pending.length && !runs.tasks.length ? <p className="tw-empty-line" role="status">{loadingMessages ? t('Loading messages…', '正在加载消息…') : t('No messages yet. Say hello.', '还没有消息，打个招呼吧。')}</p> : null}
        <ol ref={content} className="tw-messages">
          {unanswered.head.map(taskRow)}
          {rows.map(row => {
            if (row.type === 'date') return <li key={row.key} className="tw-date"><span>{row.label}</span></li>;
            if (row.type === 'unread') return <li key={row.key} className="tw-unread" data-unread-divider=""><span>{t('New messages', '新消息')}</span></li>;
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
              quote={quoteOf(message)} locale={tag} zh={zh} renderAvatar={renderAvatar} renderMarkdown={renderMarkdown} question={question && !approval} approval={!!approval}
              onReply={lock ? undefined : answering ? () => startAnswer(answering) : answer} onEdit={mine && participant && !message.decision ? edit : undefined} onRecall={mine && participant && !message.decision ? setRecall : undefined}>
              {message.attachments?.length && !message.recalled ? <MessageFiles api={workspace.api} scope={scope} files={message.attachments} zh={zh} onRevoke={mine && participant ? file => setTakeBack(file) : undefined} /> : null}
              {approval ? <div className="tw-embed"><ApprovalCard api={workspace.api} scope={scope} conversationId={conversation.id} message={message} approval={approval} zh={zh} onChanged={runs.refresh} {...approvalOf(message, waiting)} /></div> : null}
              {answering && !answerPending(message.waitId!)
                ? <div className="tw-embed"><div className="teams-row-actions"><button type="button" aria-label={t('Reply to the Bot’s question', '回复 Bot 的问题')} onClick={() => startAnswer(answering)}>{t('Reply', '回复')}</button></div></div> : null}
              {message.artifactId && !tasks.some(task => task.artifactId === message.artifactId) ? <ArtifactButton key={message.artifactId} api={workspace.api} scope={scope} artifactId={message.artifactId} zh={zh} renderMarkdown={renderMarkdown} people={snapshot} /> : null}
              {tasks.map(task => <div className="tw-embed" key={task.taskId}>{botRun(task)}</div>)}
            </MessageItem>{placed?.map(taskRow)}</Fragment>;
          })}
          {unanswered.tail.map(taskRow)}
          {snapshot.pending.map(intent => {
            const payload = pendingPayload(intent);
            const operation = 'operation' in payload ? payload.operation : 'create';
            const status = (intent.error ? messageErrorText(intent.error, zh) : '') || (intent.attempted ? t('Waiting for server confirmation. Retries keep the same message identity.', '等待服务器确认，重试会沿用同一条消息。') : t('Pending · sends when connected', '待发送 · 联网后发送'));
            return <li key={payload.changeId} className="tw-msg tw-pending" data-error={intent.error ? 'true' : undefined}>
              <div className="tw-msg-lead"><Avatar identity={{ kind: 'user', id: uid, name: nameOf(uid) }} size={32} render={renderAvatar} /></div>
              <div className="tw-msg-main">
                <div className="tw-msg-head"><b className="tw-msg-author">{nameOf(uid)}</b><small role="status">{status}</small></div>
                <p className="tw-msg-body">{operation === 'recall' ? t('Recall message', '撤回消息') : `${operation === 'edit' ? `${t('Edit', '编辑')}: ` : ''}${payload.text}`}</p>
                {'attachments' in payload && payload.attachments?.length ? <p className="tw-msg-body tw-pending-files"><Icon name="clip" size={12} /> {t(payload.attachments.length === 1 ? '1 file' : `${payload.attachments.length} files`, `${payload.attachments.length} 个文件`)}</p> : null}
                <div className="tw-pending-actions">
                  {intent.error === 'HOST_UNAVAILABLE' ? <button type="button" onClick={() => void workspace.attempt(() => model.retryHostUnavailable(payload.changeId))}>{t('Retry', '重试')}</button> : null}
                  <button type="button" disabled={Boolean(intent.attempted && !intent.error) || (intent.leaseUntil ?? 0) > Date.now()} onClick={() => void workspace.attempt(() => model.discard(payload.changeId))}>{t('Discard', '丢弃')}</button>
                </div>
              </div>
            </li>;
          })}
        </ol>
      </div>
      {!pinned ? <button type="button" className="tw-jump" onClick={() => { toBottom(); pinnedRef.current = true; setPinned(true); }}>
        <Icon name="arrowDown" />{unseen > 0 ? t(`${unseen} new`, `${unseen} 条新消息`) : t('Latest', '最新')}
      </button> : null}
    </div>

    <Composer workspace={workspace} snapshot={snapshot} conversation={conversation} label={label} zh={zh} renderAvatar={renderAvatar}
      editing={editing} reply={reply ? { message: reply, author: authorOf(reply).name } : undefined} focusToken={focusToken}
      answering={answerTask ? { waitId: answerTask.wait!.waitId, author: botIdentity(answerTask.sharedBotId).name, question: byId.get(answerTask.wait!.messageId) } : undefined}
      onClearAnswering={() => setAnswerWait(undefined)} onAnswered={runs.refresh}
      onClearEditing={() => setEditing(undefined)} onClearReply={() => setReply(undefined)} onEditLast={() => { if (lastOwn && participant) edit(lastOwn); }} />

    {takeBack ? <Dialog role="alertdialog" title={t(`Take back ${takeBack.name}?`, `收回 ${takeBack.name}？`)} onClose={() => setTakeBack(undefined)}>
      <div className="tw-form">
        <p className="tw-hint">{t('Nobody will be able to open or save it from this conversation any more, and an assistant will not read it again. Copies people already saved stay with them. This cannot be undone.', '此后任何人都无法再从此会话打开或保存它，助手也不会再读取它。他人已保存的副本仍在其设备上。此操作无法撤销。')}</p>
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setTakeBack(undefined)}>{t('Cancel', '取消')}</button>
          <button type="button" className="tw-danger-btn" disabled={snapshot.messageBusy} onClick={() => void confirmTakeBack()}>{t('Take back', '收回')}</button>
        </div>
      </div>
    </Dialog> : null}
    {recall ? <Dialog role="alertdialog" title={t('Recall this message for everyone?', '为所有人撤回此消息？')} onClose={() => setRecall(undefined)}>
      <div className="tw-form">
        <blockquote className="tw-quote">{snippet(said(recall), 140)}</blockquote>
        <p className="tw-hint">{t('Everyone in this conversation will see that it was recalled.', '会话中的所有人都会看到此消息已被撤回。')}{recall.attachments?.some(file => !file.revoked) ? ` ${t('Its files can no longer be opened.', '其中的文件将无法再打开。')}` : ''}</p>
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setRecall(undefined)}>{t('Cancel', '取消')}</button>
          <button type="button" className="tw-danger-btn" disabled={snapshot.messageBusy} onClick={() => void confirmRecall()}>{t('Recall', '撤回')}</button>
        </div>
      </div>
    </Dialog> : null}
  </section>;
}

/** The row of a Bot task that has no reply message yet: the Bot's avatar and name, then its status card. */
function TaskRow({ task, author, renderAvatar, children }: { task: TeamTaskView; author: Identity; renderAvatar?: RenderAvatar; children: ReactNode }): ReactElement {
  return <li className="tw-msg" data-grouped="false" data-kind="bot" data-task-id={task.taskId} data-task-state={task.state}>
    <div className="tw-msg-lead"><Avatar identity={author} size={32} render={renderAvatar} /></div>
    <div className="tw-msg-main">
      <div className="tw-msg-head"><b className="tw-msg-author">{author.name}</b><span className="tw-tag">Bot</span></div>
      {children}
    </div>
  </li>;
}

function MessageItem({ message, grouped, author, mine, owner, quote, question, approval, locale, zh, renderAvatar, renderMarkdown, onReply, onEdit, onRecall, children }: {
  message: TeamMessage; grouped: boolean; author: Identity; mine: boolean; owner: string; quote?: { author: string; text: string }; question?: boolean; /** A request to approve a change; the card under it says what. */ approval?: boolean; locale: string; zh: boolean; renderAvatar?: RenderAvatar; renderMarkdown?: RenderMarkdown;
  onReply?(message: TeamMessage): void; onEdit?(message: TeamMessage): void; onRecall?(message: TeamMessage): void; children?: ReactNode;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const clock = formatClock(message.createdAt, locale);
  const full = message.createdAt ? new Date(message.createdAt).toLocaleString(locale) : undefined;
  const time = clock ? <time className={grouped ? 'tw-msg-hover-time' : 'tw-msg-time'} dateTime={message.createdAt ?? undefined} title={full}>{clock}</time> : null;
  const acts = !message.recalled && (onReply || onEdit || onRecall);
  // Only a shared Bot writes Markdown; what a person types is shown exactly as typed.
  const markdown = message.author.kind === 'shared_bot' && !message.recalled && renderMarkdown !== undefined;
  return <li className="tw-msg" data-grouped={grouped} data-mine={mine} data-kind={author.kind} data-recalled={message.recalled ? 'true' : undefined} data-message-id={message.id} data-message-seq={message.seq}>
    <div className="tw-msg-lead">{grouped ? time : <Avatar identity={author} size={32} render={renderAvatar} />}</div>
    <div className="tw-msg-main">
      {!grouped ? <div className="tw-msg-head">
        <b className="tw-msg-author">{author.name}</b>
        {author.kind === 'bot' ? <span className="tw-tag">Bot</span> : null}
        {question ? <span className="tw-tag">{t('Question', '提问')}</span> : null}
        {approval ? <span className="tw-tag">{t('Approval', '审批')}</span> : null}
        {owner ? <small>{t('Owner', '所有者')}: {owner}</small> : null}
        {time}
      </div> : null}
      {quote ? <blockquote className="tw-quote">{quote.author ? <b>{quote.author}</b> : null} {quote.text}</blockquote> : null}
      {markdown ? <div className="tw-msg-body tw-md" data-markdown="true">
        {renderMarkdown(message.text)}
        {message.editedAt ? <small className="tw-edited"> ({t('edited', '已编辑')})</small> : null}
      </div> : !message.text && !message.recalled && message.attachments?.length ? null : <p className="tw-msg-body">
        {message.recalled ? <em>{t('Message recalled', '消息已撤回')}</em> : message.text}
        {message.editedAt && !message.recalled ? <small className="tw-edited"> ({t('edited', '已编辑')})</small> : null}
      </p>}
      {children}
    </div>
    {acts ? <div className="tw-msg-actions" role="group" aria-label={t('Message actions', '消息操作')}>
      {onReply ? <button type="button" className="tw-icon-btn" aria-label={t('Reply', '回复')} title={t('Reply', '回复')} onClick={() => onReply(message)}><Icon name="reply" /></button> : null}
      {onEdit ? <button type="button" className="tw-icon-btn" aria-label={t('Edit', '编辑')} title={t('Edit', '编辑')} onClick={() => onEdit(message)}><Icon name="edit" /></button> : null}
      {onRecall ? <button type="button" className="tw-icon-btn tw-danger" aria-label={t('Recall', '撤回')} title={t('Recall', '撤回')} onClick={() => onRecall(message)}><Icon name="trash" /></button> : null}
    </div> : null}
  </li>;
}
