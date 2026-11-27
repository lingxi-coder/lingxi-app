import { useCallback, useEffect, useRef, useState } from 'react';
import { executionCommand, type ExecutionScope, type TeamTaskView } from './execution.js';
import type { TeamsApi } from './index.js';

const emptyTasks: TeamTaskView[] = [];

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
    let previousTasks = '', tasks = emptyTasks;
    let previousError: string | undefined;
    const refresh = async () => {
      if (closed || running || (typeof document !== 'undefined' && document.visibilityState === 'hidden')) return;
      running = true;
      try {
        const result = await api.messages<{ tasks: TeamTaskView[] }>(executionCommand(scope, 'listTasks', conversationId ? { conversationId } : {}));
        if (closed) return;
        // Compare the whole API payload so changes to waits, usage and artifacts also reach the pane.
        const signature = JSON.stringify(result.tasks);
        if (signature === previousTasks && previousError === '') return;
        if (signature !== previousTasks) {
          previousTasks = signature;
          tasks = result.tasks.length ? result.tasks.slice().sort((a, b) => b.createdAt - a.createdAt) : emptyTasks;
        }
        previousError = '';
        setValue({ key, tasks, error: '' });
      } catch (error) {
        const code = error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
        const revoked = ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'CONVERSATION_REQUIRED'].includes(code);
        if (!closed && code !== previousError) {
          previousError = code;
          setValue(old => {
            const retained = !revoked && old.key === key ? old.tasks : emptyTasks;
            return old.key === key && old.tasks === retained && old.error === code ? old : { key, tasks: retained, error: code };
          });
        }
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
  return { ...(enabled && value.key === key ? value : { tasks: emptyTasks, error: '' }), refresh };
}
