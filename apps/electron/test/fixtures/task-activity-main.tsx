import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Stage } from '../../src/renderer/components/Stage';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { RunItem, ToolRunItem } from '../../src/renderer/model/runItem';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TranscriptAgents.css';

const params = new URLSearchParams(location.search);
localStorage.setItem('lingxi.desktop.language', params.get('lang') || 'en');
const output = (text: string) => ({ headline: text, body: text, body_lines: text.split('\n').length, body_truncated: false });
const task = (id: string, tool: string, primary?: string): ToolRunItem => ({
  type: 'tool', id, tool, status: 'done',
  view: { verb: 'generic', icon: 'wrench', label: tool, title: tool, primary },
  result: output('Task progress updated.'),
});

function Fixture() {
  const [dark, setDark] = useState(params.get('theme') === 'dark');
  const [running, setRunning] = useState(false);
  const [openedAgent, setOpenedAgent] = useState('');
  const palette = tokens(dark);
  const items: RunItem[] = [
    { type: 'narration', id: 'intro', role: 'assistant', text: '我会补齐离线教育和儿童互动学习方向的核查，再整理各项目值得参考的功能、许可证与适用边界。' },
    { ...task('tasks-list', 'TaskList'), result: output('#1 [completed] 构建家庭终身教育应用\n#2 [in_progress] 核查互动课程与离线资源') },
    task('tasks-create', 'TaskCreate', '核查儿童与互动课程项目'),
    task('tasks-search', 'ToolSearch', '+SendMessage'),
    { ...task('tasks-message', 'SendMessage'), status: 'error', result: output('The recipient is not available. Try another active agent.') },
    { ...task('tasks-update', 'TaskUpdate', '#2 核查互动课程与离线资源'), status: running ? 'running' : 'done' },
    { ...task('graphql', 'Bash'), view: { verb: 'shell', icon: 'terminal', label: 'Shell', title: 'Inspect repository', sub_line: { prefix: '$ ', text: 'gh api graphql -f query=\'query { school: repository(owner:"mblayman",name:"homeschool") { nameWithOwner description licenseInfo { spdxId } } }\'' } }, result: output('Repository metadata and license details.') },
    { ...task('offline-launch', 'Agent', '核查离线课程与百科项目'), agentId: 'offline-learning-evidence', view: { verb: 'task', icon: 'workflow', label: 'offline-learning-evidence', title: 'Offline learning', primary: '核查离线课程与百科项目' } },
    { ...task('interactive-launch', 'Agent', '核查儿童与互动课程项目'), agentId: 'interactive-learning-evidence', view: { verb: 'task', icon: 'workflow', label: 'interactive-learning-evidence', title: 'Interactive learning', primary: '核查儿童与互动课程项目' } },
    { ...task('tree', 'Bash'), view: { verb: 'shell', icon: 'terminal', label: 'Shell', title: 'Read tree', sub_line: { prefix: '$ ', text: 'gh api repos/mblayman/homeschool/git/trees/c479cb89e793a97d318bb98ce9741daeef6c3c03d?recursive=1' } }, result: output('Repository tree: courses, lessons, schedules.') },
    { ...task('fetch-one', 'WebFetch', 'https://phet.colorado.edu/en/licensing/'), view: { verb: 'fetch', icon: 'globe', label: 'Fetch', title: 'Fetch licensing', primary: 'https://phet.colorado.edu/en/licensing/' }, result: output('License information loaded.') },
    { ...task('fetch-two', 'WebFetch'), view: { verb: 'fetch', icon: 'globe', label: 'Fetch', title: 'Fetch documentation', primary: 'https://phet.colorado.edu/en/about' }, result: output('Project documentation loaded.') },
    { ...task('fetch-error', 'SendMessage', 'interactive-learning-evidence'), status: 'error', result: output('The recipient is not available. Try another active agent.') },
    { type: 'narration', id: 'after', role: 'assistant', text: '已整理可用资料，接下来汇总课程内容与授权范围。' },
  ];
  return <Theme.Provider value={palette}>
    <main style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: palette.stageBg, color: palette.text }}>
      <nav aria-label="Preview controls" style={{ display: 'flex', gap: 12, padding: 16, borderBottom: `1px solid ${palette.border}` }}>
        <button onClick={() => setDark(!dark)}>切换主题</button>
        <button onClick={() => setRunning(!running)}>{running ? '完成任务' : '运行任务'}</button>
        <output aria-live="polite">{openedAgent && `已打开 ${openedAgent}`}</output>
      </nav>
      <Stage sessionKey="task-activity" running={false} liveItems={items} activeAgentId={openedAgent} onOpenAgent={setOpenedAgent} agents={[
        { agent_id: 'offline-learning-evidence', name: 'offline-learning-evidence', agent_type: 'explore', status: 'failed', latest_activity: 'Bash' },
        { agent_id: 'interactive-learning-evidence', name: 'interactive-learning-evidence', agent_type: 'explore', status: 'failed', latest_activity: 'WebFetch' },
      ]} />
    </main>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
