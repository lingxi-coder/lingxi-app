import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { ToolRunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { ToolCall } from '../../src/renderer/components/ToolCall';
import '../../src/renderer/global.css';
import '../../src/renderer/components/ToolRows.css';
localStorage.setItem('lingxi.desktop.language', 'zh-Hans');
const items: ToolRunItem[] = [
  { type: 'tool', id: 'local-app-read', tool: 'LocalApp', status: 'done', view: { verb: 'generic', icon: 'wrench', label: 'LocalApp', title: 'LocalApp' }, result: { headline: '全家人的终身学习空间：7 个年龄阶段、14 个领域，按家庭成员的兴趣安排课程和学习计划', body: '完整应用说明：按家庭成员组织课程、学习记录与活动。', body_lines: 1 } },
  { type: 'tool', id: 'local-app-running', tool: 'LocalApp', status: 'running', view: { verb: 'generic', icon: 'wrench', label: 'LocalApp', title: 'LocalApp', primary: '正在整理学习空间' } },
  { type: 'tool', id: 'local-app-error', tool: 'LocalApp', status: 'error', view: { verb: 'generic', icon: 'wrench', label: 'LocalApp', title: 'LocalApp', primary: '读取学习空间' }, result: { headline: '读取应用失败', body: '无法读取应用信息，请重试。', body_lines: 1 } },
];
function Fixture() {
  const [dark, setDark] = useState(false); const [open, setOpen] = useState<Record<string, boolean>>({}); const t = tokens(dark);
  return <Theme.Provider value={t}><main style={{ minHeight: '100vh', padding: 24, background: t.stageBg, color: t.text }}>
    <button onClick={() => setDark(!dark)}>切换主题</button>
    <div className="local-app-tool-preview" style={{ marginTop: 24, display: 'flex', flexDirection: 'column', gap: 12, maxWidth: 800 }}>
      {items.map(item => <ToolCall key={item.id} item={item} open={open[item.id] ?? false} onSetOpen={(id, next) => setOpen(previous => ({ ...previous, [id]: next }))} />)}
    </div>
  </main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
