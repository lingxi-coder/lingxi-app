import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type FocusEvent, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BotUi } from '../../shared/bots';
import { tr } from '../i18n/locale';
import { botActivity, botRuntimes, pendingBotPath, startBotChat, type BotActivity } from '../bridge/botSession';
import { recentBots, type BotSessionsController, type BotSessionsEntry } from '../bridge/botSessionsStore';
import type { UseBridge } from '../bridge/bridgeTypes';
import { attachPopupMenu } from '../bridge/popupMenu';
import type { UseBots } from '../bridge/useBots';
import { useT } from '../theme/ThemeContext';
import { AgentAvatar } from './AgentAvatar';
import { SessionRow } from './BetaDesktop';
import { activityLabel } from './BotDetail';
import { useObservable } from './BotFields';
import { DesktopIcon } from './DesktopIcon';
import { Icon } from './Icon';
import { rovingIndex } from './rovingIndex';
import './BotsSidebar.css';

/** How many chats of a Bot are listed before "Show more". */
const FIRST_CHATS = 5;

type Point = { left: number; top: number };

/** What a menu is open on: a Bot, or one of its chats. */
export type MenuTarget =
  | { kind: 'bot'; botId: string; name: string }
  | { kind: 'chat'; botId: string; sessionId: string; title: string; transcriptPath?: string };

interface OpenMenu {
  target: MenuTarget;
  at: Point;
  /** What had the owner's focus, which the menu takes back when it closes. */
  opener: HTMLElement;
  /** The button that opened it, if one did: pressing it again closes the menu rather than opening it anew. */
  trigger?: HTMLElement;
}

export interface BotsSidebarProps {
  bots: Pick<UseBots, 'controller' | 'state'>;
  sessions: BotSessionsController;
  bridge: Pick<UseBridge, 'bootstrap' | 'activeBotId' | 'activeSession' | 'sessionLoading' | 'sessionRuntimeStatus' | 'openBotSession' | 'updateSidebarPreferences'>;
  /** What the sidebar's search box holds, trimmed and in lower case. */
  query: string;
  /** The chat view is the page on screen: only then is a chat in the list the open one. */
  chatVisible: boolean;
  /** Show the chat view, once a chat of a Bot is open in it. */
  onOpenChat(): void;
  /** Run `go`, which opens a chat and so leaves the Bots page. A Bot's page with edits that are not saved asks about them first, and `go` waits for the answer. */
  onLeave(go: () => void): void;
  /** Show a Bot's page; with `delete`, asking whether to delete it. */
  onOpenBot(botId: string, intent?: 'delete'): void;
  onCreateBot(): void;
}

const titleOf = (session: Pick<SessionRowDto, 'title'>) => session.title.trim() || tr('remote_untitled');

/** What names a row in the list the arrow keys move through: a Bot, or one of its chats. */
const botRowKey = (botId: string) => `bot/${encodeURIComponent(botId)}`;
const chatRowKey = (botId: string, sessionId: string) => `chat/${encodeURIComponent(botId)}/${encodeURIComponent(sessionId)}`;

type Chat = Pick<SessionRowDto, 'uuid' | 'title' | 'modified_rfc3339' | 'message_count'> & { path?: string };

/**
 * The chats of one open Bot: what was read of them, newest first; the ones a search keeps; and of those the first few
 * unless all are asked for. The list draws `listed`, and the sidebar names the rows from the same.
 */
