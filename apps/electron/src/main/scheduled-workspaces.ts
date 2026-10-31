import { mkdir, lstat, realpath, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

export interface ScheduledRunWorkspace {
  directory: string;
  work: string;
  outputs: string;
  memory: string;
}

async function realDirectory(path: string): Promise<string> {
  await mkdir(path, { recursive: true, mode: 0o700 });
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error('Scheduled workspace must be a real directory.');
  return realpath(path);
}

/** brandHome comes from the engine's product-paths API, which owns branding. */
export async function createScheduledRunWorkspace(brandHome: string, taskId: string, sessionId: string, title: string, now = new Date()): Promise<ScheduledRunWorkspace> {
  const root = await realDirectory(join(brandHome, 'workspaces'));
  const date = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
  const day = await realDirectory(join(root, date));
  const slug = title.normalize('NFKC').replace(/[^\p{L}\p{N}-]+/gu, '-').replace(/^-+|-+$/g, '').slice(0, 48) || 'task';
  // The host-generated session UUID is also the unique run directory identifier.
  if (!/^[a-f0-9-]{36}$/i.test(sessionId)) throw new Error('Invalid scheduled session ID.');
  const directory = join(day, `${slug}-${sessionId}`);
  await mkdir(directory, { mode: 0o700 });
  const [work, outputs] = await Promise.all(['work', 'outputs'].map(name => realDirectory(join(directory, name))));
  const taskKey = createHash('sha256').update(taskId).digest('hex');
  const memoryRoot = await realDirectory(join(brandHome, 'automations'));
  const memoryDirectory = await realDirectory(join(memoryRoot, taskKey));
  const memory = join(memoryDirectory, 'memory.md');
  try { await writeFile(memory, '', { flag: 'wx', mode: 0o600 }); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
  const memoryMetadata = await lstat(memory);
  if (!memoryMetadata.isFile() || memoryMetadata.isSymbolicLink()) throw new Error('Scheduled task memory must be a regular file.');
  return { directory, work, outputs, memory };
}

export function scheduledWorkspaceInstructions(workspace: Omit<ScheduledRunWorkspace, 'memory'> & { memory?: string }): string {
  return [
    'Scheduled task workspace:',
    `Use ${JSON.stringify(workspace.work)} for intermediate files, scripts, drafts, and temporary assets.`,
    `Use ${JSON.stringify(workspace.outputs)} for user-facing deliverables. Link saved deliverables from that directory in your final response.`,
    ...(workspace.memory ? [`Task memory: ${JSON.stringify(workspace.memory)}. Read it before working and update it with concise durable notes when useful.`] : []),
    'Answer inline when files are unnecessary. Do not write directly in the home directory.',
  ].join('\n');
}
