import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
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

  return (
    <nav className="no-drag desktop-sidebar-list" aria-label={tr('bots_title')} style={{ flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 10px 12px' }}>
      <section className="projects-sidebar-section" aria-labelledby="bots-sidebar-heading">
        <div className="projects-sidebar-heading" style={{ minHeight: 35, padding: '3px 4px 5px 8px', display: 'flex', alignItems: 'center' }}>
          <h2 id="bots-sidebar-heading" style={{ flex: 1, color: t.text4, fontSize: 12.5, fontWeight: 500, letterSpacing: '.01em' }}>{tr('bots_title')}</h2>
          <div className="projects-sidebar-actions">
            <button type="button" className="sidebar-header-action projects-sidebar-action" aria-label={tr('bots_new')} title={tr('bots_new')} onClick={onCreateBot}
              style={{ width: 28, height: 28, display: 'grid', placeItems: 'center', border: 0, borderRadius: 7, background: 'transparent', color: t.text3, cursor: 'pointer' }}>
              <Icon name="plus" size={17} stroke={1.8} />
            </button>
          </div>
        </div>

        {problem !== undefined ? <p role="alert" style={noteStyle(t.danger)}>{problem}</p> : null}
        {loading ? <p role="status" style={noteStyle(t.text4)}>{tr('bots_loading')}</p> : null}
        {state.error !== undefined ? <p role="alert" style={noteStyle(t.danger)}>
          {tr('bots_load_failed')} {state.error} <button type="button" onClick={() => void controller.reload()} style={linkButtonStyle(t.accent)}>{tr('common_retry')}</button>
        </p> : null}
        {empty ? <div style={{ ...noteStyle(t.text4), display: 'grid', justifyItems: 'start', gap: 8 }}>
          <span>{tr('bots_empty_title')}</span>
          <button type="button" className="sidebar-primary-action" onClick={onCreateBot}
            style={{ minHeight: 32, padding: '5px 10px', border: `0.5px solid ${t.border}`, borderRadius: 8, background: 'transparent', color: t.text, cursor: 'pointer', fontSize: 12.5, fontWeight: 500 }}>
            {tr('bots_create_first')}
          </button>
        </div> : null}
        {query !== '' && visible.length === 0 && state.bots.length > 0 ? <p style={noteStyle(t.text4)}>{tr('bots_sidebar_no_results')}</p> : null}

        {visible.map((bot) => {
          const open = expanded.has(bot.botId);
          const entry = entries[bot.botId];
          const activity = botActivity(bot.botId, runtimes);
          const groupId = `bot-chats-${encodeURIComponent(bot.botId)}`;
          const menuTrigger = menu?.trigger !== undefined && menu.target.kind === 'bot' && menu.target.botId === bot.botId;
          const target: MenuTarget = { kind: 'bot', botId: bot.botId, name: bot.name };
          const openingNew = opening === `${bot.botId}\0`;
          const actionStyle: CSSProperties = { position: 'absolute', top: 5, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' };
          return (
            <div key={bot.botId} style={{ position: 'relative', marginBottom: 2 }}>
              <div className="sidebar-tree-row" style={{ position: 'relative' }}>
                <button
                  type="button"
                  aria-expanded={open}
                  aria-controls={open ? groupId : undefined}
                  title={bot.name}
                  onClick={() => toggle(bot.botId)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    openMenu(target, { left: event.clientX, top: event.clientY }, event.currentTarget);
                  }}
                  style={{ width: '100%', minHeight: 37, display: 'flex', alignItems: 'center', gap: 8, padding: '7px 62px 7px 8px', border: 0, borderRadius: 8, background: 'transparent', color: t.text2, cursor: 'pointer', textAlign: 'left' }}
                >
                  <Icon name="chevron" size={13} stroke={1.7} style={{ flexShrink: 0, color: t.text4, transform: open ? 'none' : 'rotate(-90deg)' }} />
                  <AgentAvatar agentId={bot.botId} size={20} />
                  <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 13, fontWeight: 500 }}>{bot.name}</span>
                  <ActivityDot activity={activity} />
                </button>
                <button
                  type="button"
                  className="sidebar-row-action"
                  data-visible={openingNew || undefined}
                  disabled={bridge.sessionLoading || opening !== undefined}
                  aria-busy={openingNew || undefined}
                  aria-label={tr('bots_sidebar_new_chat_named', bot.name)}
                  title={tr('bots_new_chat')}
                  onClick={() => void openChat(bot.botId)}
                  style={{ ...actionStyle, right: 32, cursor: bridge.sessionLoading || opening !== undefined ? 'wait' : 'pointer' }}
                >
                  <DesktopIcon name="compose" size={14} />
                </button>
                <button
                  type="button"
                  className="sidebar-row-action"
                  data-visible={menuTrigger || undefined}
                  aria-haspopup="menu"
                  aria-expanded={menuTrigger}
                  aria-label={tr('desktop_chat_session_actions', bot.name)}
                  title={tr('desktop_chat_session_actions', bot.name)}
                  onClick={(event) => {
                    const button = event.currentTarget;
                    if (menuTrigger) { closeMenu(); return; }
                    const rect = button.getBoundingClientRect();
                    openMenu(target, { left: rect.left, top: rect.bottom + 4 }, button, button);
                  }}
                  style={{ ...actionStyle, right: 4 }}
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
                    visibleChat={chatVisible && activeBotId === bot.botId ? activeSession?.sessionId : undefined}
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

function noteStyle(color: string): CSSProperties {
  return { margin: 0, padding: '8px 10px 8px 9px', color, fontSize: 11.5, lineHeight: 1.5 };
}

function linkButtonStyle(color: string): CSSProperties {
  return { padding: 0, border: 0, background: 'transparent', color, cursor: 'pointer', font: 'inherit', textDecoration: 'underline' };
}

/** A small mark at the end of a Bot's row while it is at work or waiting for the owner; a Bot at rest has none. */
function ActivityDot({ activity }: { activity: BotActivity }) {
  const t = useT();
  if (activity === 'idle') return null;
  const label = activityLabel(activity);
  return <span role="img" aria-label={label} title={label} data-state={activity}
    style={{ flexShrink: 0, width: 6, height: 6, borderRadius: 99, background: activity === 'working' ? t.ok : t.warn }} />;
}

/** The chats of one open Bot: what was read of them, newest first, the first few unless all are asked for. */
export function BotChatList({ bot, entry, query, all, matches, visibleChat, opening, bridge, onOpen, onMenu, onToggleAll, onRetry }: {
  bot: BotUi;
  entry: BotSessionsEntry | undefined;
  query: string;
  all: boolean;
  matches(value: string): boolean;
  /** The chat of this Bot that the chat view shows, if it shows one. */
  visibleChat: string | undefined;
  opening: string | undefined;
  bridge: Pick<UseBridge, 'sessionRuntimeStatus'>;
  onOpen(sessionId: string, transcriptPath?: string): void;
  onMenu(sessionId: string, title: string, transcriptPath: string | undefined, at: Point, opener: HTMLElement): void;
  onToggleAll(): void;
  onRetry(): void;
}) {
  const t = useT();
  const read = entry?.sessions;
  // A chat just started has no transcript yet, so it is not in what was read: it stands in the list until it is.
  const pending = read !== undefined && visibleChat !== undefined && !visibleChat.startsWith('pending-new:') && !read.some((row) => row.uuid === visibleChat);
  const chats: Array<Pick<SessionRowDto, 'uuid' | 'title' | 'modified_rfc3339' | 'message_count'> & { path?: string }> = [
    ...(pending ? [{ uuid: visibleChat, title: tr('bots_new_chat'), modified_rfc3339: '', message_count: 0 }] : []),
    ...(read ?? []).map((row) => ({ ...row, title: row.title.trim() })),
  ];
  const searched = query === '' ? chats : matches(bot.name) ? chats : chats.filter((chat) => matches(titleOf(chat)));
  const listed = query !== '' || all ? searched : searched.slice(0, FIRST_CHATS);
  const failed = entry?.error;
  return <>
    {read === undefined && failed === undefined ? <p role="status" style={{ ...noteStyle(t.text4), paddingLeft: 30 }}>{tr('bots_chats_loading')}</p> : null}
    {failed !== undefined ? <p role="alert" style={{ ...noteStyle(t.danger), paddingLeft: 30 }}>
      {tr('bots_chats_failed', failed)} <button type="button" onClick={onRetry} style={linkButtonStyle(t.accent)}>{tr('common_retry')}</button>
    </p> : null}
    {read !== undefined && chats.length === 0 && failed === undefined ? <p style={{ ...noteStyle(t.text4), paddingLeft: 30 }}>{tr('bots_no_chats_yet')}</p> : null}
    {listed.map((chat) => {
      const key = `${bot.botId}\0${chat.uuid}`;
      return <SessionRow
        key={chat.uuid}
        projectPath={pendingBotPath(bot.botId)}
        session={chat}
        metadata={chat.modified_rfc3339 === '' ? tr('desktop_session_just_now') : undefined}
        active={visibleChat === chat.uuid}
        pinned={false}
        opening={opening === key}
        status={bridge.sessionRuntimeStatus(chat.uuid)}
        onClick={() => onOpen(chat.uuid, chat.path)}
        onMenu={(at, opener) => onMenu(chat.uuid, titleOf(chat), chat.path, at, opener)}
      />;
    })}
    {query === '' && searched.length > FIRST_CHATS ? (
      <button type="button" onClick={onToggleAll} style={{ minHeight: 32, marginLeft: 30, padding: '5px 8px', border: 0, borderRadius: 7, background: 'transparent', color: t.text4, cursor: 'pointer', fontSize: 11.5 }}>
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
