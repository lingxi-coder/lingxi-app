import { Fragment, useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactElement, type ReactNode, type RefObject } from 'react';
import type { TeamConversation } from './messages.js';
import { conversationParticipants, type Identity, type TeamsSnapshot } from './workspace.js';

/** Layout effects warn on the server; there they are plain effects. */
export const useIsoLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

/** Whether the page is visible; true where there is no document. */
export function useDocumentShown(): boolean {
  const [shown, setShown] = useState(() => typeof document === 'undefined' || document.visibilityState !== 'hidden');
  useEffect(() => {
    const change = () => setShown(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', change);
    return () => document.removeEventListener('visibilitychange', change);
  }, []);
  return shown;
}

// ─── Avatars ────────────────────────────────────────────────────────────────

/**
 * Lets the host draw its own avatar for a person or Bot (Desktop reuses its Bot avatar). Return
 * nothing to get the built-in initials avatar. `size` is in CSS pixels.
 */
export type RenderAvatar = (identity: Identity, size: number) => ReactNode;

/**
 * Lets the host render Markdown with the renderer its own chat uses (Desktop: `MarkdownContent`; Web: the remote
 * chat's `Markdown`). Shared Bot messages and Bot documents are Markdown; people's messages never are. Without it
 * they are shown as plain text. The host's renderer must never trust HTML or follow links other than http(s).
 */
export type RenderMarkdown = (text: string) => ReactNode;

/** Every entry has at least 4.5:1 contrast against the white initials. */
export const AVATAR_COLORS = ['#4a5fc1', '#2e7d6b', '#b04a6e', '#8a5a1f', '#6b4fa3', '#2a7aa3', '#9a4b2d', '#4f7a28', '#a1427d', '#5a6b78', '#3f6fb5', '#a8571a'] as const;

/** The same id always gets the same color (FNV-1a). */
export function avatarColor(id: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) { hash ^= id.charCodeAt(i); hash = Math.imul(hash, 0x01000193) >>> 0; }
  return AVATAR_COLORS[hash % AVATAR_COLORS.length]!;
}
const cjk = /[㐀-鿿豈-﫿぀-ヿ가-힯]/u;
/** One CJK character, or the first letters of the first two words. */
export function initials(name: string): string {
  const words = name.trim().split(/\s+/u).filter(Boolean);
  if (!words.length) return '?';
  const first = Array.from(words[0]!)[0]!;
  if (cjk.test(first)) return first;
  const second = words[1] ? Array.from(words[1])[0]! : '';
  return (first + second).toUpperCase();
}

export function Avatar({ identity, size, render }: { identity: Identity; size: number; render?: RenderAvatar }): ReactElement {
  const custom = render?.(identity, size);
  if (custom !== undefined && custom !== null && custom !== false) {
    return <span className="tw-avatar tw-avatar-custom" data-kind={identity.kind} style={{ width: size, height: size }} aria-hidden="true">{custom}</span>;
  }
  return <span className="tw-avatar" data-kind={identity.kind} aria-hidden="true"
    style={{ width: size, height: size, fontSize: Math.max(10, Math.round(size * 0.42)), background: avatarColor(identity.id) }}>{initials(identity.name)}</span>;
}

/** A team's square mark: its initials on a color that stays the same for the team. */
export function TeamMark({ id, name }: { id: string; name: string }): ReactElement {
  return <span className="tw-team-mark" aria-hidden="true" style={{ background: avatarColor(id) }}>{initials(name)}</span>;
}

/** Up to `max` avatars, then "+N". */
export function AvatarStack({ identities, size, render, max = 3, more }: { identities: Identity[]; size: number; render?: RenderAvatar; max?: number; more: (count: number) => string }): ReactElement {
  const shown = identities.slice(0, max), rest = identities.length - shown.length;
  return <span className="tw-stack">
    {shown.map(identity => <Avatar key={`${identity.kind}:${identity.id}`} identity={identity} size={size} render={render} />)}
    {rest > 0 ? <span className="tw-stack-more" title={more(rest)}>+{rest}</span> : null}
  </span>;
}

/**
 * The picture for a conversation: `#` for a channel, the other person for a dm, the Bot for a Bot dm,
 * and a short stack of members for a group. `label` is the name used when no participant is known.
 */
export function ConversationLead({ conversation, label, snapshot, zh, size = 28, render }: {
  conversation: TeamConversation; label: string; snapshot: Pick<TeamsSnapshot, 'directory' | 'bots' | 'uid'>; zh: boolean; size?: number; render?: RenderAvatar;
}): ReactElement {
  if (conversation.kind === 'channel') return <span className="tw-chan" aria-hidden="true" style={{ width: size, height: size, fontSize: Math.round(size * 0.55) }}>#</span>;
  const people = conversationParticipants(conversation, snapshot.directory, snapshot.bots, snapshot.uid, zh);
  const others = people.filter(person => !person.self);
  const fallback: Identity = { kind: conversation.kind === 'bot_dm' ? 'bot' : 'user', id: conversation.id, name: label };
  if (conversation.kind === 'group') {
    return <AvatarStack identities={others.length ? others : people.length ? people : [fallback]} size={Math.round(size * 0.7)} max={2} render={render} more={count => zh ? `另外 ${count} 人` : `${count} more`} />;
  }
  const lead = conversation.kind === 'bot_dm' ? people.find(person => person.kind === 'bot') : others[0] ?? people[0];
  return <Avatar identity={lead ?? fallback} size={size} render={render} />;
}

