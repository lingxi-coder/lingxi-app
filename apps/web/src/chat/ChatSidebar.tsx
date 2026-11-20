import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { isUnread, type InboxItem } from '../../../../packages/inbox/contracts';
import { inboxErrorText } from '../../../../packages/inbox/errors';
import type { ChatTarget } from '../app/router';
import { IconChat, IconGlobe, IconMoon, IconSun } from '../components/Icons';
import type { Locale } from '../utils/locale';
import type { AgentChatSummary } from './agent/history';
import { destinationOf, isOpen, sectionOf, type ChatSection, type RowDestination } from './routeFromInbox';

export interface ChatSidebarProps {
  locale: Locale;
  theme: 'light' | 'dark';
  target: ChatTarget;
  rows: readonly InboxItem[];
  agentChats: readonly AgentChatSummary[];
  /** The inbox could not be listed: its error code. */
  rowsError?: string;
  account: string;
  onNewChat(): void;
  onOpenAgent(id: string): void;
  onOpenRow(destination: RowDestination): void;
  onOpen(href: string): void;
  onLocaleChange(locale: Locale): void;
  onThemeToggle(): void;
  onSignOut(): void;
}

/** Local midnight `days` calendar days before `now`'s day (a day is not always 24 hours: daylight saving). */
const midnightBefore = (now: number, days: number) => { const day = new Date(now); day.setHours(0, 0, 0, 0); day.setDate(day.getDate() - days); return day.getTime(); };
/** Today, Yesterday, the last 7 days, or Earlier, by the viewer's local calendar days. */
export function agentGroup(updatedAt: number, now = Date.now()): 'today' | 'yesterday' | 'week' | 'earlier' {
  if (updatedAt >= midnightBefore(now, 0)) return 'today';
  if (updatedAt >= midnightBefore(now, 1)) return 'yesterday';
  if (updatedAt >= midnightBefore(now, 6)) return 'week';
  return 'earlier';
}

function Section({ title, children, empty }: { title: string; children: ReactNode[]; empty?: string }) {
  if (!children.length && !empty) return null;
  return <section className="chat-side-section">
    <h2>{title}</h2>
    {children.length ? <ul>{children}</ul> : <p className="chat-side-empty">{empty}</p>}
  </section>;
}

const initial = (text: string) => (text.trim()[0] ?? '·').toUpperCase();

