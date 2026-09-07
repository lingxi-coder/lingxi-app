import { useState } from 'react';
import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';
import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import type { UseBridge } from '../../src/renderer/bridge/useBridge';
import { ScheduledTasks } from '../../src/renderer/components/ScheduledTasks';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// Explicit UI fixture: in-memory backend only. No preload, bridge, credentials,
// filesystem access, real schedules, or engine process is loaded.
let jobs: CronJobDto[] = [];
let nextId = 1;
let rejection = '';
let mutationGate: Promise<void> | null = null;
let releaseMutation: (() => void) | undefined;
const requests: CronRequestDto[] = [];
const manageCron = async (request: CronRequestDto): Promise<CronJobDto[]> => {
  requests.push(structuredClone(request));
  if (request.action !== 'list' && mutationGate) { const gate = mutationGate; mutationGate = null; await gate; }
  if (rejection) { const error = rejection; rejection = ''; throw new Error(error); }
  if (request.action === 'create') jobs.push({ id: `fixture-${nextId++}`, cron: request.cron!, prompt: request.prompt!, recurring: request.recurring ?? true, durable: true, permanent: false, created_at: Date.now(), session_id: 'fixture-session', ...(request.expires_at ? { expires_at: request.expires_at } : {}) });
  if (request.action === 'update') jobs = jobs.map((job) => job.id === request.id ? { ...job, cron: request.cron ?? job.cron, prompt: request.prompt ?? job.prompt, recurring: request.recurring ?? job.recurring, ...(request.no_expiry ? { expires_at: undefined } : request.expires_at ? { expires_at: request.expires_at } : {}) } : job);
  if (request.action === 'delete') jobs = jobs.filter((job) => job.id !== request.id);
  return structuredClone(jobs);
};
Object.assign(window, { scheduledCronFixture: {
  state: () => ({ jobs: structuredClone(jobs), requests: structuredClone(requests) }),
  holdMutation: () => { mutationGate = new Promise<void>((resolve) => { releaseMutation = resolve; }); },
  releaseMutation: () => releaseMutation?.(),
  rejectNext: (message: string) => { rejection = message; },
  addExternal: () => { jobs.push({ id: 'external', cron: '0 12 * * *', prompt: 'External fixture task', recurring: true, durable: true, permanent: false, created_at: Date.now() }); },
} });
const bridge = { activeSession: { sessionId: 'fixture-session', projectPath: '/fixture/LingXi-Next' }, connected: true, sessionLoading: false, manageCron } as unknown as UseBridge;
const palette = tokens(false);
function Fixture() {
  const [visible, setVisible] = useState(true);
  Object.assign((window as unknown as { scheduledCronFixture: object }).scheduledCronFixture, { setVisible: (value: boolean) => flushSync(() => setVisible(value)) });
  return <Theme.Provider value={palette}><main style={{ height: '100vh', background: palette.windowBg, color: palette.text }}><ScheduledTasks bridge={bridge} visible={visible} /></main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
