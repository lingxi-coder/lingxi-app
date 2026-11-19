import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { InboxController, type InboxSnapshot } from '../../../../../packages/inbox/controller';
import type { InboxItem } from '../../../../../packages/inbox/contracts';
import { useAccountState } from './useAccountState';
import { registerDesktopInboxReads } from './useTeams';
import { AssistantInboxReader, assistantInboxActivity, type AssistantReadingEntry, type AssistantReadingPosition } from './inboxAssistantReading';

const emptySnapshot: InboxSnapshot = { items: [], filter: 'all', archived: false, badge: 0, hasMore: false, busy: false, offline: false, error: '', pending: 0 };
const noSubscribe = () => () => {};
const noSnapshot = () => emptySnapshot;

export function useDesktopInbox() {
  const { state } = useAccountState();
  const uid = state?.user?.uid ?? null;
  const api = window.lingxi?.inbox;
  const [opened, setOpened] = useState<{ uid: string | null; controller: InboxController }>();
  useEffect(() => {
    if (!api) return;
    const controller = new InboxController({ uid: uid ?? 'local', scope: 'messages', request: command => api.request(uid, command),
      watch: (dirty, badge, error) => api.watch(uid, dirty, badge, error) });
    setOpened({ uid, controller });
    const releaseReads = registerDesktopInboxReads(uid, controller);
    void controller.start();
    return () => { releaseReads(); controller.close(); setOpened(current => current?.controller === controller ? undefined : current); };
  }, [api, uid]);
  // Account changes may render before effect cleanup. Never expose the previous account for that frame.
  const controller = opened?.uid === uid ? opened.controller : undefined;
  const snapshot = useSyncExternalStore(controller?.subscribe ?? noSubscribe, controller?.getSnapshot ?? noSnapshot, noSnapshot);
  return { uid, api, controller, snapshot };
}

export function useAssistantInboxReading(inbox: ReturnType<typeof useDesktopInbox>,
  sessionId: string | undefined, activityJson: string | null | undefined, visible: boolean, preview: boolean) {
  const source = useMemo(() => assistantInboxActivity(activityJson), [activityJson]);
  const candidate = inbox.snapshot.items.find(item => item.ref.kind === 'assistant' && item.ref.conversationId === sessionId
    && (!source || item.ref.sourceId === source.sourceId));
  const key = candidate?.key ?? (source && sessionId ? `assistant:${source.sourceId}:${sessionId}` : undefined);
  const [fetched, setFetched] = useState<{ uid: string | null; item: InboxItem }>();
  const item = candidate ?? (fetched?.uid === inbox.uid && fetched?.item.key === key ? fetched.item : undefined);
  useEffect(() => {
    if (!key || !inbox.api || candidate) return;
    let current = true;
    void inbox.api.request<{ item: InboxItem }>(inbox.uid, { action: 'get', key }).then(value => {
      if (current) setFetched({ uid: inbox.uid, item: value.item });
    }).catch(() => { if (current) setFetched(undefined); });
    return () => { current = false; };
  }, [key, candidate, inbox.api, inbox.uid, inbox.snapshot.items]);
  const [page, setPage] = useState<{ key: string; cursor: number; baselineCursor: number; entries: AssistantReadingEntry[] }>();
  const [target, setTarget] = useState<{ key: string; uuid: string }>();
  const [foreground, setForeground] = useState(() => document.visibilityState === 'visible' && document.hasFocus());
  const [error, setError] = useState('');
  const latest = useRef({ item, page, foreground, visible, preview });
  latest.current = { item, page, foreground, visible, preview };
  const position = useRef<AssistantReadingPosition>();
  const reader = useMemo(() => inbox.controller ? new AssistantInboxReader(async command => {
    await inbox.controller!.mutate(command);
    setError('');
  },
    () => setError('READ_STATE_NOT_SAVED')) : undefined, [inbox.controller]);
  const observe = useCallback(() => {
    const current = latest.current;
    if (!current.foreground || !current.visible) { reader?.leave(); return; }
    if (!reader || !current.item || !current.page || current.page.key !== current.item.key || !position.current) return;
    const latestEntry = current.page.entries.find(entry => entry.cursor === current.page!.cursor);
    reader.observe({ item: current.item, activity: { sourceId: current.item.ref.sourceId, cursor: current.page.cursor,
      baselineCursor: current.page.baselineCursor, latestResultUuids: [...(latestEntry?.resultUuids ?? [])] },
      entries: current.page.entries, position: { ...position.current, preview: current.preview || position.current.preview },
      foreground: current.foreground && current.visible });
  }, [reader]);
  useEffect(() => {
    const changed = () => {
      const foreground = document.visibilityState === 'visible' && document.hasFocus();
      latest.current = { ...latest.current, foreground };
      observe();
      setForeground(foreground);
    };
    window.addEventListener('focus', changed); window.addEventListener('blur', changed);
    document.addEventListener('visibilitychange', changed);
    return () => { window.removeEventListener('focus', changed); window.removeEventListener('blur', changed); document.removeEventListener('visibilitychange', changed); };
  }, [observe]);
  useEffect(() => { observe(); }, [observe, item, page, foreground, visible, preview]);
  useEffect(() => () => reader?.leave(), [reader, sessionId, key]);
  useEffect(() => { setTarget(undefined); setPage(undefined); position.current = undefined; setError(''); }, [key, inbox.uid]);
  useEffect(() => {
    if (!item || !inbox.api) return;
    let current = true;
    void inbox.api.activityPage(item.key, item.readThrough).then(value => {
      if (!current) return;
      setPage(previous => {
        const rows = new Map((previous?.key === item.key ? previous.entries : []).map(entry => [entry.cursor, entry]));
        for (const entry of value.items) rows.set(entry.cursor, entry);
        return { key: item.key, cursor: value.cursor, baselineCursor: value.baselineCursor,
          entries: [...rows.values()].sort((a, b) => a.cursor - b.cursor).slice(-200) };
      });
      if (!preview && value.items[0]?.resultUuids[0]) setTarget(previous => previous?.key === item.key ? previous : { key: item.key, uuid: value.items[0]!.resultUuids[0]! });
    }).catch(() => { if (current) setError('READ_POSITION_UNAVAILABLE'); });
    return () => { current = false; };
  }, [inbox.api, item?.key, item?.ref.generation, item?.sourceCursor, item?.readThrough, preview]);
  const onReadingPosition = useCallback((value: AssistantReadingPosition) => {
    position.current = value;
    observe();
    const current = latest.current;
    if (value.explicitLatest && current.item && inbox.api && current.page?.cursor
      && !current.page.entries.some(entry => entry.cursor === current.item!.sourceCursor)) {
      const expectedKey = current.item.key;
      void inbox.api.activityPage(expectedKey, Math.max(0, current.item.sourceCursor - 1)).then(result => {
        if (latest.current.item?.key !== expectedKey) return;
        setPage(previous => previous?.key === expectedKey ? { ...previous, cursor: result.cursor,
          entries: [...previous.entries.filter(entry => !result.items.some(next => next.cursor === entry.cursor)), ...result.items].slice(-200) } : previous);
      }).catch(() => setError('READ_POSITION_UNAVAILABLE'));
    }
  }, [observe, inbox.api]);
  return { onReadingPosition, firstUnreadUuid: key && target?.key === key && !preview ? target.uuid : undefined, item, error };
}