export function ChatSidebar(props: ChatSidebarProps) {
  const { locale, target, rows, agentChats } = props;
  const zh = locale === 'zh';
  const t = (en: string, cn: string) => zh ? cn : en;
  const [menu, setMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const items = () => [...(menuRef.current?.querySelectorAll<HTMLElement>('[role=menuitem]') ?? [])];
  // The account menu: focus moves to its first item, arrow keys, Home and End move, Escape closes and returns to the
  // trigger, and a click or Tab elsewhere closes it.
  useEffect(() => {
    if (!menu) return;
    items()[0]?.focus();
    const outside = (event: Event) => { if (!(event.target instanceof Node && menuRef.current?.contains(event.target))) setMenu(false); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [menu]);
  const onMenuKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const list = items(), at = list.indexOf(document.activeElement as HTMLElement);
    const go = (index: number) => { event.preventDefault(); list[(index + list.length) % list.length]?.focus(); };
    if (event.key === 'ArrowDown') go(at + 1);
    else if (event.key === 'ArrowUp') go(at - 1);
    else if (event.key === 'Home') go(0);
    else if (event.key === 'End') go(list.length - 1);
    else if (event.key === 'Escape') { event.preventDefault(); setMenu(false); trigger.current?.focus(); }
    else if (event.key === 'Tab') setMenu(false);
  };

  const bySection: Record<ChatSection, InboxItem[]> = { teams: [], friends: [], bots: [], computers: [] };
  for (const row of rows) if (!row.archived) bySection[sectionOf(row)].push(row);

  const rowItem = (row: InboxItem) => {
    const destination = destinationOf(row);
    const unread = isUnread(row);
    const subtitle = row.preview || (row.sourceLabel && row.sourceLabel !== row.title ? row.sourceLabel : '');
    return <li key={row.key}>
      <button type="button" className="chat-side-row" aria-current={isOpen(row, target) ? 'page' : undefined} disabled={!destination}
        onClick={() => destination && props.onOpenRow(destination)}>
        <span className="chat-side-avatar" aria-hidden="true">{initial(row.title)}</span>
        <span className="chat-side-text"><span className="chat-side-title">{row.title || t('Chat', '对话')}</span>{subtitle ? <span className="chat-side-sub">{subtitle}</span> : null}</span>
        {unread ? <span className="chat-side-unread" aria-label={row.unreadCount > 0 ? t(`${row.unreadCount} unread`, `${row.unreadCount} 条未读`) : t('Unread', '未读')}>{row.unreadCount > 99 ? '99+' : row.unreadCount || ''}</span> : null}
      </button>
    </li>;
  };

  const groups = { today: t('Today', '今天'), yesterday: t('Yesterday', '昨天'), week: t('Previous 7 days', '7 天内'), earlier: t('Earlier', '更早') };
  const agentItems: ReactNode[] = [];
  let last = '';
  for (const chat of agentChats) {
    const group = agentGroup(chat.updatedAt);
    if (group !== last) { agentItems.push(<li key={`g:${group}`} className="chat-side-group" aria-hidden="true">{groups[group]}</li>); last = group; }
    agentItems.push(<li key={chat.id}><button type="button" className="chat-side-row chat-side-agent" aria-current={target.kind === 'agent' && target.id === chat.id ? 'page' : undefined} onClick={() => props.onOpenAgent(chat.id)}>
      <span className="chat-side-text"><span className="chat-side-title">{chat.title || t('New chat', '新对话')}</span></span>
    </button></li>);
  }

  return <nav className="chat-side" aria-label={t('Conversations', '对话列表')}>
    <div className="chat-side-top">
      <a className="chat-side-brand" href="/?home" aria-label={t('LingXi home', '灵犀首页')}>
        <img className="product-brand-light" src="/brand/lockup-bilingual-ink.svg" alt="" width={120} height={28} />
        <img className="product-brand-dark" src="/brand/lockup-bilingual-white.svg" alt="" width={120} height={28} />
      </a>
    </div>
    <button type="button" className="chat-side-new" onClick={props.onNewChat}><IconChat width={18} height={18} />{t('New chat', '开启新对话')}</button>
    <div className="chat-side-scroll">
      {props.rowsError ? <p className="chat-side-empty" role="alert">{inboxErrorText(props.rowsError, zh)}</p> : null}
      <Section title={t('Agent', 'Agent 对话')}>{agentItems}</Section>
      <Section title={t('Teams', '团队')}>{bySection.teams.map(rowItem)}</Section>
      <Section title={t('Friends', '好友')}>{bySection.friends.map(rowItem)}</Section>
      <Section title="Bot">{bySection.bots.map(rowItem)}</Section>
      <Section title={t('My computers', '我的电脑')}>{bySection.computers.map(rowItem)}</Section>
    </div>
    <div className="chat-side-account" ref={menuRef}>
      <button ref={trigger} type="button" className="chat-side-me" aria-expanded={menu} aria-haspopup="menu" onClick={() => setMenu(open => !open)}>
        <span className="chat-side-avatar" aria-hidden="true">{initial(props.account)}</span><span className="chat-side-title">{props.account}</span>
      </button>
      {menu ? <div className="chat-side-menu" role="menu" aria-label={t('Account', '账号')} onKeyDown={onMenuKey}>
        <button type="button" role="menuitem" onClick={() => { setMenu(false); props.onOpen('/chat/settings/providers'); }}>{t('Model providers', '模型服务')}</button>
        <button type="button" role="menuitem" onClick={() => { setMenu(false); props.onOpen('/teams'); }}>{t('Manage teams', '管理团队')}</button>
        <button type="button" role="menuitem" onClick={() => { setMenu(false); props.onOpen('/friends/add'); }}>{t('Add a friend', '添加好友')}</button>
        <button type="button" role="menuitem" onClick={() => { setMenu(false); props.onOpen('/remote'); }}>{t('Remote Control', '远程控制')}</button>
        <button type="button" role="menuitem" onClick={props.onThemeToggle}>{props.theme === 'light' ? <IconMoon width={16} height={16} /> : <IconSun width={16} height={16} />}{props.theme === 'light' ? t('Dark mode', '深色模式') : t('Light mode', '浅色模式')}</button>
        <button type="button" role="menuitem" onClick={() => props.onLocaleChange(zh ? 'en' : 'zh')}><IconGlobe width={16} height={16} />{zh ? 'English' : '中文'}</button>
        <button type="button" role="menuitem" onClick={() => { setMenu(false); props.onOpen('/download'); }}>{t('Download the app', '下载 App')}</button>
        <button type="button" role="menuitem" className="chat-side-danger" onClick={() => { setMenu(false); props.onSignOut(); }}>{t('Sign out', '退出登录')}</button>
      </div> : null}
    </div>
  </nav>;
}
