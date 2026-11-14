import type { User } from 'firebase/auth';
import { teamsTransport } from '../../../../packages/teams';
import { isServiceMessageCommand, servicesTransport, type HostedServicesApi, type ServiceCommand } from '../../../../packages/teams/hosted-service';

/**
 * Bot services for one signed-in account in the browser. Everything goes through the API: a service's records cannot be
 * read directly, so this opens no database. The browser hosts no Bots, so it only uses services; offering one is done
 * on LingXi Desktop.
 */
export function webServicesApi(user: User, currentUid: () => string | undefined): HostedServicesApi & { close(): void } {
  let closed = false;
  const current = () => { if (closed || currentUid() !== user.uid) throw new Error('INVALID_SESSION'); };
  const token = async () => { current(); const value = await user.getIdToken(); current(); return value; };
  const services = servicesTransport('', token), messages = teamsTransport('', token);
  return {
    async services<T>(command: ServiceCommand) { current(); const result = await services<T>(command); current(); return result; },
    async serviceMessages<T>(command: Record<string, unknown>) {
      current(); if (!isServiceMessageCommand(command)) throw new Error('INVALID_INPUT');
      const result = await messages<T>('/messages', command); current(); return result;
    },
    close() { closed = true; },
  };
}