function chatsOf({ bot, entry, query, all, matches, visibleChat }: {
  bot: BotUi;
  entry: BotSessionsEntry | undefined;
  query: string;
  all: boolean;
  matches(value: string): boolean;
  visibleChat: string | undefined;
}): { chats: Chat[]; searched: Chat[]; listed: Chat[] } {
  const read = entry?.sessions;
  // A chat just started has no transcript yet, so it is not in what was read: it stands in the list until it is.
  const pending = read !== undefined && visibleChat !== undefined && !visibleChat.startsWith('pending-new:') && !read.some((row) => row.uuid === visibleChat);
  const chats: Chat[] = [
    ...(pending ? [{ uuid: visibleChat, title: tr('bots_new_chat'), modified_rfc3339: '', message_count: 0 }] : []),
    ...(read ?? []).map((row) => ({ ...row, title: row.title.trim() })),
  ];
  const searched = query === '' ? chats : matches(bot.name) ? chats : chats.filter((chat) => matches(titleOf(chat)));
  return { chats, searched, listed: query !== '' || all ? searched : searched.slice(0, FIRST_CHATS) };
}

/** The sidebar in Bots mode: each Bot, and under the ones that are open, its chats. */
export function BotsSidebar({ bots, sessions, bridge, query, chatVisible, onOpenChat, onLeave, onOpenBot, onCreateBot }: BotsSidebarProps) {
  const t = useT();
  const { controller, state } = bots;
  const { entries } = useObservable(sessions);
  const { activeBotId, activeSession } = bridge;
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(
    () => new Set(bridge.bootstrap?.settings?.sidebar?.expandedBots ?? (activeBotId !== undefined ? [activeBotId] : [])),
  );
  const [showAll, setShowAll] = useState<ReadonlySet<string>>(() => new Set());
  const [opening, setOpening] = useState<string>();
  const [problem, setProblem] = useState<string>();
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  /** The row that last had the focus: the one Tab comes back to. */
  const [focusedRow, setFocusedRow] = useState<string>();

  const runtimes = botRuntimes(bridge.bootstrap?.runtimes ?? [], bridge.sessionRuntimeStatus);
  const runtimeKey = runtimes.map((runtime) => `${runtime.sessionId}\t${runtime.botId}\t${runtime.turnActive ? 1 : 0}`).join('\n');
  const ordered = useMemo(() => recentBots(state.bots, entries), [state.bots, entries]);
  const matches = (value: string) => value.toLocaleLowerCase().includes(query);
  const shownKey = ordered.filter((bot) => expanded.has(bot.botId)).map((bot) => bot.botId).join('\0');

  // The chat on screen is a Bot's: its Bot is open, as the chat's project is in the project list.
  useEffect(() => {
    if (activeBotId !== undefined) setExpanded((current) => current.has(activeBotId) ? current : new Set([...current, activeBotId]));
  }, [activeBotId]);
  // The sidebar reads the chats of the open Bots, and none while it lists something else.
  useEffect(() => { sessions.show(shownKey === '' ? [] : shownKey.split('\0')); }, [sessions, shownKey]);
  useEffect(() => () => sessions.show([]), [sessions]);
  useEffect(() => { sessions.noteRuntimes(runtimes); }, [sessions, runtimeKey]);
  useEffect(() => { sessions.noteBots(state.bots); }, [sessions, state.bots]);
  // The list is read when the sidebar comes up and whenever the owner comes back to the window, as the Bots page does.
  useEffect(() => {
    const reload = () => { void controller.reload(); };
    reload();
    window.addEventListener('focus', reload);
    return () => window.removeEventListener('focus', reload);
  }, [controller]);

  const toggle = (botId: string) => {
    const next = new Set(expanded);
    if (!next.delete(botId)) next.add(botId);
    setExpanded(next);
    const listed = new Set(state.bots.map((bot) => bot.botId));
    void bridge.updateSidebarPreferences({ expandedBots: [...next].filter((id) => listed.size === 0 || listed.has(id)) });
  };
  const openChat = (botId: string, sessionId?: string, transcriptPath?: string) => {
    if (bridge.sessionLoading || opening !== undefined) return;
    onLeave(async () => {
      const key = `${botId}\0${sessionId ?? ''}`;
      setOpening(key);
      setProblem(undefined);
      const outcome = await startBotChat(() => bridge.openBotSession(botId, sessionId, transcriptPath));
      setOpening((current) => current === key ? undefined : current);
      if (outcome.ok) onOpenChat();
      else setProblem(outcome.message);
    });
  };
  const closeMenu = useCallback(() => setMenu(null), []);
  const openMenu = (target: MenuTarget, at: Point, opener: HTMLElement, trigger?: HTMLElement) => setMenu({ target, at, opener, trigger });

  const busy = bridge.sessionLoading || opening !== undefined;

  const loading = state.availability === undefined || (state.loading && state.bots.length === 0);
  const empty = !loading && state.error === undefined && state.bots.length === 0;
  const visible = query === ''
    ? ordered
    : ordered.filter((bot) => matches(bot.name) || (expanded.has(bot.botId) && (entries[bot.botId]?.sessions ?? []).some((session) => matches(titleOf(session)))));

  // The chat of a Bot that the chat view shows, if it shows one.
  const visibleChatOf = (botId: string) => chatVisible && activeBotId === botId ? activeSession?.sessionId : undefined;
  const listedOf = (bot: BotUi) => chatsOf({ bot, entry: entries[bot.botId], query, all: showAll.has(bot.botId), matches, visibleChat: visibleChatOf(bot.botId) }).listed;
  // The rows, in the order they are drawn, are one list for the arrow keys; Tab reaches one of them: the row that last had
  // the focus, else the chat on screen, else the first.
  const rows = visible.flatMap((bot) => [botRowKey(bot.botId), ...(expanded.has(bot.botId) ? listedOf(bot).map((chat) => chatRowKey(bot.botId, chat.uuid)) : [])]);
  const shownRow = activeBotId !== undefined && visibleChatOf(activeBotId) !== undefined ? chatRowKey(activeBotId, visibleChatOf(activeBotId)!) : undefined;
  const tabStop = focusedRow !== undefined && rows.includes(focusedRow) ? focusedRow : shownRow !== undefined && rows.includes(shownRow) ? shownRow : rows[0];
  const noteFocus = (event: FocusEvent<HTMLElement>) => {
    const row = event.target as HTMLElement;
    if (row.matches?.('[data-row]')) setFocusedRow(row.dataset.row);
  };
  const moveFocus = (event: KeyboardEvent<HTMLElement>) => {
    const target = event.target as HTMLElement;
    if (event.altKey || event.ctrlKey || event.metaKey || !target.matches?.('[data-row]')) return;
    const list = [...event.currentTarget.querySelectorAll<HTMLElement>('[data-row]')];
    const to = rovingIndex(list.length, list.indexOf(target), event.key);
    const next = to === undefined ? undefined : list[to];
    if (next === undefined) return;
    event.preventDefault();
    // The stop for Tab goes where the arrow key took the focus, without waiting for the focus event to say so.
    setFocusedRow(next.dataset.row);
    next.focus();
  };
  const style = {
    '--bots-sidebar-text': t.text, '--bots-sidebar-text2': t.text2, '--bots-sidebar-text3': t.text3, '--bots-sidebar-text4': t.text4,
    '--bots-sidebar-accent': t.accent, '--bots-sidebar-danger': t.danger, '--bots-sidebar-ok': t.ok, '--bots-sidebar-warn': t.warn, '--bots-sidebar-line': t.border,
  } as CSSProperties;

  return (
    <nav className="no-drag desktop-sidebar-list bots-sidebar" aria-label={tr('bots_title')} style={style} onFocus={noteFocus} onKeyDown={moveFocus}>
      <section className="projects-sidebar-section" aria-labelledby="bots-sidebar-heading">
        <div className="projects-sidebar-heading bots-sidebar-heading">
          <h2 id="bots-sidebar-heading">{tr('bots_title')}</h2>
          <div className="projects-sidebar-actions">
            <button type="button" className="sidebar-header-action projects-sidebar-action bots-sidebar-add" aria-label={tr('bots_new')} title={tr('bots_new')} onClick={onCreateBot}>
              <Icon name="plus" size={17} stroke={1.8} />
            </button>
          </div>
        </div>

        {problem !== undefined ? <p role="alert" className="bots-sidebar-note" data-tone="danger">{problem}</p> : null}
        {loading ? <p role="status" className="bots-sidebar-note">{tr('bots_loading')}</p> : null}
        {state.error !== undefined ? <p role="alert" className="bots-sidebar-note" data-tone="danger">
          {tr('bots_load_failed')} {state.error} <button type="button" className="bots-sidebar-link" onClick={() => void controller.reload()}>{tr('common_retry')}</button>
        </p> : null}
        {empty ? <div className="bots-sidebar-note bots-sidebar-empty">
          <span>{tr('bots_empty_title')}</span>
          <button type="button" className="sidebar-primary-action bots-sidebar-create" onClick={onCreateBot}>
            {tr('bots_create_first')}
          </button>
        </div> : null}
        {query !== '' && visible.length === 0 && state.bots.length > 0 ? <p className="bots-sidebar-note">{tr('bots_sidebar_no_results')}</p> : null}

        {visible.map((bot) => {
          const open = expanded.has(bot.botId);
          const entry = entries[bot.botId];
          const activity = botActivity(bot.botId, runtimes);
          const groupId = `bot-chats-${encodeURIComponent(bot.botId)}`;
          const menuTrigger = menu?.trigger !== undefined && menu.target.kind === 'bot' && menu.target.botId === bot.botId;
          const target: MenuTarget = { kind: 'bot', botId: bot.botId, name: bot.name };
          const openingNew = opening === `${bot.botId}\0`;
          const rowKey = botRowKey(bot.botId);
          // The row that Tab reaches takes its two buttons with it; the others are reached by the arrow keys.
          const rowTab = rowKey === tabStop ? 0 : -1;
          return (
            <div key={bot.botId} className="bots-sidebar-bot">
              <div className="sidebar-tree-row bots-sidebar-row">
                <button
                  type="button"
                  className="bots-sidebar-toggle"
                  aria-expanded={open}
                  aria-controls={open ? groupId : undefined}
                  data-row={rowKey}
                  tabIndex={rowTab}
                  title={bot.name}
                  onClick={() => toggle(bot.botId)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    openMenu(target, { left: event.clientX, top: event.clientY }, event.currentTarget);
                  }}
                >
                  <AgentAvatar agentId={bot.botId} size={20} />
                  <span className="bots-sidebar-name">{bot.name}</span>
                  <ActivityDot activity={activity} />
                </button>
                <button
                  type="button"
                  className="sidebar-row-action"
                  data-slot="new-chat"
                  data-visible={openingNew || undefined}
                  disabled={bridge.sessionLoading || opening !== undefined}
                  aria-busy={openingNew || undefined}
                  aria-label={tr('bots_sidebar_new_chat_named', bot.name)}
                  title={tr('bots_new_chat')}
                  tabIndex={rowTab}
                  onClick={() => void openChat(bot.botId)}
                >
                  <DesktopIcon name="compose" size={14} />
                </button>
                <button
                  type="button"
                  className="sidebar-row-action"
                  data-slot="menu"
                  data-visible={menuTrigger || undefined}
                  aria-haspopup="menu"
                  aria-expanded={menuTrigger}
                  aria-label={tr('desktop_chat_session_actions', bot.name)}
                  title={tr('desktop_chat_session_actions', bot.name)}
                  tabIndex={rowTab}
                  onClick={(event) => {
                    const button = event.currentTarget;
                    if (menuTrigger) { closeMenu(); return; }
                    const rect = button.getBoundingClientRect();
                    openMenu(target, { left: rect.left, top: rect.bottom + 4 }, button, button);
                  }}
                >
                  <Icon name="more" size={14} stroke={1.9} />
                </button>
              </div>

              {open ? (
                <div id={groupId}>
                  <BotChatList
                    bot={bot}
                    entry={entry}
                    query={query}
                    all={showAll.has(bot.botId)}
                    matches={matches}
                    visibleChat={visibleChatOf(bot.botId)}
                    tabStop={tabStop}
                    opening={opening}
                    bridge={bridge}
                    onOpen={(sessionId, transcriptPath) => void openChat(bot.botId, sessionId, transcriptPath)}
                    onMenu={(sessionId, title, transcriptPath, at, opener) => openMenu({ kind: 'chat', botId: bot.botId, sessionId, title, transcriptPath }, at, opener)}
                    onToggleAll={() => setShowAll((current) => {
                      const next = new Set(current);
                      if (!next.delete(bot.botId)) next.add(bot.botId);
                      return next;
                    })}
                    onRetry={() => void sessions.load(bot.botId)}
                  />
                </div>
              ) : null}
            </div>
          );
        })}
      </section>

      {menu ? <RowMenu menu={menu} onClose={closeMenu}>
        <MenuItems target={menu.target} busy={busy} danger={t.danger}
          onNewChat={(botId) => { closeMenu(); void openChat(botId); }}
          onSettings={(botId) => { closeMenu(); onOpenBot(botId); }}
          onDelete={(botId) => { closeMenu(); onOpenBot(botId, 'delete'); }}
          onOpen={(botId, sessionId, transcriptPath) => { closeMenu(); void openChat(botId, sessionId, transcriptPath); }} />
      </RowMenu> : null}
    </nav>
  );
}

