import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import {
  isUnread,
  type InboxCategory,
  type InboxFilter,
  type InboxItem,
  type InboxNoticeTarget,
} from './contracts.js';
import type { InboxController } from './controller.js';
import { NoticeHistory } from './NoticeHistory.js';
import { inboxErrorText } from './errors.js';
import './inbox.css';

export interface InboxListProps {
  controller: InboxController;
  onOpen: (item: InboxItem, noticeTarget?: InboxNoticeTarget) => void | Promise<void>;
  locale?: string;
  activeKey?: string;
}
const filters: InboxFilter[] = ['all', 'unread', 'attention'];
const messageCategories: InboxCategory[] = ['friend', 'team', 'service'];
const categories: InboxCategory[] = ['assistant', ...messageCategories];
const en = {
  all: 'All',
  unread: 'Unread',
  attention: 'Needs attention',
  assistant: 'Assistant',
  friend: 'Friends',
  team: 'Teams',
  service: 'Services',
};
const zh = {
  all: '全部',
  unread: '未读',
  attention: '需要处理',
  assistant: '助手',
  friend: '好友',
  team: '团队',
  service: '服务',
};
export { inboxErrorText } from './errors.js';
const cappedCount = (n: number) => (n >= 100 ? '99+' : String(n));
function statusText(row: InboxItem, cn: boolean) {
  if (row.state === 'waiting_for_approval' || row.attentionKind === 'approval')
    return cn ? '待批准' : 'Approval needed';
  if (row.state === 'waiting_for_input' || row.attentionKind === 'input')
    return cn ? '待输入' : 'Input needed';
  if (row.state === 'failed' || row.attentionKind === 'failure') return cn ? '失败' : 'Failed';
  if (row.offline) return cn ? '离线' : 'Offline';
  if (row.state === 'running') return cn ? '进行中' : 'Running';
  return '';
}

