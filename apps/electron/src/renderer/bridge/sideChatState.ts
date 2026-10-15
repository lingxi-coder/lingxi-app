import type { AskUserQuestionRequestDto, ClientEvent, PermissionRequest, ImageRefDto } from '@lingxi/bridge-client';
import type { ImageAttachment } from './imageInput';
import { promptWithFileMentions } from './fileMentions';
import { appendUserPrompt, conversationFromMessages, emptyConversation, reduceEvent, type ConversationState } from './conversation';
import { emptyDesktopState, reduceDesktopEvent, type DesktopState } from './desktopState';

export interface SideChatState {
  id: string;
  hostId: string;
  conversation: ConversationState;
  desktop: DesktopState;
  draft: string;
  images: ImageAttachment[];
  permissions: PermissionRequest[];
  questions: AskUserQuestionRequestDto[];
  closing: boolean;
  ending: boolean;
  error?: string;
}
export function newSideChat(id: string, hostId: string): SideChatState {
  return { id, hostId, conversation: { ...emptyConversation(), sessionKey: `side:${id}` }, desktop: emptyDesktopState(), draft: '', images: [], permissions: [], questions: [], closing: false, ending: false };
}
export function reduceSideChat(side: SideChatState, event: ClientEvent): SideChatState {
  // Parent history must never become visible in the side transcript.
  if (event.type === 'session_resumed') return side;
  if (event.type === 'side_chat_permission') {
    if (event.side_id !== side.id) return side;
    return { ...side, permissions: [...side.permissions.filter((request) => request.request_id !== event.request.request_id), event.request] };
  }
  // The manager records only new side user messages for RAM snapshot replay.
  if (event.type === 'message_complete' && event.message?.role === 'user') {
    const restored = conversationFromMessages([event.message]);
    let nextId = side.conversation.nextId;
    const items = restored.items.map((item) => ({ ...item, id: `i${nextId++}` }));
    return { ...side, conversation: { ...side.conversation, items: [...side.conversation.items, ...items], nextId } };
  }
  const permission = event as unknown as { type: string; request?: PermissionRequest; payload?: PermissionRequest };
  const next = { ...side, conversation: reduceEvent(side.conversation, event), desktop: reduceDesktopEvent(side.desktop, event) };
  if (permission.type === 'permission_request') {
    const request = permission.request ?? permission.payload;
    if (request) next.permissions = [...side.permissions.filter((entry) => entry.request_id !== request.request_id), request];
  }
  if (event.type === 'permission_request_resolved') next.permissions = side.permissions.filter((entry) => entry.request_id !== event.request_id);
  if (event.type === 'ask_user_question') next.questions = [...side.questions.filter((entry) => entry.request_id !== event.request.request_id), event.request];
  if (event.type === 'ask_user_question_resolved') next.questions = side.questions.filter((entry) => entry.request_id !== event.request_id);
  return next;
}
export function acknowledgeSidePrompt(side: SideChatState, text: string, images: readonly ImageRefDto[] = []): SideChatState {
  return { ...side, draft: side.draft === text ? '' : side.draft, error: undefined,
    conversation: /^\s*\//.test(text) ? side.conversation : appendUserPrompt(side.conversation, text, images) };
}
export function sideCommandInput(text: string) {
  return /^\s*\//.test(text) ? { type: 'run_slash_command', raw: text.trim() } : { type: 'send_prompt', text, images: [] };
}
export function sideCreationInput(text: string, files: string[] = []): string | undefined | null {
  const match = /^\s*\/side(?:\s+([\s\S]*))?\s*$/.exec(text);
  return match ? promptWithFileMentions(match[1]?.trim() ?? '', files) || undefined : null;
}
