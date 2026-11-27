/** @jsxRuntime automatic */
import type { ReactNode } from 'react';
import './bot-card.css';

export function BotCardSummary({ avatar, name, description, status, metadata, heading, compact }: {
  avatar: ReactNode;
  name: string;
  description?: string;
  status: ReactNode;
  metadata: ReactNode;
  heading?: 'h3' | 'h4';
  compact?: boolean;
}) {
  const Layout = heading ? 'div' : 'span';
  const Title = heading ?? 'span';
  return <Layout className="bot-card-summary" data-compact={compact || undefined}>
    <span className="bot-card-avatar" aria-hidden="true">{avatar}</span>
    <Title className="bot-card-name">{name}</Title>
    {description ? <span className="bot-card-description">{description}</span> : null}
    <span className="bot-card-meta">{status}<span>{metadata}</span></span>
  </Layout>;
}
