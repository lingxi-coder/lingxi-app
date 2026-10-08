import type { SlashCommandDto } from '@lingxi/bridge-client';
import { desktopCommandIsShadowed, parseSlashLine } from './slashDispatch';
import { openRuntimeCenterItem, reduceRuntimeCenterEvent, type RuntimeCenterState } from './runtimeCenterState';

export const SIDE_QUESTION_AGENT_PREFIX = 'btw:';

export function isSideQuestionCommand(raw: string, catalog: readonly SlashCommandDto[]): boolean {
  return parseSlashLine(raw)?.name === 'btw' && !desktopCommandIsShadowed(raw, catalog);
}

/** Adapt the existing isolated slash query to the ordinary subagent inspector. */
export function beginSideQuestion(state: RuntimeCenterState, sessionId: string, turnId: number, raw: string): RuntimeCenterState {
  const agentId = `${SIDE_QUESTION_AGENT_PREFIX}${turnId}`;
  const question = parseSlashLine(raw)?.args || 'Usage: /btw <your question>';
  let next = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_updated', session_id: sessionId,
    agent: { agent_id: agentId, name: '/btw', agent_type: 'side_question', status: 'running', latest_activity: question },
  }, sessionId);
  next = reduceRuntimeCenterEvent(next, {
    type: 'session_agent_message', session_id: sessionId, agent_id: agentId,
    message_index: 0, message_uuid: crypto.randomUUID(),
    message: { role: 'user', blocks: [{ type: 'text', text: question }] },
  }, sessionId);
  return openRuntimeCenterItem(next, { kind: 'agent', id: agentId });
}

export function finishSideQuestion(state: RuntimeCenterState, sessionId: string, turnId: number, answer: string, isError = false): RuntimeCenterState {
  const agentId = `${SIDE_QUESTION_AGENT_PREFIX}${turnId}`;
  const agent = state.agents[agentId];
  if (!agent || agent.status !== 'running') return state;
  let next = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_updated', session_id: sessionId,
    agent: { ...agent, status: isError ? 'failed' : 'completed', latest_activity: undefined },
  }, sessionId);
  next = reduceRuntimeCenterEvent(next, {
    type: 'session_agent_message', session_id: sessionId, agent_id: agentId,
    message_index: 1, message_uuid: crypto.randomUUID(),
    message: { role: 'assistant', blocks: [{ type: 'text', text: answer }] },
  }, sessionId);
  return next;
}
