import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import { Appearance } from '../../src/renderer/components/settings/pages/Appearance';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

function Fixture() {
  const [preference, setPreference] = useState<boolean | undefined>();
  const [done, setDone] = useState(false);
  const [sessionKey, setSessionKey] = useState('a');
  Object.assign(window, { thoughtFixture: {
    finish() { flushSync(() => setDone(true)); },
    switchSession() { flushSync(() => setSessionKey('b')); },
  } });
  const palette = tokens(false);
  return <Theme.Provider value={palette}>
    <main style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: palette.stageBg, padding: 24 }}>
      <Appearance {...{ bridge: {
        bootstrap: { settings: { collapseThoughtsByDefault: preference } },
        setCollapseThoughtsByDefault: async (value: boolean) => setPreference(value),
      }, onTheme: () => undefined } as never} />
      <Stage sessionKey={sessionKey} collapseThoughtsByDefault={preference} liveItems={[
        { id: 'thought', type: 'thinking', streamed: true, done, text: done ? 'Finished reasoning' : 'Streaming reasoning' },
      ]} />
    </main>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
