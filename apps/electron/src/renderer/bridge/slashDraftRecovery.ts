import type { ImageAttachment } from './imageInput.js';
import type { ComposerSubmissionSnapshot } from './composerSubmission.js';
import type { SlashCommandTerminalResult } from './slashCommandTerminals.js';

export interface SlashDraftRecoverySnapshot extends ComposerSubmissionSnapshot {
  images: readonly ImageAttachment[];
}

export interface SlashDraftRecoveryOwner {
  sessionId: string;
  draftGeneration: number;
}

export interface SlashDraftRecoveryCurrentState {
  activeSessionId: string | null;
  draftGeneration: number;
  savedDraftExists: boolean;
  text: string;
  files: readonly string[];
  images: readonly ImageAttachment[];
}

export interface RestoredSlashDraft {
  kind: 'visible' | 'saved';
  text: string;
  html?: string;
  files: string[];
  images: ImageAttachment[];
}

/** Restore only a matching terminal failure when the submitted session has not been edited. */
export function restoreSlashDraftAfterTerminalError(
  submitted: SlashDraftRecoverySnapshot,
  owner: SlashDraftRecoveryOwner,
  terminal: SlashCommandTerminalResult,
  current: SlashDraftRecoveryCurrentState,
): RestoredSlashDraft | null {
  if (!terminal.isError || owner.sessionId !== submitted.sessionId || current.draftGeneration !== owner.draftGeneration) return null;
  const imageIds = new Set(submitted.imageIds);
  const images = submitted.images.filter((image) => imageIds.has(image.id));
  if (current.activeSessionId === submitted.sessionId) {
    if (current.text || current.files.length || current.images.length) return null;
    return { kind: 'visible', text: submitted.text, html: submitted.html, files: [...submitted.files], images };
  }
  if (current.savedDraftExists) return null;
  return { kind: 'saved', text: submitted.text, html: submitted.html, files: [...submitted.files], images };
}
