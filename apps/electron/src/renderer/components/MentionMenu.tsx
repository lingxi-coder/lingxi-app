import { useI18n } from '../i18n/useI18n';
import { useEffect, useRef, type CSSProperties, type KeyboardEvent, type RefObject } from 'react';
import type { MentionMenuEntry } from '../bridge/composerMentions';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export function commandMenuStyle(t: ReturnType<typeof useT>): CSSProperties {
  return {
    '--slash-surface': t.surface, '--slash-hover': t.surfaceHover, '--slash-active': t.surfaceActive,
    '--slash-border': t.border, '--slash-border-strong': t.borderStrong, '--slash-text': t.text,
    '--slash-secondary': t.text2, '--slash-muted': t.text3, '--slash-focus': t.accent,
    '--slash-shadow': t.dark ? 'rgba(0, 0, 0, .32)' : 'rgba(0, 0, 0, .12)',
  } as CSSProperties;
}

export function MentionMenu({ entries, selectedIndex, query, filesOnly, searchInput, status, truncated,
  onQuery, onSelectIndex, onChoose, onKeyDown, onClose, onBack }: {
  entries: readonly MentionMenuEntry[];
  selectedIndex: number;
  query: string;
  filesOnly: boolean;
  searchInput: RefObject<HTMLInputElement>;
  status: 'idle' | 'loading' | 'ready' | 'error';
  truncated: boolean;
  onQuery(value: string): void;
  onSelectIndex(index: number): void;
  onChoose(entry: MentionMenuEntry): void;
  onKeyDown(event: KeyboardEvent<HTMLInputElement | HTMLDivElement>): void;
  onClose(): void;
  onBack(): void;
}) {
  const { tr } = useI18n();
  const t = useT();
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => {
    menu.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex]);
  const groupLabel = (group: MentionMenuEntry['group']) => tr(({
    Add: 'workspace_confirm_add', Plugins: 'desktop_extension_plugins', Skills: 'settings_title_skills', 'Files and folders': 'desktop_navigation_files_folders',
  } as const)[group]);
  const actionNameKeys: Record<string, string> = { files: 'desktop_navigation_files_folders', attach: 'desktop_attach_files', goal: 'desktop_navigation_goal', plan: 'desktop_chat_permission_plan' };
  const actionDescriptionKeys: Record<string, string> = {
    'Search this workspace': 'desktop_navigation_search_workspace', 'Add files or images from your computer': 'desktop_navigation_attach_description',
    'Available after the current turn': 'desktop_navigation_after_turn', 'Remove attachments to set a goal': 'desktop_navigation_remove_goal_attachments',
    'Set a goal to keep pursuing': 'desktop_navigation_set_goal', 'Plan mode is on': 'desktop_navigation_plan_on', 'Turn plan mode on': 'desktop_navigation_plan_enable',
  };
  const entryName = (entry: MentionMenuEntry) => entry.action && actionNameKeys[entry.action] ? tr(actionNameKeys[entry.action]!) : entry.name;
  const entryDescription = (entry: MentionMenuEntry) => entry.action && actionDescriptionKeys[entry.description] ? tr(actionDescriptionKeys[entry.description]!) : entry.description;
  const groups = [...new Set(entries.map((entry) => entry.group))];
  return <div ref={menu} className="slash-command-menu mention-command-menu" role="dialog" aria-label={filesOnly ? tr('desktop_navigation_search_files') : tr('desktop_add_context')} style={commandMenuStyle(t)}>
    <div className="slash-command-header">
      {filesOnly ? <button className="mention-menu-control" type="button" aria-label={tr('desktop_navigation_back_mentions')} onMouseDown={(event) => event.preventDefault()} onClick={onBack}><Icon name="chevron" size={14} style={{ transform: 'rotate(90deg)' }} /></button>
        : <span className="slash-command-header-mark" aria-hidden="true">@</span>}
      <strong>{filesOnly ? tr('desktop_navigation_files_folders') : tr('desktop_add_context')}</strong>
      <input ref={searchInput} className="mention-menu-search" role="searchbox" type="text" value={query}
        onChange={(event) => onQuery(event.target.value)} onKeyDown={onKeyDown}
        placeholder={filesOnly ? tr('desktop_navigation_search_files') : tr('desktop_navigation_search_references')}
        aria-label={filesOnly ? tr('desktop_navigation_file_query') : tr('desktop_navigation_search_mentions')} aria-controls="mention-results"
        aria-activedescendant={entries[selectedIndex] ? `mention-result-${selectedIndex}` : undefined} />
      {query && <button className="mention-menu-control" type="button" aria-label={tr('desktop_navigation_clear_mentions')} onClick={() => { onQuery(''); searchInput.current?.focus(); }}><Icon name="x" size={12} /></button>}
      <button className="mention-menu-control" type="button" aria-label={tr('desktop_navigation_close_mentions')} onMouseDown={(event) => event.preventDefault()} onClick={onClose}><Icon name="x" size={14} /></button>
    </div>
    <div id="mention-results" className="slash-command-list" role="listbox" aria-label={filesOnly ? tr('desktop_navigation_workspace_files') : tr('desktop_navigation_mentions')}>
      {groups.map((group) => <div key={group} role="group" aria-label={groupLabel(group)}>
        <div className="mention-menu-group">{groupLabel(group)}</div>
        {entries.map((entry, index) => entry.group === group ? <button key={entry.id} id={`mention-result-${index}`}
          className="slash-command-row" role="option" type="button" tabIndex={-1} aria-selected={selectedIndex === index} aria-disabled={entry.disabled || undefined}
          title={`${entryName(entry)}\n${entryDescription(entry)}`} onMouseDown={(event) => event.preventDefault()}
          onMouseEnter={() => onSelectIndex(index)} onClick={() => { if (!entry.disabled) onChoose(entry); }}
          style={{ '--slash-command-color': entry.group === 'Plugins' || entry.group === 'Skills' ? t.accent : t.text2 } as CSSProperties}>
          <span className="slash-command-icon" aria-hidden="true"><Icon name={entry.icon} size={19} /></span>
          <span className="slash-command-copy"><span className="slash-command-heading"><span className="slash-command-name">{entryName(entry)}</span></span><span className="slash-command-description">{entryDescription(entry)}</span></span>
          <kbd className="slash-command-complete" aria-hidden="true">↵</kbd>
        </button> : null)}
      </div>)}
      {status === 'loading' && <div className="slash-command-empty" role="status">{tr('desktop_navigation_searching_workspace')}</div>}
      {status === 'error' && <div className="slash-command-empty" role="alert">{tr('desktop_navigation_search_failed')}</div>}
      {!entries.length && status !== 'loading' && status !== 'error' && <div className="slash-command-empty" role="status">{tr('desktop_navigation_no_references')}</div>}
    </div>
    <div id="mention-hint" className="slash-command-footer">
      <span><kbd>↑</kbd><kbd>↓</kbd> {tr('desktop_chat_navigate')}</span><span><kbd>↵</kbd><kbd>Tab</kbd> {tr('workspace_confirm_add')}</span><span><kbd>esc</kbd> {tr('common_close')}</span>
      {truncated && <span>{tr('desktop_navigation_narrow_results')}</span>}
    </div>
  </div>;
}
