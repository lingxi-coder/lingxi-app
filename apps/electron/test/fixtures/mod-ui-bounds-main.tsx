import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { ModUiParentSite, type ModUiClientHostApi } from '../../src/renderer/components/modUiAbovePrompt';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { NativeUiControlRequest, NativeUiControlResponseFor, UiControlCallResultDto, UiInvalidateEventDto, UiClientOperation, UiClientOperationResponseFor } from '@lingxi/bridge-client';

const renders: Array<{ instance: string; revision: number; intact: boolean }> = [];
const unmounts: number[] = [];
const listeners = new Set<(event: UiInvalidateEventDto) => void>();
let change: (value: string) => void;
let currentProps: object;
let revision = 0;
const viewport = { columns: 80, rows: 10 };
const host: ModUiClientHostApi = {
  async control<T extends NativeUiControlRequest>(_session: string, request: T) {
    if (request.subtype !== 'ui_render') throw new Error('Unexpected request');
    const renderRevision = ++revision;
    renders.push({ instance: request.instance_id, revision: renderRevision, intact: request.instance_id !== 'large-row' || request.props === currentProps });
    return {
      response: { rewritten: true, hooked: true, props: request.props, tree: { type: 'Text', children: [request.instance_id === 'large-row' ? `Mod: ${request.props.text}` : 'Weather sibling'] } },
      metadata: { renderRevision, clientRuntimeEpochs: {} },
    } as unknown as UiControlCallResultDto<NativeUiControlResponseFor<T>>;
  },
  async operation<T extends UiClientOperation>(_session: string, operation: T) {
    if (operation.type === 'draw_unmount') unmounts.push(operation.render_revision);
    return { handled: true, renderRevision: 'render_revision' in operation ? operation.render_revision : undefined } as UiClientOperationResponseFor<T>;
  },
  onFrame: () => () => {},
  onInvalidate: listener => { listeners.add(listener); return () => listeners.delete(listener); },
};

function Fixture() {
  const [text, setText] = React.useState('small');
  change = setText;
  const props = React.useMemo(() => ({ text }), [text]);
  currentProps = props;
  return <Theme.Provider value={tokens(false)}><main className="desktop-main" style={{ width: 720 }}>
    <section id="large"><ModUiParentSite sessionId="bounds" host={host} component="ToolResult" surface="desktop" instanceId="large-row" viewport={viewport} props={props}>
      <pre id="native-output">{text}</pre>
    </ModUiParentSite></section>
    <section id="weather"><ModUiParentSite sessionId="bounds" host={host} component="AbovePrompt" surface="desktop" instanceId="weather" viewport={viewport} props={{}} /></section>
  </main></Theme.Provider>;
}

Object.assign(window, { boundsFixture: {
  set: (value: string) => change(value),
  invalidate: () => listeners.forEach(listener => listener({ sessionId: 'bounds', uuid: `invalidate-${Date.now()}`, instances: [{ component: 'ToolResult', surface: 'desktop', instance_id: 'large-row' }] })),
  snapshot: () => ({ renders, unmounts }),
} });
createRoot(document.getElementById('root')!).render(<Fixture />);
