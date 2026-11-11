import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Stage } from '../../src/renderer/components/Stage';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { RunItem, ToolRunItem } from '../../src/renderer/model/runItem';
import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TranscriptAgents.css';

const params = new URLSearchParams(location.search);
localStorage.setItem('lingxi.desktop.language', params.get('lang') || 'en');
const names = ['Claude 319 oracle', 'Compact provider mapping', 'Autocompact core', 'Provider routes', 'Settings review', 'Additional checks'];
const launch = (index: number): ToolRunItem => ({
  type: 'tool', id: `launch-${index}`, tool: 'Agent', agentId: `worker-${index}`, status: 'done',
  view: { verb: 'task', icon: 'workflow', label: 'Agent', title: names[index]!, primary: `核对 ${names[index]} 的行为与状态` },
});
const read = (id: string): ToolRunItem => ({ type: 'tool', id, tool: 'Read', status: 'done', view: { verb: 'read', icon: 'read', label: 'Read', title: `Read ${id}`, primary: `/project/src/${id}.ts` }, result: { body: 'Configuration and model routing checked.', body_lines: 1 } });
const message = (id: string, text: string): RunItem => ({ type: 'narration', id, text, role: 'assistant' });

function Fixture() {
  const [dark, setDark] = useState(params.get('theme') === 'dark');
  const [mode, setMode] = useState('running');
  const [count, setCount] = useState(3);
  const [openedAgent, setOpenedAgent] = useState('');
  const [session, setSession] = useState('subagent-preview');
  const palette = tokens(dark);
  const workers: SessionAgentSummaryDto[] = Array.from({ length: count }, (_, index) => ({
    agent_id: `worker-${index}`, name: names[index]!, agent_type: 'explore',
    status: mode === 'done' ? 'completed' : mode === 'mixed' && index === 1 ? 'failed' : 'running',
    ...(mode === 'done' ? { report_delivery: 'delivered' } : {}),
    latest_activity: `Inspecting ${names[index]}`,
  }));
  const items: RunItem[] = [
    message('intro', '正在核对 compact 与 autocompact 的模型选择、状态和设置入口。'),
    read('model-routing'), read('compact-config'),
    ...Array.from({ length: count }, (_, index) => launch(index)),
    read('provider-routing'),
    message('progress', '会继续检查压缩前后模型与 provider 路由是否一致，并补齐相关回归。'),
    { ...launch(0), id: 'settings-launch', agentId: 'settings', view: { verb: 'task', icon: 'workflow', label: 'Agent', title: 'Autocompact settings', primary: '核对自动压缩设置' } },
    read('autocompact-settings'),
    message('after', '已核对设置状态，继续汇总结果。'),
    { ...launch(0), id: 'verification-launch', agentId: 'verification', view: { verb: 'task', icon: 'workflow', label: 'Agent', title: 'Compact verification', primary: '核对验证结果' } },
  ];
  return <Theme.Provider value={palette}>
    <main style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: palette.stageBg, color: palette.text }}>
      <nav aria-label="Preview controls" style={{ display: 'flex', flexWrap: 'wrap', gap: 12, padding: 16, borderBottom: `1px solid ${palette.border}` }}>
        <button onClick={() => setDark(!dark)}>切换主题</button>
        <button onClick={() => setMode('done')}>完成协作</button>
        <button onClick={() => setMode('mixed')}>混合状态</button>
        <button onClick={() => setCount(count === 3 ? 6 : 3)}>增减成员</button>
        <button onClick={() => setSession(session === 'subagent-preview' ? 'other-session' : 'subagent-preview')}>切换会话</button>
        <output data-opened-agent={openedAgent} aria-live="polite">{openedAgent}</output>
      </nav>
      <Stage liveItems={items} sessionKey={session} running={false} agents={[
        ...workers,
        { agent_id: 'settings', name: 'Autocompact settings', agent_type: 'explore', status: 'running' },
        { agent_id: 'verification', name: 'Compact verification', agent_type: 'explore', status: 'completed', report_delivery: 'delivered' },
      ]} activeAgentId={openedAgent} onOpenAgent={setOpenedAgent} />
    </main>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
