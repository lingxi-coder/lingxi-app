import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { readFile, writeFile } from 'node:fs/promises';
import { parseArgs } from 'node:util';
import { planRetiredTeamCleanup, applyRetiredTeamCleanup } from '../src/retired-teams-data.mjs';

const { values } = parseArgs({ options: {
  project: { type: 'string' }, team: { type: 'string', multiple: true }, plan: { type: 'string' },
  apply: { type: 'boolean', default: false }, 'old-execution-stopped': { type: 'boolean', default: false },
} });
if (!values.project || !values.plan || !values.team?.length) throw new Error('Usage: --project PROJECT --team TEAM [--team TEAM] --plan FILE [--apply --old-execution-stopped]');
if (process.env.FIRESTORE_EMULATOR_HOST && !values.project.startsWith('demo-')) throw new Error('EMULATOR_REQUIRES_DEMO_PROJECT');
const app = initializeApp({ projectId: values.project }), db = getFirestore(app);
try {
  if (values.apply) {
    const envelope = JSON.parse(await readFile(values.plan, 'utf8'));
    if (envelope.project !== values.project || JSON.stringify(envelope.plan.teamIds) !== JSON.stringify([...values.team].sort())) throw new Error('CLEANUP_PROJECT_OR_TEAMS_CHANGED');
    console.log(JSON.stringify(await applyRetiredTeamCleanup(db, envelope.plan, { oldExecutionStopped: values['old-execution-stopped'] })));
  } else {
    const plan = await planRetiredTeamCleanup(db, values.team);
    await writeFile(values.plan, JSON.stringify({ project: values.project, plan }, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    console.log(JSON.stringify({ mode: 'dry-run', obsoleteRoots: plan.entries.length, digest: plan.digest }));
  }
} finally { await db.terminate(); await deleteApp(app); }