// ─── Icons ──────────────────────────────────────────────────────────────────

const paths = {
  chevronDown: 'M6 9l6 6 6-6',
  chevronLeft: 'M15 6l-6 6 6 6',
  chevronRight: 'M9 6l6 6-6 6',
  plus: 'M12 5v14M5 12h14',
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  x: 'M6 6l12 12M18 6L6 18',
  info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 11v5M12 8h.01',
  reply: 'M9 14L4 9l5-5M4 9h10a6 6 0 0 1 6 6v3',
  edit: 'M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4',
  trash: 'M5 7h14M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3',
  at: 'M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0zm0 0v1.5a2.5 2.5 0 0 0 5 0V12a9 9 0 1 0-3.5 7.1',
  send: 'M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z',
  check: 'M5 12l5 5 9-10',
  arrowDown: 'M12 5v14M6 13l6 6 6-6',
  panel: 'M4 5h16v14H4zM15 5v14',
  copy: 'M9 9h11v11H9zM5 15V5h10',
  bot: 'M12 8V4H8M6 8h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2zM2 14h2M20 14h2M15 13v2M9 13v2',
  clip: 'M21 11.5l-8.6 8.6a5.5 5.5 0 0 1-7.8-7.8l8.6-8.6a3.7 3.7 0 0 1 5.2 5.2l-8.6 8.6a1.8 1.8 0 0 1-2.6-2.6l7.9-7.9',
  file: 'M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5M9 13h6M9 17h6',
  image: 'M5 4h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM9 9.5h.01M20 15l-4.5-4.5L6 20',
  download: 'M12 4v11M7 11l5 5 5-5M5 20h14',
} as const;
export type IconName = keyof typeof paths;
export function Icon({ name, size = 16 }: { name: IconName; size?: number }): ReactElement {
  return <svg className="tw-icon" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={name === 'more' ? 3 : 2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"><path d={paths[name]} /></svg>;
}

/** Numeric unread badge. The label is the accessible name; the digits are only decoration. */
export function UnreadBadge({ count, zh }: { count: number; zh: boolean }): ReactElement | null {
  if (count <= 0) return null;
  return <span className="tw-badge" role="img" aria-label={zh ? `${count} 条未读` : `${count} unread`}>{count > 99 ? '99+' : count}</span>;
}

// ─── Dialog ─────────────────────────────────────────────────────────────────

/**
 * A modal built on the native `<dialog>`: it sits in the top layer (no clipping by an ancestor),
 * keeps focus inside, closes on Escape or a click outside its panel, and gives focus back to what
 * opened it. Render it only while it is open; theme variables still inherit from its parent.
 */
export function Dialog({ title, role = 'dialog', onClose, children }: { title: string; role?: 'dialog' | 'alertdialog'; onClose(): void; children: ReactNode }): ReactElement {
  const ref = useRef<HTMLDialogElement>(null), id = useId();
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (typeof node.showModal === 'function') { if (!node.open) node.showModal(); } else node.setAttribute('open', '');
    (node.querySelector<HTMLElement>('[data-autofocus]') ?? node.querySelector<HTMLElement>('input:not([type=hidden]), textarea, select, button'))?.focus();
    return () => { if (node.open && typeof node.close === 'function') node.close(); opener?.focus(); };
  }, []);
  return <dialog ref={ref} className="tw-dialog" role={role} aria-labelledby={id}
    onCancel={event => { event.preventDefault(); close.current(); }}
    onClick={event => { if (event.target === event.currentTarget) close.current(); }}>
    <div className="tw-dialog-panel">
      <h2 id={id} className="tw-dialog-title">{title}</h2>
      {children}
    </div>
  </dialog>;
}

// ─── Menus ──────────────────────────────────────────────────────────────────

export interface MenuItem { id: string; label: string; onSelect(): void; danger?: boolean; disabled?: boolean; /** A radio choice: `checked` says which. */ radio?: boolean; checked?: boolean; detail?: ReactNode; icon?: IconName; separated?: boolean }

/** Index of the next enabled item in a direction, wrapping around. `-1` when none is enabled. */
export function nextEnabled(items: readonly Pick<MenuItem, 'disabled'>[], from: number, step: 1 | -1): number {
  const count = items.length;
  for (let i = 1; i <= count; i++) {
    const index = ((from + step * i) % count + count) % count;
    if (!items[index]!.disabled) return index;
  }
  return -1;
}

