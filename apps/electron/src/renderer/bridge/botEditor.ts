/**
 * One Bot open for editing: the record and persona as last read, the draft the
 * owner is changing, and where saving stands. The draft covers the parts of a
 * Bot that are saved together (name, description, model, persona, permission
 * mode, folders); memory and skills act at once and have their own controllers.
 */

import type { BotUi, BotsApi } from '../../shared/bots';
import {
  botChanges,
  botDraftProblems,
  draftFromBot,
  emptyBotDraft,
  hasBotDraftProblems,
  ipcErrorMessage,
  isEmptyPatch,
  saveBot,
  type BotDraft,
} from './botForm';
import { ObservableState } from './observableState';

export type BotSaveStatus =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'saved' }
  /** The Bot changed since it was read; nothing was written. */
  | { kind: 'conflict' }
  | { kind: 'failed'; message: string };

export interface BotEditorState {
  phase: 'loading' | 'ready' | 'failed';
  loadError: string | undefined;
  /** As last read or saved. */
  bot: BotUi | undefined;
  /** The persona as last read or saved. */
  persona: string;
  draft: BotDraft;
  save: BotSaveStatus;
}

/** Whether the draft differs from what is saved. */
export function botEditorDirty(state: BotEditorState): boolean {
  if (!state.bot) return false;
  const { patch, personaChanged } = botChanges(state.bot, state.persona, state.draft);
  return personaChanged || !isEmptyPatch(patch);
}

export class BotEditorController extends ObservableState<BotEditorState> {
  private request = 0;

  constructor(
    private readonly api: Pick<BotsApi, 'get' | 'update' | 'persona'>,
    private readonly botId: string,
    /** Told of every version of the record that was written, so that the list can follow. */
    private readonly onRecord: (bot: BotUi) => void,
  ) {
    super({ phase: 'loading', loadError: undefined, bot: undefined, persona: '', draft: emptyBotDraft(), save: { kind: 'idle' } });
  }

  /** Read the Bot and its persona, replacing the draft. Also what "reload" after a conflict does. */
  async load(): Promise<void> {
    const request = ++this.request;
    this.update({ phase: 'loading', loadError: undefined });
    try {
      const [bot, persona] = await Promise.all([this.api.get(this.botId), this.api.persona.get(this.botId)]);
      if (request !== this.request) return;
      this.set({ phase: 'ready', loadError: undefined, bot, persona, draft: draftFromBot(bot, persona), save: { kind: 'idle' } });
      this.onRecord(bot);
    } catch (error) {
      if (request === this.request) this.update({ phase: 'failed', loadError: ipcErrorMessage(error) });
    }
  }

  setDraft(change: Partial<BotDraft>): void {
    const state = this.getSnapshot();
    if (state.phase !== 'ready' || state.save.kind === 'saving') return;
    this.update({ draft: { ...state.draft, ...change }, save: { kind: 'idle' } });
  }

  async save(): Promise<void> {
    const state = this.getSnapshot();
    if (state.phase !== 'ready' || !state.bot || state.save.kind === 'saving') return;
    if (hasBotDraftProblems(botDraftProblems(state.draft)) || !botEditorDirty(state)) return;
    const { bot, persona, draft } = state;
    this.update({ save: { kind: 'saving' } });
    const result = await saveBot(this.api, bot, persona, draft);
    if (result.ok) {
      this.set({ ...this.getSnapshot(), bot: result.bot, persona: result.persona, draft: draftFromBot(result.bot, result.persona), save: { kind: 'saved' } });
      this.onRecord(result.bot);
    } else if (result.conflict) {
      this.update({ save: { kind: 'conflict' } });
    } else {
      // The record may have been written even though the persona was not: take its new revision, so that trying again does not conflict with ourselves.
      this.update({ ...(result.saved ? { bot: result.saved } : {}), save: { kind: 'failed', message: result.message } });
      if (result.saved) this.onRecord(result.saved);
    }
  }
}
