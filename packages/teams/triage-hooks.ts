import { useCallback, useEffect, useRef, useState } from 'react';
import type { ExecutionScope, TriageLogItem } from './execution.js';
import type { TeamsApi } from './index.js';
import { findTriage, TRIAGE_LOADING, TriageLogReader, triageLogRead, type TriageLogView } from './triage-log.js';

const none: ReadonlyMap<string, TriageLogItem> = new Map();

/**
 * The auto-reply log of one Bot as a dialog shows it: the first page now, the next ones when asked for. A change of Bot, Chat or Team starts a list
 * over, and nothing read for the old one is shown for the new one.
 */
export function useTriageLog(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, request: { sharedBotId: string; conversationId?: string }) {
  // The list says whose it is, so that for the render in which the Bot or the Chat changed it is still the old list and is not taken for the new one.
  const [state, setState] = useState<{ key: string; view: TriageLogView }>({ key: '', view: TRIAGE_LOADING });
  const reader = useRef<TriageLogReader>();
  const key = `${scope.teamId}:${scope.generation}:${request.sharedBotId}:${request.conversationId ?? ''}`;
  useEffect(() => {
    const next = new TriageLogReader(triageLogRead(api, scope, request), view => setState({ key, view }));
    reader.current = next;
    void next.load();
    return () => { next.close(); if (reader.current === next) reader.current = undefined; };
  // `key` is what `scope` and `request` say.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, key]);
  return { view: state.key === key ? state.view : TRIAGE_LOADING, refresh: useCallback(() => void reader.current?.load(), []), more: useCallback(() => void reader.current?.more(), []) };
}

/**
 * Why a Bot stayed silent, for the person who maintains it: the verdicts of the given tasks of one Chat, read from its log the first time a task
 * is asked for (never for anyone else, never for a task whose reason is known) and kept while the Chat is open. A task with no row in the
 * log, or a log that cannot be read, simply has no reason: the line that says the Bot did not reply is all there is.
 * `request` is absent where this person does not maintain the Bot, and then nothing is read. Nothing is read while `online` is false either: the
 * tasks are looked for when the connection returns, not given up on.
 */
export function useTriageReasons(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, request: { sharedBotId: string; conversationId: string } | undefined, taskIds: readonly string[], online = true): ReadonlyMap<string, TriageLogItem> {
  const key = request ? `${scope.teamId}:${scope.generation}:${request.sharedBotId}:${request.conversationId}` : '';
  const [state, setState] = useState<{ key: string; found: ReadonlyMap<string, TriageLogItem> }>({ key: '', found: none });
  /** The tasks already looked for in this Chat, found or not: a task is looked for once. */
  const looked = useRef<{ key: string; ids: Set<string> }>({ key: '', ids: new Set() });
  const current = useRef(key);
  current.current = key;
  const wanted = taskIds.join(',');
  useEffect(() => {
    if (!request || !online || !taskIds.length) return;
    if (looked.current.key !== key) looked.current = { key, ids: new Set() };
    const fresh = taskIds.filter(id => !looked.current.ids.has(id));
    if (!fresh.length) return;
    for (const id of fresh) looked.current.ids.add(id);
    void findTriage(triageLogRead(api, scope, request), fresh).then(found => {
      if (current.current !== key || !found.size) return;
      setState(old => ({ key, found: new Map([...(old.key === key ? old.found : none), ...found]) }));
    });
  // `key` is what `scope` and `request` say, `wanted` is `taskIds`.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, key, wanted, online]);
  return state.key === key ? state.found : none;
}
