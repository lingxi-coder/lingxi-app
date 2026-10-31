import type { BaseEvent, ContentPart, Message, RunAgentInput } from '@ag-ui/core';
import type { RunAgentResult } from '@ag-ui/client';
import {
  AG_UI_PROTOCOL_VERSION,
  LOCAL_APP_EXTENSION_KEY,
  createLocalAppAgent,
  validateEvent,
  validateRunAgentInput,
} from '@lingxi/local-app-agui/client';
import {
  fileToContentPart,
  type LocalAppChatCreationOptions,
  type LocalAppChatHandle,
  type LocalAppChats,
  type LocalAppPendingChatCreation,
} from '@lingxi/local-app-agui';
import { installBrowserAgent } from '@lingxi/local-app-agui/browser';

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

// Compile the public browser entry point as an App author would consume it.
async function checkBrowserChatContract(agent: ReturnType<typeof installBrowserAgent>) {
  const request: LocalAppChatCreationOptions = {
    requestId: '550e8400-e29b-41d4-a716-446655440000', title: 'Meal log',
  };
  const chats: LocalAppChats = agent.chats;
  const created: Promise<LocalAppChatHandle> = chats.create(request);
  const status: Promise<LocalAppChatHandle | null> = chats.creationStatus(request);
  const pending: LocalAppPendingChatCreation[] = await chats.pendingCreations();
  const states: Array<'pending' | 'accepted' | 'deleted' | 'rejected'> = pending.map(value => value.state);
  const forgotten: Promise<void> = chats.forgetCreation(request.requestId);
  const page = await chats.list({ limit: 10, includeArchived: true });
  const unavailable: Array<{ chatId: string; code: 'chat_storage_uncertain' }> | undefined = page.unavailableChats;
  const uncertain: boolean | undefined = page.hasUncertainChats;
  void [created, status, states, forgotten, unavailable, uncertain];

  // @ts-expect-error Creation requires a persisted caller-generated requestId.
  agent.chats.create();
  // @ts-expect-error The old title-only creation input is no longer public.
  agent.chats.create({ title: 'Meal log' });
  // @ts-expect-error Explicit undefined must not restore the old optional input.
  agent.chats.create(undefined);
  // @ts-expect-error Creation status must use the same complete request input.
  agent.chats.creationStatus({ title: 'Meal log' });
}
void checkBrowserChatContract;
