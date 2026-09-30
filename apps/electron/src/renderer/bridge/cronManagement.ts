import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import type { LingxiApi } from './lingxi';

type CronHost = Pick<LingxiApi, 'command' | 'onEvent' | 'onConnectionStateChanged'>;

/** Correlate both the runtime owner and request, including concurrent requests. */
export function requestCronManagement(
  host: CronHost,
  sessionId: string,
  request: CronRequestDto,
  timeoutMs = 30_000,
): Promise<CronJobDto[]> {
  const request_id = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    let settled = false;
    let offEvent = () => {};
    let offState = () => {};
    const finish = (error?: Error, jobs: CronJobDto[] = []) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      offEvent();
      offState();
      if (error) reject(error);
      else resolve(jobs);
    };
    const timer = setTimeout(() => finish(new Error('Timed out waiting for scheduled tasks.')), timeoutMs);
    offEvent = host.onEvent((envelope) => {
      const event = envelope.event;
      if (envelope.sessionId !== sessionId || event.type !== 'cron_result' || event.request_id !== request_id) return;
      finish(event.error ? new Error(event.error) : undefined, event.jobs);
    });
    offState = host.onConnectionStateChanged((envelope) => {
      if (envelope.sessionId === sessionId && envelope.event.status !== 'connected') {
        finish(new Error('The scheduled task connection was interrupted.'));
      }
    });
    try {
      void host.command(sessionId, { type: 'cron_manage', request_id, request }).catch((cause: unknown) => {
        finish(cause instanceof Error ? cause : new Error(String(cause)));
      });
    } catch (cause) {
      finish(cause instanceof Error ? cause : new Error(String(cause)));
    }
  });
}
