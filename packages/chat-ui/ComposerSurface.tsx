/** @jsxRuntime automatic */
import { useState, type ComponentPropsWithoutRef, type ReactNode } from 'react';
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

export function ComposerImageThumbnail({ src }: { src: string }) {
  const [dimensions, setDimensions] = useState('');
  return <>
    <img className="composer-image-thumbnail" src={src} alt="" draggable={false}
      onLoad={({ currentTarget }) => setDimensions(`${currentTarget.naturalWidth} × ${currentTarget.naturalHeight}`)} />
    {dimensions && <span className="composer-image-dimensions">{dimensions}</span>}
  </>;
}
