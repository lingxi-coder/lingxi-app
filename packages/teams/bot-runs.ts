import { useCallback, useEffect, useRef, useState } from 'react';
import { executionCommand, type ExecutionScope, type TeamTaskView } from './execution.js';
import type { TeamsApi } from './index.js';

/** One foreground observer per rendered pane; no separate Bot cache or task authority. `enabled` false (a friend chat has no Bots) keeps it idle. */
export function useTeamTasks(api: TeamsApi, scope: ExecutionScope, conversationId?: string, enabled = true) {
  const [value, setValue] = useState<{ key: string; tasks: TeamTaskView[]; error: string }>({ key: '', tasks: [], error: '' });
  const key = `${scope.teamId}:${scope.generation}:${conversationId ?? ''}`;
  const again = useRef<() => void>();
  /** Fetch the tasks now, after a command that changes one. */
  const refresh = useCallback(() => again.current?.(), []);
  useEffect(() => {
    if (!enabled) return;
    let closed = false, running = false;
    const refresh = async () => {
      if (closed || running || (typeof document !== 'undefined' && document.visibilityState === 'hidden')) return;
      running = true;
      try {
        const result = await api.messages<{ tasks: TeamTaskView[] }>(executionCommand(scope, 'listTasks', conversationId ? { conversationId } : {}));
        if (!closed) setValue({ key, tasks: result.tasks.sort((a, b) => b.createdAt - a.createdAt), error: '' });
      } catch (error) {
        const code = error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
        const revoked = ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'CONVERSATION_REQUIRED'].includes(code);
        if (!closed) setValue(old => ({ key, tasks: !revoked && old.key === key ? old.tasks : [], error: code }));
      } finally { running = false; }
    };
    again.current = () => void refresh();
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    const stop = api.onMessagesChanged(() => void refresh());
    const visible = () => void refresh();
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', visible);
    return () => { closed = true; again.current = undefined; clearInterval(timer); stop(); if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', visible); };
  }, [api, key, enabled]);
  return { ...(value.key === key ? value : { tasks: [] as TeamTaskView[], error: '' }), refresh };
}
