/**
 * What the owner does with a Bot's memory: look at an entry, change its text,
 * delete it. Deleting asks first; nothing is sent until the owner confirms.
 */

import type { BotMemoryEntryUi, BotsApi } from '../../shared/bots';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

/** The most one memory file may hold, as the main process enforces it. */
export const BOT_MEMORY_MAX_BYTES = 128 * 1024;

export type BotMemoryProblem = 'too_large' | 'invalid';

export function botMemoryProblem(text: string): BotMemoryProblem | undefined {
  if (text.includes('\0')) return 'invalid';
  return new TextEncoder().encode(text).length > BOT_MEMORY_MAX_BYTES ? 'too_large' : undefined;
}

export interface BotMemoryState {
  /** Unknown until the first read. */
  entries: readonly BotMemoryEntryUi[] | undefined;
  loading: boolean;
  error: string | undefined;
  /** The entry being looked at. */
  open: { name: string; content: string; draft: string; editing: boolean; busy: boolean; error: string | undefined } | undefined;
  /** The entry the owner was asked whether to delete. */
  deleting: { name: string; busy: boolean; error: string | undefined } | undefined;
}

export class BotMemoryController extends ObservableState<BotMemoryState> {
  private request = 0;

  constructor(private readonly api: BotsApi['memory'], private readonly botId: string) {
    super({ entries: undefined, loading: false, error: undefined, open: undefined, deleting: undefined });
  }

  async load(): Promise<void> {
    const request = ++this.request;
    this.update({ loading: true });
    try {
      const entries = await this.api.list(this.botId);
      if (request === this.request) this.update({ entries, error: undefined, loading: false });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error), loading: false });
    }
  }

  async open(name: string): Promise<void> {
    this.update({ open: { name, content: '', draft: '', editing: false, busy: true, error: undefined } });
    try {
      const content = await this.api.read(this.botId, name);
      if (this.getSnapshot().open?.name === name) this.update({ open: { name, content, draft: content, editing: false, busy: false, error: undefined } });
    } catch (error) {
      if (this.getSnapshot().open?.name === name) this.update({ open: { name, content: '', draft: '', editing: false, busy: false, error: ipcErrorMessage(error) } });
    }
  }

  close(): void {
    this.update({ open: undefined });
  }

  edit(): void {
    const { open } = this.getSnapshot();
    if (open && !open.busy && open.error === undefined) this.update({ open: { ...open, editing: true, draft: open.content } });
  }

  setDraft(draft: string): void {
    const { open } = this.getSnapshot();
    if (open?.editing) this.update({ open: { ...open, draft, error: undefined } });
  }

  cancelEdit(): void {
    const { open } = this.getSnapshot();
    if (open) this.update({ open: { ...open, editing: false, draft: open.content, error: undefined } });
  }

  /** Write the changed text. An entry that was not changed is not written. */
  async save(): Promise<void> {
    const { open } = this.getSnapshot();
    if (!open?.editing || open.busy || botMemoryProblem(open.draft) !== undefined) return;
    if (open.draft === open.content) {
      this.update({ open: { ...open, editing: false } });
      return;
    }
    this.update({ open: { ...open, busy: true, error: undefined } });
    try {
      await this.api.write(this.botId, open.name, open.draft);
    } catch (error) {
      this.update({ open: { ...open, busy: false, error: ipcErrorMessage(error) } });
      return;
    }
    this.update({ open: { ...open, content: open.draft, editing: false, busy: false, error: undefined } });
    await this.load();
  }

  askDelete(name: string): void {
    this.update({ deleting: { name, busy: false, error: undefined } });
  }

  cancelDelete(): void {
    if (!this.getSnapshot().deleting?.busy) this.update({ deleting: undefined });
  }

  async confirmDelete(): Promise<void> {
    const { deleting, open } = this.getSnapshot();
    if (!deleting || deleting.busy) return;
    this.update({ deleting: { ...deleting, busy: true, error: undefined } });
    try {
      await this.api.delete(this.botId, deleting.name);
    } catch (error) {
      this.update({ deleting: { ...deleting, busy: false, error: ipcErrorMessage(error) } });
      return;
    }
    this.update({ deleting: undefined, ...(open?.name === deleting.name ? { open: undefined } : {}) });
    await this.load();
  }
}
