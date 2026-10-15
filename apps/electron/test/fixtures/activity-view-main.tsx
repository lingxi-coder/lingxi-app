import { NeedsInputBadge } from '../../src/renderer/components/NeedsInputBadge';
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { Stage } from '../../src/renderer/components/Stage';
import type { RunItem } from '../../src/renderer/model/runItem';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TranscriptAgents.css';
const items: RunItem[] = [
  {type: 'narration', id: 'n1', role: 'assistant', text: 'Inspecting the project and validating the changes.'},
  {type: 'tool', id: 't1', tool: 'Read', status: 'done', view: {verb: 'read', label: 'Read files', title: 'Read files', icon: 'file'}, note: 'File contents'},
  {type: 'tool', id: 't2', tool: 'Bash', status: 'error', view: {verb: 'shell', label: 'Ran commands', title: 'Ran commands', icon: 'terminal'}, note: 'Build failed'},
  {type: 'narration', id: 'n2', role: 'assistant', text: 'The build reported an error. Fixing it next.'},
];
function Fixture() {
  const [activityView, setActivityView] = useState(true);
  const [needsInput, setNeedsInput] = useState(true);
  const [dark, setDark] = useState(false);
  return <Theme.Provider value={tokens(dark)}><div style={{height: '100vh', display: 'flex', flexDirection: 'column', background: tokens(dark).stageBg}}>
    <button id="toggle" aria-pressed={activityView} onClick={() => setActivityView(!activityView)}>Activity view</button>
    <button id="theme" onClick={() => setDark(!dark)}>Theme</button>
    <button id="input" onClick={() => setNeedsInput(!needsInput)} style={{display: "flex", alignItems: "center"}}>Session needing approval {needsInput && <NeedsInputBadge />}</button>
    <Stage activityView={activityView} liveItems={items} sessionKey="activity-fixture" />
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