export function InboxList({ controller, onOpen, activeKey, locale = 'en' }: InboxListProps) {
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
  const cn = locale.startsWith('zh'),
    labels = cn ? zh : en;
  const date = useMemo(() => new Intl.DateTimeFormat(locale, { month: 'short', day: 'numeric' }), [locale]);
  const time = useMemo(
    () => new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit' }),
    [locale],
  );
  const [search, setSearch] = useState(''),
    [scroll, setScroll] = useState(0),
    [viewport, setViewport] = useState(560);
  const [history, setHistory] = useState(false);
  const [actionKey, setActionKey] = useState(''),
    [error, setError] = useState('');
  const [height, setHeight] = useState(64),
    [heading, setHeading] = useState(23);
  const probe = useRef<HTMLDivElement>(null),
    groupProbe = useRef<HTMLDivElement>(null),
    menu = useRef<HTMLDetailsElement>(null);
  const box = useRef<HTMLDivElement>(null),
    searchId = useId(),
    catId = useId();
  const prefix = search.trim().normalize('NFKC').toLowerCase();
  const rows = useMemo(
    () =>
      prefix
        ? state.items.filter((row) => row.title.normalize('NFKC').toLowerCase().startsWith(prefix))
        : state.items,
    [state.items, prefix],
  );
  useEffect(() => {
    const timer = setTimeout(
      () => void controller.setFilter(state.filter, state.category, state.archived, prefix),
      250,
    );
    return () => clearTimeout(timer);
  }, [controller, state.filter, state.category, state.archived, prefix]);
  const firstRecent = useMemo(() => rows.findIndex((row) => row.pinOrder === null), [rows]);
  const grouped = !prefix && !!rows.length && firstRecent !== 0;
  const sections = grouped ? (firstRecent > 0 ? 2 : 1) : 0;
  function topOf(index: number) {
    return index * height + (grouped ? heading * (firstRecent > 0 && index >= firstRecent ? 2 : 1) : 0);
  }
  function indexAt(offset: number) {
    const headers = grouped ? 1 + (firstRecent > 0 && offset >= firstRecent * height + heading ? 1 : 0) : 0;
    return Math.max(0, Math.min(rows.length - 1, Math.floor((offset - headers * heading) / height)));
  }
  const start = Math.max(0, indexAt(scroll) - 3);
  const end = Math.min(rows.length, indexAt(scroll + viewport) + 5);
  const drafts = useMemo(
    () => new Map(rows.slice(start, end).map((row) => [row.key, controller.draftPreview(row)])),
    [controller, rows, start, end],
  );
  const current = rows.find((row) => row.key === actionKey);
  useEffect(() => {
    const node = box.current;
    if (!node || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      setViewport(node.clientHeight);
      if (probe.current) setHeight(probe.current.offsetHeight || 64);
      if (groupProbe.current) setHeading(groupProbe.current.offsetHeight || 23);
    });
    if (probe.current) observer.observe(probe.current);
    if (groupProbe.current) observer.observe(groupProbe.current);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (box.current) box.current.scrollTop = 0;
    setScroll(0);
    setActionKey('');
  }, [state.filter, state.category, state.archived, search]);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (menu.current?.open && event.target instanceof Node && !menu.current.contains(event.target))
        menu.current.open = false;
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, []);
  const previous = useRef<{ top: string; offset: number; rows: InboxItem[] }>();
  useLayoutEffect(() => {
    const last = previous.current,
      node = box.current;
    if (!node) return;
    if (last && last.rows !== rows && node.scrollTop > 0) {
      const pos = rows.findIndex((row) => row.key === last.top);
      if (pos >= 0) node.scrollTop = topOf(pos) + last.offset;
    }
    const position = indexAt(node.scrollTop),
      top = rows[position];
    previous.current = top ? { top: top.key, offset: node.scrollTop - topOf(position), rows } : undefined;
  }, [rows, scroll, height, heading]);
  function run(action: () => void | Promise<unknown>) {
    setError('');
    void Promise.resolve()
      .then(action)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }
  function move(index: number) {
    const node = box.current;
    const row = rows[index];
    if (!node || !row) return;
    if (topOf(index) < node.scrollTop) node.scrollTop = topOf(index);
    if (topOf(index) + height > node.scrollTop + node.clientHeight)
      node.scrollTop = topOf(index) + height - node.clientHeight;
    setScroll(node.scrollTop);

    requestAnimationFrame(() =>
      node
        .querySelector<HTMLButtonElement>('[data-row-key="' + CSS.escape(row.key) + '"]')
        ?.focus({ preventScroll: true }),
    );
  }
  return (
    <section className="lx-inbox">
      <div className="lx-inbox-probe" ref={probe} aria-hidden="true" />
      <div className="lx-inbox-group-probe" ref={groupProbe} aria-hidden="true" />
      <div className="lx-inbox-tools">
        <div className="lx-inbox-search">
          <label className="lx-inbox-sr" htmlFor={searchId}>
            {cn ? '搜索会话' : 'Search conversations'}
          </label>
          <input
            id={searchId}
            type="search"
            maxLength={120}
            value={search}
            placeholder={cn ? '搜索会话' : 'Search conversations'}
            onChange={(e) => setSearch(e.target.value)}
          />
          <details
            ref={menu}
            className="lx-inbox-more"
            onKeyDown={(e) => {
              if (e.key === 'Escape' && menu.current) {
                menu.current.open = false;
                menu.current.querySelector('summary')?.focus();
              }
            }}
          >
            <summary aria-label={cn ? '更多收件箱操作' : 'More inbox actions'}>⋯</summary>
            <div
              className="lx-inbox-menu"
              onClick={(e) => {
                if ((e.target as Element).closest('button') && menu.current) {
                  menu.current.open = false;
                  menu.current.querySelector('summary')?.focus();
                }
              }}
            >
              <button type="button" aria-pressed={history} onClick={() => setHistory(!history)}>
                {cn ? '通知历史' : 'Notification history'}
              </button>
              <button
                type="button"
                aria-pressed={state.archived}
                onClick={() => run(() => controller.setFilter(state.filter, state.category, !state.archived))}
              >
                {cn ? '已归档' : 'Archived'}
              </button>
              <button
                type="button"
                disabled={!rows.some(isUnread)}
                onClick={() => run(() => controller.markAllRead(rows.filter(isUnread)))}
              >
                {cn ? '将已列出的会话标为已读' : 'Mark listed conversations read'}
              </button>
              <button
                type="button"
                disabled={state.offline || state.busy || state.batchBusy}
                onClick={() => run(() => controller.markAllReadSnapshot())}
              >
                {cn ? '全部标为已读' : 'Mark all read'}
              </button>
            </div>
          </details>
        </div>
        <div className="lx-inbox-filterbar">
          <div className="lx-inbox-switch" role="group" aria-label={cn ? '收件箱视图' : 'Inbox views'}>
            {filters.map((filter) => (
              <button
                key={filter}
                type="button"
                aria-pressed={filter === state.filter}
                onClick={() => run(() => controller.setFilter(filter, state.category, state.archived))}
              >
                {filter === 'attention' ? (cn ? '待处理' : 'Attention') : labels[filter]}
              </button>
            ))}
          </div>
          {controller.scope !== 'assistant' && (
            <>
              <label className="lx-inbox-sr" htmlFor={catId}>
                {cn ? '类别' : 'Category'}
              </label>
              <select
                id={catId}
                value={state.category ?? ''}
                onChange={(e) => {
                  const cat = e.target.value as InboxCategory;
                  run(() => controller.setFilter(state.filter, cat || undefined, state.archived));
                }}
              >
                <option value="">{cn ? '类别' : 'Category'}</option>
                {(controller.scope === 'messages' ? messageCategories : categories).map((category) => (
                  <option key={category} value={category}>
                    {labels[category]}
                  </option>
                ))}
              </select>
            </>
          )}
        </div>
      </div>
      {state.batchBusy ? <p role="status">{cn ? '正在标为已读…' : 'Marking conversations read…'}</p> : null}
      {history ? <NoticeHistory controller={controller} cn={cn} onOpen={onOpen} /> : null}
      {state.offline ? <p role="status">{state.cloudUnavailable
        ? (cn ? '云端会话暂不可用，本机会话仍可使用。' : 'Cloud conversations are unavailable. Local chats are still available.')
        : (cn ? '离线' : 'Offline')}</p> : null}
      {state.error || error ? (
        <p role="alert">
          {inboxErrorText(error || state.error, cn)}{' '}
          <button type="button" onClick={() => run(() => controller.refresh())}>
            {cn ? '重试' : 'Retry'}
          </button>
        </p>
      ) : null}
      <div
        className="lx-inbox-scroll"
        ref={box}
        onScroll={(event) => setScroll(event.currentTarget.scrollTop)}
      >
        <ol
          role="list"
          aria-label={cn ? '会话' : 'Conversations'}
          style={{ height: rows.length * height + sections * heading }}
        >
          {grouped ? (
            <li className="lx-inbox-section" role="presentation" style={{ height: heading, top: 0 }}>
              <span role="heading" aria-level={3}>
                {cn ? '置顶' : 'Pinned'}
              </span>
            </li>
          ) : null}
          {grouped && firstRecent > 0 ? (
            <li
              className="lx-inbox-section"
              role="presentation"
              style={{ height: heading, top: topOf(firstRecent) - heading }}
            >
              <span role="heading" aria-level={3}>
                {cn ? '最近' : 'Recent'}
              </span>
            </li>
          ) : null}
          {rows.slice(start, end).map((row, i) => {
            const status = statusText(row, cn),
              draft = drafts.get(row.key);
            const preview =
              [status, draft ? (cn ? '草稿 ' : 'Draft: ') + draft : row.preview]
                .filter(Boolean)
                .join(' · ') ||
              row.sourceLabel ||
              labels[row.category];
            const context = [
              row.sourceLabel || labels[row.category],
              row.notificationMode === 'none' ? (cn ? '已静音' : 'Muted') : '',
            ]
              .filter(Boolean)
              .join(' · ');
            return (
              <li
                key={row.key}
                aria-posinset={start + i + 1}
                aria-setsize={rows.length}
                className="lx-inbox-entry"
                data-current={row.key === activeKey || undefined}
                style={{ height, top: topOf(start + i) }}
              >
                <button
                  type="button"
                  className="lx-inbox-row"
                  data-row-key={row.key}
                  aria-current={row.key === activeKey ? 'page' : undefined}
                  title={[row.title, context, preview].join(' · ')}
                  onKeyDown={(event) => {
                    const idx = start + i;
                    const next = { ArrowDown: idx + 1, ArrowUp: idx - 1, Home: 0, End: rows.length - 1 }[
                      event.key
                    ];
                    if (next !== undefined) {
                      event.preventDefault();
                      move(next);
                    }
                  }}
                  onClick={() => run(() => onOpen(row))}
                >
                  <i className="lx-inbox-avatar" aria-hidden="true">
                    {(row.title || labels[row.category]).trim().slice(0, 1)}
                  </i>
                  <span className="lx-inbox-line lx-inbox-headline">
                    <strong className="lx-inbox-title">{row.title || labels[row.category]}</strong>
                    {row.sourceLabel?.trim() && row.sourceLabel.trim() !== row.title.trim() && (
                      <small className="lx-inbox-context" title={row.sourceLabel}>
                        {row.sourceLabel}
                      </small>
                    )}
                    {row.lastActivityAt > 0 ? (
                      <time dateTime={new Date(row.lastActivityAt).toISOString()}>
                        {(new Date(row.lastActivityAt).toDateString() === new Date().toDateString()
                          ? time
                          : date
                        ).format(row.lastActivityAt)}
                      </time>
                    ) : null}
                  </span>
                  <span className="lx-inbox-line lx-inbox-subline">
                    <span
                      className="lx-inbox-preview"
                      data-attention={
                        Boolean(status && status !== 'Running' && status !== '进行中') || undefined
                      }
                    >
                      {preview}
                    </span>
                    {row.mentionCount > 0 ? (
                      <span
                        className="lx-inbox-chip"
                        aria-label={(cn ? '提及 ' : 'Mentions ') + cappedCount(row.mentionCount)}
                      >
                        @{cappedCount(row.mentionCount)}
                      </span>
                    ) : null}
                    {row.pendingActionCount > 0 ? (
                      <span
                        className="lx-inbox-chip"
                        data-action=""
                        aria-label={
                          (cn ? '待处理 ' : 'Pending actions ') + cappedCount(row.pendingActionCount)
                        }
                      >
                        !{cappedCount(row.pendingActionCount)}
                      </span>
                    ) : null}
                    {isUnread(row) ? (
                      <b
                        className="lx-inbox-chip"
                        data-unread=""
                        aria-label={
                          row.unreadCount
                            ? (cn ? '未读 ' : 'Unread ') + cappedCount(row.unreadCount)
                            : labels.unread
                        }
                      >
                        {row.unreadCount ? cappedCount(row.unreadCount) : '•'}
                      </b>
                    ) : null}
                  </span>
                  <span className="lx-inbox-sr">{context}</span>
                </button>
                <button
                  className="lx-inbox-actions"
                  type="button"
                  aria-label={(cn ? '操作 ' : 'Actions for ') + row.title}
                  aria-expanded={actionKey === row.key}
                  onClick={() => setActionKey((old) => (old === row.key ? '' : row.key))}
                >
                  ⋯
                </button>
              </li>
            );
          })}
        </ol>
      </div>
      {!state.busy && !rows.length ? <p>{cn ? '没有会话' : 'No conversations'}</p> : null}
      {state.hasMore ? (
        <button type="button" disabled={state.busy} onClick={() => run(() => controller.more())}>
          {search ? (cn ? '在更多会话中搜索' : 'Search more conversations') : cn ? '加载更多' : 'Load more'}
        </button>
      ) : null}
      {current ? (
        <div className="lx-inbox-actions-bar" role="group" aria-label={current.title}>
          <strong>{current.title}</strong>
          <button
            type="button"
            onClick={() =>
              run(() => (isUnread(current) ? controller.markRead(current) : controller.markUnread(current)))
            }
          >
            {isUnread(current) ? (cn ? '标为已读' : 'Mark read') : cn ? '标为未读' : 'Mark unread'}
          </button>
          <button
            type="button"
            aria-pressed={current.pinOrder !== null}
            onClick={() =>
              run(() =>
                controller.preferences(current, { pinOrder: current.pinOrder === null ? Date.now() : null }),
              )
            }
          >
            {cn ? '置顶' : 'Pin'}
          </button>
          <button
            type="button"
            aria-pressed={current.archived}
            onClick={() => run(() => controller.preferences(current, { archived: !current.archived }))}
          >
            {cn ? '归档' : 'Archive'}
          </button>
          {current.pinOrder !== null && !prefix ? (
            <>
              <button
                type="button"
                disabled={rows.findIndex((i) => i.key === current.key) <= 0}
                onClick={() => run(() => controller.movePinned(current, -1))}
              >
                {cn ? '上移' : 'Move up'}
              </button>
              <button
                type="button"
                disabled={rows[rows.findIndex((i) => i.key === current.key) + 1]?.pinOrder == null}
                onClick={() => run(() => controller.movePinned(current, 1))}
              >
                {cn ? '下移' : 'Move down'}
              </button>
            </>
          ) : null}
          <label>
            {cn ? '通知' : 'Notifications'}{' '}
            <select
              value={current.notificationMode}
              onChange={(e) => {
                const notificationMode = e.target.value as InboxItem['notificationMode'];
                run(() => controller.preferences(current, { notificationMode }));
              }}
            >
              <option value="all">{labels.all}</option>
              <option value="mentions">{cn ? '提及' : 'Mentions'}</option>
              <option value="none">{cn ? '静音' : 'Mute'}</option>
            </select>
          </label>
        </div>
      ) : null}
    </section>
  );
}
