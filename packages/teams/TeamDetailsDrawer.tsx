import { useEffect, useId, useRef, useState, type KeyboardEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { useTeamsText } from './i18n.js';
import type { TeamMember } from './index.js';
import { Avatar, ConversationLead, Icon, type RenderAvatar } from './ui';
import { conversationLabel, conversationParticipants, type TeamsSnapshot } from './workspace.js';

export const DRAWER_MIN = 280;
export const DRAWER_MAX = 480;
export const DRAWER_DEFAULT = 360;

/** A drawer width kept within its limits. Anything that is not a finite number becomes the default. */
export const clampDrawerWidth = (width: number): number => Number.isFinite(width) ? Math.min(DRAWER_MAX, Math.max(DRAWER_MIN, Math.round(width))) : DRAWER_DEFAULT;
/** The drawer is on the right, so dragging its left edge to the left widens it. */
export const drawerWidthForDrag = (startWidth: number, startX: number, x: number): number => clampDrawerWidth(startWidth + (startX - x));
export function drawerWidthForKey(width: number, key: string, shift = false): number | undefined {
  const step = shift ? 48 : 16;
  if (key === 'ArrowLeft') return clampDrawerWidth(width + step);
  if (key === 'ArrowRight') return clampDrawerWidth(width - step);
  if (key === 'Home') return DRAWER_MIN;
  if (key === 'End') return DRAWER_MAX;
  return undefined;
}

export interface TeamDetailsDrawerProps {
  snapshot: TeamsSnapshot;
  /** The width in CSS pixels, when the host keeps it; otherwise the drawer does. */
  width?: number;
  defaultWidth?: number;
  onWidthChange?(width: number): void;
  onClose(): void;
  renderAvatar?: RenderAvatar;
  locale: string;
  onAddFriend?(member: TeamMember): void;
}

export function TeamDetailsDrawer({ snapshot, width: controlledWidth, defaultWidth, onWidthChange, onClose, renderAvatar, locale, onAddFriend }: TeamDetailsDrawerProps) {
  const t = useTeamsText(locale);
  const id = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const drag = useRef<{ x: number; width: number }>();
  const [ownWidth, setOwnWidth] = useState(() => clampDrawerWidth(defaultWidth ?? DRAWER_DEFAULT));
  const { conversation, directory, bots, uid } = snapshot;
  const hasConversation = !!conversation;
  useEffect(() => {
    if (!hasConversation) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    heading.current?.focus({ preventScroll: true });
    return () => { if (opener?.isConnected) opener.focus({ preventScroll: true }); };
  }, [hasConversation]);
  if (!conversation) return null;

  const width = clampDrawerWidth(controlledWidth ?? ownWidth);
  const setWidth = (next: number) => {
    const value = clampDrawerWidth(next);
    if (controlledWidth === undefined) setOwnWidth(value);
    if (value !== width) onWidthChange?.(value);
  };
  const resizeDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    drag.current = { x: event.clientX, width };
    try { event.currentTarget.setPointerCapture(event.pointerId); } catch { /* A pointer can end before capture. */ }
  };
  const resizeMove = (event: ReactPointerEvent<HTMLDivElement>) => { if (drag.current) setWidth(drawerWidthForDrag(drag.current.width, drag.current.x, event.clientX)); };
  const resizeUp = () => { drag.current = undefined; };
  const resizeKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = drawerWidthForKey(width, event.key, event.shiftKey);
    if (next === undefined) return;
    event.preventDefault();
    setWidth(next);
  };

  const label = conversationLabel(conversation, directory, uid, { bots, locale });
  const channel = conversation.kind === 'channel';
  const people = channel && !conversation.participants.length
    ? directory.map(member => ({ kind: 'user' as const, id: member.uid, name: member.displayName || t('teams_member'), self: member.uid === uid }))
    : conversationParticipants(conversation, directory, bots, uid, locale);
  const memberCount = people.filter(person => person.kind === 'user').length;
  const count = t(memberCount === 1 ? 'teams_members_count_one_fmt' : 'teams_members_count_other_fmt', memberCount);
  const detailsLabel = t(channel ? 'tw_channel_details' : 'teams_conversation_details');
  return <aside className="tw-scope tw-drawer" role="complementary" aria-label={detailsLabel} style={{ width }} onKeyDown={event => {
    if (event.key === 'Escape' && !event.defaultPrevented) { event.stopPropagation(); onClose(); }
  }}>
    <div className="tw-resize" role="separator" aria-orientation="vertical" aria-label={t('tw_team_resize')}
      aria-valuemin={DRAWER_MIN} aria-valuemax={DRAWER_MAX} aria-valuenow={width} tabIndex={0}
      onPointerDown={resizeDown} onPointerMove={resizeMove} onPointerUp={resizeUp} onPointerCancel={resizeUp}
      onLostPointerCapture={resizeUp} onKeyDown={resizeKey} />
    <header className="tw-drawer-head">
      <ConversationLead conversation={conversation} label={label} snapshot={snapshot} locale={locale} size={40} render={renderAvatar} />
      <div className="tw-drawer-title">
        <h2 ref={heading} tabIndex={-1}>{label}</h2>
        <p>{channel ? `${t('teams_conversation_channel')} · ${count}` : count}</p>
      </div>
      <button type="button" className="tw-icon-btn" aria-label={t('tw_team_close_details')} title={t('tw_team_close_details')} onClick={onClose}><Icon name="x" /></button>
    </header>
    {channel ? <div className="tw-tabs" role="tablist" aria-label={detailsLabel}>
      <button type="button" role="tab" className="tw-tab" id={`${id}-members`} aria-selected="true" aria-controls={`${id}-people`}>{t('teams_members')}</button>
    </div> : null}
    <div className="tw-drawer-body" {...(channel ? { role: 'tabpanel', id: `${id}-people`, 'aria-labelledby': `${id}-members`, tabIndex: 0 } : {})}>
      <ul className="tw-people" aria-label={t('teams_members')}>
        {people.map(person => {
          const member = person.kind === 'user' && !person.self ? directory.find(candidate => candidate.uid === person.id) : undefined;
          const identity = <>
            <Avatar identity={person} size={40} render={renderAvatar} />
            <div className="tw-person-text"><b>{person.name}</b>{person.self && person.name !== t('teams_you') ? <small>{t('teams_you')}</small>
              : person.kind === 'bot' ? <small>{t('teams_bot_tag')}</small> : null}</div>
          </>;
          return <li key={`${person.kind}:${person.id}`}>
            {member && onAddFriend ? <details className="tw-member-actions">
              <summary className="tw-person-row" aria-label={`${person.name} · ${t('tw_more')}`}>
                {identity}<span className="tw-person-more"><Icon name="more" /></span>
              </summary>
              <div className="tw-person-actions"><button type="button" onClick={() => onAddFriend(member)}>{t('friends_add')}</button></div>
            </details> : <div className="tw-person-row">{identity}</div>}
          </li>;
        })}
      </ul>
    </div>
  </aside>;
}
