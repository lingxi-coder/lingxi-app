import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import '../../src/renderer/global.css';
const projectPath = '/fixture/LingXi';
const activeSession = { projectPath, sessionId: 'parent' };
const connectionCallbacks = new Set<(envelope: any) => void>();
const records = new Map<string, string[]>();
const callbacks = new Set<(envelope: any) => void>();
const state = { creationInputs: [] as (string | undefined)[], parentPrompts: [] as string[], sequence: 0, creates: 0, submits: 0, promptSubmits: 0, commands: [] as {sideId: string; type: string}[], holdNextTurn: false, rejectNext: false, delayAck: false, failCleanup: false };
const emit = (event: any) => { const envelope = { sessionId: 'parent', sequence: ++state.sequence, event }; callbacks.forEach((callback) => callback(envelope)); };
const models = { type: 'model_list', models: ['openai/gpt-6.1-sol', 'openai/gpt-6-sol'], current: 'openai/gpt-6.1-sol' };
const controls = { type: 'conversation_controls_changed', controls: { qualified_model: 'openai/gpt-6.1-sol', permission: { requested: 'default', effective: 'default', options: [] }, reasoning: { requested: { type: 'level', id: 'xhigh' }, effective: { type: 'level', id: 'xhigh' }, spec: { options: [{ selection: { type: 'level', id: 'high' }, persistable: true }, { selection: { type: 'level', id: 'xhigh' }, persistable: true }], provider_default: { type: 'level', id: 'high' }, forced_reasoning: false, editable: true } } } };
const inner = (side_id: string, event: any) => { records.get(side_id)?.push(JSON.stringify(event)); emit({ type: 'side_chat_event', side_id, event_json: JSON.stringify(event) }); };
(window as any).sideFixture = { state, emit, inner, reconnect: () => connectionCallbacks.forEach((callback) => callback({ sessionId: 'parent', event: { status: 'connected' } })) };
const snapshot = {
  revision: 1,
  settings: { version: 1, projects: [projectPath], activeProject: projectPath, activeSession, pinnedSessions: [], archivedSessions: [], theme: new URLSearchParams(location.search).get('theme') ?? 'light' },
  activeSession, workspace: { path: projectPath, trusted: true }, connection: { status: 'connected' },
  runtimes: [{ ...activeSession, connection: { status: 'connected' }, turnActive: true, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  projectCatalogs: { [projectPath]: { sessions: [] } }, providerCredentials: [{ providerId: 'openai', configured: true }], diagnostics: [], versions: { app: '0.1.0', electron: '43.1.1' },
};
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: (callback: any) => { callbacks.add(callback); return () => callbacks.delete(callback); }, onConnectionStateChanged: (callback: any) => { connectionCallbacks.add(callback); return () => connectionCallbacks.delete(callback); }, onPermission: () => () => {}, onComputerAccess: () => () => {},
  bootstrap: async () => snapshot,
  sendPrompt: async (_session: string, text: string) => { state.parentPrompts.push(text); },
  listProjectSessions: async () => ({ projectPath, sessions: [] }),
  command: async (_session: string, command: any) => {
    if (command.type === 'side_chat_create') {
      state.creates++;
      state.creationInputs.push(command.initial_input);
      if (state.rejectNext) { state.rejectNext = false; emit({ type: 'side_chat_error', request_id: command.request_id, message: 'Creation refused' }); return; }
      const id = `side-${state.creates}`;
      setTimeout(() => {
        emit({ type: 'side_chat_created', request_id: command.request_id, side_id: id });
        records.set(id, []);
        if (command.initial_input) records.get(id)!.push(JSON.stringify({ type: 'message_complete', message: { role: 'user', blocks: [{ type: 'text', text: command.initial_input }] } }));
        if (command.initial_input) {
          inner(id, { type: 'turn_started', turn_id: 1 });
          inner(id, { type: 'text_delta', text: 'A Side Chat keeps the parent context while its conversation stays independent.\n\nYou can ask questions here, approve tools, and keep the main task running.' });
          inner(id, { type: 'turn_ended', turn_id: 1, outcome: { type: 'completed' } });
        }
      }, state.delayAck ? 400 : 0);
    }
    if (command.type === 'side_chat_submit') {
      state.submits++;
      const input = JSON.parse(command.command_json);
      state.commands.push({ sideId: command.side_id, type: input.type });
      if (input.type === 'send_prompt') state.promptSubmits++;
      const hold = input.type === 'send_prompt' && state.holdNextTurn;
      if (hold) state.holdNextTurn = false;
      if (state.rejectNext || input.raw?.startsWith('/side')) { state.rejectNext = false; emit({ type: 'side_chat_error', request_id: command.request_id, side_id: command.side_id, message: 'This command is not allowed in Side Chat' }); return; }
      setTimeout(() => {
        emit({ type: 'side_chat_submitted', request_id: command.request_id, side_id: command.side_id });
        if (input.type === 'list_models') inner(command.side_id, models);
        if (input.type === 'set_model') controls.controls.qualified_model = input.model;
        if (input.type === 'get_conversation_controls' || input.type === 'set_model') inner(command.side_id, controls);
        if (input.type === 'refresh_listings') inner(command.side_id, { type: 'slash_commands', commands: [] });
        if (input.type === 'cancel') inner(command.side_id, { type: 'turn_ended', turn_id: 1, outcome: { type: 'completed' } });
        if (input.type === 'send_prompt') {
          records.get(command.side_id)?.push(JSON.stringify({ type: 'message_complete', message: { role: 'user', blocks: [{ type: 'text', text: input.text }] } }));
          inner(command.side_id, { type: 'turn_started', turn_id: 2 });
          inner(command.side_id, { type: 'text_delta', text: 'This reply belongs only to the Side Chat.' });
          if (!hold) inner(command.side_id, { type: 'turn_ended', turn_id: 2, outcome: { type: 'completed' } });
        }
      }, state.delayAck ? 400 : 0);
    }
    if (command.type === 'side_chat_list') emit({ type: 'side_chat_list', side_ids: [...records.keys()] });
    if (command.type === 'side_chat_get') emit({ type: 'side_chat_snapshot', side_id: command.side_id, events_json: records.get(command.side_id) ?? [] });
    if (command.type === 'side_chat_end') emit(state.failCleanup ? { type: 'side_chat_error', side_id: command.side_id, message: 'Cleanup is still pending' } : { type: 'side_chat_ended', side_id: command.side_id });
  },
};
createRoot(document.getElementById('root')!).render(<App />);
setTimeout(() => { emit(models); emit({ type: 'turn_started', turn_id: 1 }); emit({ type: 'text_delta', text: 'The main task is running. This timeline must stay independent of Side Chat.' }); }, 100);
