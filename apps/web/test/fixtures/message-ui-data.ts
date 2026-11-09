import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import { emptyTranscript, type Transcript } from '../../src/remote/sessionModel';
import type { RunItem, ToolRunItem } from '../../src/remote/reuse';

export function messageUIData(mode = 'running', count = 3): Transcript {
  const names = ['Claude 319 oracle', 'Compact provider mapping', 'Autocompact core', 'Provider routes', 'Settings review', 'Additional checks'];
  const agents: SessionAgentSummaryDto[] = Array.from({ length: count }, (_, i) => ({
    agent_id: `worker-${i}`, name: names[i]!, agent_type: 'explore',
    status: mode === 'done' ? 'completed' : mode === 'mixed' && i === 1 ? 'completed' : 'running',
    ...(mode === 'done' ? { report_delivery: 'delivered' as const } : mode === 'mixed' && i === 1 ? { report_delivery: 'withheld' as const } : {}),
    latest_activity: `Inspecting ${names[i]}`,
  }));
  const tool = (id: string, name: string, verb: ToolRunItem['view']['verb'], primary: string, status: ToolRunItem['status'] = 'done'): ToolRunItem => ({
    type: 'tool', id, tool: name, status, view: { verb, label: name, title: name, primary },
    result: status === 'running' ? undefined : { headline: status === 'error' ? 'Call unsuccessful' : 'Result', body: `Details for ${primary}`, body_lines: 1 },
  });
  const items: RunItem[] = [
    { type: 'narration', id: 'intro', role: 'assistant', text: 'Checking compact and autocompact behavior.' },
    ...agents.map((agent, i) => ({ ...tool(`spawn-${i}`, 'Agent', 'task', `Check ${agent.name}`), agentId: agent.agent_id })),
    tool('read-a', 'Read', 'read', '/project/model.ts'), tool('read-b', 'Read', 'read', '/project/compact.ts'),
    { type: 'narration', id: 'progress', role: 'assistant', text: 'The provider route is preserved through compact.' },
    tool('task-list', 'TaskList', 'generic', 'Current tasks'), tool('search', 'ToolSearch', 'generic', '+SendMessage'),
    tool('send', 'SendMessage', 'generic', 'Notify reviewer', 'error'), tool('task-update', 'TaskUpdate', 'generic', 'Check progress', 'running'),
    tool('local-app', 'LocalApp', 'generic', '全家人的终身学习空间：7 个年龄阶段、14 个领域'),
    tool('shell', 'Bash', 'shell', 'gh api repos/example/project/git/trees/main'),
  ];
  const base = emptyTranscript();
  return { ...base, agents, conversation: { ...base.conversation, items } };
}
