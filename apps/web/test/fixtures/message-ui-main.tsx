import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { TranscriptView } from '../../src/remote/Transcript';
import { messageUIData } from './message-ui-data';
import '../../src/theme.css';
import '../../src/styles.css';
import '../../src/remote/remote.css';
function Fixture() {
  const [mode, setMode] = useState('running'); const [count, setCount] = useState(3);
  const [session, setSession] = useState('a'); const [locale, setLocale] = useState<'en' | 'zh'>('en');
  const [dark, setDark] = useState(false);
  return <main style={{ maxWidth: 880, margin: 'auto', padding: 16 }}>
    <nav style={{ display: 'flex', flexWrap: 'wrap', gap: 12, marginBottom: 24 }}>
      <button onClick={() => setCount(1)}>Single agent</button><button onClick={() => setMode('done')}>Finish</button><button onClick={() => setMode('mixed')}>Mixed</button>
      <button onClick={() => setCount(count === 3 ? 6 : 3)}>More agents</button>
      <button onClick={() => { document.documentElement.dataset.theme = dark ? 'light' : 'dark'; setDark(!dark); }}>Theme</button>
      <button onClick={() => setLocale(locale === 'en' ? 'zh' : 'en')}>Language</button>
      <button onClick={() => setSession(session === 'a' ? 'b' : 'a')}>Switch session</button>
    </nav>
    <TranscriptView key={session} locale={locale} transcript={messageUIData(mode, count)} running={false} deviceName="Preview" fetchImage={async () => ''} />
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
