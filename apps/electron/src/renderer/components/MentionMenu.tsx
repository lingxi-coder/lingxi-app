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
  const t = useT();
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => {
    menu.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex]);
  const groups = [...new Set(entries.map((entry) => entry.group))];
  return <div ref={menu} className="slash-command-menu mention-command-menu" role="dialog" aria-label={filesOnly ? 'Search workspace files' : 'Add context'} style={commandMenuStyle(t)}>
    <div className="slash-command-header">
      {filesOnly ? <button className="mention-menu-control" type="button" aria-label="Back to all mentions" onMouseDown={(event) => event.preventDefault()} onClick={onBack}><Icon name="chevron" size={14} style={{ transform: 'rotate(90deg)' }} /></button>
        : <span className="slash-command-header-mark" aria-hidden="true">@</span>}
      <strong>{filesOnly ? 'Files and folders' : 'Add context'}</strong>
      <input ref={searchInput} className="mention-menu-search" role="searchbox" type="text" value={query}
        onChange={(event) => onQuery(event.target.value)} onKeyDown={onKeyDown}
        placeholder={filesOnly ? 'Search workspace files' : 'Search references…'}
        aria-label={filesOnly ? 'File search query' : 'Search mentions'} aria-controls="mention-results"
        aria-activedescendant={entries[selectedIndex] ? `mention-result-${selectedIndex}` : undefined} />
      {query && <button className="mention-menu-control" type="button" aria-label="Clear mention search" onClick={() => { onQuery(''); searchInput.current?.focus(); }}><Icon name="x" size={12} /></button>}
      <button className="mention-menu-control" type="button" aria-label="Close mentions" onMouseDown={(event) => event.preventDefault()} onClick={onClose}><Icon name="x" size={14} /></button>
    </div>
    <div id="mention-results" className="slash-command-list" role="listbox" aria-label={filesOnly ? 'Workspace files' : 'Mentions'}>
      {groups.map((group) => <div key={group} role="group" aria-label={group}>
        <div className="mention-menu-group">{group}</div>
        {entries.map((entry, index) => entry.group === group ? <button key={entry.id} id={`mention-result-${index}`}
          className="slash-command-row" role="option" type="button" tabIndex={-1} aria-selected={selectedIndex === index} aria-disabled={entry.disabled || undefined}
          title={`${entry.name}\n${entry.description}`} onMouseDown={(event) => event.preventDefault()}
          onMouseEnter={() => onSelectIndex(index)} onClick={() => { if (!entry.disabled) onChoose(entry); }}
          style={{ '--slash-command-color': entry.group === 'Plugins' || entry.group === 'Skills' ? t.accent : t.text2 } as CSSProperties}>
          <span className="slash-command-icon" aria-hidden="true"><Icon name={entry.icon} size={19} /></span>
          <span className="slash-command-copy"><span className="slash-command-heading"><span className="slash-command-name">{entry.name}</span></span><span className="slash-command-description">{entry.description}</span></span>
          <kbd className="slash-command-complete" aria-hidden="true">↵</kbd>
        </button> : null)}
      </div>)}
      {status === 'loading' && <div className="slash-command-empty" role="status">Searching workspace…</div>}
      {status === 'error' && <div className="slash-command-empty" role="alert">Could not search this workspace. Try again.</div>}
      {!entries.length && status !== 'loading' && status !== 'error' && <div className="slash-command-empty" role="status">No matching references.</div>}
    </div>
    <div id="mention-hint" className="slash-command-footer">
      <span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span><span><kbd>↵</kbd><kbd>Tab</kbd> Add</span><span><kbd>esc</kbd> Close</span>
      {truncated && <span>Keep typing to narrow results</span>}
    </div>
  </div>;
}
