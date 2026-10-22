import React from 'react';
import { createRoot } from 'react-dom/client';
import { LocalApps } from '../../src/renderer/components/LocalApps';
import '../../src/renderer/components/LocalApps.css';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
createRoot(document.getElementById('root')!).render(<Theme.Provider value={tokens(false)}><LocalApps visible onCreate={() => undefined}/></Theme.Provider>);
