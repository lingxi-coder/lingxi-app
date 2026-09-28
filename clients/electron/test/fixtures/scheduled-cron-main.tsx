import { useState } from 'react';
import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';
import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { ScheduledTasks } from '../../src/renderer/components/ScheduledTasks';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { emptyDesktopState } from '../../src/renderer/bridge/desktopState';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// Explicit UI fixture: in-memory backend only. No preload, bridge, credentials,
// filesystem access, real schedules, or engine process is loaded.
let jobs: CronJobDto[] = [];
let nextId = 1;
let rejection = '';
let mutationGate: Promise<void> | null = null;
let releaseMutation: (() => void) | undefined;
const refreshCallbacks = new Map<number, () => void>();
const nativeSetInterval = window.setInterval.bind(window);
const nativeClearInterval = window.clearInterval.bind(window);
window.setInterval = ((handler: TimerHandler, timeout?: number, ...args: unknown[]) => {
  const id = nativeSetInterval(handler, timeout, ...args);
  if (timeout === 15_000 && typeof handler === 'function') refreshCallbacks.set(id, () => handler(...args));
  return id;
}) as typeof window.setInterval;
window.clearInterval = (id) => { if (id !== undefined) refreshCallbacks.delete(id); nativeClearInterval(id); };
const requests: CronRequestDto[] = [];
const manageCron = async (request: CronRequestDto): Promise<CronJobDto[]> => {
  requests.push(structuredClone(request));
  if (request.action !== 'list' && mutationGate) { const gate = mutationGate; mutationGate = null; await gate; }
  if (rejection) { const error = rejection; rejection = ''; throw new Error(error); }
  if (request.action === 'create') jobs.push({ id: `fixture-${nextId++}`, cron: request.cron!, prompt: request.prompt!, recurring: request.recurring ?? true, durable: true, permanent: false, created_at: Date.now(), session_id: 'fixture-session', automation: request.automation, ...(request.expires_at ? { expires_at: request.expires_at } : {}) });
  if (request.action === 'update') jobs = jobs.map((job) => job.id === request.id ? { ...job, automation: request.automation ?? job.automation, cron: request.cron ?? job.cron, prompt: request.prompt ?? job.prompt, recurring: request.recurring ?? job.recurring, ...(request.no_expiry ? { expires_at: undefined } : request.expires_at ? { expires_at: request.expires_at } : {}) } : job);
  if (request.action === 'delete') jobs = jobs.filter((job) => job.id !== request.id);
  return structuredClone(jobs);
};
Object.assign(window, { scheduledCronFixture: {
  tickRefresh: () => refreshCallbacks.forEach((callback) => callback()),
  completeJob: (id: string) => { jobs = jobs.map((job) => job.id === id && job.automation ? { ...job, automation: { ...job.automation, status: 'completed', runs: [{ id: 'fixture-run', taskId: job.id, scheduledAt: Date.now(), finishedAt: Date.now(), status: 'succeeded', model: job.automation.model, reasoning: job.automation.reasoning, summary: 'Automatic result refreshed' }] } } : job); },
  state: () => ({ jobs: structuredClone(jobs), requests: structuredClone(requests) }),
  holdMutation: () => { mutationGate = new Promise<void>((resolve) => { releaseMutation = resolve; }); },
  releaseMutation: () => releaseMutation?.(),
  rejectNext: (message: string) => { rejection = message; },
  addExternal: () => { jobs.push({ id: 'external', cron: '0 12 * * *', prompt: 'External fixture task', recurring: true, durable: true, permanent: false, created_at: Date.now() }); },
} });
const bridge = { activeSession: { sessionId: 'fixture-session', projectPath: '/fixture/LingXi-Next' }, connected: true, sessionLoading: false, manageCron, scheduledScopes: [{id: 'global', label: 'None'}], manageScheduled: (_scope: string, request: CronRequestDto) => manageCron(request), scheduledContext: async () => ({ models: bridge.desktop.modelDetails, currentModel: 'openai/gpt-test', sessions: [{ uuid: 'fixture-session', title: 'Fixture chat' }] }), readScheduledHistory: async () => [], openScheduledSession: async () => {}, desktop: { ...emptyDesktopState(), currentModel: 'openai/gpt-test', modelDetails: [{reference: 'openai/gpt-test', display_name: 'Test Model', provider_label: 'OpenAI', reasoning: { options: [{selection: {type:'automatic'}, persistable: true}, {selection:{type:'level',id:'high'},persistable:true}], provider_default: {type:'automatic'} }}] } } as unknown as UseBridge;
function Fixture() {
  const [visible, setVisible] = useState(true);
  const [dark, setDark] = useState(false);
  const palette = tokens(dark);
  Object.assign((window as unknown as { scheduledCronFixture: object }).scheduledCronFixture, { setVisible: (value: boolean) => flushSync(() => setVisible(value)), setDark: (value: boolean) => flushSync(() => setDark(value)) });
  return <Theme.Provider value={palette}><main style={{ height: '100vh', background: palette.windowBg, color: palette.text }}><ScheduledTasks bridge={bridge} visible={visible} /></main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
