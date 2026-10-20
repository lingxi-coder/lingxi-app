import { appendFileSync, mkdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { app } from 'electron';
import { crashCategory, isIsolatedCrashProbe } from './crashReportingPolicy.js';

interface NativeReporter {
  start(config: string, channel: string): boolean;
  recordEvent(category: string, code: number): void;
  previousExecutionCrashed(): boolean;
}

export function installCrashReporting(): void {
  const logDirectory = join(app.getPath('userData'), 'logs');
  mkdirSync(logDirectory, { recursive: true, mode: 0o700 });
  const log = (event: string, details: Record<string, string | number | boolean> = {}) => {
    try {
      appendFileSync(join(logDirectory, 'crashes.jsonl'), JSON.stringify({ timestamp: new Date().toISOString(), event, ...details }) + '\n', { mode: 0o600 });
    } catch { /* A failed diagnostic write must not cause another crash. */ }
  };
  let native: NativeReporter | undefined;
  // Only the signed macOS package contains the official Apple SDK bridge.
  // Do not install Crashpad alongside its Mach/signal handlers in this process.
  if (process.platform === 'darwin' && app.isPackaged) {
    try {
      const root = join(process.resourcesPath, 'crashlytics');
      const metadata = createRequire(import.meta.url)(join(root, 'configuration.json')) as { channel: string };
      const loaded = createRequire(import.meta.url)(join(root, 'crashlytics.node')) as NativeReporter;
      if (!loaded.start(join(root, 'GoogleService-Info.plist'), metadata.channel)) throw new Error('SDK initialization rejected');
      native = loaded;
      log('crashlytics_started', { platform: process.platform, version: app.getVersion(), channel: metadata.channel, previousExecutionCrashed: loaded.previousExecutionCrashed() });
      // No renderer IPC or deep link can trigger this explicit CLI acceptance
      // run. An existing user's home/userData is never eligible.
      if (process.argv.includes('--verify-crashlytics-isolated')
          && isIsolatedCrashProbe(process.env.LINGXI_CRASHLYTICS_TEST_ROOT,
            process.env.LINGXI_CRASHLYTICS_TEST_TMP_BASE ?? tmpdir(), app.getPath('userData'), process.env.HOME, process.env.CFFIXED_USER_HOME)) {
        void app.whenReady().then(() => { setTimeout(() => process.crash(), 5_000); });
      }
    } catch {
      log('crashlytics_initialization_failed');
    }
  } else {
    log('crashlytics_not_available', { platform: process.platform, packaged: app.isPackaged });
  }
  const report = (category: string, code: number) => {
    log(category, { code });
    try { native?.recordEvent(category, code); } catch { /* Preserve existing exit behavior. */ }
  };
  // Observe only: Node keeps its default fatal behavior. Never swallow an
  // uncaught exception and continue running an inconsistent application.
  process.on('uncaughtExceptionMonitor', () => report('main-js-uncaught', 0));
  app.on('render-process-gone', (_event, _contents, details) => {
    log('renderer_exit', { reason: details.reason, exitCode: details.exitCode });
    const category = crashCategory('renderer', details.reason);
    if (category) report(category, details.exitCode);
  });
  app.on('child-process-gone', (_event, details) => {
    log('child_exit', { type: details.type, reason: details.reason, exitCode: details.exitCode });
    const category = crashCategory(details.type, details.reason);
    if (category) report(category, details.exitCode);
  });
}
