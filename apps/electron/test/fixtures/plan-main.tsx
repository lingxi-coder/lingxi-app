import '../../src/renderer/global.css';
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { Stage } from '../../src/renderer/components/Stage';
import { PlanPreview, PlanDocument } from '../../src/renderer/components/PlanDocument';
import { narrationPlans } from '../../src/renderer/bridge/planDocuments';
import { emptySubmittedPlanState, reduceSubmittedPlanMessages } from '../../src/renderer/bridge/submittedPlan';
import type { RunItem } from '../../src/renderer/model/runItem';
const markdown = '# 环境切换 / org info 失效 — 修改方案\n\n## 必须成立的不变量\n\nHttpClientConfig 的所有端点必须属于同一环境。组织信息必须与环境一致。\n\n## 实现步骤\n\n- 初始化环境时保留变化信号，避免重复初始化吞掉切换。\n- 切换时清除旧组织信息，再应用新环境端点。\n- 用不可变快照保障并发读取的一致性。\n\n## 验证\n\n- 检查 dev → prod 和 prod → dev。\n- 验证旧异步请求不会覆盖新状态。\n\n## 默认范围\n\n保留现有会话和用户数据。';
const history: RunItem[] = [{ id: 'proposal', type: 'narration', role: 'assistant', text: markdown }];
const restored = reduceSubmittedPlanMessages(emptySubmittedPlanState(), [{ role: 'assistant', blocks: [
  { type: 'tool_use', id: 'write', tool: 'Write', input_json: JSON.stringify({ file_path: '.lingxi/plans/fix.md', content: markdown }) },
  { type: 'tool_result', id: 'write', tool: 'Write', result_json: '{}', is_error: false },
  { type: 'tool_use', id: 'exit', tool: 'ExitPlanMode', input_json: '{}' },
  { type: 'tool_result', id: 'exit', tool: 'ExitPlanMode', result_json: '{"plan":null,"plan_mode":false}', is_error: false },
] }], 'restored');
function Fixture() {
  const [open, setOpen] = useState(false);
  const [dark, setDark] = useState(false);
  const [scenario, setScenario] = useState('document');
  Object.assign(window, { planFixture: { theme: setDark, scenario: (value: string) => { setScenario(value); setOpen(false); }, markdown } });
  window.lingxi = { copyText: async (text: string) => { Object.assign(window, { copiedPlan: text }); } } as typeof window.lingxi;
  return <Theme.Provider value={tokens(dark)}><div style={{ display: 'flex', height: '100vh', fontFamily: 'system-ui', color: dark ? '#eee' : '#222', background: dark ? '#17181b' : 'white' }}>
    <main style={{ flex: 1, padding: 40, display: 'flex', flexDirection: 'column', justifyContent: 'end', gap: 24 }}>
      {scenario === 'history' ? <Stage sessionKey="history" liveItems={history} submittedPlans={narrationPlans(history)} />
        : scenario === 'restored' ? <Stage sessionKey="restored" liveItems={[{ id: 'exit', type: 'tool', tool: 'ExitPlanMode', status: 'done', view: { label: 'Plan' } as never }]} submittedPlans={restored.calls} />
        : <><p>计划已准备好，可以打开查看完整内容。</p><PlanPreview content={markdown} onOpen={() => setOpen(true)} /></>}
      <textarea placeholder="Describe your task to generate a plan…" style={{ padding: 24, border: '1px solid #8883', borderRadius: 22, background: 'transparent', color: 'inherit' }} />
    </main>
    {open && <aside style={{ width: 520, borderLeft: '1px solid #8883', overflow: 'auto' }}><header style={{ padding: 16, borderBottom: '1px solid #8883' }}>Plan</header><PlanDocument content={markdown} /></aside>}
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
