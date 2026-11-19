import { useEffect, useRef, useState, type ReactNode } from 'react';
import { isUnread, type InboxItem } from '../../../../packages/inbox/contracts';
import type { ChatTarget } from '../app/router';
import { IconChat, IconGlobe, IconMoon, IconSun } from '../components/Icons';
import type { Locale } from '../utils/locale';
import { destinationOf, isOpen, sectionOf, type ChatSection, type RowDestination } from './routeFromInbox';

/** An Agent chat kept in this browser, as the sidebar lists it. */
export interface AgentChatEntry { id: string; title: string; updatedAt: number }

export interface ChatSidebarProps {
  locale: Locale;
  theme: 'light' | 'dark';
  target: ChatTarget;
  rows: readonly InboxItem[];
  agentChats: readonly AgentChatEntry[];
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

const DAY = 86_400_000;
/** Today, Yesterday, the last 7 days, or Earlier, by the viewer's local midnight. */
export function agentGroup(updatedAt: number, now = Date.now()): 'today' | 'yesterday' | 'week' | 'earlier' {
  const midnight = new Date(now); midnight.setHours(0, 0, 0, 0);
  const start = midnight.getTime();
  if (updatedAt >= start) return 'today';
  if (updatedAt >= start - DAY) return 'yesterday';
  if (updatedAt >= start - 6 * DAY) return 'week';
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
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (event: Event) => { if (event instanceof KeyboardEvent ? event.key === 'Escape' : !(event.target instanceof Node && menuRef.current?.contains(event.target))) setMenu(false); };
    document.addEventListener('keydown', close); document.addEventListener('pointerdown', close);
    return () => { document.removeEventListener('keydown', close); document.removeEventListener('pointerdown', close); };
  }, [menu]);

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
        {unread ? <span className="chat-side-unread" aria-label={t(`${row.unreadCount} unread`, `${row.unreadCount} 条未读`)}>{row.unreadCount > 99 ? '99+' : row.unreadCount || ''}</span> : null}
      </button>
    </li>;
  };

  const groups = { today: t('Today', '今天'), yesterday: t('Yesterday', '昨天'), week: t('Previous 7 days', '7 天内'), earlier: t('Earlier', '更早') };
  const agentItems: ReactNode[] = [];
  let last = '';
  for (const chat of [...agentChats].sort((a, b) => b.updatedAt - a.updatedAt)) {
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
      {props.rowsError ? <p className="chat-side-empty" role="alert">{t('Conversations could not be loaded.', '对话列表暂时无法加载。')}</p> : null}
      <Section title={t('Agent', 'Agent 对话')}>{agentItems}</Section>
      <Section title={t('Teams', '团队')}>{bySection.teams.map(rowItem)}</Section>
      <Section title={t('Friends', '好友')}>{bySection.friends.map(rowItem)}</Section>
      <Section title="Bot">{bySection.bots.map(rowItem)}</Section>
      <Section title={t('My computers', '我的电脑')}>{bySection.computers.map(rowItem)}</Section>
    </div>
    <div className="chat-side-account" ref={menuRef}>
      <button type="button" className="chat-side-me" aria-expanded={menu} aria-haspopup="menu" onClick={() => setMenu(open => !open)}>
        <span className="chat-side-avatar" aria-hidden="true">{initial(props.account)}</span><span className="chat-side-title">{props.account}</span>
      </button>
      {menu ? <div className="chat-side-menu" role="menu">
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
