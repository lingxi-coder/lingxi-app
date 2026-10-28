import { tr } from '../i18n/locale';
import type { CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { MarkdownContent } from './MarkdownContent';

export function LocalAppInstructions({ id, text, open, onSetOpen }: {
  id: string;
  text: string;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const t = useT();
  const contentId = `local-app-instructions-${id}`;
  return <section className="local-app-instructions" style={{
    '--instructions-text': t.text2,
    '--instructions-muted': t.text3,
    '--instructions-border': t.border,
    '--instructions-hover': t.surfaceHover,
    '--instructions-focus': t.accent,
  } as CSSProperties}>
    <button type="button" className="local-app-instructions-toggle"
      aria-expanded={open} aria-controls={contentId} onClick={() => onSetOpen(id, !open)}>
      <Icon name="spark" size={15} stroke={1.6} />
      <span className="local-app-instructions-title">{tr('desktop_local_apps')}</span>
      <span className="local-app-instructions-label">{tr('desktop_instructions')}</span>
      <Icon name={open ? 'chevron' : 'chevronR'} size={12} stroke={1.8} />
    </button>
    <div id={contentId} hidden={!open} className="local-app-instructions-body">
      {open && <MarkdownContent text={text} variant="compact" />}
    </div>
  </section>;
}
