import { promises as fs } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import type { LocalAppAgentExecution, LocalAppAgentExecutorResult } from '../../src/main/local-apps/agentBroker.ts';

export async function withFakeChatCheckpoint(execution: LocalAppAgentExecution, completion: LocalAppAgentExecutorResult): Promise<LocalAppAgentExecutorResult> {
  const chat = execution.chat;
  if (!chat) return completion;
  const base = chat.baseCheckpoint ? await fs.readFile(join(chat.sessionDirectory, chat.baseCheckpoint.generationId, `${chat.sessionId}.jsonl`)) : Buffer.alloc(0);
  const rows = [
    { type: 'user', uuid: chat.nativeUserMessageId, sessionId: chat.sessionId, message: { role: 'user', content: execution.input.prompt } },
    { type: 'assistant', uuid: completion.result.messageId, sessionId: chat.sessionId, message: { role: 'assistant', content: completion.result.text } },
  ];
  const bytes = Buffer.concat([base, Buffer.from(rows.map(row => JSON.stringify(row)).join('\n') + '\n')]);
  const directory = join(chat.sessionDirectory, chat.generationId);
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  await fs.writeFile(join(directory, `${chat.sessionId}.jsonl`), bytes, { flag: 'wx', mode: 0o600 });
  return { ...completion, chatCheckpoint: { generationId: chat.generationId, sessionId: chat.sessionId, byteLen: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') } };
}
