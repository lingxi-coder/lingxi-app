import { useEffect, useState, type CSSProperties } from 'react';
import type { AppDevelopmentChat, AppWorkspaceGroup } from '../bridge/localAppWorkspace';
import { useT } from '../theme/ThemeContext';
import { tr } from '../i18n/locale';
import { AppsGlyph } from './LocalApps';
import { Icon } from './Icon';

export function LocalAppsSidebar({ groups, activeSessionId, chatVisible, libraryVisible, query, busy, error, loading = false,
  onLibrary, onCreate, onOpen, onEdit, onNewChat, onChat, onRetry }: {
  groups: readonly AppWorkspaceGroup[]; activeSessionId?: string; chatVisible: boolean; libraryVisible: boolean;
  query: string; busy: boolean; error: string; loading?: boolean; onLibrary(): void; onCreate(): void;
  onOpen(id: string): void; onEdit(id: string): void; onChat(chat: AppDevelopmentChat): void; onRetry(): void;
  onNewChat(group: AppWorkspaceGroup): void;
}) {
  const t = useT();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set(groups
    .filter(group => group.chats.some(chat => chat.sessionId === activeSessionId)).map(group => group.key)));
  useEffect(() => {
    const current = groups.find(group => group.chats.some(chat => chat.sessionId === activeSessionId));
    if (current) setExpanded(before => before.has(current.key) ? before : new Set([...before, current.key]));
  }, [groups, activeSessionId]);
  const matches = (text: string) => text.toLocaleLowerCase().includes(query);
  const visible = groups.filter(group => !query || matches(group.name) || group.chats.some(chat => matches(chat.title)));
  return <nav className="no-drag desktop-sidebar-list local-apps-sidebar" aria-label={tr('apps_workspace_title')} style={{
    '--apps-text': t.text, '--apps-muted': t.text3, '--apps-accent': t.accent, '--apps-accent-bg': t.accentBg,
    '--apps-hover': t.surfaceHover, '--apps-border': t.border, '--apps-error': t.danger,
  } as CSSProperties}>
    <div className="local-apps-sidebar-header"><h2>{tr('apps_title')}</h2>
      <button type="button" disabled={busy} className="sidebar-header-action" aria-label={tr('apps_create')} title={tr('apps_create')} onClick={onCreate}><Icon name="plus" size={17}/></button>
    </div>
    <button type="button" className="local-apps-library-link" aria-current={libraryVisible ? 'page' : undefined} onClick={onLibrary}>
      <AppsGlyph size={18}/><span>{tr('apps_library')}</span>
    </button>
    {error && <p className="local-apps-sidebar-note" role="alert">{error} <button type="button" onClick={onRetry}>{tr('common_retry')}</button></p>}
    {loading && !groups.length && <p className="local-apps-sidebar-note" role="status">{tr('apps_loading')}</p>}
    {!loading && !groups.length && !error && <div className="local-apps-sidebar-empty"><p>{tr('apps_sidebar_empty')}</p><button type="button" disabled={busy} onClick={onCreate}>{tr('apps_create')}</button></div>}
    {groups.length > 0 && <h3 className="local-apps-sidebar-section">{tr('apps_your_apps')}</h3>}
    {visible.map(group => {
      const open = expanded.has(group.key) || Boolean(query);
      const id = `app-chats-${encodeURIComponent(group.key)}`;
      return <section className="local-apps-sidebar-group" key={group.key}>
        <div className="local-apps-sidebar-app">
          <button type="button" className="local-apps-sidebar-disclosure" aria-expanded={open} aria-controls={open ? id : undefined}
            title={group.name} onClick={() => setExpanded(before => { const next = new Set(before); if (!next.delete(group.key)) next.add(group.key); return next; })}>
            <Icon name="chevron" size={12} style={{ transform: open ? 'none' : 'rotate(-90deg)', flexShrink: 0 }}/>
            <span className="local-apps-sidebar-avatar">{Array.from(group.name)[0]}</span>
            <span className="local-apps-sidebar-name">{group.name}</span>
            {!group.app && <span className="local-apps-draft-dot" title={tr('apps_in_progress')}/>}
          </button>
          {(group.app || group.projectPath) && <div className="local-apps-sidebar-actions">
            {group.app && <button type="button" disabled={busy} title={tr('desktop_open_app_named', group.name)} aria-label={tr('desktop_open_app_named', group.name)} onClick={() => onOpen(group.app!.id)}><Icon name="play" size={13}/></button>}
            {group.projectPath && !group.app?.authoringSourceMissing
              ? <button type="button" disabled={busy} title={tr('apps_new_session_named', group.name)} aria-label={tr('apps_new_session_named', group.name)} onClick={() => onNewChat(group)}><Icon name="compose" size={13}/></button>
              : group.app && <button type="button" disabled={busy || Boolean(group.app.marketOrigin && !group.app.authoring?.sourceDirectory && !group.app.manifest.remix?.allowed)} title={tr('apps_edit_named', group.name)} aria-label={tr('apps_edit_named', group.name)} onClick={() => onEdit(group.app!.id)}><Icon name="pencil" size={13}/></button>}
          </div>}
        </div>
        {open && <div id={id} className="local-apps-sidebar-chats" aria-label={tr('apps_development_chats')}>
          {group.chats.filter(chat => !query || matches(group.name) || matches(chat.title)).map(chat => <button type="button" key={chat.sessionId}
            disabled={busy} aria-current={chatVisible && chat.sessionId === activeSessionId ? 'page' : undefined}
            title={chat.title || tr('apps_development_chat')} onClick={() => onChat(chat)}>
            <Icon name="chat" size={14}/><span>{chat.title || tr('apps_development_chat')}</span>
          </button>)}
          {!group.chats.length && <p className="local-apps-sidebar-note">{tr('apps_no_development_chats')}</p>}
        </div>}
      </section>;
    })}
    {query && !visible.length && groups.length > 0 && <p className="local-apps-sidebar-note">{tr('desktop_apps_no_results', query)}</p>}
  </nav>;
}
