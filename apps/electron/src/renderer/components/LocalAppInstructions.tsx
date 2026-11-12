import { tr } from '../i18n/locale';
import type { CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { MarkdownContent } from './MarkdownContent';
import { skillInstructionText } from './localAppInstructionPresentation';

export function LocalAppInstructions({ id, text, name = 'local-app', open, onSetOpen }: {
  id: string;
  text: string;
  name?: string;
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
      <span className="local-app-instructions-icon" aria-hidden="true"><Icon name="spark" size={16} stroke={1.6} /></span>
      <span className="local-app-instructions-title" title={name}>{name === 'local-app' ? tr('desktop_local_apps') : name}</span>
      <span className="local-app-instructions-label">{tr('desktop_instructions')}</span>
      <span className="local-app-instructions-chevron" aria-hidden="true"><Icon name={open ? 'chevron' : 'chevronR'} size={12} stroke={1.8} /></span>
    </button>
    <div id={contentId} hidden={!open} className="local-app-instructions-body" tabIndex={open ? 0 : undefined}
      role="region" aria-label={`${name} · ${tr('desktop_instructions')}`}>
      {open && <MarkdownContent text={skillInstructionText(text)} variant="compact" />}
    </div>
  </section>;
}
