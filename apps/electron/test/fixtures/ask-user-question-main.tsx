import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { ToolCall } from '../../src/renderer/components/ToolCall';
import { AskUserQuestionPrompt } from '../../src/renderer/components/AskUserQuestionPrompt';
import '../../src/renderer/global.css';
const question = '这次统一语音服务，要不要同时纳入 provider 原生实时语音对话（双向音频、打断、会话管理）？';
function Fixture() {
  const [open, setOpen] = useState<boolean>();
  const [result, setResult] = useState('');
  const params = new URLSearchParams(location.search);
  const theme = tokens(params.get('theme') === 'dark');
  const pending = params.has('pending');
  return <Theme.Provider value={theme}><main style={{ background: theme.windowBg, minHeight: '100vh', padding: 24, fontFamily: '-apple-system, sans-serif', '--conversation-gutter': '0px', '--conversation-width': '860px' } as React.CSSProperties}>
    {pending ? <AskUserQuestionPrompt request={{ request_id: 1, questions: [{ question, header: '范围', multi_select: false, options: [{ label: '同时纳入原生实时对话', description: '双向音频、打断与会话管理' }, { label: '先统一基础语音', description: '后续再扩展' }] }] }} onSubmit={(_, answers) => setResult(JSON.stringify(answers))} onCancel={() => setResult('cancelled')} />
      : <ToolCall item={{ type: 'tool', id: 'ask', tool: 'AskUserQuestion', status: 'done', view: { verb: 'generic', label: 'AskUserQuestion', title: 'AskUserQuestion' }, nativeOutput: { questions: [{ question }], answers: { [question]: '同时纳入原生实时对话' } } }} open={open} onSetOpen={(_, value) => setOpen(value)} />}
    <output>{result}</output>
  </main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
