import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { SessionStorageNotice } from '../../src/renderer/components/SessionStorageNotice';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';

localStorage.setItem(LANGUAGE_STORAGE_KEY, 'zh-Hans');

function Fixture() {
  const [paused, setPaused] = useState(true);
  const [dark, setDark] = useState(false);
  const fixture = (window as any).storageFixture ??= { calls: 0 };
  fixture.ready = () => setPaused(false);
  fixture.theme = setDark;
  return <Theme.Provider value={tokens(dark)}><main style={{ padding: 12, fontFamily: 'system-ui', background: tokens(dark).stageBg }}>
    <p id="history" style={{ color: tokens(dark).text }}>原 session 的已有历史</p>
    {paused ? <SessionStorageNotice disabled={false} onRetry={() => {
      fixture.calls += 1;
      return new Promise<void>((resolve, reject) => { fixture.resolve = resolve; fixture.reject = reject; });
    }} /> : <p id="continued">已恢复原 session</p>}
  </main></Theme.Provider>;
}

createRoot(document.getElementById('root')!).render(<Fixture />);
