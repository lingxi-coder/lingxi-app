import { useEffect, useState } from 'react';
import type { InboxController } from './controller.js';
import type { InboxCursor, InboxItem, InboxNotice,InboxNoticeTarget } from './contracts.js';
export function NoticeHistory({
  controller,
  cn,
  onOpen,
}: {
  controller: InboxController;
  cn: boolean;
  onOpen: (item: InboxItem,target?:InboxNoticeTarget) => void | Promise<void>;
}) {
  const [rows, setRows] = useState<InboxNotice[]>([]),
    [next, setNext] = useState<InboxCursor | null>(),
    [error, setError] = useState(false),
    [busy, setBusy] = useState(true);
  useEffect(() => {
    let live = true;
    void controller
      .notices()
      .then((page) => {
        if (live) {
          setRows(page.items);
          setNext(page.next);
        }
      })
      .catch(() => {
        if (live) setError(true);
      })
      .finally(() => {
        if (live) setBusy(false);
      });
    return () => {
      live = false;
    };
  }, [controller]);
  async function more() {
    if (!next || busy) return;
    setBusy(true);
    try {
      const page = await controller.notices(next);
      setRows((old) => [...old, ...page.items]);
      setNext(page.next);
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  }
  const names: Record<string, [string, string]> = {
    mention: ['Mention', '提及'],
    reply: ['Reply', '回复'],
    dm: ['Direct message', '私聊'],
    message: ['Message', '消息'],
    input: ['Input requested', '等待输入'],
    approval: ['Approval requested', '等待批准'],
    failure: ['Failure', '失败'],
    completion: ['Completed', '已完成'],
  };
  return (
    <div>
      {error ? <p role="alert">{cn ? '暂时无法打开通知。' : 'Notification unavailable.'}</p> : null}
      <ol>
        {rows
          .filter((n) => n.lastActivityAt > Date.now() - 30 * 86400000)
          .map((n) => (
            <li key={n.id}>
              <button
                type="button"
                onClick={() =>
                  void controller
                    .get(n.key, n.generation)
                    .then(item=>onOpen(item,{key:n.key,generation:n.generation,through:n.through,messageId:n.messageId,taskId:n.taskId}))
                    .catch(() => setError(true))
                }
              >
                {names[n.kind]?.[cn ? 1 : 0] ?? (cn ? '会话' : 'Conversation')} ·{' '}
                {new Date(n.lastActivityAt).toLocaleString(cn ? 'zh' : 'en')}
              </button>
            </li>
          ))}
      </ol>
      {!busy && !rows.length ? <p>{cn ? '没有最近通知' : 'No recent notifications'}</p> : null}
      {next ? (
        <button type="button" disabled={busy} onClick={() => void more()}>
          {cn ? '加载更多' : 'Load more'}
        </button>
      ) : null}
    </div>
  );
}
