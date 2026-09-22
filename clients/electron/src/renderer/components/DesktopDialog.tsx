import {
  forwardRef,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type FocusEventHandler,
  type KeyboardEventHandler,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';

import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

type DesktopDialogSize = 'compact' | 'regular' | 'wide';
type DesktopDialogTone = 'default' | 'danger';
type DesktopDialogButtonVariant = 'cancel' | 'secondary' | 'primary';

export interface DesktopDialogProps {
  title: ReactNode;
  summary?: ReactNode;
  eyebrow?: ReactNode;
  icon?: string;
  size?: DesktopDialogSize;
  tone?: DesktopDialogTone;
  ariaLabel?: string;
  ariaLabelledBy?: string;
  ariaDescribedBy?: string;
  titleId?: string;
  summaryId?: string;
  footer?: ReactNode;
  children: ReactNode;
  className?: string;
  tabIndex?: number;
  zIndex?: number;
  onEscape?(): void;
  onFocusCapture?: FocusEventHandler<HTMLDivElement>;
  onBlurCapture?: FocusEventHandler<HTMLDivElement>;
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>;
}

export const DesktopDialog = forwardRef<HTMLDivElement, DesktopDialogProps>(function DesktopDialog({
  title,
  summary,
  eyebrow,
  icon = 'info',
  size = 'regular',
  tone = 'default',
  ariaLabel,
  ariaLabelledBy,
  ariaDescribedBy,
  titleId,
  summaryId,
  footer,
  children,
  className,
  tabIndex,
  zIndex = 60,
  onEscape,
  onFocusCapture,
  onBlurCapture,
  onKeyDown,
}, ref) {
  const t = useT();
  const variables = {
    '--dialog-overlay': 'rgba(0, 0, 0, .32)',
    '--dialog-window': t.windowBg,
    '--dialog-surface': t.surface,
    '--dialog-surface-hover': t.surfaceHover,
    '--dialog-border': t.border,
    '--dialog-border-strong': t.borderStrong,
    '--dialog-text': t.text,
    '--dialog-text-2': t.text2,
    '--dialog-text-3': t.text3,
    '--dialog-accent': t.accent,
    '--dialog-accent-border': t.accentBorder,
    '--dialog-danger': t.danger,
    zIndex,
  } as CSSProperties;

  const dialog = (
    <div className="desktop-dialog-overlay" style={variables}>
      <div
        ref={ref}
        role="dialog"
        aria-label={ariaLabel}
        aria-labelledby={ariaLabelledBy}
        aria-describedby={ariaDescribedBy}
        tabIndex={tabIndex}
        data-tone={tone}
        className={`desktop-dialog-panel desktop-dialog-panel--${size}${className ? ` ${className}` : ''}`}
        onFocusCapture={onFocusCapture}
        onBlurCapture={onBlurCapture}
        onKeyDown={(event) => {
          if (event.key === 'Escape' && onEscape) {
            event.preventDefault();
            onEscape();
            return;
          }
          onKeyDown?.(event);
        }}
      >
        <header className="desktop-dialog-header">
          <span className="desktop-dialog-icon" aria-hidden="true">
            <Icon name={icon} size={24} stroke={1.8} />
          </span>
          <span className="desktop-dialog-heading-copy">
            {eyebrow && <span className="desktop-dialog-eyebrow">{eyebrow}</span>}
            <h2 id={titleId}>{title}</h2>
            {summary && <span id={summaryId} className="desktop-dialog-summary">{summary}</span>}
          </span>
        </header>

        <div className="desktop-dialog-body">{children}</div>

        {footer && <footer className="desktop-dialog-footer">{footer}</footer>}
      </div>
    </div>
  );

  // Dialogs can be rendered from inside an inert background (for example,
  // while SettingsScreen is open). Portaling to body keeps the visible modal
  // in the active interaction tree instead of letting clicks pass through to
  // the inert view underneath.
  return typeof document === 'undefined' || !document.body
    ? dialog
    : createPortal(dialog, document.body);
});

export function DesktopDialogActions({ children }: { children: ReactNode }) {
  return <div className="desktop-dialog-actions">{children}</div>;
}

export interface DesktopDialogButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: DesktopDialogButtonVariant;
}

export const DesktopDialogButton = forwardRef<HTMLButtonElement, DesktopDialogButtonProps>(function DesktopDialogButton({
  variant = 'secondary',
  className,
  type = 'button',
  ...props
}, ref) {
  return (
    <button
      ref={ref}
      type={type}
      className={`desktop-dialog-action desktop-dialog-action--${variant}${className ? ` ${className}` : ''}`}
      {...props}
    />
  );
});
