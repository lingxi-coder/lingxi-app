import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { memo, type CSSProperties } from 'react';
import type { TurnFileChange } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { DiffView } from './DiffView';
import { Icon } from './Icon';

export const TurnFileSummary = memo(function TurnFileSummary({ files, onReview }: {
  files: readonly TurnFileChange[];
  onReview?(path?: string): void;
}) {
  const t = useT();
  useI18n();
  if (!files.length) return null;
  return <section className="turn-file-summary" aria-label={tr("desktop_ui_files_edited")} style={{
    '--files-border': t.border, '--files-surface': t.surface,
    '--files-text': t.text, '--files-muted': t.text3, '--files-add': t.ok,
    '--files-remove': t.danger,
  } as CSSProperties}>
    <header>
      <span className="turn-file-summary-icon" aria-hidden="true"><Icon name="compose" size={22} /></span>
      <div className="turn-file-summary-heading"><strong>{tr(files.length === 1 ? 'desktop_ui_edited_one' : 'desktop_ui_edited_count', files.length)}</strong><span>{tr("desktop_ui_review_changes")}</span></div>
      <button type="button" className="turn-file-review" aria-controls="runtime-inspector-panel" disabled={!onReview} onClick={() => onReview?.()}>{tr("desktop_ui_review")}</button>
    </header>
    <ul>
      {files.map(file => {
        const split = Math.max(file.path.lastIndexOf('/'), file.path.lastIndexOf('\\')) + 1;
        return <li key={file.path}>
          <button type="button" className="turn-file-path" title={file.path} onClick={() => onReview?.(file.path)} disabled={!onReview} aria-controls="runtime-inspector-panel">
            <bdi><span>{file.path.slice(0, split)}</span>{file.path.slice(split)}</bdi>
          </button>
          <span className="turn-file-counts" aria-label={tr('desktop_ui_edit_counts', file.additions, file.removals)}><span>+{file.additions}</span><span>−{file.removals}</span></span>
        </li>;
      })}
    </ul>
  </section>;
});

/** Historical tool diffs stay tied to this turn, including untracked files. */
export function TurnFileReview({ files, path }: { files: readonly TurnFileChange[]; path?: string }) {
  const t = useT();
  useI18n();
  const selected = path ? files.filter(file => file.path === path) : files;
  return <section className="turn-file-diffs" aria-label={tr("desktop_ui_file_changes")} style={{ color: t.text, '--files-muted': t.text3 } as CSSProperties}>
    {selected.map(file => <div key={file.path}>
      <h3><bdi>{file.path}</bdi></h3>
      <p>+{file.additions} / −{file.removals}</p>
      {file.diffs.length > 1 && <p>{tr('desktop_ui_edits', file.diffs.length)}</p>}
      {file.diffs.map((diff, index) => <DiffView key={index} diff={diff} />)}
    </div>)}
  </section>;
}
