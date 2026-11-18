import { useEffect, useRef, useState } from 'react';
import type { InboxItem } from '../../../../../packages/inbox/contracts';
import type { DesktopInboxApi, InboxNotification } from '../../shared/inbox';
import { tr } from '../i18n/locale';
import { Icon } from './Icon';

const visibleMs = 8_000;
const eventLabels: Record<string, readonly [string, string]> = {
  mention: ['提到了你', 'Mentioned you'],
  reply: ['有新回复', 'New reply'],
  dm: ['新私聊消息', 'New direct message'],
  message: ['有新消息', 'New message'],
  input: ['等待你的输入', 'Waiting for your input'],
  approval: ['等待你的批准', 'Waiting for your approval'],
  completion: ['任务已完成', 'Task completed'],
  failure: ['任务执行失败', 'Task failed'],
};

type Props = {
  notice?: InboxNotification;
  api?: DesktopInboxApi;
  uid: string | null;
  locale: string;
  onOpen: (notice: InboxNotification) => void;
  onDismiss: (notice: InboxNotification) => void;
};

export function DesktopInboxToast({ notice, api, uid, locale, onOpen, onDismiss }: Props) {
  const [resolved, setResolved] = useState<{
    notice: InboxNotification; uid: string | null; item: InboxItem; preview: string;
  }>();
  useEffect(() => {
    setResolved(undefined);
    const key = notice?.key;
    if (!notice || !key || !api) return;
    let current = true;
    void api.request<{ item: InboxItem }>(uid, { action: 'get', key, generation: notice.generation })
      .then(async ({ item }) => {
        if (!current || item.key !== key || item.ref.generation !== notice.generation) return;
        // The row holds the latest message. It may already have moved on to a newer event.
        const preview = item.ref.kind !== 'assistant' && item.sourceCursor === notice.through ? item.preview : '';
        setResolved({ notice, uid, item, preview });
        if (item.ref.kind === 'assistant' && notice.through && ['reply', 'completion'].includes(notice.kind ?? '')) {
          const result = await api.activityPreview(key, notice.through);
          if (!current || !result.available) return;
          const firstText = result.messages.flatMap(message => message.blocks)
            .find(block => block.type === 'text' && block.text.trim());
          if (firstText?.type === 'text') setResolved({ notice, uid, item, preview: firstText.text });
        }
      }).catch(() => { /* The event label stays available if details cannot be loaded. */ });
    return () => { current = false; };
  }, [api, notice, uid]);

  const details = resolved && resolved.notice === notice && resolved.uid === uid ? resolved : undefined;
  const chinese = locale.startsWith('zh');
  const title = details?.item.title.trim() || notice?.title;
  const label = (notice?.kind && eventLabels[notice.kind]?.[chinese ? 0 : 1])
    || (chinese && notice?.body === 'Conversation update' ? '会话有更新' : notice?.body);
  const previewText = details?.preview.replace(/\s+/gu, ' ').trim() ?? '';
  const characters = Array.from(previewText);
  const preview = characters.length > 100 ? characters.slice(0, 100).join('') + '…' : previewText;
  const body = preview && preview !== label ? `${label} · ${preview}` : label;

  return <>
    <div className="desktop-inbox-toast-announcement" role="status">
      {notice && `${title}. ${body}`}
    </div>
    {notice && <InboxToastCard notice={notice} title={title ?? notice.title} body={body ?? notice.body}
      viewLabel={chinese ? '查看' : 'View'} onOpen={onOpen} onDismiss={onDismiss} />}
  </>;
}

function InboxToastCard({ notice, title, body, viewLabel, onOpen, onDismiss }: {
  notice: InboxNotification;
  title: string;
  body: string;
  viewLabel: string;
  onOpen: Props['onOpen'];
  onDismiss: Props['onDismiss'];
}) {
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const clock = useRef<{ notice: InboxNotification; remaining: number }>();
  const paused = hovered || focused;
  useEffect(() => {
    if (clock.current?.notice !== notice) clock.current = { notice, remaining: visibleMs };
    if (paused) return;
    const state = clock.current;
    const start = performance.now();
    const timer = window.setTimeout(() => onDismiss(notice), state.remaining);
    return () => {
      window.clearTimeout(timer);
      state.remaining = Math.max(0, state.remaining - (performance.now() - start));
    };
  }, [notice, onDismiss, paused]);

  return <div className="desktop-inbox-toast"
    onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)}
    onFocusCapture={() => setFocused(true)}
    onBlurCapture={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false);
    }}>
    <span className="desktop-inbox-toast-icon" aria-hidden="true"><Icon name="bell" size={16} /></span>
    <div className="desktop-inbox-toast-copy">
      <strong>{title}</strong>
      <p>{body}</p>
    </div>
    <button type="button" className="desktop-inbox-toast-view" onClick={() => onOpen(notice)}>{viewLabel}</button>
    <button type="button" className="desktop-inbox-toast-dismiss" aria-label={tr('desktop_ui_dismiss_notification')} onClick={() => onDismiss(notice)}>
      <span aria-hidden="true"><Icon name="x" size={14} /></span>
    </button>
  </div>;
}
