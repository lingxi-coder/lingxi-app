import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TerminalPanel.css';
import { Stage } from '../../src/renderer/components/Stage';
import { conversationFromMessages } from '../../src/renderer/bridge/conversation';
import { ScheduledTasks } from '../../src/renderer/components/ScheduledTasks';
import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import { TerminalPanel, useTerminalPanel } from '../../src/renderer/components/TerminalPanel';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { settingsLinkRequest } from '../../src/renderer/deepLinkRoutes';
import type { TerminalSnapshot } from '../../src/shared/terminal';
const calls: { domain: string; command: any }[] = [];
const records: TerminalSnapshot[] = [];
const inputs: string[] = [];
const located: boolean[] = []; const copiedUuids: string[] = []; let scrolls = 0;
const originalScroll = HTMLElement.prototype.scrollIntoView;
HTMLElement.prototype.scrollIntoView = function(options) { if (this.dataset.messageUuids) scrolls++; return originalScroll?.call(this, options); };
const readHeader = { verb: 'read', label: 'Read', title: 'Read' } as const;
const stageConversation = conversationFromMessages([
  { role: 'assistant', transcript_uuid: 'call-first', blocks: [{ type: 'tool_use', id: 'tool-first', tool: 'Read', input_json: '{}', header: readHeader }] },
  { role: 'assistant', transcript_uuid: 'call-second', blocks: [{ type: 'tool_use', id: 'tool-second', tool: 'Read', input_json: '{}', header: readHeader }] },
  { role: 'user', transcript_uuid: 'result-second', blocks: [{ type: 'tool_result', id: 'tool-second', tool: 'Read', is_error: false, result_json: '"ok"', display: { headline: 'Read complete', body: 'ok', body_lines: 1 } }] },
  { role: 'system', transcript_uuid: 'wake-first', blocks: [], loop_wakeup: { message: 'First wakeup', streak: 0, since_ms: 0 } },
  { role: 'user', transcript_uuid: 'hidden-message', blocks: [{ type: 'text', text: 'Hidden target\n'.repeat(20) }] },
  { role: 'system', transcript_uuid: 'wake-second', blocks: [], loop_wakeup: { message: 'Latest wakeup', streak: 1, since_ms: 1 } },
  { role: 'assistant', transcript_uuid: 'reasoning-message', blocks: [{ type: 'thinking', thinking: 'Persisted reasoning-only target' }] },
]);
const noop = async () => undefined;
const scheduledJob = { id: 'job-1', cron: '0 9 * * *', prompt: '# Linked task\n\nInspect this project', recurring: true, durable: true };
const scheduledRuns = [{ id: 'run-1', status: 'succeeded', scheduledAt: 1700000000000, model: 'test/model', summary: 'Linked run result' }];
const bridge = {
  activeSession: { projectPath: '/test/project', sessionId: 'session-a' }, connected: true, sessionLoading: false,
  bootstrap: { workspace: { path: '/test/project', trusted: true }, settings: { projects: ['/test/project'] }, providerCredentials: [], projectCatalogs: {} },
  settingsSnapshotEvent: { effective_json: '{}', provenance_json: '{}', layers_json: '{"user":{},"project":{},"local":{}}' },
  refreshSettingsSnapshot: noop, refreshProviderCredential: noop,
  mcpConfigurationSnapshotEvent: { snapshot_json: JSON.stringify({ scopes: [{ scope: 'user', path: '/test/.lingxi.json', revision_sha256: 'r1', raw_json: '{"mcpServers":{"known":{"command":"echo","args":["ok"]}}}' }] }) },
  skillCatalogEvent: { catalog_json: JSON.stringify({ entries: [{ id: 'skill-id', name: 'known-skill', source: 'user', rootDir: '/skills', directory: '/skills/known-skill', writable: true }], trash: [] }) },
  pluginCatalogEvent: { catalog_json: JSON.stringify({ installed: [{ id: 'known-plugin', name: 'known-plugin' }], available: [], revisions: { user: 'r1' } }) },
  mcpAdmin: async command => { calls.push({ domain: 'mcp', command }); },
  skillAdmin: async command => { calls.push({ domain: 'skill', command }); },
  pluginAdmin: async command => { calls.push({ domain: 'plugin', command }); return { details_json: 'Ready for confirmation' }; },
  updateProviderSettings: async (scope, patch) => { calls.push({ domain: 'provider', command: { scope, patch } }); },
  desktop: { providerModelCatalog: [], modelDetails: [] },
  scheduledScopes: [{ id: 'global', label: 'None' }],
  manageScheduled: async (scope, command) => { calls.push({ domain: 'cron', command }); return [scheduledJob]; },
  readScheduledHistory: async () => scheduledRuns,
  scheduledContext: async () => ({ models: [], currentModel: null, sessions: [] }),
  configurationOperations: {}, refreshMcpServers: noop, refreshSkills: noop,
} as any;
window.lingxi = { terminal: {
  list: async () => records.slice(),
  create: async scope => { const tab = { id: 'term-1', scope, title: 'Shell', status: 'running' as const, exitCode: null, output: '', sequence: 0 }; records.push(tab); return tab; },
  input: async (_id, data) => { inputs.push(data); }, resize: noop, acknowledge: noop, close: noop, onEvent: () => () => {},
} } as any;
function Fixture() {
  const [request, setRequest] = useState<any>(null);
  const [visible, setVisible] = useState(true);
  const [stageTarget, setStageTarget] = useState<string | null>(null);
  const [stageItems, setStageItems] = useState(stageConversation.items);
  const [cronTarget, setCronTarget] = useState<any>(null);
  const [terminalView, setTerminalView] = useState(false);
  const terminal = useTerminalPanel({ projectPath: '/test/project', sessionId: 'session-a' }, terminalView);
  Object.assign(window, { linkPreviewFixture: {
    show: (route, payload, scope) => { setStageTarget(null); setCronTarget(null); setTerminalView(false); setVisible(true); setRequest(settingsLinkRequest({ route, payload, params: scope ? { scope } : {} }, String(Date.now()))); },
    stage: target => { setVisible(false); setTerminalView(false); setCronTarget(null); setStageTarget(target); },
    stream: () => setStageItems(previous => [...previous, { type: 'narration', id: 'stream', role: 'assistant', text: 'Streaming update' }]),
    toggleTerminal: terminal.toggle,
    cron: (kind, id) => { setVisible(false); setTerminalView(false); setCronTarget({ requestId: String(Date.now()), kind, id }); },
    terminal: async command => { setVisible(false); setTerminalView(true); await terminal.openLink({ projectPath: '/test/project', sessionId: 'session-a' }, 'interactive', command); },
    state: () => ({ calls, inputs, terminals: records.length, visible, located, copiedUuids, scrolls }),
  } });
  return <Theme.Provider value={tokens(false)}><div style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
    {visible && request && <SettingsScreen key={request.linkRequest.requestId} bridge={bridge} theme="light" onTheme={() => {}} onClose={() => setVisible(false)} initialPageId={request.pageId} linkRequest={request.linkRequest} />}
    {stageTarget && <Stage sessionKey="message-fixture" messageTarget={stageTarget} liveItems={stageItems} foldedItemIds={stageConversation.foldedItemIds} onMessageLocated={found => located.push(found)} onCopyMessage={uuid => copiedUuids.push(uuid)} />}
    {cronTarget && <ScheduledTasks bridge={bridge} visible={true} initialTarget={cronTarget} />}
    <TerminalPanel controller={terminal} />
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
