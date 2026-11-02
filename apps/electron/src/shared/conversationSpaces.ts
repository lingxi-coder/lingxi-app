/** The visible navigation space and the immutable owner of a conversation. */
export type ConversationSpace = 'chat' | 'code';

/** `projectId: null` is an ordinary conversation with no project. */
export interface ConversationOwner {
  mode: ConversationSpace;
  projectId: string | null;
}

/** The workspace is a real directory even when the logical project is null. */
export interface HandoffBacklink {
  sourceSessionId: string;
  sourceProjectPath: string;
  sourceMode: ConversationSpace;
  cutoff: string;
  requestId: string;
}

export interface OwnedConversation extends ConversationOwner {
  handoff?: HandoffBacklink;
  projectPath: string;
  sessionId: string;
}

export interface SpaceNavigation {
  projectId: string | null;
  session?: OwnedConversation;
}

export interface ConversationSpacesSnapshot {
  activeMode: ConversationSpace;
  projects: Record<string, ConversationSpace[]>;
  navigation: Record<ConversationSpace, SpaceNavigation>;
  sessions: OwnedConversation[];
  /** Live, unsubmitted drafts, which may include more than one per space. */
  drafts?: OwnedConversation[];
  globalWorkspace: string;
}

export const CH_CONVERSATION_SPACE = 'lingxi:conversation-space';
export const CH_CONVERSATION_SHOWN = 'lingxi:conversation-space:shown';

export const CH_SESSION_HANDOFF = 'lingxi:session:handoff';
export const CH_SESSION_HANDOFF_CANCEL = 'lingxi:session:handoff:cancel';
