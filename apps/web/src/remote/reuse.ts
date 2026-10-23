/**
 * The one door through which the web viewer reaches code that lives in the
 * Electron renderer. Only dependency-free modules are reused (the conversation
 * reducer chain and its helpers); `test/remoteImports.test.ts` walks the import
 * graph from the remote entry points and fails if anything reached outside
 * `apps/web` imports a package this app does not declare, and
 * `test/remoteBoundary.test.ts` fails if another file imports `@lingxi/electron`.
 *
 * Components such as `MarkdownContent`, `ToolCall`, `PermissionPrompt` and
 * `DiffView` stay out: they pull `react-markdown`, `rehype-*` and Electron-only
 * modules, so `src/remote` has lean versions of its own.
 */

export {
  appendUserPrompt,
  conversationFromMessages,
  emptyConversation,
  reduceEvent,
} from '@lingxi/electron/renderer/bridge/conversation';
export type { ConversationState } from '@lingxi/electron/renderer/bridge/conversation';

export { toolDisplayHeader, toolHasBody, toolTruncationNotice } from '@lingxi/electron/renderer/model/runItem';
export type {
  CommandRunItem,
  CompactionRunItem,
  MetaRunItem,
  NarrationRunItem,
  RunItem,
  ThinkingRunItem,
  ToolRunItem,
  TurnFileChange,
} from '@lingxi/electron/renderer/model/runItem';

export { transcriptRows } from '@lingxi/electron/renderer/components/transcriptRows';
export type { TranscriptRow, TranscriptToolGroup } from '@lingxi/electron/renderer/components/transcriptRows';

export {
  bytesToBase64,
  isSupportedImageMediaType,
  MAX_IMAGE_ATTACHMENTS,
} from '@lingxi/electron/shared/imageInput';

export { redactSensitiveText } from '@lingxi/bridge-client/toolview';
