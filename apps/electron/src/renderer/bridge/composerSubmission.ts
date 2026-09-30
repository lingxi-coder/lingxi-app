export interface ComposerSubmissionSnapshot {
  sessionId: string | null;
  generation: number;
  html?: string;
  text: string;
  files: readonly string[];
  imageIds: readonly string[];
}

export function matchesComposerSubmission(
  submitted: ComposerSubmissionSnapshot,
  draft: { html?: string; text: string; files: readonly string[]; images: readonly { id: string }[] },
): boolean {
  return submitted.html === draft.html && submitted.text === draft.text
    && submitted.files.length === draft.files.length
    && submitted.files.every((path, index) => path === draft.files[index])
    && submitted.imageIds.length === draft.images.length
    && submitted.imageIds.every((id, index) => id === draft.images[index]?.id);
}
