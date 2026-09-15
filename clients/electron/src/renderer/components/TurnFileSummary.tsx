import { memo, type CSSProperties } from 'react';
import type { TurnFileChange } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { DiffView } from './DiffView';
import { Icon } from './Icon';

export const TurnFileSummary = memo(function TurnFileSummary({ id, files, open, onSetOpen }: {
  id: string;
  files: readonly TurnFileChange[];
  open: boolean;
  onSetOpen(id: string, open: boolean): void;
}) {
  const t = useT();
  if (!files.length) return null;
  const bodyId = `turn-files-${id}`;
  return <section className="turn-file-summary" aria-label="Files edited this turn" style={{
    '--files-border': t.border, '--files-surface': t.surface,
    '--files-text': t.text, '--files-muted': t.text3, '--files-add': t.ok,
    '--files-remove': t.danger,
  } as CSSProperties}>
    <header>
      <span className="turn-file-summary-icon" aria-hidden="true"><Icon name="compose" size={22} /></span>
      <div className="turn-file-summary-heading"><strong>Edited {files.length} {files.length === 1 ? 'file' : 'files'}</strong><span>Review changes ↗</span></div>
      <button type="button" className="turn-file-review" aria-expanded={open} aria-controls={bodyId} onClick={() => onSetOpen(id, !open)}>{open ? 'Close review' : 'Review'}</button>
    </header>
    <ul>
      {files.map(file => {
        const split = Math.max(file.path.lastIndexOf('/'), file.path.lastIndexOf('\\')) + 1;
        return <li key={file.path}>
          <button type="button" className="turn-file-path" title={file.path} onClick={() => onSetOpen(id, !open)} aria-expanded={open} aria-controls={bodyId}>
            <bdi><span>{file.path.slice(0, split)}</span>{file.path.slice(split)}</bdi>
          </button>
          <span className="turn-file-counts" aria-label={`${file.additions} added, ${file.removals} removed`}><span>+{file.additions}</span><span>−{file.removals}</span></span>
        </li>;
      })}
    </ul>
    {open && <div className="turn-file-diffs" id={bodyId}>
      {files.map(file => <div key={file.path}>
        <h3><bdi>{file.path}</bdi></h3>
        {file.diffs.length > 1 && <p>{file.diffs.length} edits, shown in order. Counts include each edit.</p>}
        {file.diffs.map((diff, index) => <DiffView key={index} diff={diff} />)}
      </div>)}
    </div>}
  </section>;
});
