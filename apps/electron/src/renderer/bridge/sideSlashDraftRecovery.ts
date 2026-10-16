import type { ClientEvent } from '@lingxi/bridge-client';
import type { ImageAttachment } from './imageInput.js';
import type { SideChatState } from './sideChatState.js';

export interface SideSlashSubmissionSnapshot {
  hostId: string;
  sideId: string;
  turnId: number;
  text: string;
  images: readonly ImageAttachment[];
  draftRevision: number;
  acknowledged: boolean;
  terminalError: boolean;
}

export function isSideSlashTerminalError(event: ClientEvent, submission: SideSlashSubmissionSnapshot): boolean {
  return event.type === 'slash_command_result' && event.turn_id === submission.turnId && event.is_error === true;
}

export function acknowledgeSideSlashPrompt(side: SideChatState, submission: SideSlashSubmissionSnapshot): SideChatState {
  if (submission.terminalError || side.hostId !== submission.hostId || side.id !== submission.sideId) return side;
  const imageIds = new Set(submission.images.map((image) => image.id));
  return {
    ...side,
    draft: side.draft === submission.text ? '' : side.draft,
    images: side.images.filter((image) => !imageIds.has(image.id)),
    error: undefined,
  };
}

export function restoreSideSlashPromptAfterError(
  side: SideChatState,
  submission: SideSlashSubmissionSnapshot,
): SideChatState {
  if (!submission.acknowledged || side.hostId !== submission.hostId || side.id !== submission.sideId
    || side.draftRevision !== submission.draftRevision || side.draft !== '' || side.images.length !== 0) return side;
  const imageIds = new Set(submission.images.map((image) => image.id));
  const images = submission.images.filter((image) => imageIds.has(image.id));
  return { ...side, draft: submission.text, images: [...images], draftRevision: side.draftRevision + 1 };
}
