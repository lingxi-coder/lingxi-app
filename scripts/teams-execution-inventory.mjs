import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const scopes = [
  'apps/firebase/src', 'packages/teams', 'apps/electron/src/main/teams',
  'apps/electron/src/preload', 'apps/electron/src/renderer', 'apps/web/src/teams',
  'apps/ios/native/Sources/Teams', 'apps/android/native/app/src/main/java/com/lingxi/code/teams',
  'infra/firebase', 'apps/bridge-server/src',
];
const retired = [
  'prepareBotTriggers', 'claimBot', 'startBot', 'finishBot', 'recoverBot', 'botQueue',
  'bindLocalBot', 'bindBot', 'unbindBot', 'heartbeatBot', 'botRuns', 'triggerIdentity',
  'legacyBotKey', 'legacyTriggerId', 'executionProtocol',
  'team_guardian',
];
const preserved = [
  'apps/firebase/src/teams.mjs', 'apps/firebase/src/team-messages.mjs',
  'apps/electron/src/main/teams/messageWindow.ts', 'apps/electron/src/renderer/teams-data.ts',
  'apps/electron/src/main/account.ts', 'apps/electron/src/main/provider-accounts.ts',
  'apps/electron/src/main/bots/service.ts', 'packages/teams/BotDirectory.tsx',
  'packages/teams/BotRunCards.tsx', 'apps/electron/src/main/local-apps/serviceBroker.ts',
];
const run = (args, cwd = root) => {
  const result = spawnSync(args[0], args.slice(1), { cwd, encoding: 'utf8' });
  if (result.error) throw result.error;
  if (![0, 1].includes(result.status)) throw new Error(result.stderr || `${args[0]} failed`);
  return result.stdout.trim();
};
const args = process.argv.slice(2);
if (args.some(value => value !== '--assert-removed')) throw new Error('Usage: node scripts/teams-execution-inventory.mjs [--assert-removed]');
const matches = [];
for (const symbol of retired) {
  // Rejection vectors and the one-time deletion selector must name retired
  // formats; neither participates in live execution or decoding.
  const output = run(['rg', '--json', '--word-regexp', '--glob', '!**/test/**', '--glob', '!**/*.test.*',
    '--glob', '!packages/teams/contracts/execution-vectors.json',
    '--glob', '!apps/firebase/src/retired-teams-data.mjs',
    symbol, ...scopes.filter(value => existsSync(path.join(root, value)))]);
  for (const line of output.split('\n').filter(Boolean)) {
    const record = JSON.parse(line);
    if (record.type === 'match') matches.push({ symbol, file: record.data.path.text, line: record.data.line_number });
  }
}
const harness = path.resolve(root, '../harness-runtime');
const harnessScopes = ['crates/runtime/src/desktop/team_connector', 'crates/runtime/src/desktop/team_process', 'crates/runtime/src/desktop/team_invocation.rs'];
const retiredWorkerMatches = [];
for (const symbol of ['team_guardian', 'enable_team_guardian', '--team-shell-guardian', 'claimBot', 'startBot', 'finishBot', 'legacyBotKey']) {
  const output = run(['rg', '--json', '--word-regexp', '--', symbol,
    'crates/runtime/src', 'crates/platforms/posix/src/process'], harness);
  for (const line of output.split('\n').filter(Boolean)) {
    const record = JSON.parse(line);
    if (record.type === 'match') retiredWorkerMatches.push({ symbol, file: record.data.path.text, line: record.data.line_number });
  }
}
const report = {
  schemaVersion: 1, productHead: run(['git', 'rev-parse', 'HEAD']),
  productStatus: run(['git', 'status', '--short']),
  retiredSymbolMatches: matches,
  retiredWorkerMatches,
  workerReplacementScope: harnessScopes.map(file => ({ file, exists: existsSync(path.join(harness, file)) })),
  protectedModules: preserved.map(file => ({ file, exists: existsSync(path.join(root, file)) })),
  obsoleteData: {
    cloud: ['teams/{teamId}/bots', 'teams/{teamId}/conversations/{id}/triggers', 'teams/{teamId}/conversations/{id}/botRuns'],
    conditional: 'Only conversations containing a retired bot principal and their descendants; never all conversations.',
    local: 'Resolve exact owner/project/account-scoped private store paths before cleanup; no guessed home directory or recursive application-data deletion.',
  },
  scope: 'Read-only source inventory. No deletion, deployment, migration or execution is performed.',
};
console.log(JSON.stringify(report, null, 2));
// Worker path names may be reused; the final protocol/build review checks their contents.
if (args.includes('--assert-removed') && (matches.length || retiredWorkerMatches.length)) process.exitCode = 1;
