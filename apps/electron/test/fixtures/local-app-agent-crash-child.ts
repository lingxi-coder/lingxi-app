import { promises as fs, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { LocalAppAgentBroker, type LocalAppAgentAuthorization, type LocalAppAgentOwner } from '../../src/main/local-apps/agentBroker.ts';

const root = process.env.LINGXI_TEST_AGENT_ROOT!;
const owner: LocalAppAgentOwner = { appId: 'a11111111111111111111111111111111', releaseDigest: '1'.repeat(64), invocationId: 'invocation-1', webContentsId: 123 };
const readGrant = { id: 'builtin:Read', kind: 'builtin' as const, name: 'Read' };
const skill = { id: 'extract-text', path: 'skills/extract-text/SKILL.md' };
const authorization: LocalAppAgentAuthorization = {
  settings: { enabled: true, model: { providerId: 'test-provider', modelId: 'test-model' }, tools: [readGrant], skills: [skill.id], background: false, maxTurns: 20, timeoutMs: 600_000 },
  requestedTools: [readGrant.id], requestedSkills: [skill.id], releaseRevision: 1, releaseSourceDirectory: '/release/source',
  skillSpecs: [skill], releaseFiles: [{ path: skill.path, sha256: 'a'.repeat(64), bytes: 18, executable: false }],
  model: { providerId: 'test-provider', modelId: 'test-model', label: 'Test', canSelect: true, supportsAttachments: true, supportsVision: true, supportsStructuredOutput: true },
};
const broker = new LocalAppAgentBroker({ root, authorize: async () => authorization, revalidate: async () => true,
  execute: async execution => {
    // Stand in for an already-issued external side effect, then die before a
    // provider/tool result can be durably recorded.
    await fs.appendFile(join(root, 'external-effect.log'), 'performed\n');
    process.stdout.write(`${JSON.stringify({ runId: execution.runId, chatId: execution.chat?.chatId, threadId: execution.input.protocolThreadId, taskId: execution.taskStorageId })}\n`);
    return new Promise(() => {});
  },
});
const input = JSON.parse(readFileSync(new URL('../../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json', import.meta.url), 'utf8'));
input.runId = 'crash-after-external-effect';
if (process.env.LINGXI_TEST_AGENT_STATELESS === '1') {
  input.threadId = 'stateless-crash-thread';
  await broker.run(owner, input);
} else {
  const chat = await broker.createChat(owner, { requestId: 'abc11111-1111-4111-8111-111111111111', title: 'Crash recovery' });
  input.threadId = chat.chatId;
  await broker.sendChat(owner, chat.chatId, input);
}
await new Promise(() => {});
