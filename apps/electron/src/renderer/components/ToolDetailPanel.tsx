import type { CSSProperties, ReactNode } from 'react';
import { useT } from '../theme/ThemeContext';

/** The shared surface beneath tool and skill disclosure headers. */
export function ToolDetailPanel({ label, footer, children }: {
  label: string;
  footer?: ReactNode;
  children: ReactNode;
}) {
  const t = useT();
  return <div className="tool-detail-panel" style={{
    '--tool-panel-background': t.dark ? t.surface : t.surfaceHover,
    '--tool-panel-border': t.border,
    '--tool-panel-text': t.text2,
    '--tool-panel-muted': t.text3,
    '--tool-panel-focus': t.accent,
  } as CSSProperties}>
    <div className="tool-detail-label">{label}</div>
    <div className="tool-detail-content" tabIndex={0} role="region" aria-label={label}>
      {children}
    </div>
    {footer && <div className="tool-detail-footer">{footer}</div>}
  </div>;
}
