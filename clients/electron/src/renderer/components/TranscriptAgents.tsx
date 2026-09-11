import { useState, type CSSProperties } from 'react';
import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export interface TranscriptAgentsProps {
  agents?: readonly SessionAgentSummaryDto[];
  onOpenAgent?: (agentId: string) => void;
}

/** Keep background agents in first-seen order as listing refreshes arrive. */
export function orderedTranscriptAgents(previousIds: readonly string[], agents: readonly SessionAgentSummaryDto[]): SessionAgentSummaryDto[] {
  const byId = new Map(agents.map((agent) => [agent.agent_id, agent]));
  // SessionAgentList reserves this ID for the main conversation, not a background worker.
  byId.delete('main');
  const ids = new Set([...previousIds, ...byId.keys()]);
  return [...ids].flatMap((id) => byId.has(id) ? [byId.get(id)!] : []);
}

const EMPTY_AGENTS: readonly SessionAgentSummaryDto[] = [];

export function TranscriptAgents({ agents = EMPTY_AGENTS, onOpenAgent }: TranscriptAgentsProps) {
  const t = useT();
  const [snapshot, setSnapshot] = useState(() => ({ source: agents, rows: orderedTranscriptAgents([], agents) }));
  let rows = snapshot.rows;
  if (snapshot.source !== agents) {
    rows = orderedTranscriptAgents(snapshot.rows.map((agent) => agent.agent_id), agents);
    setSnapshot({ source: agents, rows });
  }
  if (rows.length === 0) return null;

  return <div className="transcript-agents" role="group" aria-label="Session agents" style={{
    color: t.text2,
    '--agent-hover': t.surfaceHover,
    '--agent-focus': t.accent,
  } as CSSProperties}>
    {rows.map((agent) => {
      const name = agent.name.trim() || agent.agent_type || 'Agent';
      const running = agent.status === 'running' || agent.status === 'working';
      const failed = agent.status === 'failed' || agent.status === 'error';
      const complete = agent.status === 'completed' || agent.status === 'done';
      const cancelled = ['cancelled', 'canceled', 'killed', 'stopped'].includes(agent.status);
      const icon = failed ? 'shieldAlert' : complete ? 'check' : cancelled ? 'stop' : 'subagent';
      const status = running ? 'Running' : agent.status ? agent.status[0].toUpperCase() + agent.status.slice(1).replace(/_/g, ' ') : 'Unknown';
      return <button
        key={agent.agent_id}
        type="button"
        className="transcript-agent-row"
        data-agent-id={agent.agent_id}
        data-agent-running={running ? 'true' : undefined}
        disabled={!onOpenAgent}
        aria-label={`${name} · ${status}${onOpenAgent ? ' · Open agent details' : ''}`}
        onClick={() => onOpenAgent?.(agent.agent_id)}
      >
        <span className="transcript-agent-icon" style={{ color: failed ? t.danger : complete ? t.ok : t.text3 }}><Icon name={icon} size={16} /></span>
        <span className="transcript-agent-name" title={name}>{name}</span>
        <span className="transcript-agent-status" style={{ color: failed ? t.danger : t.text3 }}>{status}</span>
        {agent.latest_activity && <span className="transcript-agent-activity" title={agent.latest_activity} style={{ color: t.text3 }}>{agent.latest_activity}</span>}
        {onOpenAgent && <Icon name="chevronR" size={12} color={t.text3} />}
      </button>;
    })}
  </div>;
}
