import { useEffect, useMemo, useRef, useState } from 'react';
import { teamBotTriggerId, type TeamBotState } from './bots.js';
import type { TeamsApi } from './index.js';
import type { TeamConversation, TeamMessage } from './messages.js';

const NO_STATES: Record<string, TeamBotState> = {};
const NO_TRIGGERS: Record<string, string[]> = {};
const FINAL = ['completed', 'failed', 'missed', 'cancelled', 'recovered', 'revoked'];

export interface BotRuns {
  /** Observed run state per trigger id, for the open conversation only. */
  states: Record<string, TeamBotState>;
  /** Trigger ids per message id (a message can ask several Bots). */
  messageTriggers: Record<string, string[]>;
  /** The newest visible message of each trigger: the one that shows its run card. */
  latestMessageByTrigger: Map<string, string>;
}

/**
 * Which visible messages started Bot runs, and where those runs stand: trigger discovery and `botStates`
 * polling. A failed observation never authorizes question or recovery controls, finished runs are remembered,
 * and a running lease schedules its own re-check.
 */
export function useBotRuns({ api, uid, team, selected, conversation, visibleMessages }: {
  api: TeamsApi; uid: string; team: { id: string; generation: number }; selected: string; conversation: TeamConversation | undefined; visibleMessages: readonly TeamMessage[];
}): BotRuns {
  const checkedRuns = useRef(new Map<string, TeamBotState | null>());
  const [runStates, setRunStates] = useState<{ conversationId: string; values: Record<string, TeamBotState> }>({ conversationId: '', values: NO_STATES });
  const states = runStates.conversationId === selected ? runStates.values : NO_STATES;
  const triggerSources = useMemo(() => JSON.stringify(visibleMessages.flatMap<{ messageId: string; triggerId?: string; botKey?: string }>(message => {
    if (message.triggerId) return [{ messageId: message.id, triggerId: message.triggerId }];
    if (message.author.kind !== 'user' || message.recalled) return [];
    const botKeys = conversation?.kind === 'bot_dm'
      ? conversation.participants.filter(value => value.startsWith('bot:'))
      : message.mentions ?? [];
    return botKeys.map(botKey => ({ messageId: message.id, botKey }));
  })), [visibleMessages, conversation]);
  const [triggerMap, setTriggerMap] = useState<{ conversationId: string; values: Record<string, string[]> }>({ conversationId: '', values: NO_TRIGGERS });
  useEffect(() => {
    let closed = false;
    void (async () => {
      const values: Record<string, string[]> = {};
      const sources = JSON.parse(triggerSources) as { messageId: string; triggerId?: string; botKey?: string }[];
      for (const source of sources) {
        const id = source.triggerId ?? await teamBotTriggerId(source.messageId, source.botKey!);
        (values[source.messageId] ??= []).push(id);
      }
      if (!closed) setTriggerMap({ conversationId: selected, values });
    })().catch(() => { if (!closed) setTriggerMap({ conversationId: selected, values: {} }); });
    return () => { closed = true; };
  }, [selected, triggerSources]);
  const messageTriggers = triggerMap.conversationId === selected ? triggerMap.values : NO_TRIGGERS;
  const latestMessageByTrigger = useMemo(() => {
    const latest = new Map<string, string>();
    for (const message of visibleMessages) for (const trigger of messageTriggers[message.id] ?? []) latest.set(trigger, message.id);
    return latest;
  }, [visibleMessages, messageTriggers]);
  useEffect(() => { for (const [key, value] of checkedRuns.current) if (!value) checkedRuns.current.delete(key); }, [triggerSources]);
  const triggers = [...new Set(Object.values(messageTriggers).flat())].sort().join(',');
  useEffect(() => {
    let closed = false, running = false, again = false;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      if (!selected || !triggers) return;
      if (running) { again = true; return; } running = true;
      const ids = triggers.split(','), values: Record<string, TeamBotState> = {};
      const keys = new Map(ids.map(triggerId => [triggerId, JSON.stringify([uid, team.id, team.generation, selected, triggerId])]));
      const missing = ids.filter(triggerId => {
        const key = keys.get(triggerId)!;
        if (!checkedRuns.current.has(key)) return true;
        const cached = checkedRuns.current.get(key);
        if (cached) values[triggerId] = cached;
        return false;
      });
      try {
        for (let offset = 0; offset < missing.length && !closed; offset += 30) {
          const result = await api.messages<{ states: Record<string, TeamBotState | null> }>({ action: 'botStates', teamId: team.id, generation: team.generation, conversationId: selected, triggerIds: missing.slice(offset, offset + 30) });
          if (closed) return;
          for (const [triggerId, state] of Object.entries(result.states)) {
            if (state) values[triggerId] = state;
            if (!state || FINAL.includes(state.state)) {
              checkedRuns.current.set(keys.get(triggerId)!, state);
              if (checkedRuns.current.size > 512) checkedRuns.current.delete(checkedRuns.current.keys().next().value!);
            }
          }
        }
      } catch { /* A failed observation cannot authorize question or recovery controls. */ }
      if (!closed) setRunStates({ conversationId: selected, values });
      clearTimeout(deadline);
      const expirations = Object.values(values).filter(state => ['claimed', 'running'].includes(state.state) && (state.leaseUntil ?? 0) > Date.now()).map(state => state.leaseUntil!);
      if (!closed && expirations.length) deadline = setTimeout(() => void refresh(), Math.max(1, Math.min(...expirations) - Date.now()));
      running = false;
      if (again && !closed) { again = false; void refresh(); }
    };
    void refresh();
    const stop = api.onMessagesChanged(signal => { if (signal && !signal.bots && signal.conversationId !== selected) return; for (const [key, value] of checkedRuns.current) if (!value) checkedRuns.current.delete(key); void refresh(); });
    return () => { closed = true; stop(); clearTimeout(deadline); };
  }, [api, uid, selected, triggers, team.id, team.generation]);
  return { states, messageTriggers, latestMessageByTrigger };
}