/**
 * A small mark at the end of a Bot's row while it is at work (a filled dot) or waiting for the owner (a ring); a Bot at
 * rest has none. The two differ in shape as well as in color, and the label says which.
 */
function ActivityDot({ activity }: { activity: BotActivity }) {
  if (activity === 'idle') return null;
  const label = activityLabel(activity);
  return <span role="img" aria-label={label} title={label} data-state={activity} className="bots-sidebar-activity" />;
}

/** The chats of one open Bot: what was read of them, newest first, the first few unless all are asked for. */
export function BotChatList({ bot, entry, query, all, matches, visibleChat, tabStop, opening, bridge, onOpen, onMenu, onToggleAll, onRetry }: {
  bot: BotUi;
  entry: BotSessionsEntry | undefined;
  query: string;
  all: boolean;
  matches(value: string): boolean;
  /** The chat of this Bot that the chat view shows, if it shows one. */
  visibleChat: string | undefined;
  /** The row of the sidebar that Tab reaches. Given, the chats are rows of the list the arrow keys move through; not given, they are reached by Tab each. */
  tabStop?: string;
  opening: string | undefined;
  bridge: Pick<UseBridge, 'sessionRuntimeStatus'>;
  onOpen(sessionId: string, transcriptPath?: string): void;
  onMenu(sessionId: string, title: string, transcriptPath: string | undefined, at: Point, opener: HTMLElement): void;
  onToggleAll(): void;
  onRetry(): void;
}) {
  const read = entry?.sessions;
  const { chats, searched, listed } = chatsOf({ bot, entry, query, all, matches, visibleChat });
  const failed = entry?.error;
  return <>
    {read === undefined && failed === undefined ? <p role="status" className="bots-sidebar-note" data-nested>{tr('bots_chats_loading')}</p> : null}
    {failed !== undefined ? <p role="alert" className="bots-sidebar-note" data-tone="danger" data-nested>
      {tr('bots_chats_failed', failed)} <button type="button" className="bots-sidebar-link" onClick={onRetry}>{tr('common_retry')}</button>
    </p> : null}
    {read !== undefined && chats.length === 0 && failed === undefined ? <p className="bots-sidebar-note" data-nested>{tr('bots_no_chats_yet')}</p> : null}
    {listed.map((chat) => {
      const key = `${bot.botId}\0${chat.uuid}`;
      const rowKey = chatRowKey(bot.botId, chat.uuid);
      return <SessionRow
        key={chat.uuid}
        projectPath={pendingBotPath(bot.botId)}
        session={chat}
        metadata={chat.modified_rfc3339 === '' ? tr('desktop_session_just_now') : undefined}
        active={visibleChat === chat.uuid}
        pinned={false}
        opening={opening === key}
        status={bridge.sessionRuntimeStatus(chat.uuid)}
        roving={tabStop === undefined ? undefined : { id: rowKey, tabStop: rowKey === tabStop }}
        onClick={() => onOpen(chat.uuid, chat.path)}
        onMenu={(at, opener) => onMenu(chat.uuid, titleOf(chat), chat.path, at, opener)}
      />;
    })}
    {query === '' && searched.length > FIRST_CHATS ? (
      <button type="button" className="bots-sidebar-more" onClick={onToggleAll}>
        {all ? tr('chat_tool_show_less') : tr('desktop_chat_show_more', searched.length - FIRST_CHATS)}
      </button>
    ) : null}
  </>;
}