/**
 * A menu opened from a trigger. Arrow keys, Home and End move, Enter or Space choose, Escape closes
 * and asks the host to focus the trigger again; a click or Tab elsewhere closes it without that.
 * Place it inside a `position: relative` wrapper; it opens downward and flips up near the window's bottom edge.
 */
export function PopupMenu({ label, items, onClose, anchor, align = 'start' }: {
  label: string; items: MenuItem[]; onClose(restoreFocus: boolean): void; anchor?: RefObject<HTMLElement | null>; align?: 'start' | 'end';
}): ReactElement {
  const ref = useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = useState('');
  const close = useRef(onClose);
  close.current = onClose;
  useIsoLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const box = node.getBoundingClientRect();
    setPlacement(`${box.bottom > window.innerHeight - 8 && box.top > box.height ? ' tw-menu-up' : ''}${box.right > window.innerWidth - 8 ? ' tw-menu-left' : ''}`);
  }, []);
  useEffect(() => {
    const buttons = ref.current?.querySelectorAll<HTMLElement>('[role^=menuitem]');
    const start = items.findIndex(item => item.checked && !item.disabled);
    buttons?.[start >= 0 ? start : Math.max(0, items.findIndex(item => !item.disabled))]?.focus();
    const outside = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && (ref.current?.contains(target) || anchor?.current?.contains(target))) return;
      close.current(false);
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  // The first focus and the outside-click listener belong to opening, not to every change of the items.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const move = (event: KeyboardEvent<HTMLDivElement>) => {
    const buttons = [...(ref.current?.querySelectorAll<HTMLElement>('[role^=menuitem]') ?? [])];
    const current = buttons.indexOf(document.activeElement as HTMLElement);
    let target = -1;
    if (event.key === 'ArrowDown') target = nextEnabled(items, current, 1);
    else if (event.key === 'ArrowUp') target = nextEnabled(items, current < 0 ? 0 : current, -1);
    else if (event.key === 'Home') target = nextEnabled(items, -1, 1);
    else if (event.key === 'End') target = nextEnabled(items, 0, -1);
    else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close.current(true); return; }
    else if (event.key === 'Tab') { close.current(false); return; }
    else return;
    event.preventDefault();
    if (target >= 0) buttons[target]?.focus();
  };
  const choose = (item: MenuItem) => { if (item.disabled) return; close.current(true); item.onSelect(); };
  return <div ref={ref} role="menu" aria-label={label} className={`tw-menu${align === 'end' ? ' tw-menu-end' : ''}${placement}`} onKeyDown={move}>
    {items.map(item => <Fragment key={item.id}>
      {item.separated ? <div role="separator" className="tw-menu-sep" /> : null}
      <button type="button" role={item.radio ? 'menuitemradio' : 'menuitem'} aria-checked={item.radio ? !!item.checked : undefined} tabIndex={-1}
        className={`tw-menu-item${item.danger ? ' tw-danger' : ''}`} disabled={item.disabled} onClick={() => choose(item)}>
        {item.icon ? <Icon name={item.icon} /> : <span className="tw-menu-gap">{item.checked ? <Icon name="check" /> : null}</span>}
        <span className="tw-menu-label">{item.label}</span>
        {item.detail ? <span className="tw-menu-detail">{item.detail}</span> : null}
      </button>
    </Fragment>)}
  </div>;
}

// ─── Keyboard ───────────────────────────────────────────────────────────────

/**
 * Where roving focus goes for a key press in a list or tab strip: the next or previous item
 * (wrapping only when asked to), Home and End. `undefined` when the key does not move focus.
 */
export function rovingIndex(count: number, current: number, key: string, options: { prev?: string; next?: string; wrap?: boolean } = {}): number | undefined {
  if (count <= 0) return undefined;
  const { prev = 'ArrowUp', next = 'ArrowDown', wrap = false } = options;
  if (key === 'Home') return 0;
  if (key === 'End') return count - 1;
  if (key !== next && key !== prev) return undefined;
  const from = current < 0 ? (key === next ? -1 : count) : current;
  const to = key === next ? from + 1 : from - 1;
  if (wrap) return (to % count + count) % count;
  return Math.max(0, Math.min(count - 1, to));
}

// ─── Shared wording ─────────────────────────────────────────────────────────

/** Which tab of the details drawer to show. */
export type DrawerTab = 'members' | 'invites' | 'bots';

/** Who can read Team data. Shown wherever a person creates, joins or opens a team, and on the conversation header and the details panel. */
export const platformNotice = (zh: boolean): string => zh
  ? '团队资料与消息平台可读。与 Bot 的对话对该 Bot 的所有者可见。'
  : 'Team data and messages are readable by the platform. A Bot’s owner can read conversations with their Bot.';
export const platformChip = (zh: boolean): string => zh ? '平台可读' : 'Platform readable';
