import { AccountService as ProductAccount } from '../../src/main/account.js';
import { accountSessionDiagnosticScope } from '../../src/main/account-broker-storage.js';
import { TeamsService as ProductTeams } from '../../src/main/teams/service.js';

declare const __LINGXI_DATA_STAGING_PORTS__: { auth: number; firestore: number; api: number };
const ports = __LINGXI_DATA_STAGING_PORTS__;
const config = { projectId: 'demo-lingxi-data', apiKey: 'demo', appId: 'signed-data-staging', authDomain: 'localhost' };

const demoFetch: typeof fetch = (input, init) => {
  const upstream = new URL(String(input));
  if (upstream.protocol !== 'https:' || !['identitytoolkit.googleapis.com', 'securetoken.googleapis.com'].includes(upstream.hostname)) {
    throw new Error('Unexpected demo identity origin');
  }
  if (upstream.pathname.endsWith('signInWithPassword')) {
    const { email } = JSON.parse(String(init?.body));
    if (!['owner@fixture.invalid', 'reader@fixture.invalid'].includes(email)) throw new Error('Expected a demo account');
  }
  return fetch(`http://127.0.0.1:${ports.auth}/${upstream.hostname}${upstream.pathname}${upstream.search}`, init);
};

export class AccountService extends ProductAccount {
  constructor(...args: ConstructorParameters<typeof ProductAccount>) {
    if (!accountSessionDiagnosticScope(process.argv, 'development') || !args[1].persistent) {
      throw new Error('LINGXI_SIGNED_DATA_DEMO_ONLY');
    }
    super(config, args[1], args[2], demoFetch);
  }
}

export class TeamsService extends ProductTeams {
  constructor(...args: ConstructorParameters<typeof ProductTeams>) {
    super(args[0], config, `http://127.0.0.1:${ports.api}`, { host: '127.0.0.1', authPort: ports.auth, firestorePort: ports.firestore });
  }
}