const clampLeft = (left: number) => Math.max(12, Math.min(left, window.innerWidth - 220));
const clampTop = (top: number) => Math.max(12, Math.min(top, window.innerHeight - 190));

/**
 * The popup over a row, shown at the pointer for a right click and below the
 * button for ⋯. It takes the focus in, moves it with the arrow keys, closes on
 * Escape, Tab or a press outside it, and hands the focus back to what had it.
 */
function RowMenu({ menu, onClose, children }: { menu: OpenMenu; onClose(): void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const { opener, trigger, target } = menu;
  useEffect(() => attachPopupMenu({ menu: ref.current, opener, trigger, onClose }), [onClose, opener, trigger]);
  const label = target.kind === 'bot' ? tr('desktop_chat_session_actions', target.name) : tr('desktop_chat_session_actions', target.title);
  return createPortal(
    <div ref={ref} className="session-actions-menu" role="menu" aria-label={label} style={{ left: clampLeft(menu.at.left), top: clampTop(menu.at.top) }}>
      {children}
    </div>,
    document.body,
  );
}

/** What a menu offers: for a Bot, a new chat, its page, and its deletion, which only asks; for one of its chats, to open it. */
export function MenuItems({ target, busy, danger, onNewChat, onSettings, onDelete, onOpen }: {
  target: MenuTarget;
  /** A chat is being opened, or the window is: no other can be started. */
  busy: boolean;
  /** The color of what is destructive. */
  danger: string;
  onNewChat(botId: string): void;
  onSettings(botId: string): void;
  /** Ask whether to delete the Bot; the Bot is not deleted until that is answered. */
  onDelete(botId: string): void;
  onOpen(botId: string, sessionId: string, transcriptPath?: string): void;
}) {
  if (target.kind === 'chat') {
    const { botId, sessionId, transcriptPath } = target;
    return <button type="button" role="menuitem" disabled={busy} onClick={() => onOpen(botId, sessionId, transcriptPath)}>
      <Icon name="chat" size={16} /> {tr('common_open')}
    </button>;
  }
  const { botId } = target;
  return <>
    <button type="button" role="menuitem" disabled={busy} onClick={() => onNewChat(botId)}>
      <DesktopIcon name="compose" size={16} /> {tr('bots_new_chat')}
    </button>
    <button type="button" role="menuitem" onClick={() => onSettings(botId)}>
      <Icon name="cog" size={16} /> {tr('bots_sidebar_settings')}
    </button>
    <button type="button" role="menuitem" onClick={() => onDelete(botId)} style={{ color: danger }}>
      <Icon name="trash" size={16} /> {tr('bots_sidebar_delete')}
    </button>
  </>;
}
