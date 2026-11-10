import { currentLocale, tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useState, type CSSProperties } from 'react';
import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { AgentAvatar } from './AgentAvatar';
import { Disclosure } from './Disclosure';
import { statusLabel } from '../bridge/agentStatus';
import type { ToolRunItem } from '../model/runItem';

export interface TranscriptAgentsProps {
  id: string;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
  agents?: readonly SessionAgentSummaryDto[];
  launches?: ReadonlyMap<string, ToolRunItem>;
  activeAgentId?: string;
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

function agentStatusText(status: string): string {
  const shown = statusLabel(status);
  switch (shown) {
    case 'done': return tr('common_done');
    case 'failed': return tr('chat_status_failed');
    case 'error': return tr('desktop_ui_agent_error');
    case 'waiting_for_input': return tr('desktop_ui_agent_waiting');
    case 'pending': return tr('chat_plan_state_pending');
    case 'cancelled': return tr('chat_status_cancelled');
    case 'killed': return tr('desktop_ui_agent_killed');
    case 'idle': return tr('remote_status_idle');
    default: return shown ? shown[0].toUpperCase() + shown.slice(1).replace(/_/g, ' ') : tr('voice_permission_unknown_label');
  }
}

const EMPTY_AGENTS: readonly SessionAgentSummaryDto[] = [];

export function TranscriptAgents({ id, open, onSetOpen, agents = EMPTY_AGENTS, launches, onOpenAgent, activeAgentId }: TranscriptAgentsProps) {
  const t = useT();
  useI18n();
  const [snapshot, setSnapshot] = useState(() => ({ source: agents, rows: orderedTranscriptAgents([], agents) }));
  let rows = snapshot.rows;
  if (snapshot.source !== agents) {
    rows = orderedTranscriptAgents(snapshot.rows.map((agent) => agent.agent_id), agents);
    setSnapshot({ source: agents, rows });
  }
  if (rows.length === 0) return null;

  const entries = rows.map((agent) => {
    const name = agent.name.trim() || agent.agent_type || 'Agent';
    const launch = launches?.get(agent.agent_id);
    const task = launch?.view.primary && launch.view.primary !== name ? launch.view.primary : launch?.view.qualifier;
    const title = task ? `${name} (${task})` : name;
    const running = agent.status === 'running' || agent.status === 'working';
    const activity = agent.latest_activity === 'SubagentHandback' ? undefined : agent.latest_activity;
    const failed = agent.status === 'failed' || agent.status === 'error' || agent.report_delivery === 'withheld';
    const delivery = agent.report_delivery;
    const status = running ? tr('remote_status_running')
      : delivery === 'withheld' ? tr('chat_agent_report_withheld')
      : delivery === 'flagged' ? tr('chat_agent_report_flagged')
      : delivery === 'pending' ? tr('chat_agent_report_pending')
      : agent.status === 'completed' && !delivery ? tr('chat_agent_execution_ended')
      : agentStatusText(agent.status);
    const summaryStatus = running ? tr('desktop_ui_agent_started_working')
      : agent.status === 'completed' && delivery === 'delivered' ? tr('desktop_ui_agent_finished')
      : currentLocale() === 'en' ? status.toLowerCase() : status;
    return { agent, name, title, running, activity, failed, status, summaryStatus };
  });
  const names = entries.length === 1 ? entries[0]!.name : tr(
    entries.length === 2 ? 'desktop_ui_agent_names_pair' : 'desktop_ui_agent_names_many',
    entries.slice(0, -1).map(entry => entry.name).join(tr('desktop_ui_agent_names_separator')),
    entries.at(-1)!.name,
  );
  const states = new Map<string, { entry: typeof entries[number]; count: number }>();
  for (const entry of entries) {
    const state = states.get(entry.summaryStatus);
    if (state) state.count++;
    else states.set(entry.summaryStatus, { entry, count: 1 });
  }
  const summaryStates = [...states.values()].sort((a, b) => Number(b.entry.failed) - Number(a.entry.failed)).map(({ entry, count }) => ({
    failed: entry.failed,
    text: states.size === 1 ? entry.summaryStatus
      : entry.failed ? tr(entry.agent.report_delivery === 'withheld' ? 'desktop_ui_agent_attention_count' : 'desktop_ui_failed_count', count)
      : entry.running ? tr('desktop_ui_running_count', count)
      : `${count} ${entry.summaryStatus}`,
  }));
  const description = entries.map(entry => `${entry.title} · ${entry.status}${entry.activity ? ` · ${entry.activity}` : ''}`).join('; ');
  const agentRow = (entry: typeof entries[number]) => {
    const { agent, name, title, running, activity, failed, status } = entry;
    return <button
      key={agent.agent_id}
      type="button"
      className="transcript-agent-row transcript-disclosure-trigger"
      aria-expanded={onOpenAgent ? activeAgentId === agent.agent_id : undefined}
      data-agent-id={agent.agent_id}
      data-agent-running={running ? 'true' : undefined}
      disabled={!onOpenAgent}
      title={`${title} · ${status}${activity ? ` · ${activity}` : ''}`}
      aria-label={`${title} · ${status}${onOpenAgent ? ` · ${tr('desktop_ui_agent_open_details')}` : ''}`}
      onClick={() => onOpenAgent?.(agent.agent_id)}
    >
      <span className="transcript-agent-icon"><AgentAvatar agentId={agent.agent_id} size={18} /></span>
      <span className="transcript-agent-copy">
        <span className="transcript-agent-name">{name}</span>
        <span className="transcript-agent-status" data-danger={failed || undefined}>{entry.summaryStatus}</span>
      </span>
      {onOpenAgent && <span className="transcript-disclosure-chevron" aria-hidden="true"><Icon name="chevronR" size={12} /></span>}
    </button>;
  };

  return <div className="transcript-agents" role="group" aria-label={tr("desktop_ui_session_agents")} style={{
    color: t.text3,
    '--agent-ink-focus': t.text,
    '--agent-focus': t.accent,
    '--agent-danger': t.danger,
    '--agent-border': t.border,
  } as CSSProperties}>
    {entries.length === 1 ? agentRow(entries[0]!) : <>
      <Disclosure id={id} open={open} onToggle={() => onSetOpen(id, !open)}
        buttonClassName="transcript-agent-row transcript-agent-group-trigger"
        buttonStyle={{ color: t.text3, fontSize: 13, gap: 9 }}
        label={`${description} · ${tr('desktop_ui_agent_group_details')}`}
        summary={<>
          <span className="transcript-agent-icons" aria-hidden="true">
            {entries.slice(0, 3).map(entry => <span className="transcript-agent-icon" key={entry.agent.agent_id} data-agent-active={entry.running || undefined}>
              <AgentAvatar agentId={entry.agent.agent_id} size={18} />
            </span>)}
            {entries.length > 3 && <span className="transcript-agent-extra">+{entries.length - 3}</span>}
          </span>
          <span className="transcript-agent-copy" title={description}>
            <span className="transcript-agent-name">{names}</span>
            <span className="transcript-agent-status">{summaryStates.map((state, index) => <span key={state.text} data-danger={state.failed || undefined}>{index > 0 ? ' · ' : ''}{state.text}</span>)}</span>
          </span>
        </>}>
        <div className="transcript-agent-members" role="list">
          {entries.map(entry => <div role="listitem" key={entry.agent.agent_id}>{agentRow(entry)}</div>)}
        </div>
      </Disclosure>
    </>}
  </div>;
}
