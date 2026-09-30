import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Stage } from '../../src/renderer/components/Stage';
import { RuntimeCenterInspector } from '../../src/renderer/components/RuntimeCenter';
import {
  closeRuntimeCenterItem,
  emptyRuntimeCenterState,
  openRuntimeCenterItem,
} from '../../src/renderer/bridge/runtimeCenterState';
import { emptyConversation } from '../../src/renderer/bridge/conversation';
import { emptyDesktopState } from '../../src/renderer/bridge/desktopState';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import '../../src/renderer/components/RuntimeCenter.css';

const files = ['apps/electron/src/renderer/components/Stage.tsx', 'apps/electron/src/renderer/global.css'].map((path, index) => ({
  path, additions: index ? 2 : 44, removals: index ? 0 : 10,
  diffs: [{ file_path: path, additions: index ? 2 : 44, removals: index ? 0 : 10, gutter_width: 2, truncated_rows: 0,
    rows: [{ kind: 'add' as const, line_no: 1, hunk: 0, segments: [{ text: index ? 'Historical stylesheet change' : 'Historical component change', class: 'plain' as const }] }] }],
}));
function Fixture() {
  const [runtimeCenter, setCenter] = useState(emptyRuntimeCenterState);
  const t = tokens(new URLSearchParams(location.search).has('dark'));
  const bridge = {
    runtimeCenter,
    conversation: { ...emptyConversation(), sessionKey: 'turn-review-fixture' },
    desktop: emptyDesktopState(),
    openRuntimeItem: item => setCenter(center => openRuntimeCenterItem(center, item)),
    closeRuntimeItem: item => setCenter(center => closeRuntimeCenterItem(center, item)),
    setRuntimeInspectorOpen: inspectorOpen => setCenter(center => ({ ...center, inspectorOpen })),
    setRuntimeCenterOverviewOpen: overviewOpen => setCenter(center => ({ ...center, overviewOpen })),
  } as UseBridge;
  return <Theme.Provider value={t}><div style={{ display: 'flex', position: 'relative', height: '100vh', overflow: 'hidden', background: t.transcriptBg }}>
    <main style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
      <Stage sessionKey="turn-review-fixture" liveItems={[{ type: 'meta', id: 'turn-1', dur: '', tokens: '', files }]}
        onReviewFiles={(id, changes, path) => bridge.openRuntimeItem({ kind: 'turn-review', id, files: changes, path })} />
    </main>
    <RuntimeCenterInspector bridge={bridge} />
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
