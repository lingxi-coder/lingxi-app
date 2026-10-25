import type { BaseEvent, ContentPart, Message, RunAgentInput } from '@ag-ui/core';
import type { RunAgentResult } from '@ag-ui/client';
import {
  AG_UI_PROTOCOL_VERSION,
  LOCAL_APP_EXTENSION_KEY,
  createLocalAppAgent,
  validateEvent,
  validateRunAgentInput,
} from '@lingxi/local-app-agui/client';
import { fileToContentPart } from '@lingxi/local-app-agui';

const userMessage: Message = { id: 'user-1', role: 'user', content: 'hello' };
const input: RunAgentInput = {
  threadId: 'thread-1', runId: 'run-1', protocolVersion: AG_UI_PROTOCOL_VERSION,
  messages: [userMessage], tools: [], context: [],
  forwardedProps: { [LOCAL_APP_EXTENSION_KEY]: { version: 1, maxTurns: 2 } },
};
const checked: RunAgentInput = validateRunAgentInput(input);
const started: BaseEvent = validateEvent({ type: 'RUN_STARTED', threadId: checked.threadId, runId: checked.runId });
const attachment: Promise<ContentPart> = fileToContentPart(new File(['hello'], 'hello.txt', { type: 'text/plain' }));
const client = createLocalAppAgent({
  transport: {
    start: async value => ({ runId: value.runId }),
    events: async () => ({ events: [], sequence: 0 }),
  },
});
const stream = client.run(checked);
const result: RunAgentResult = { result: undefined, newMessages: [userMessage] };
void [started, attachment, stream, result];
