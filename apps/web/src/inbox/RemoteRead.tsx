import { useEffect, useRef, useState, type RefObject } from 'react';
import { auth } from '../app/firebase';
import { webInboxRequest } from './session';
import { InboxController } from '../../../../packages/inbox/controller';
import { freezeRead, inboxKey, isUnread, type InboxItem } from '../../../../packages/inbox/contracts';
import {
  durableReadFrontier,
  observeTranscript,
  type VisibleReadFrontier,
} from '../../../../packages/inbox/visibility';
import type { RemoteClient } from '../remote/remoteClient';

function sourceKey(sessionId: string, json?: string) {
  try {
    const value: unknown = json ? JSON.parse(json) : null;
    if (value && typeof value === 'object' && 'sourceId' in value && typeof value.sourceId === 'string')
      return inboxKey({ kind: 'assistant', sourceId: value.sourceId, conversationId: sessionId });
  } catch {
    /* An older source can still be opened from a checked inbox link. */
  }
  const query = new URLSearchParams(location.search).get('inbox');
  return query &&
    /^assistant:[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{1,128}$/u.test(query) &&
    query.split(':')[2] === sessionId
    ? query
    : undefined;
}

/** Match source semantic results to the UUIDs actually in view. Transcript indexes include tool rows and gaps. */
export function RemoteRead({
  sessionId,
  hostId,
  live,
  locale,
  client,
  scroller,
  activityJson,
}: {
  sessionId: string;
  hostId: string;
  live: boolean;
  locale: string;
  client: RemoteClient;
  scroller: RefObject<HTMLElement>;
  activityJson?: string;
}) {
  const [current, setCurrent] = useState<{ controller: InboxController; row: InboxItem }>(),
    [error, setError] = useState(false);
  const key = sourceKey(sessionId, activityJson),
    user = auth.currentUser,
    refresh = useRef<() => void>(),
    latestActivity = useRef(activityJson);
  latestActivity.current = activityJson;
  useEffect(() => {
    setCurrent(undefined);
    setError(false);
    if (!live || !user || !key || !scroller.current) return;
    const query = new URLSearchParams(location.search);
    const preview = query.has('message') || query.has('preview') || !!location.hash;
    const node = scroller.current,
      controller = new InboxController({ uid: user.uid, request: webInboxRequest(user) });
    const uuids = new Map<string, number>();
    let liveView = true,
      working = false,
      again = false,
      stopObserve: (() => void) | undefined,
      recheck: (() => void) | undefined;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let capture: InboxItem | undefined,
      frontier: VisibleReadFrontier | undefined,
      after = 0,
      sourceThrough = 0;
    const load = async () => {
      if (!liveView) return;
      if (working) {
        again = true;
        return;
      }
      working = true;
      try {
        const row = await controller.get(key);
        if (
          !liveView ||
          (row.remoteHostId && row.remoteHostId !== hostId) ||
          row.ref.conversationId !== sessionId ||
          user.uid !== auth.currentUser?.uid
        )
          return;
        if (capture && row.ref.generation !== capture.ref.generation) {
          stopObserve?.();
          setCurrent(undefined);
          setError(true);
          return;
        }
        setCurrent({ controller, row });
        setError(false);
        if (!capture) {
          capture = row;
          sourceThrough = row.readThrough;
          after = row.readThrough;
          frontier = durableReadFrontier(
            user.uid,
            key,
            row.ref.generation,
            row.readThrough,
            row.manualUnreadRevision,
            (through, manualUnreadRevision) => {
              const frozen = { ...freezeRead(row, through), manualUnreadRevision };
              void controller
                .mutate({ action: 'markRead', ...frozen })
                .then(() => {
                  if (liveView) {
                    sourceThrough = Math.max(sourceThrough, through);
                    void load();
                  }
                })
                .catch(() => {
                  if (liveView) setError(true);
                });
            },
            () => {
              if (liveView) setError(true);
            },
          );
          if (!preview)
            stopObserve = observeTranscript(node, frontier, {
              selector: '[data-inbox-uuids]',
              cursors: (el) =>
                (el.dataset.inboxUuids ?? '')
                  .split(' ')
                  .flatMap((id) => (uuids.has(id) ? [uuids.get(id)!] : [])),
              onRecheck: (fn) => {
                recheck = fn;
                return () => {
                  recheck = undefined;
                };
              },
            });
        }
        frontier?.advance(row.readThrough);
        sourceThrough = Math.max(row.readThrough, sourceThrough);
        for (const [uuid, cursor] of uuids) if (cursor <= sourceThrough) uuids.delete(uuid);
        // Only the next four semantic pages are useful until an earlier gap closes.
        let cursor = Math.max(after, sourceThrough);
        for (let pages = 0; pages < 4 && cursor < row.sourceCursor && cursor - sourceThrough < 200; pages++) {
          const page = await client.activity(sessionId, cursor);
          if (!liveView || page.items.length > 50) return;
          frontier?.advance(Math.min(page.baselineCursor, row.sourceCursor));
          let next = cursor;
          for (const entry of page.items) {
            if (
              !Number.isSafeInteger(entry.cursor) ||
              entry.cursor <= next ||
              entry.cursor > row.sourceCursor ||
              !Array.isArray(entry.resultUuids)
            )
              continue;
            for (const id of entry.resultUuids.slice(0, 16))
              if (typeof id === 'string') uuids.set(id, entry.cursor);
            next = entry.cursor;
          }
          if (next === cursor) break;
          cursor = next;
          after = next;
          if (!page.hasMore) break;
        }
        recheck?.();
        const source = latestActivity.current;
        if (source)
          try {
            const cursor = JSON.parse(source).cursor;
            if (Number.isSafeInteger(cursor) && cursor > row.sourceCursor) {
              clearTimeout(retry);
              retry = setTimeout(() => void load(), 1500);
            }
          } catch {
            /* No projection watermark is available on older hosts. */
          }
      } catch (e) {
        if (!liveView) return;
        if (
          e instanceof Error &&
          ['SOURCE_ACCESS_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'INBOX_ITEM_MISSING'].includes(
            e.message,
          )
        ) {
          setCurrent(undefined);
          stopObserve?.();
        } else if (!capture) setError(true);
      } finally {
        working = false;
        if (again) {
          again = false;
          void load();
        }
      }
    };
    refresh.current = () => void load();
    void load();
    return () => {
      stopObserve?.();
      clearTimeout(retry);
      liveView = false;
      refresh.current = undefined;
      controller.close();
    };
  }, [key, user, sessionId, hostId, live, client, scroller]);
  useEffect(() => {
    refresh.current?.();
  }, [activityJson]);
  if (error)
    return <span role="status">{locale === 'zh' ? '无法同步已读状态' : 'Read status unavailable'}</span>;
  if (!current || !isUnread(current.row)) return null;
  return (
    <button
      type="button"
      onClick={() =>
        void current.controller
          .markRead(current.row)
          .then(() => refresh.current?.())
          .catch(() => setError(true))
      }
    >
      {locale === 'zh' ? '标为已读' : 'Mark read'}
    </button>
  );
}
