import { createRoot } from 'react-dom/client';
import { ContextWindow } from '../../src/renderer/components/ContextWindow';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

createRoot(document.getElementById('root')!).render(
  <Theme.Provider value={tokens(false)}>
    <main style={{ padding: 180, display: 'flex', gap: 24, alignItems: 'center' }}>
      <ContextWindow capacity={475000} usage={{ inputTokens: 79000, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0 }} />
      <button id="other" type="button">Other control</button>
    </main>
  </Theme.Provider>,
);
