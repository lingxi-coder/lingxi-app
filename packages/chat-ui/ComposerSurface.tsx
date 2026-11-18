/** @jsxRuntime automatic */
import type { ComponentPropsWithoutRef, ReactNode } from 'react';
import './composer-surface.css';

type DivProps = ComponentPropsWithoutRef<'div'>;

export function ComposerSurface({ className, ...props }: DivProps) {
  return <div {...props} className={className ? `composer-surface ${className}` : 'composer-surface'} />;
}

export function ComposerEditor({ children }: { children: ReactNode }) {
  return <div className="composer-input-view">{children}</div>;
}

export function ComposerToolbar({ children }: { children: ReactNode }) {
  return <div className="composer-toolbar">{children}</div>;
}
