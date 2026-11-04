import { NeedsInputBadge } from '../../src/renderer/components/NeedsInputBadge';
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { Stage } from '../../src/renderer/components/Stage';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import type { RunItem, ToolRunItem } from '../../src/renderer/model/runItem';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TranscriptAgents.css';

function call(id: string, tool: string, verb: ToolRunItem['view']['verb'], primary: string, status: ToolRunItem['status'] = 'done'): ToolRunItem {
  return { type: 'tool', id, tool, status, view: { verb, label: tool, primary, title: `${tool}(${primary})` },
    result: status === 'running' ? undefined : { body: status === 'error' ? 'File was unavailable on this attempt.' : `Result for ${primary}`, body_lines: 1 } };
}
setLanguagePreference('en');
function Fixture() {
  const [activityView, setActivityView] = useState(true);
  const [needsInput, setNeedsInput] = useState(true);
  const [dark, setDark] = useState(false);
  const [running, setRunning] = useState(true);
  const [allFailed, setAllFailed] = useState(false);
  const [activeAgent, setActiveAgent] = useState('');
  const items: RunItem[] = [
    { type: 'narration', id: 'n1', role: 'assistant', text: '我会先核对项目配置，再检查消息的显示效果。' },
    { ...call('spawn', 'Agent', 'task', '核对消息布局'), agentId: 'agent:guide' },
    call('read-1', 'Read', 'read', 'src/config.ts', 'error'),
    call('read-2', 'Read', 'read', 'src/config.ts', allFailed ? 'error' : 'done'),
    call('read-3', 'Read', 'read', 'src/theme.ts', allFailed ? 'error' : 'done'),
    call('shell-1', 'Bash', 'shell', 'npm run typecheck'),
    call('shell-2', 'Bash', 'shell', 'npm test', running ? 'running' : 'done'),
    call('read-4', 'Read', 'read', 'reports/results.txt'),
    { type: 'narration', id: 'n2', role: 'assistant', text: '已完成配置核对，接下来检查最终的显示效果。' },
  ];
  return <Theme.Provider value={tokens(dark)}><div style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: tokens(dark).stageBg }}>
    <nav style={{ display: 'flex', flexWrap: 'wrap', gap: 12, padding: 12 }}>
      <button id="toggle" aria-pressed={activityView} onClick={() => setActivityView(!activityView)}>Activity view</button>
      <button id="theme" onClick={() => setDark(!dark)}>Theme</button>
      <button id="finish" onClick={() => setRunning(false)}>Finish tools</button>
      <button id="all-failed" onClick={() => setAllFailed(!allFailed)}>All attempts unsuccessful</button>
      <button id="locale" onClick={() => setLanguagePreference('zh-Hans')}>中文</button>
      <button id="input" onClick={() => setNeedsInput(!needsInput)}>Session approval {needsInput && <NeedsInputBadge />}</button>
    </nav>
    <output data-opened-agent={activeAgent} hidden />
    <Stage activityView={activityView} liveItems={items} sessionKey="activity-fixture"
      agents={[{ agent_id: 'agent:guide', name: 'lingxi-code-guide', agent_type: 'explorer', status: 'completed', report_delivery: 'delivered', latest_activity: 'SubagentHandback' }]}
      activeAgentId={activeAgent} onOpenAgent={setActiveAgent} />
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
