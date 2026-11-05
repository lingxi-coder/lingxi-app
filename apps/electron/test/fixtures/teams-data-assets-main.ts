import assert from 'node:assert/strict';
import { app } from 'electron';
import { join } from 'node:path';

app.disableHardwareAcceleration();
app.setPath('userData', join(app.getAppPath(), 'profile'));
app.on('window-all-closed', () => {});

async function main(): Promise<void> {
  await app.whenReady();
  console.log('READY');
  const loaded: string[] = [], failures: string[] = [];
  app.on('web-contents-created', (_event, contents) => {
    contents.on('preload-error', (_event, _path, error) => failures.push(error.message));
    contents.on('did-fail-load', (_event, _code, description) => failures.push(description));
    contents.on('did-finish-load', () => loaded.push(contents.getTitle()));
  });
  // A dynamic import reproduces production's out/main/chunks layout. The real
  // sandboxed page must send its initialization error through its real preload;
  // omitting the API key avoids contacting any real account or backend.
  const { openTeamsMessageSession } = await import('../../src/main/teams/messageWindow.js');
  await assert.rejects(openTeamsMessageSession({ config: { projectId: 'demo-teams-assets' }, uid: 'assets-probe' }), /MESSAGE_LISTENER_FAILED/u);
  assert.deepEqual(failures, []);
  assert.deepEqual(loaded, ['Teams data']);
  console.log(JSON.stringify({ passed: true, checks: ['split-main-chunk', 'sandboxed-preload', 'production-data-page', 'message-port-error'] }));
}
void main().then(() => app.exit(0), error => { console.error(error); app.exit(1); });
