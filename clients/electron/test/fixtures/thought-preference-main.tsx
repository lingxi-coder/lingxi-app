import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import { Appearance } from '../../src/renderer/components/settings/pages/Appearance';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TranscriptAgents.css';

function Fixture() {
  const [dark, setDark] = useState(false);
  const [openedAgent, setOpenedAgent] = useState('');
  const [preference, setPreference] = useState(false);
  const [running, setRunning] = useState(true);
  const [toolsDone, setToolsDone] = useState(false);
  const [done, setDone] = useState(false);
  const [sessionKey, setSessionKey] = useState('a');
  Object.assign(window, { thoughtFixture: {
    setDark(value: boolean) { flushSync(() => setDark(value)); },
    finishTools() { flushSync(() => setToolsDone(true)); },
    finish() { flushSync(() => setDone(true)); },
    switchSession() { flushSync(() => { setSessionKey('b'); setRunning(false); setDone(false); }); },
    stop() { flushSync(() => setRunning(false)); },
    restart() { flushSync(() => { setRunning(true); setDone(false); }); },
    changeLegacyPreference() { flushSync(() => setPreference((value) => !value)); },
  } });
  const palette = tokens(dark);
  return <Theme.Provider value={palette}>
    <main style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: palette.stageBg, padding: 24 }}>
      <Appearance {...{ bridge: {
        bootstrap: { settings: { collapseThoughtsByDefault: preference, theme: dark ? 'dark' : 'light' } },
        setCollapseThoughtsByDefault: async (value: boolean) => setPreference(value),
      }, onTheme: (value: string) => setDark(value === 'dark') } as never} />
      <output data-opened-agent={openedAgent} hidden>{openedAgent}</output>
      <Stage onOpenAgent={setOpenedAgent} agents={[
        { agent_id: 'explorer', name: 'Explorer', agent_type: 'explore', status: 'running', latest_activity: 'Inspecting the message renderer' },
        { agent_id: 'reviewer', name: 'Reviewer', agent_type: 'reviewer', status: 'completed', latest_activity: 'Checked the layout changes' },
      ]} running={running} sessionKey={sessionKey} collapseThoughtsByDefault={preference} liveItems={[
        { id: 'before', type: 'narration', role: 'assistant', text: 'Checking the project.' },
        { id: 'read', type: 'tool', tool: 'Read', status: 'done', view: { verb: 'read', label: 'Read', primary: 'Stage.tsx', title: 'Read Stage.tsx' } },
        { id: 'thought', type: 'thinking', streamed: true, done, text: done ? 'Finished reasoning' : 'Streaming reasoning' },
        { id: 'shell', type: 'tool', tool: 'Bash', status: toolsDone ? 'done' : 'running', view: { verb: 'run', label: 'Shell', primary: 'npm run typecheck', title: 'Check TypeScript' } },
        { id: 'after', type: 'narration', role: 'assistant', text: 'Following message.' },
      ]} />
    </main>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
